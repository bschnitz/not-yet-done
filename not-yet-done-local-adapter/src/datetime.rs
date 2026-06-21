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
//! This mirrors the CLI's `datetime.rs` / `offset.rs` (same chrono-english →
//! dateparser fallback, same offset grammar) so both frontends accept exactly
//! the same inputs now that `track split`/`track move` are adapter actions
//! rather than hardcoded CLI commands.

use chrono::{DateTime, FixedOffset, Local, NaiveTime, Utc};
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

        // 1. chrono-english: relative expressions (yesterday, next friday 8pm,
        //    today 9am, …)
        if let Ok(dt) =
            chrono_english::parse_date_string(s, Local::now(), chrono_english::Dialect::Us)
        {
            return Ok(LocalDateTime {
                utc: dt.with_timezone(&Utc),
                timezone,
                original: s.to_string(),
            });
        }

        // 2. dateparser fallback: broad absolute formats (RFC3339, unix
        //    timestamps, "2026-03-22 09:15", "6:15pm", …). Local default
        //    timezone, midnight as default time for date-only strings.
        dateparser::parse_with(s, &Local, NaiveTime::from_hms_opt(0, 0, 0).unwrap())
            .map(|utc| LocalDateTime {
                utc,
                timezone,
                original: s.to_string(),
            })
            .map_err(|_| {
                format!(
                    "Cannot parse '{}' as a date/time. \
                     Accepted formats include: '2026-03-22', '2026-03-22 09:15', \
                     'yesterday', 'today 9am', 'next friday 8pm'",
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
        use chrono::Duration;

        let s = s.trim();

        let (sign, rest) = if let Some(r) = s.strip_prefix('+') {
            (1i64, r)
        } else if let Some(r) = s.strip_prefix('-') {
            (-1i64, r)
        } else {
            return Err(format!(
                "Invalid offset '{}': must start with '+' or '-' (e.g. +1h, -30min, +2days)",
                s
            ));
        };

        let split = rest
            .find(|c: char| c.is_alphabetic())
            .ok_or_else(|| format!("Invalid offset '{}': missing unit", s))?;
        let (num_str, unit) = rest.split_at(split);

        let num: i64 = num_str
            .parse()
            .map_err(|_| format!("Invalid offset '{}': '{}' is not a number", s, num_str))?;

        let duration = match unit.to_lowercase().as_str() {
            "s" | "sec" | "secs" | "second" | "seconds" => Duration::seconds(sign * num),
            "m" | "min" | "mins" | "minute" | "minutes" => Duration::minutes(sign * num),
            "h" | "hr" | "hrs" | "hour" | "hours" => Duration::hours(sign * num),
            "d" | "day" | "days" => Duration::days(sign * num),
            "w" | "week" | "weeks" => Duration::weeks(sign * num),
            other => {
                return Err(format!("Unknown time unit '{}'. Use: s, min, h, d, w", other));
            }
        };

        Ok(LocalOffset { duration })
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
