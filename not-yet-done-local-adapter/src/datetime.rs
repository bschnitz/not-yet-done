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

use chrono::{DateTime, FixedOffset, Local, Utc};
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

    #[test]
    fn datetime_rejects_garbage() {
        assert!(LocalDateTime::from_str("not a date at all").is_err());
    }
}
