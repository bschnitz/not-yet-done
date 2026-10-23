//! Saved-query bodies for the timesheet listing.
//!
//! A body is a YAML mapping in two halves. The top-level keys are the filters
//! Kimai's `GET /api/timesheets` understands, written with names instead of
//! ids and with natural-language dates; they go to the server. `where:` holds
//! a [`FilterExpr`] in the shared filter language for what the server cannot
//! do (OR across fields, duration ranges, matching project names) and is
//! evaluated against the fetched rows.
//!
//! ```yaml
//! begin: start of month        # Kimai filters on the begin of an entry
//! end: today                   # date only = the whole day
//! customer: Acme               # name or `#<id>`; a list means any of them
//! project: [Acme/Website, Intranet]   # `Customer/Project` narrows a name
//! activity: Development
//! tags: [backend]
//! billable: true
//! exported: false
//! running: false
//! term: "deploy !hotfix"       # words in the description, `!` excludes
//! where:
//!   or:
//!     - [duration, '>=', 14400]
//!     - [description, has, review]
//! ```
//!
//! Every server filter narrows the result (AND); the values of one list are
//! alternatives (OR). A name that matches several entities (one project name
//! under two customers) selects all of them. Without `begin` the listing
//! starts `lookback_days` ago, like an empty query.

use std::borrow::Cow;
use std::collections::HashMap;

use chrono::{DateTime, Local, NaiveDateTime, NaiveTime, Utc};
use not_yet_done_filter::eval::{self, Field, RowFields};
use not_yet_done_filter::{FilterExpr, resolve_dates};
use serde::Deserialize;

use crate::client::{KimaiActivity, KimaiCustomer, KimaiProject, KimaiTimesheet};

/// Every column a `where:` clause may name.
const WHERE_COLUMNS: &[&str] = &[
    "project",
    "customer",
    "activity",
    "description",
    "tags",
    "duration",
    "begin",
    "end",
    "running",
];

/// The `where:` columns compared as instants.
const DATETIME_COLUMNS: &[&str] = &["begin", "end"];

/// A single value or a list of them.
#[derive(Deserialize, Debug, Clone, Default)]
#[serde(untagged)]
enum OneOrMany {
    #[default]
    None,
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn values(&self) -> Vec<&str> {
        match self {
            OneOrMany::None => Vec::new(),
            OneOrMany::One(v) => vec![v.as_str()],
            OneOrMany::Many(v) => v.iter().map(String::as_str).collect(),
        }
    }
}

/// The body as written.
#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
struct QueryDoc {
    #[serde(default)]
    begin: Option<String>,
    #[serde(default)]
    end: Option<String>,
    #[serde(default)]
    customer: OneOrMany,
    #[serde(default)]
    project: OneOrMany,
    #[serde(default)]
    activity: OneOrMany,
    #[serde(default)]
    tags: OneOrMany,
    #[serde(default)]
    billable: Option<bool>,
    #[serde(default)]
    exported: Option<bool>,
    #[serde(default)]
    running: Option<bool>,
    #[serde(default)]
    term: Option<String>,
    #[serde(default, rename = "where")]
    where_: Option<serde_yaml::Value>,
}

/// A parsed query: the server filters with dates already resolved, and the
/// local filter. Names are resolved later, against the lookups of the listing.
#[derive(Debug, Default)]
pub(super) struct KimaiQuery {
    /// Local `YYYY-MM-DDTHH:MM:SS`, as Kimai expects it.
    pub begin: Option<String>,
    pub end: Option<String>,
    customers: Vec<String>,
    projects: Vec<String>,
    activities: Vec<String>,
    tags: Vec<String>,
    billable: Option<bool>,
    exported: Option<bool>,
    running: Option<bool>,
    term: Option<String>,
    filter: Option<FilterExpr>,
}

/// Format for the `begin`/`end` query parameters.
const KIMAI_LOCAL: &str = "%Y-%m-%dT%H:%M:%S";

impl KimaiQuery {
    /// Parse a body. An empty body is the empty query.
    pub(super) fn parse(raw: &str) -> Result<Self, String> {
        Self::parse_at(raw, Local::now())
    }

