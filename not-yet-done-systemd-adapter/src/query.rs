//! In-memory query filtering for unit rows.
//!
//! What a systemd level shows is a **query**, not configuration — decision D2
//! in `docs/plan-systemd-adapter.md`. The language is the same `FilterExpr` YAML
//! the tasks, trackings and calendar levels use, so the whole query surface (the
//! `q` menu, saved queries, `${var}` placeholders, extended queries) applies here
//! without any of it being written again.
//!
//! There is nothing to push the filter down to: `ListUnits` and `ListUnitFiles`
//! are one D-Bus call each for everything, so the rows are already in hand by
//! the time the query is evaluated. (`ListUnitsByPatterns` could pre-filter on
//! the name alone — a pushdown worth having only if a level ever gets slow.)
//!
//! Each level validates against **its own** column set, so `[mem, gt, 0]` on the
//! Unit files level is a named mistake rather than a silently empty table.

use std::borrow::Cow;

use chrono::{DateTime, Utc};
use not_yet_done_filter::eval::{self, Field, RowFields};
use not_yet_done_filter::{FilterExpr, query_filter};

use crate::model::{LogRow, PropertyRow, ServiceRow, TimerRow, UnitFileRow};

/// Columns a Services query may reference.
pub const SERVICE_COLUMNS: &[&str] = &[
    "name",
    "description",
    "load",
    "active",
    "sub",
    "enabled",
    "since",
    "pid",
    "mem",
    "tasks",
    "cpu",
    "restarts",
    "needs_reload",
    "fragment",
];

/// Columns a Timers query may reference.
pub const TIMER_COLUMNS: &[&str] = &[
    "name",
    "description",
    "active",
    "sub",
    "enabled",
    "next",
    "last",
    "unit",
    "result",
    "persistent",
];

/// Columns a Properties query may reference.
///
/// Three, and all text: the level is deliberately untyped (see
/// [`model::render`](crate::model)), so `[name, like, Timeout]` is the query
/// this level is for.
pub const PROPERTY_COLUMNS: &[&str] = &["name", "value", "interface"];

/// Columns a Journal query may reference.
///
/// `prio` and `level` are the same severity twice — the number to compare
/// against (`[prio, lte, 3]`) and the word to read and to paint by
/// (`[level, =, err]`). Both are queryable because a person writing the query
/// by hand reaches for whichever is on their screen.
pub const LOG_COLUMNS: &[&str] = &["time", "level", "prio", "pid", "message"];

/// Columns a Unit files query may reference.
pub const UNIT_FILE_COLUMNS: &[&str] = &["name", "state", "path", "vendor"];

/// The columns compared as instants. A comparison against one of these needs a
/// right-hand side that resolved to a real date, or the query would silently
/// match nothing.
const DATETIME_COLUMNS: &[&str] = &["since", "next", "last", "time"];

/// A compiled level query: parsed, date-resolved, validated against the level's
/// own columns.
#[derive(Debug)]
pub struct UnitQuery {
    expr: FilterExpr,
}

impl UnitQuery {
    /// The compiled expression, for a level that can hand part of it to the
    /// backend — see [`crate::journal`], the only one that has a backend to
    /// hand anything to.
    pub fn expr(&self) -> &FilterExpr {
        &self.expr
    }

    /// Parse a raw query body against `columns`. The error is meant for the
    /// status bar — a malformed query must say so rather than empty the table.
    pub fn parse(raw: &str, columns: &[&str]) -> Result<Self, String> {
        let parsed = query_filter::parse(raw).map_err(|e| e.to_string())?;
        eval::validate_fields(&parsed.expr, columns, "systemd column")?;
        eval::validate_datetime_literals(&parsed.expr, DATETIME_COLUMNS)?;
        Ok(Self { expr: parsed.expr })
    }

    /// Compile the pane's query if there is one. `None`/blank means "show
    /// everything", which is what every level does by default (decision D4: a
    /// query that hides rows is not a default, it is a bug report waiting to
    /// happen).
    pub fn compile(raw: Option<&str>, columns: &[&str]) -> Result<Option<Self>, String> {
        match raw.map(str::trim) {
            Some(q) if !q.is_empty() => Self::parse(q, columns).map(Some),
            _ => Ok(None),
        }
    }

    pub fn matches(&self, row: &impl RowFields) -> bool {
        eval::matches(&self.expr, row)
    }
}

/// Retain the rows an optional query accepts.
pub fn retain<T: RowFields>(rows: Vec<T>, query: &Option<UnitQuery>) -> Vec<T> {
    match query {
        None => rows,
        Some(q) => rows.into_iter().filter(|r| q.matches(r)).collect(),
    }
}

/// A text column that is empty when the value is absent — `is_null` then works
/// and every comparison against it is false, which is what an absent value
/// means.
fn text_or_null(value: &str) -> Field<'_> {
    if value.is_empty() {
        Field::Null
    } else {
        Field::Text(Cow::Borrowed(value))
    }
}

fn num_or_null(value: Option<u64>) -> Field<'static> {
    match value {
        Some(n) => Field::Number(n as f64),
        None => Field::Null,
    }
}

fn time_or_null(value: Option<DateTime<Utc>>) -> Field<'static> {
    match value {
        Some(t) => Field::DateTime(t),
        None => Field::Null,
    }
}

