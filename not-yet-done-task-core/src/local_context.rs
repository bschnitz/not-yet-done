use chrono::{DateTime, FixedOffset, Utc};

/// Carries a UTC timestamp together with the user's local UTC offset at the
/// time of input.  The offset is used by services that need to compute
/// day/month boundaries in the user's local time (e.g. `summary`,
/// `move_tracking` with gravity).
///
/// `original` is intentionally absent here — it is a CLI concern used only
/// for `Granularity::from_original` and stays in the CLI's `LocalDateTime`.
#[derive(Clone, Copy, Debug)]
pub struct LocalContext {
    pub utc: DateTime<Utc>,
    pub timezone: FixedOffset,
}

impl LocalContext {
    pub fn new(utc: DateTime<Utc>, timezone: FixedOffset) -> Self {
        Self { utc, timezone }
    }

    /// Convert the stored UTC timestamp to local time using the stored offset.
    pub fn to_local(&self) -> DateTime<FixedOffset> {
        self.utc.with_timezone(&self.timezone)
    }
}