    fn parse_at(raw: &str, now: DateTime<Local>) -> Result<Self, String> {
        let doc: QueryDoc = if raw.trim().is_empty() {
            QueryDoc::default()
        } else {
            serde_yaml::from_str::<Option<QueryDoc>>(raw)
                .map_err(|e| format!("kimai query: {e}"))?
                .unwrap_or_default()
        };
        let begin = doc
            .begin
            .as_deref()
            .map(|s| resolve_bound(s, false, now))
            .transpose()?;
        let end = doc
            .end
            .as_deref()
            .map(|s| resolve_bound(s, true, now))
            .transpose()?;
        let filter = doc.where_.map(parse_where).transpose()?;
        let owned = |v: &OneOrMany| v.values().into_iter().map(str::to_string).collect();
        Ok(Self {
            begin,
            end,
            customers: owned(&doc.customer),
            projects: owned(&doc.project),
            activities: owned(&doc.activity),
            tags: owned(&doc.tags),
            billable: doc.billable,
            exported: doc.exported,
            running: doc.running,
            term: doc.term.filter(|t| !t.trim().is_empty()),
            filter,
        })
    }

    /// Whether resolving the `customer:` names needs `/api/customers`.
    pub(super) fn needs_customers(&self) -> bool {
        !self.customers.is_empty()
    }

    /// The `/api/timesheets` parameters besides `begin` and paging, with every
    /// name resolved to the ids it stands for.
    pub(super) fn server_params(
        &self,
        customers: &[KimaiCustomer],
        projects: &HashMap<u64, KimaiProject>,
        activities: &HashMap<u64, KimaiActivity>,
    ) -> Result<Vec<(String, String)>, String> {
        let mut params: Vec<(String, String)> = Vec::new();
        if let Some(end) = &self.end {
            params.push(("end".into(), end.clone()));
        }
        for name in &self.customers {
            for id in resolve_customer(name, customers)? {
                params.push(("customers[]".into(), id.to_string()));
            }
        }
        for name in &self.projects {
            for id in resolve_project(name, projects)? {
                params.push(("projects[]".into(), id.to_string()));
            }
        }
        for name in &self.activities {
            for id in resolve_activity(name, activities)? {
                params.push(("activities[]".into(), id.to_string()));
            }
        }
        for tag in &self.tags {
            params.push(("tags[]".into(), tag.clone()));
        }
        let flag = |b: bool| if b { "1" } else { "0" }.to_string();
        if let Some(b) = self.billable {
            params.push(("billable".into(), flag(b)));
        }
        if let Some(b) = self.exported {
            params.push(("exported".into(), flag(b)));
        }
        if let Some(b) = self.running {
            params.push(("active".into(), flag(b)));
        }
        if let Some(term) = &self.term {
            params.push(("term".into(), term.clone()));
        }
        Ok(params)
    }

    /// Whether a fetched timesheet passes the `where:` clause.
    pub(super) fn keeps(&self, row: &TimesheetRow<'_>) -> bool {
        self.filter
            .as_ref()
            .is_none_or(|expr| eval::matches(expr, row))
    }
}

/// Resolve a `begin`/`end` value to Kimai's local format. A phrase without a
/// time of day means the start of that day, or for `end` its last second.
fn resolve_bound(s: &str, is_end: bool, now: DateTime<Local>) -> Result<String, String> {
    let key = if is_end { "end" } else { "begin" };
    let res = natural_date::resolve_detailed(s.trim(), now).ok_or_else(|| {
        format!(
            "kimai query: could not read '{s}' as a date for '{key}'. Use e.g. 'today', \
             'monday', 'start of month', '2 weeks ago' or 2026-01-01."
        )
    })?;
    let local: NaiveDateTime = res.instant.with_timezone(&Local).naive_local();
    let local = match (res.has_time, is_end) {
        (true, _) => local,
        (false, false) => local.date().and_time(NaiveTime::MIN),
        (false, true) => local
            .date()
            .and_hms_opt(23, 59, 59)
            .expect("23:59:59 is a valid time"),
    };
    Ok(local.format(KIMAI_LOCAL).to_string())
}

/// Parse the `where:` value: natural dates resolved, columns checked.
fn parse_where(value: serde_yaml::Value) -> Result<FilterExpr, String> {
    let expr: FilterExpr = serde_yaml::from_value(resolve_dates(value))
        .map_err(|e| format!("kimai query: invalid where: {e}"))?;
    expr.validate_custom(&[])
        .map_err(|e| format!("kimai query: {e}"))?;
    eval::validate_fields(&expr, WHERE_COLUMNS, "kimai column")
        .and_then(|()| eval::validate_datetime_literals(&expr, DATETIME_COLUMNS))
        .map_err(|e| format!("kimai query: {e}"))?;
    Ok(expr)
}

