use chrono::{DateTime, FixedOffset, Local, Utc};
use not_yet_done_task_core::local_context::LocalContext;

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
        // The grammar lives in the shared, app-agnostic `natural-date` crate.
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
