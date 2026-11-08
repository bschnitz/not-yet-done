use chrono::{DateTime, Local, Utc};
use not_yet_done_task_core::local_context::{LocalContext, LocalZone};

/// A parsed datetime, always stored as UTC internally.
/// User input without explicit timezone is interpreted as local time.
/// Day boundaries are computed in the system time zone.
#[derive(Clone)]
pub struct LocalDateTime {
    pub utc: DateTime<Utc>,
    /// The original input string, used for Granularity::from_original().
    pub original: String,
}

impl From<LocalDateTime> for LocalContext {
    fn from(dt: LocalDateTime) -> Self {
        LocalContext::new(dt.utc, LocalZone::System)
    }
}

impl std::str::FromStr for LocalDateTime {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // The grammar lives in the shared, app-agnostic `natural-date` crate.
        natural_date::resolve_datetime(s, Local::now())
            .map(|utc| LocalDateTime {
                utc,
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