/// The `#<id>` escape, if `name` is one.
fn id_escape(name: &str) -> Option<u64> {
    name.trim().strip_prefix('#')?.parse().ok()
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn unknown(kind: &str, name: &str) -> String {
    format!("kimai query: no {kind} named '{name}' (or write #<id>)")
}

fn resolve_customer(name: &str, customers: &[KimaiCustomer]) -> Result<Vec<u64>, String> {
    if let Some(id) = id_escape(name) {
        return Ok(vec![id]);
    }
    let ids: Vec<u64> = customers
        .iter()
        .filter(|c| same_name(&c.name, name))
        .map(|c| c.id)
        .collect();
    if ids.is_empty() {
        return Err(unknown("customer", name));
    }
    Ok(ids)
}

/// A project by its name, or by `Customer/Project` when the name alone is
/// ambiguous. A name that itself contains `/` is tried whole first.
fn resolve_project(name: &str, projects: &HashMap<u64, KimaiProject>) -> Result<Vec<u64>, String> {
    if let Some(id) = id_escape(name) {
        return Ok(vec![id]);
    }
    let mut ids: Vec<u64> = projects
        .values()
        .filter(|p| same_name(&p.name, name))
        .map(|p| p.id)
        .collect();
    if ids.is_empty() {
        ids = projects
            .values()
            .filter(|p| {
                let customer = p.parent_title.as_deref().unwrap_or("");
                name.match_indices('/').any(|(i, _)| {
                    same_name(&name[..i], customer) && same_name(&name[i + 1..], &p.name)
                })
            })
            .map(|p| p.id)
            .collect();
    }
    if ids.is_empty() {
        return Err(unknown("project", name));
    }
    ids.sort_unstable();
    Ok(ids)
}

fn resolve_activity(
    name: &str,
    activities: &HashMap<u64, KimaiActivity>,
) -> Result<Vec<u64>, String> {
    if let Some(id) = id_escape(name) {
        return Ok(vec![id]);
    }
    let mut ids: Vec<u64> = activities
        .values()
        .filter(|a| same_name(&a.name, name))
        .map(|a| a.id)
        .collect();
    if ids.is_empty() {
        return Err(unknown("activity", name));
    }
    ids.sort_unstable();
    Ok(ids)
}

/// A timesheet with its resolved names, viewed as a row for `where:`.
pub(super) struct TimesheetRow<'a> {
    pub ts: &'a KimaiTimesheet,
    pub project: &'a str,
    pub customer: &'a str,
    pub activity: &'a str,
}

fn instant(value: &str) -> Field<'static> {
    let normalized = super::normalize_iso_offset(value);
    match DateTime::parse_from_rfc3339(&normalized) {
        Ok(dt) => Field::DateTime(dt.with_timezone(&Utc)),
        Err(_) => Field::Null,
    }
}

