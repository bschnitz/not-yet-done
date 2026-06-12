//! Adapter-side grouping vocabulary (tree grouping, M3 follow-up).
//!
//! Flat lists group **engine-side**: the frontend partitions the loaded rows
//! itself (`group_by` in the view YAML) and the adapter never knows. A *tree*
//! can't be grouped that way — the adapter owns the fold (each bucket's
//! subtree durations must be re-folded from that bucket's rows only), so the
//! engine instead passes the active grouping along in
//! [`ListParams::group_by`](crate::ListParams::group_by) and the adapter
//! returns one bucket node per group as the root level. Adapters that
//! support this advertise
//! [`AdapterCapabilities::group_by_via_adapter`](crate::AdapterCapabilities::group_by_via_adapter).
//!
//! The bucket-key helpers here are the **single source of truth** for what a
//! date bucket's key and display label look like; the frontend's engine-side
//! grouping delegates to them so a day reads identically in a flat grouped
//! list and an adapter-grouped tree.

use chrono::{DateTime, Datelike, Local, NaiveDate};

use crate::SortDirection;

/// Date-bucket granularity for a [`GroupSpec`]. When set, the group column's
/// value is parsed as an RFC 3339 instant and truncated to this boundary so
/// all items in the same day / week / month / year coalesce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupBucket {
    Day,
    Week,
    Month,
    Year,
}

/// The active grouping, as the engine hands it to an adapter in
/// [`ListParams::group_by`](crate::ListParams::group_by). Mirrors the view
/// config's `group_by` block: which column keys the groups, an optional date
/// bucket, and the order of the groups themselves (not of the rows inside).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupSpec {
    /// Column key whose value identifies the group.
    pub column: String,
    /// When set, group by the date bucket the column's RFC 3339 value falls
    /// into instead of the verbatim value.
    pub bucket: Option<GroupBucket>,
    /// Order of the groups. Group keys are ISO-formatted (lexical order =
    /// chronological), so `Desc` puts the newest bucket first.
    pub order: SortDirection,
}

/// Group key for a column's canonical `raw` value under an optional date
/// `bucket`.
///
/// - No bucket → the value groups verbatim.
/// - A bucket → `raw` is parsed as an RFC 3339 instant and reduced to an
///   ISO-sortable bucket key (see [`bucket_key`]); lexical order over the
///   keys is therefore chronological.
///
/// A value that fails to parse falls back to grouping **verbatim** (under
/// its raw string) rather than collapsing all bad rows into one bogus
/// bucket — malformed data stays visible.
pub fn group_key(raw: &str, bucket: Option<GroupBucket>) -> String {
    match bucket {
        None => raw.to_string(),
        Some(b) => match DateTime::parse_from_rfc3339(raw.trim()) {
            Ok(dt) => bucket_key(dt.with_timezone(&Local), b),
            Err(_) => raw.to_string(),
        },
    }
}

/// Reduce a local instant to its bucket key. Keys are ISO-formatted so
/// lexical ordering equals chronological ordering:
///
/// - `Day`   → `2026-06-09`
/// - `Week`  → `2026-W23` (ISO week-year + ISO week number)
/// - `Month` → `2026-06`
/// - `Year`  → `2026`
pub fn bucket_key(dt: DateTime<Local>, bucket: GroupBucket) -> String {
    match bucket {
        GroupBucket::Day => dt.format("%Y-%m-%d").to_string(),
        GroupBucket::Week => {
            let iso = dt.iso_week();
            format!("{}-W{:02}", iso.year(), iso.week())
        }
        GroupBucket::Month => dt.format("%Y-%m").to_string(),
        GroupBucket::Year => dt.format("%Y").to_string(),
    }
}

/// Human-facing label for a bucket's ISO group key. The ISO key stays the
/// group's *identity and sort key*; this is a pure display mapping:
///
/// - `Day`  `2026-06-08` → `W24 2026-06-08 Mon` (ISO week + weekday, like
///   the native trackings view's day headers)
/// - `Week` `2026-W23`   → `W23 2026`
/// - `Month` / `Year` / verbatim (unbucketed or unparseable) keys pass
///   through unchanged.
pub fn bucket_display_label(key: &str, bucket: Option<GroupBucket>) -> String {
    match bucket {
        Some(GroupBucket::Day) => match NaiveDate::parse_from_str(key, "%Y-%m-%d") {
            Ok(date) => {
                let iso = date.iso_week();
                format!("W{:02} {} {}", iso.week(), key, date.format("%a"))
            }
            Err(_) => key.to_string(),
        },
        Some(GroupBucket::Week) => match key.split_once("-W") {
            Some((year, week)) if !year.is_empty() && !week.is_empty() => {
                format!("W{week} {year}")
            }
            _ => key.to_string(),
        },
        _ => key.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An RFC 3339 instant at local noon of `date`, so bucket truncation
    /// never crosses a day boundary regardless of the host timezone.
    fn local_noon(date: &str) -> String {
        let offset = Local::now().format("%:z");
        format!("{date}T12:00:00{offset}")
    }

    #[test]
    fn no_bucket_groups_verbatim() {
        assert_eq!(group_key("alpha", None), "alpha");
    }

    #[test]
    fn day_week_month_year_keys() {
        let raw = local_noon("2026-06-09");
        assert_eq!(group_key(&raw, Some(GroupBucket::Day)), "2026-06-09");
        assert_eq!(group_key(&raw, Some(GroupBucket::Week)), "2026-W24");
        assert_eq!(group_key(&raw, Some(GroupBucket::Month)), "2026-06");
        assert_eq!(group_key(&raw, Some(GroupBucket::Year)), "2026");
    }

    #[test]
    fn unparseable_with_bucket_falls_back_to_verbatim() {
        assert_eq!(
            group_key("not-a-date", Some(GroupBucket::Week)),
            "not-a-date"
        );
    }

    #[test]
    fn day_display_label_adds_iso_week_and_weekday() {
        assert_eq!(
            bucket_display_label("2026-06-08", Some(GroupBucket::Day)),
            "W24 2026-06-08 Mon"
        );
    }

    #[test]
    fn week_display_label_reorders_week_and_year() {
        assert_eq!(
            bucket_display_label("2026-W23", Some(GroupBucket::Week)),
            "W23 2026"
        );
    }

    #[test]
    fn other_display_labels_pass_through() {
        assert_eq!(
            bucket_display_label("2026-06", Some(GroupBucket::Month)),
            "2026-06"
        );
        assert_eq!(bucket_display_label("2026", Some(GroupBucket::Year)), "2026");
        assert_eq!(bucket_display_label("alpha", None), "alpha");
    }
}
