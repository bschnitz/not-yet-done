//! Typed grouping / aggregation extraction (plan mechanism M3).
//!
//! The generic partition + totalling mechanism lives in the
//! framework-agnostic [`not_yet_done_table::group`]. This module supplies the
//! *typed* half that belongs with the view config: turning a column's
//! canonical value into a **group label** (verbatim or date-bucketed) and
//! into the **integer** a `sum` aggregate consumes. They are pure
//! `&str → String` / `&str → Option<i64>` functions, mirroring
//! [`crate::views::column_format`], so they unit-test without any UI or data
//! model in the way. The content view does the `NodeSummary → raw value`
//! lookup and feeds the raw strings in here.

use chrono::{DateTime, Datelike, Local, NaiveDate};

use crate::config::view_config::{AggregateOp, DateBucket};

/// Group label for a column's canonical `raw` value under an optional date
/// `bucket`.
///
/// - No bucket → the value groups verbatim (so `kind: text` columns like a
///   category group by their string).
/// - A bucket → `raw` is parsed as an RFC 3339 instant and reduced to an
///   ISO-sortable bucket label (see [`bucket_label`]); lexical order over
///   the labels is therefore chronological.
///
/// A value that fails to parse falls back to grouping **verbatim** (under its
/// raw string) rather than collapsing all bad rows into one bogus bucket —
/// the same "malformed data stays visible" stance as `column_format`.
pub fn group_label(raw: &str, bucket: Option<DateBucket>) -> String {
    match bucket {
        None => raw.to_string(),
        Some(b) => match DateTime::parse_from_rfc3339(raw.trim()) {
            Ok(dt) => bucket_label(dt.with_timezone(&Local), b),
            Err(_) => raw.to_string(),
        },
    }
}

/// Reduce a local instant to its bucket label. Labels are ISO-formatted so
/// lexical ordering equals chronological ordering — the content view groups
/// in label order, so this is what makes the buckets come out in time order:
///
/// - `Day`   → `2026-06-09`
/// - `Week`  → `2026-W23` (ISO week-year + ISO week number)
/// - `Month` → `2026-06`
/// - `Year`  → `2026`
fn bucket_label(dt: DateTime<Local>, bucket: DateBucket) -> String {
    match bucket {
        DateBucket::Day => dt.format("%Y-%m-%d").to_string(),
        DateBucket::Week => {
            let iso = dt.iso_week();
            format!("{}-W{:02}", iso.year(), iso.week())
        }
        DateBucket::Month => dt.format("%Y-%m").to_string(),
        DateBucket::Year => dt.format("%Y").to_string(),
    }
}

/// Human-facing header text for a bucket's ISO group key. The ISO label from
/// [`group_label`] stays the group's *identity and sort key* (lexical =
/// chronological — the invariant the content view orders by); this is a pure
/// display mapping applied only when rendering the header:
///
/// - `Day`  `2026-06-08` → `W24 2026-06-08 Mon` (ISO week + weekday, like the
///   native trackings view's day headers)
/// - `Week` `2026-W23`   → `W23 2026`
/// - `Month` / `Year` / verbatim (unbucketed or unparseable) keys pass through
///   unchanged.
pub fn bucket_display_label(key: &str, bucket: Option<DateBucket>) -> String {
    match bucket {
        Some(DateBucket::Day) => match NaiveDate::parse_from_str(key, "%Y-%m-%d") {
            Ok(date) => {
                let iso = date.iso_week();
                format!("W{:02} {} {}", iso.week(), key, date.format("%a"))
            }
            Err(_) => key.to_string(),
        },
        Some(DateBucket::Week) => match key.split_once("-W") {
            Some((year, week)) if !year.is_empty() && !week.is_empty() => {
                format!("W{week} {year}")
            }
            _ => key.to_string(),
        },
        _ => key.to_string(),
    }
}

