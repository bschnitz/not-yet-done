//! In-memory query filtering for calendar events.
//!
//! The trackings tab filters through the database (`find_filtered` → a SeaORM
//! `Condition`); the calendar adapter instead holds its merged events in
//! memory, so there is nothing to push a `WHERE` clause down to. This module
//! evaluates the very same [`FilterExpr`] DSL directly against each
//! [`CalEvent`].
//!
//! The query body is the identical YAML shape the trackings query uses — a
//! mapping with a `query:` key holding a `FilterExpr` — parsed by the shared
//! [`query_filter::parse`], so **natural-language dates** on the right-hand
//! side (`[start, gte, "next monday"]`, `[end, lt, "in 3 days"]`) are resolved
//! to RFC3339 before we ever see them, exactly as on the trackings tab. Title
//! search is a plain `[title, has, "standup"]` (case-insensitive substring).
//!
//! Columns map onto the event fields the row projection also exposes:
//!
//! | Column                        | Field              | Kind      |
//! |-------------------------------|--------------------|-----------|
//! | `title` / `subject`           | `CalEvent::title`  | text      |
//! | `start`                       | `CalEvent::start`  | datetime  |
//! | `end`                         | `CalEvent::end`    | datetime  |
//! | `account` / `calendar`        | connection label   | text      |
//! | `location`                    | `CalEvent::location` (nullable) | text |
//! | `organizer`                   | `CalEvent::organizer` (nullable) | text |
//! | `show_as` / `status`          | `CalEvent::show_as`| text      |
//! | `body`                        | `CalEvent::body` (nullable) | text |
//! | `all_day`                     | `CalEvent::all_day`| bool      |
//!
//! The query only *narrows the loaded time window* — it is applied to the
//! events the adapter has already fetched (`window_past_days` /
//! `window_future_days`), not pushed to the backend. A date bound outside that
//! window therefore matches nothing until the window is widened.

use std::borrow::Cow;

use chrono::{DateTime, Utc};
use not_yet_done_calendar_core::CalEvent;
use not_yet_done_filter::eval::{self, Field, RowFields};
use not_yet_done_filter::{FilterExpr, Literal, Operator, Rhs, query_filter};

/// Every column name a calendar query may reference (all aliases). Used both to
/// resolve a leaf's column and to reject typos at parse time with a helpful
/// message rather than silently matching nothing.
const KNOWN_COLUMNS: &[&str] = &[
    "title",
    "subject",
    "start",
    "end",
    "account",
    "calendar",
    "location",
    "organizer",
    "show_as",
    "status",
    "body",
    "all_day",
];

/// A compiled calendar query: a parsed, date-resolved filter expression.
#[derive(Debug)]
pub(crate) struct CalendarQuery {
    expr: FilterExpr,
}

impl CalendarQuery {
    /// Parse a raw query body. Returns an error string suitable for the status
    /// bar on malformed YAML, a missing `query:` key, or an unknown column.
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        let parsed = query_filter::parse(raw).map_err(|e| e.to_string())?;
        validate(&parsed.expr)?;
        Ok(Self { expr: parsed.expr })
    }

    /// Whether `event` (surfaced under the `account` label) matches the query.
    pub(crate) fn matches(&self, event: &CalEvent, account: &str) -> bool {
        eval::matches(&self.expr, &EventRow { event, account })
    }

    /// The instant bounds this query implies over the `start`/`end` columns, as
    /// `(lower, upper)` — used to size the load window (a query that reaches
    /// only into next week must not make the backend page whole months ahead;
    /// one reaching into next year must). Walks every leaf comparing a datetime
    /// column against a resolved RFC3339 literal and returns the **min lower**
    /// and **max upper** it can prove, so an OR of ranges widens to cover all
    /// its branches. `None` on a side means the query places no bound there —
    /// the caller falls back to its configured window for that side.
    ///
    /// Only literal date comparisons contribute; a bound hidden behind a `Not`
    /// cannot be turned into a covering window, so a negated subtree is skipped
    /// (its events, if any, are caught by the configured window fallback).
    pub(crate) fn date_bounds(&self) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
        let (mut lo, mut hi) = (None, None);
        collect_bounds(&self.expr, &mut lo, &mut hi);
        (lo, hi)
    }
}

