//! Human-friendly datetime / offset parsing for the trackings adapter's
//! `split` / `move` actions.
//!
//! Those actions receive their time arguments as plain `InputSpec::Form`
//! strings (whatever the user typed in the TUI form or passed via the CLI's
//! `--field at=…`), so the adapter — not the frontend — owns turning a string
//! like `"yesterday 9am"` or `"2026-03-22 09:15"` into a
//! [`LocalContext`](not_yet_done_task_core::local_context::LocalContext), and
//! `"+1h"` / `"-30min"` into a [`chrono::Duration`].
//!
//! The parsing grammar itself lives in the shared, app-agnostic [`natural_date`]
//! crate (period boundaries, `in X`, part-of-day, abbreviations, chrono-english,
//! dateparser fallback, the signed-offset grammar); this module only keeps the
//! app-specific wrapper (`original` for `Granularity`, the local UTC offset for
//! day-boundary math, the `LocalContext` conversion).

use chrono::{DateTime, Datelike, FixedOffset, Local, NaiveDate, NaiveTime, TimeZone, Utc};
use not_yet_done_task_core::entity::granularity::Granularity;
use not_yet_done_task_core::local_context::LocalContext;

/// A parsed datetime, always stored as UTC internally.
///
/// User input without an explicit timezone is interpreted as local time. The
/// user's local UTC offset at parse time is carried so the service can compute
/// day boundaries correctly, and the [`original`](LocalDateTime::original)
/// string is kept so the `move` action can derive a
/// [`Granularity`](not_yet_done_task_core::entity::granularity::Granularity)
/// from how precisely the user expressed the time.
#[derive(Clone)]
pub struct LocalDateTime {
    pub utc: DateTime<Utc>,
    /// The user's local UTC offset at parse time.
    pub timezone: FixedOffset,
    /// The original input string, used for `Granularity::from_original()`.
    pub original: String,
}

impl LocalDateTime {
    fn current_offset() -> FixedOffset {
        *Local::now().offset()
    }
}

impl From<LocalDateTime> for LocalContext {
    fn from(dt: LocalDateTime) -> Self {
        LocalContext::new(dt.utc, dt.timezone)
    }
}

impl std::str::FromStr for LocalDateTime {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let timezone = Self::current_offset();
        natural_date::resolve_datetime(s, Local::now())
            .map(|utc| LocalDateTime {
                utc,
                timezone,
                original: s.to_string(),
            })
            .ok_or_else(|| {
                format!(
                    "Cannot parse '{}' as a date/time. \
                     Accepted formats include: '2026-03-22', '2026-03-22 09:15', \
                     'yesterday', 'today 9am', 'next friday 8pm', 'in 2 hours', 'eod'",
                    s
                )
            })
    }
}

/// A signed time offset (e.g. `+1h`, `-30min`, `+2days`) applied after a
/// `move`'s gravity snap.
#[derive(Clone)]
pub struct LocalOffset {
    pub duration: chrono::Duration,
}

impl std::str::FromStr for LocalOffset {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        natural_date::resolve_offset(s)
            .map(|duration| LocalOffset { duration })
            .ok_or_else(|| {
                format!(
                    "Invalid offset '{}': must start with '+' or '-' and a unit \
                     (e.g. +1h, -30min, +2days)",
                    s.trim()
                )
            })
    }
}

/// Which side of a task's time window a bound sits on. Decides where a bound
/// given only to the day (or month) lands: a window *opens* at the first
/// second of its day and *closes* at the last one, so `due: 2026-10-15` still
/// leaves the whole 15th to finish in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowEdge {
    Start,
    End,
}

impl WindowEdge {
    /// The local time of day a whole-day bound on this edge is stored at.
    fn day_time(self) -> NaiveTime {
        match self {
            WindowEdge::Start => NaiveTime::MIN,
            WindowEdge::End => NaiveTime::from_hms_opt(23, 59, 59).expect("valid time"),
        }
    }
}

/// Parse one bound of a task's time window (`not_before` / `due`).
///
/// Accepts everything [`LocalDateTime`] does. A phrase that names a time
/// (`friday 17:00`, `in 2 hours`) is taken exactly; one that names only a day
/// (`2026-10-15`, `tomorrow`, `next friday`) or a month (`november`) is
/// widened to the start or end of that day / month, per `edge`.
///
/// The widening uses the local offset *of that date*, not today's, so a
/// deadline across a DST switch still closes at local midnight.
pub fn parse_window_bound(s: &str, edge: WindowEdge) -> Result<DateTime<Utc>, String> {
    let parsed: LocalDateTime = s.parse()?;
    let local = parsed.utc.with_timezone(&Local);
    // A day phrase resolves to local midnight; anything else carried a time
    // of its own, whatever `Granularity` guesses from the wording.
    if local.time() != NaiveTime::MIN {
        return Ok(parsed.utc);
    }
    let date = match (Granularity::from_original(s), edge) {
        (Granularity::Month, WindowEdge::Start) => local.date_naive().with_day(1),
        (Granularity::Month, WindowEdge::End) => last_day_of_month(local.date_naive()),
        _ => Some(local.date_naive()),
    }
    .ok_or_else(|| format!("Cannot place '{s}' in the calendar"))?;
    Local
        .from_local_datetime(&date.and_time(edge.day_time()))
        .earliest()
        .map(|d| d.with_timezone(&Utc))
        .ok_or_else(|| format!("'{s}' does not exist in the local timezone"))
}

