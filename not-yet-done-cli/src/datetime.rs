use chrono::{DateTime, FixedOffset, Local, NaiveTime, Utc};
use not_yet_done_core::local_context::LocalContext;

/// A parsed datetime, always stored as UTC internally.
/// User input without explicit timezone is interpreted as local time.
/// Carries the user's local UTC offset so services can compute day boundaries
/// correctly.
#[derive(Clone)]
pub struct LocalDateTime {
    pub utc: DateTime<Utc>,
    /// The user's local UTC offset at parse time.
    pub timezone: FixedOffset,
    /// The original input string, used for Granularity::from_original().
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

        // 1. chrono-english: relative expressions (yesterday, next friday 8pm, today 9am, ...)
        if let Ok(dt) = chrono_english::parse_date_string(
            s,
            Local::now(),
            chrono_english::Dialect::Us,
        ) {
            return Ok(LocalDateTime {
                utc: dt.with_timezone(&Utc),
                timezone,
                original: s.to_string(),
            });
        }

        // 2. dateparser fallback: broad absolute formats (RFC3339, unix timestamps,
        //    "2026-03-22 09:15", "6:15pm", ...)
        //    Uses Local as default timezone, midnight as default time for date-only strings.
        dateparser::parse_with(s, &Local, NaiveTime::from_hms_opt(0, 0, 0).unwrap())
            .map(|utc| LocalDateTime {
                utc,
                timezone,
                original: s.to_string(),
            })
            .map_err(|_| format!(
                "Cannot parse '{}' as a date/time. \
                 Accepted formats include: '2026-03-22', '2026-03-22 09:15', \
                 'yesterday', 'today 9am', 'next friday 8pm'",
                s
            ))
    }
}