/// Accumulate the min-lower / max-upper datetime bounds a query proves. See
/// [`CalendarQuery::date_bounds`]. `Eq` bounds both sides; `Gt`/`Gte` a lower;
/// `Lt`/`Lte` an upper. Negated subtrees are skipped (a `Not` cannot be widened
/// into a covering window).
fn collect_bounds(
    expr: &FilterExpr,
    lo: &mut Option<DateTime<Utc>>,
    hi: &mut Option<DateTime<Utc>>,
) {
    match expr {
        FilterExpr::And(children) | FilterExpr::Or(children) => {
            children.iter().for_each(|c| collect_bounds(c, lo, hi))
        }
        FilterExpr::Not(_) => {}
        FilterExpr::Leaf(leaf) => {
            if !DATETIME_COLUMNS.contains(&leaf.lhs.column.as_str()) {
                return;
            }
            let Rhs::Lit(Literal::String(s)) = &leaf.rhs else {
                return;
            };
            let Ok(dt) = DateTime::parse_from_rfc3339(s) else {
                return;
            };
            let dt = dt.with_timezone(&Utc);
            if matches!(leaf.op, Operator::Gt | Operator::Gte | Operator::Eq) {
                *lo = Some(lo.map_or(dt, |cur: DateTime<Utc>| cur.min(dt)));
            }
            if matches!(leaf.op, Operator::Lt | Operator::Lte | Operator::Eq) {
                *hi = Some(hi.map_or(dt, |cur: DateTime<Utc>| cur.max(dt)));
            }
        }
    }
}

/// Columns compared as instants. A comparison against one of these needs an
/// RHS that resolved to a real date; otherwise the query would silently match
/// nothing (see [`validate`]).
const DATETIME_COLUMNS: &[&str] = &["start", "end"];

/// Validate a parsed query up front so mistakes surface as an error message
/// instead of an empty view: unknown columns, and date comparisons whose
/// right-hand side never resolved to a real date. Both checks are the shared
/// ones — the calendar's contribution is only *which* columns exist and which
/// of them are instants.
fn validate(expr: &FilterExpr) -> Result<(), String> {
    eval::validate_columns(expr, KNOWN_COLUMNS, "calendar column")?;
    eval::validate_datetime_literals(expr, DATETIME_COLUMNS)
}

/// A nullable text column: absent means [`Field::Null`], so `is_null` works
/// and every comparison against it is false.
fn text_or_null(value: &Option<String>) -> Field<'_> {
    match value {
        Some(s) => Field::Text(Cow::Borrowed(s.as_str())),
        None => Field::Null,
    }
}

/// A calendar event, viewed as a row of named columns.
///
/// This mapping is the whole calendar-specific part of filtering; operators,
/// null handling and `LIKE` semantics come from the shared evaluator, so a
/// query means the same thing here as everywhere else.
struct EventRow<'a> {
    event: &'a CalEvent,
    account: &'a str,
}