/// Render a stored window bound the way [`parse_window_bound`] reads it back
/// unchanged: a whole-day bound as `YYYY-MM-DD`, anything else with its time.
pub fn fmt_window_bound(dt: DateTime<Utc>, edge: WindowEdge) -> String {
    let local = dt.with_timezone(&Local);
    if local.time() == edge.day_time() {
        local.format("%Y-%m-%d").to_string()
    } else {
        local.format("%Y-%m-%d %H:%M").to_string()
    }
}

fn last_day_of_month(date: NaiveDate) -> Option<NaiveDate> {
    let (year, month) = if date.month() == 12 {
        (date.year() + 1, 1)
    } else {
        (date.year(), date.month() + 1)
    };
    NaiveDate::from_ymd_opt(year, month, 1)?.pred_opt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn offset_parses_units_and_sign() {
        assert_eq!(
            LocalOffset::from_str("+1h").unwrap().duration,
            chrono::Duration::hours(1)
        );
        assert_eq!(
            LocalOffset::from_str("-30min").unwrap().duration,
            chrono::Duration::minutes(-30)
        );
        assert_eq!(
            LocalOffset::from_str("+2days").unwrap().duration,
            chrono::Duration::days(2)
        );
    }

    #[test]
    fn offset_rejects_missing_sign_and_unit() {
        assert!(LocalOffset::from_str("1h").is_err());
        assert!(LocalOffset::from_str("+5").is_err());
        assert!(LocalOffset::from_str("+5fortnights").is_err());
    }

    #[test]
    fn datetime_parses_absolute_and_keeps_original() {
        let dt = LocalDateTime::from_str("2026-03-22 09:15").unwrap();
        assert_eq!(dt.original, "2026-03-22 09:15");
    }

    fn local(s: &str) -> DateTime<Utc> {
        let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap();
        Local
            .from_local_datetime(&naive)
            .earliest()
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn window_bare_day_spans_the_whole_day() {
        assert_eq!(
            parse_window_bound("2026-10-15", WindowEdge::Start).unwrap(),
            local("2026-10-15 00:00:00")
        );
        assert_eq!(
            parse_window_bound("2026-10-15", WindowEdge::End).unwrap(),
            local("2026-10-15 23:59:59")
        );
    }

    #[test]
    fn window_bound_with_time_is_exact() {
        assert_eq!(
            parse_window_bound("2026-10-15 17:00", WindowEdge::End).unwrap(),
            local("2026-10-15 17:00:00")
        );
        assert_eq!(
            parse_window_bound("2026-10-15 09:30", WindowEdge::Start).unwrap(),
            local("2026-10-15 09:30:00")
        );
    }

    #[test]
    fn window_bound_across_dst_keeps_local_midnight() {
        // Written in summer or winter alike, the stored instant is the local
        // midnight of *that* date.
        assert_eq!(
            parse_window_bound("2026-12-24", WindowEdge::Start).unwrap(),
            local("2026-12-24 00:00:00")
        );
        assert_eq!(
            parse_window_bound("2026-07-01", WindowEdge::End).unwrap(),
            local("2026-07-01 23:59:59")
        );
    }

    #[test]
    fn window_bound_rejects_garbage() {
        assert!(parse_window_bound("whenever", WindowEdge::End).is_err());
    }

    #[test]
    fn window_bound_round_trips_through_its_rendering() {
        for (text, edge) in [
            ("2026-10-15", WindowEdge::Start),
            ("2026-10-15", WindowEdge::End),
            ("2026-10-15 17:00", WindowEdge::End),
            ("2026-10-15 08:15", WindowEdge::Start),
        ] {
            let stored = parse_window_bound(text, edge).unwrap();
            assert_eq!(fmt_window_bound(stored, edge), text);
        }
    }

    #[test]
    fn last_day_of_month_handles_december_and_february() {
        assert_eq!(
            last_day_of_month(NaiveDate::from_ymd_opt(2026, 12, 3).unwrap()),
            NaiveDate::from_ymd_opt(2026, 12, 31)
        );
        assert_eq!(
            last_day_of_month(NaiveDate::from_ymd_opt(2028, 2, 10).unwrap()),
            NaiveDate::from_ymd_opt(2028, 2, 29)
        );
    }

    #[test]
    fn datetime_rejects_garbage() {
        assert!(LocalDateTime::from_str("not a date at all").is_err());
    }
}