impl RowFields for TimesheetRow<'_> {
    fn field(&self, column: &str) -> Field<'_> {
        let ts = self.ts;
        match column {
            "project" => Field::Text(Cow::Borrowed(self.project)),
            "customer" => Field::Text(Cow::Borrowed(self.customer)),
            "activity" => Field::Text(Cow::Borrowed(self.activity)),
            "description" => match ts.description.as_deref() {
                Some(d) => Field::Text(Cow::Borrowed(d)),
                None => Field::Null,
            },
            "tags" => Field::List(
                ts.tags
                    .iter()
                    .map(|t| Field::Text(Cow::Borrowed(t.as_str())))
                    .collect(),
            ),
            "duration" => Field::Number(ts.duration.unwrap_or(0) as f64),
            "begin" => instant(&ts.begin),
            "end" => ts.end.as_deref().map_or(Field::Null, instant),
            "running" => Field::Bool(ts.end.is_none()),
            // Unreachable: parse_where validated the column set.
            _ => Field::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2030, 3, 13, 15, 30, 0).unwrap()
    }

    fn parse(raw: &str) -> Result<KimaiQuery, String> {
        KimaiQuery::parse_at(raw, now())
    }

    fn lookups() -> (
        Vec<KimaiCustomer>,
        HashMap<u64, KimaiProject>,
        HashMap<u64, KimaiActivity>,
    ) {
        let customers = vec![
            KimaiCustomer {
                id: 1,
                name: "Acme Corp".into(),
            },
            KimaiCustomer {
                id: 2,
                name: "Globex".into(),
            },
        ];
        let project = |id: u64, name: &str, customer: &str| {
            (
                id,
                KimaiProject {
                    id,
                    name: name.into(),
                    parent_title: Some(customer.into()),
                },
            )
        };
        let projects = HashMap::from([
            project(7, "Website", "Acme Corp"),
            project(8, "Website", "Globex"),
            project(9, "Intranet", "Globex"),
        ]);
        let activity = |id: u64, name: &str| {
            (
                id,
                KimaiActivity {
                    id,
                    name: name.into(),
                    project: None,
                    parent_title: None,
                },
            )
        };
        let activities = HashMap::from([activity(3, "Development"), activity(4, "Meeting")]);
        (customers, projects, activities)
    }

    fn params(raw: &str) -> Result<Vec<(String, String)>, String> {
        let (c, p, a) = lookups();
        parse(raw)?.server_params(&c, &p, &a)
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn empty_body_is_the_empty_query() {
        let q = parse("").unwrap();
        assert!(q.begin.is_none());
        assert!(params("").unwrap().is_empty());
        assert!(parse("# only a comment\n").unwrap().begin.is_none());
    }

    #[test]
    fn date_only_bounds_cover_whole_days() {
        let q = parse("begin: 2030-01-01\nend: today\n").unwrap();
        assert_eq!(q.begin.as_deref(), Some("2030-01-01T00:00:00"));
        assert_eq!(q.end.as_deref(), Some("2030-03-13T23:59:59"));
    }

    #[test]
    fn unreadable_date_is_an_error() {
        let err = parse("begin: someday maybe\n").unwrap_err();
        assert!(err.contains("'begin'"), "{err}");
    }

    #[test]
    fn names_resolve_to_ids() {
        let got = params("customer: acme corp\nproject: Intranet\nactivity: [Development, '#4']\n")
            .unwrap();
        assert_eq!(
            got,
            pairs(&[
                ("customers[]", "1"),
                ("projects[]", "9"),
                ("activities[]", "3"),
                ("activities[]", "4"),
            ])
        );
    }

    #[test]
    fn ambiguous_project_selects_all_unless_narrowed() {
        assert_eq!(
            params("project: Website\n").unwrap(),
            pairs(&[("projects[]", "7"), ("projects[]", "8")])
        );
        assert_eq!(
            params("project: Globex/Website\n").unwrap(),
            pairs(&[("projects[]", "8")])
        );
    }

    #[test]
    fn unknown_name_is_an_error() {
        let err = params("project: Nope\n").unwrap_err();
        assert!(err.contains("project named 'Nope'"), "{err}");
    }

    #[test]
    fn flags_and_term_pass_through() {
        assert_eq!(
            params("billable: true\nexported: false\nrunning: false\nterm: deploy\ntags: x\n")
                .unwrap(),
            pairs(&[
                ("tags[]", "x"),
                ("billable", "1"),
                ("exported", "0"),
                ("active", "0"),
                ("term", "deploy"),
            ])
        );
    }

    #[test]
    fn unknown_key_is_an_error() {
        assert!(parse("customers: Acme\n").is_err());
    }

    fn timesheet(duration: i64, tags: &[&str], end: Option<&str>) -> KimaiTimesheet {
        KimaiTimesheet {
            id: 1,
            project: 7,
            activity: 3,
            begin: "2030-01-15T09:00:00+0100".into(),
            end: end.map(str::to_string),
            duration: Some(duration),
            description: Some("Review the login form".into()),
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    fn keeps(raw: &str, ts: &KimaiTimesheet) -> bool {
        let row = TimesheetRow {
            ts,
            project: "Website",
            customer: "Acme Corp",
            activity: "Development",
        };
        parse(raw).unwrap().keeps(&row)
    }

    #[test]
    fn where_filters_rows_locally() {
        let long = timesheet(5 * 3600, &["backend"], Some("2030-01-15T14:00:00+0100"));
        let short = timesheet(600, &[], None);
        let q = "where:\n  or:\n    - [duration, '>=', 14400]\n    - [tags, in, [frontend]]\n";
        assert!(keeps(q, &long));
        assert!(!keeps(q, &short));
        assert!(keeps("where: [running, '=', true]\n", &short));
        assert!(keeps("where: [description, has, login]\n", &short));
        assert!(keeps("where: [begin, '<', 2030-02-01]\n", &short));
        assert!(!keeps("where: [customer, '=', Globex]\n", &short));
    }

    #[test]
    fn where_rejects_unknown_columns() {
        let err = parse("where: [colour, '=', red]\n").unwrap_err();
        assert!(err.contains("colour"), "{err}");
    }
}