impl RowFields for EventRow<'_> {
    fn field(&self, column: &str) -> Field<'_> {
        let event = self.event;
        match column {
            "title" | "subject" => Field::Text(Cow::Borrowed(event.title.as_str())),
            "start" => Field::DateTime(event.start),
            "end" => Field::DateTime(event.end),
            "account" | "calendar" => Field::Text(Cow::Borrowed(self.account)),
            "location" => text_or_null(&event.location),
            "organizer" => text_or_null(&event.organizer),
            "body" => text_or_null(&event.body),
            "show_as" | "status" => Field::Text(Cow::Borrowed(event.show_as.as_str())),
            "all_day" => Field::Bool(event.all_day),
            // Unreachable: parse() validated the column set up front.
            _ => Field::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use not_yet_done_calendar_core::ShowAs;

    fn event(title: &str, hour: u32) -> CalEvent {
        CalEvent {
            uid: "u".into(),
            calendar: "Work".into(),
            title: title.into(),
            start: Utc.with_ymd_and_hms(2030, 1, 15, hour, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2030, 1, 15, hour + 1, 0, 0).unwrap(),
            all_day: false,
            location: Some("Room 1".into()),
            organizer: Some("Alice".into()),
            show_as: ShowAs::Busy,
            body: None,
            url: None,
        }
    }

    fn matches(query_body: &str, ev: &CalEvent) -> bool {
        CalendarQuery::parse(query_body)
            .unwrap()
            .matches(ev, &ev.calendar)
    }

    #[test]
    fn title_substring_is_case_insensitive() {
        let ev = event("Sprint Planning", 9);
        assert!(matches("query:\n  [title, has, planning]", &ev));
        assert!(matches("query:\n  [title, has, PLAN]", &ev));
        assert!(!matches("query:\n  [title, has, retro]", &ev));
    }

    #[test]
    fn title_like_honours_wildcards() {
        let ev = event("Weekly Standup", 9);
        assert!(matches("query:\n  [title, like, '%standup']", &ev));
        assert!(matches("query:\n  [title, like, 'weekly%']", &ev));
        assert!(!matches("query:\n  [title, like, 'standup%']", &ev));
    }

    #[test]
    fn natural_language_start_bounds_filter() {
        let ev = event("Meeting", 9); // 2030-01-15
        // Everything from 2030-01-01 on matches; anything after 2031 does not.
        assert!(matches("query:\n  [start, gte, 2030-01-01]", &ev));
        assert!(matches("query:\n  [end, lt, 2031-01-01]", &ev));
        assert!(!matches("query:\n  [start, gte, 2030-06-01]", &ev));
    }

    #[test]
    fn combined_and_query() {
        let ev = event("Sprint Planning", 9);
        let q = "query:\n  and:\n    - [title, has, sprint]\n    - [start, gte, 2030-01-01]\n    - [show_as, =, busy]";
        assert!(matches(q, &ev));
        let q_miss = "query:\n  and:\n    - [title, has, sprint]\n    - [show_as, =, free]";
        assert!(!matches(q_miss, &ev));
    }

    #[test]
    fn nullable_column_is_null() {
        let ev = event("X", 9); // body is None, location is Some
        assert!(matches("query:\n  [body, is_null]", &ev));
        assert!(matches("query:\n  [location, is_not_null]", &ev));
        assert!(!matches("query:\n  [location, is_null]", &ev));
    }

    #[test]
    fn account_in_list() {
        let ev = event("X", 9); // calendar/account = "Work"
        assert!(matches("query:\n  [account, in, [Work, Personal]]", &ev));
        assert!(!matches("query:\n  [account, in, [Personal]]", &ev));
    }

    #[test]
    fn unresolved_date_phrase_is_a_parse_error_not_empty_view() {
        // A phrase the resolver cannot make sense of must fail loudly at parse
        // time rather than silently matching nothing.
        let q = "query:\n  [start, lt, \"next blorpday\"]";
        let err = CalendarQuery::parse(q).unwrap_err();
        assert!(
            err.contains("next blorpday"),
            "error should quote the phrase: {err}"
        );
    }

    #[test]
    fn relative_in_phrase_resolves() {
        // Guards the counterpart: `in 2 weeks` *is* resolvable since the
        // `natural-date` consolidation, so it must not be rejected. Keeping
        // both directions asserted stops either from rotting unnoticed.
        assert!(CalendarQuery::parse("query:\n  [start, lt, \"in 2 weeks\"]").is_ok());
    }

    #[test]
    fn dynamic_or_and_range_query_matches() {
        use chrono::Local;
        // Event 3 days out — inside a rolling "today .. 2 weeks" window.
        let start = (Local::now() + chrono::Duration::days(3)).with_timezone(&Utc);
        let mut ev = event("X", 9);
        ev.start = start;
        ev.end = start + chrono::Duration::hours(1);

        let q = "query:\n  or:\n    - and:\n        - [start, gte, \"today\"]\n        - [start, lt, \"2 weeks\"]\n    - and:\n        - [end, gte, \"today\"]\n        - [end, lt, \"2 weeks\"]\n";
        assert!(matches(q, &ev), "rolling OR/AND range query should match");
    }

    #[test]
    fn unknown_column_is_rejected() {
        let err = CalendarQuery::parse("query:\n  [colour, =, blue]").unwrap_err();
        assert!(err.contains("unknown calendar column 'colour'"));
    }

    /// Bare `YYYY-MM-DD` literals resolve to *local* midnight before we ever see
    /// them (via `query_filter::parse`), so expected bounds must be built the
    /// same way — hard-coding a UTC instant would fail off UTC.
    fn local_midnight(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        chrono::Local
            .with_ymd_and_hms(y, m, d, 0, 0, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn date_bounds_from_and_range() {
        let q = CalendarQuery::parse(
            "query:\n  and:\n    - [start, gte, 2030-01-01]\n    - [start, lte, 2030-01-31]",
        )
        .unwrap();
        let (lo, hi) = q.date_bounds();
        assert_eq!(lo, Some(local_midnight(2030, 1, 1)));
        assert_eq!(hi, Some(local_midnight(2030, 1, 31)));
    }

    #[test]
    fn date_bounds_or_widens_to_cover_all_branches() {
        // An OR of two ranges must widen to the min lower / max upper across both.
        let q = CalendarQuery::parse(
            "query:\n  or:\n    - [start, gte, 2030-03-01]\n    - [end, lte, 2030-01-15]",
        )
        .unwrap();
        let (lo, hi) = q.date_bounds();
        assert_eq!(lo, Some(local_midnight(2030, 3, 1)));
        assert_eq!(hi, Some(local_midnight(2030, 1, 15)));
    }

    #[test]
    fn date_bounds_absent_when_no_date_leaf() {
        // A title-only query places no date bound → both sides None (caller
        // falls back to its configured window).
        let q = CalendarQuery::parse("query:\n  [title, has, standup]").unwrap();
        assert_eq!(q.date_bounds(), (None, None));
    }

    #[test]
    fn date_bounds_one_sided() {
        // A lone lower bound leaves the upper open.
        let q = CalendarQuery::parse("query:\n  [start, gte, 2030-06-01]").unwrap();
        let (lo, hi) = q.date_bounds();
        assert_eq!(lo, Some(local_midnight(2030, 6, 1)));
        assert_eq!(hi, None);
    }
}