impl RowFields for ServiceRow {
    fn field(&self, column: &str) -> Field<'_> {
        match column {
            "name" => Field::Text(Cow::Borrowed(&self.name)),
            "description" => text_or_null(&self.description),
            "load" => text_or_null(&self.load),
            "active" => text_or_null(&self.active),
            "sub" => text_or_null(&self.sub),
            "enabled" => text_or_null(&self.enabled),
            "since" => time_or_null(self.since),
            "pid" => num_or_null(self.pid),
            "mem" => num_or_null(self.mem),
            "tasks" => num_or_null(self.tasks),
            "cpu" => num_or_null(self.cpu),
            "restarts" => num_or_null(self.restarts),
            "needs_reload" => Field::Bool(self.needs_reload),
            "fragment" => text_or_null(&self.fragment),
            // Unreachable: parse() validated the column set up front.
            _ => Field::Null,
        }
    }
}

impl RowFields for TimerRow {
    fn field(&self, column: &str) -> Field<'_> {
        match column {
            "name" => Field::Text(Cow::Borrowed(&self.name)),
            "description" => text_or_null(&self.description),
            "active" => text_or_null(&self.active),
            "sub" => text_or_null(&self.sub),
            "enabled" => text_or_null(&self.enabled),
            "next" => time_or_null(self.next),
            "last" => time_or_null(self.last),
            "unit" => text_or_null(&self.unit),
            "result" => text_or_null(&self.result),
            "persistent" => Field::Bool(self.persistent),
            _ => Field::Null,
        }
    }
}

impl RowFields for PropertyRow {
    fn field(&self, column: &str) -> Field<'_> {
        match column {
            "name" => Field::Text(Cow::Borrowed(&self.name)),
            "value" => text_or_null(&self.value),
            "interface" => text_or_null(&self.interface),
            _ => Field::Null,
        }
    }
}

impl RowFields for LogRow {
    fn field(&self, column: &str) -> Field<'_> {
        match column {
            "time" => time_or_null(self.time),
            "level" => text_or_null(&self.level),
            "prio" => num_or_null(self.prio),
            "pid" => num_or_null(self.pid),
            "message" => text_or_null(&self.message),
            _ => Field::Null,
        }
    }
}

impl RowFields for UnitFileRow {
    fn field(&self, column: &str) -> Field<'_> {
        match column {
            "name" => Field::Text(Cow::Borrowed(&self.name)),
            "state" => text_or_null(&self.state),
            "path" => text_or_null(&self.path),
            "vendor" => text_or_null(&self.vendor),
            _ => Field::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(name: &str, active: &str, restarts: Option<u64>) -> ServiceRow {
        ServiceRow {
            name: name.into(),
            active: active.into(),
            restarts,
            fragment: format!("/usr/lib/systemd/user/{name}"),
            ..Default::default()
        }
    }

    fn matches(raw: &str, row: &ServiceRow) -> bool {
        UnitQuery::parse(raw, SERVICE_COLUMNS)
            .unwrap()
            .matches(row)
    }

    #[test]
    fn the_failed_query_selects_only_failed_units() {
        let ok = service("backup-photos.service", "active", None);
        let bad = service("backup-photos.service", "failed", None);
        assert!(!matches("query:\n  [active, =, failed]", &ok));
        assert!(matches("query:\n  [active, =, failed]", &bad));
    }

    #[test]
    fn a_numeric_column_compares_numerically() {
        let flapping = service("backup-photos.service", "active", Some(12));
        let calm = service("backup-photos.service", "active", Some(0));
        assert!(matches("query:\n  [restarts, gt, 0]", &flapping));
        assert!(!matches("query:\n  [restarts, gt, 0]", &calm));
    }

    #[test]
    fn an_absent_number_is_null_not_zero() {
        let unknown = service("backup-photos.service", "active", None);
        assert!(matches("query:\n  [restarts, is_null]", &unknown));
        assert!(!matches("query:\n  [restarts, gte, 0]", &unknown));
    }

    #[test]
    fn my_units_are_the_ones_under_a_home_directory() {
        let mut mine = service("backup-photos.service", "active", None);
        mine.fragment = "/home/someone/.config/systemd/user/backup-photos.service".into();
        let theirs = service("dbus.service", "active", None);
        let q = "query:\n  [fragment, like, '%/.config/systemd/%']";
        assert!(matches(q, &mine));
        assert!(!matches(q, &theirs));
    }

    #[test]
    fn a_column_of_another_level_is_a_named_mistake() {
        let err = UnitQuery::parse("query:\n  [mem, gt, 0]", UNIT_FILE_COLUMNS).unwrap_err();
        assert!(err.contains("mem"), "error should name the column: {err}");
    }

    #[test]
    fn no_query_means_no_filtering() {
        let rows = vec![service("a.service", "active", None)];
        let none = UnitQuery::compile(None, SERVICE_COLUMNS).unwrap();
        let blank = UnitQuery::compile(Some("  "), SERVICE_COLUMNS).unwrap();
        assert!(none.is_none() && blank.is_none());
        assert_eq!(retain(rows, &none).len(), 1);
    }

    #[test]
    fn a_natural_language_date_bound_resolves() {
        let q = UnitQuery::parse("query:\n  [since, gte, \"today\"]", SERVICE_COLUMNS);
        assert!(q.is_ok(), "{q:?}");
        let bad = UnitQuery::parse("query:\n  [since, gte, \"next blorpday\"]", SERVICE_COLUMNS);
        assert!(bad.is_err());
    }
}