/// The integer an aggregate consumes from a column's canonical `raw` value.
///
/// For [`AggregateOp::Sum`] the canonical `duration` encoding (integer
/// seconds) is parsed; empty or unparseable values contribute nothing
/// (`None`), matching how `column_format::format_duration_secs` leaves bad
/// data untouched. The framework-agnostic grouping treats `None` as a
/// zero contribution.
pub fn agg_value(raw: &str, op: AggregateOp) -> Option<i64> {
    match op {
        AggregateOp::Sum => raw.trim().parse::<i64>().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn no_bucket_groups_verbatim() {
        assert_eq!(group_label("Frontend", None), "Frontend");
        assert_eq!(group_label("", None), "");
    }

    /// Build an RFC 3339 instant at local noon on the given date, in `Local`'s
    /// own offset, so the bucket boundary is timezone-stable (a ±14h shift
    /// never crosses midday off its day).
    fn local_noon(date: &str) -> String {
        let offset = Local::now().offset().to_string();
        format!("{date}T12:00:00{offset}")
    }

    #[test]
    fn day_bucket_is_iso_date() {
        assert_eq!(
            group_label(&local_noon("2026-06-09"), Some(DateBucket::Day)),
            "2026-06-09"
        );
    }

    #[test]
    fn month_and_year_buckets() {
        assert_eq!(
            group_label(&local_noon("2026-06-09"), Some(DateBucket::Month)),
            "2026-06"
        );
        assert_eq!(
            group_label(&local_noon("2026-06-09"), Some(DateBucket::Year)),
            "2026"
        );
    }

    #[test]
    fn week_bucket_is_iso_week() {
        // 2026-01-05 is a Monday; ISO week 1 of 2026 is the week containing
        // Thu 2026-01-01 (Mon 2025-12-29 .. Sun 2026-01-04), so Jan 5 starts
        // ISO week 2.
        assert_eq!(
            group_label(&local_noon("2026-01-05"), Some(DateBucket::Week)),
            "2026-W02"
        );
    }

    #[test]
    fn bucket_labels_sort_chronologically() {
        // Lexical order over day labels must match time order — this is the
        // property the content view relies on to order groups.
        let mut labels = [
            group_label(&local_noon("2026-06-09"), Some(DateBucket::Day)),
            group_label(&local_noon("2026-01-05"), Some(DateBucket::Day)),
            group_label(&local_noon("2026-12-31"), Some(DateBucket::Day)),
        ];
        labels.sort();
        assert_eq!(labels, ["2026-01-05", "2026-06-09", "2026-12-31"]);
    }

    #[test]
    fn unparseable_with_bucket_falls_back_to_verbatim() {
        assert_eq!(
            group_label("not-a-date", Some(DateBucket::Week)),
            "not-a-date"
        );
    }

    #[test]
    fn day_display_label_adds_iso_week_and_weekday() {
        // 2026-06-08 is a Monday in ISO week 24.
        assert_eq!(
            bucket_display_label("2026-06-08", Some(DateBucket::Day)),
            "W24 2026-06-08 Mon"
        );
    }

    #[test]
    fn week_display_label_reorders_week_and_year() {
        assert_eq!(
            bucket_display_label("2026-W23", Some(DateBucket::Week)),
            "W23 2026"
        );
    }

    #[test]
    fn other_display_labels_pass_through() {
        assert_eq!(
            bucket_display_label("2026-06", Some(DateBucket::Month)),
            "2026-06"
        );
        assert_eq!(bucket_display_label("2026", Some(DateBucket::Year)), "2026");
        // Verbatim fallback key (unparseable date) stays untouched.
        assert_eq!(
            bucket_display_label("not-a-date", Some(DateBucket::Day)),
            "not-a-date"
        );
        assert_eq!(bucket_display_label("Frontend", None), "Frontend");
    }

    #[test]
    fn sum_parses_canonical_seconds() {
        assert_eq!(agg_value("5400", AggregateOp::Sum), Some(5400));
        assert_eq!(agg_value("  90 ", AggregateOp::Sum), Some(90));
    }

    #[test]
    fn sum_ignores_empty_and_garbage() {
        assert_eq!(agg_value("", AggregateOp::Sum), None);
        assert_eq!(agg_value("1:30:00", AggregateOp::Sum), None);
    }

    /// Guards the contract relied on by the engine layer: the duration a
    /// `kind: elapsed`/`duration` column stores is plain seconds, so summing
    /// raw values then formatting equals formatting the summed span.
    #[test]
    fn summed_seconds_are_just_addition() {
        let a = agg_value("3600", AggregateOp::Sum).unwrap();
        let b = agg_value("1800", AggregateOp::Sum).unwrap();
        assert_eq!(Duration::seconds(a + b), Duration::seconds(5400));
    }
}
