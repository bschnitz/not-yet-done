use chrono::{
    DateTime, FixedOffset, Local, LocalResult, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc,
};

/// Carries a UTC timestamp together with the user's local time zone. The zone
/// is used by services that need to compute day/month boundaries in the
/// user's local time (e.g. `summary`, `move_tracking` with gravity).
///
/// `original` is intentionally absent here — it is a CLI concern used only
/// for `Granularity::from_original` and stays in the CLI's `LocalDateTime`.
#[derive(Clone, Copy, Debug)]
pub struct LocalContext {
    pub utc: DateTime<Utc>,
    pub timezone: LocalZone,
}

impl LocalContext {
    pub fn new(utc: DateTime<Utc>, timezone: impl Into<LocalZone>) -> Self {
        Self {
            utc,
            timezone: timezone.into(),
        }
    }

    /// Convert the stored UTC timestamp to local time in the stored zone.
    pub fn to_local(&self) -> DateTime<FixedOffset> {
        self.utc.with_timezone(&self.timezone).fixed_offset()
    }
}

/// The zone local day and month boundaries are drawn in.
///
/// `System` is the machine's own zone with its DST rules, so every date gets
/// the offset in force on it — not the one of the moment the user typed.
/// `Fixed` keeps one offset all year, for callers (and tests) that have
/// nothing more than an offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalZone {
    System,
    Fixed(FixedOffset),
}

impl From<FixedOffset> for LocalZone {
    fn from(offset: FixedOffset) -> Self {
        LocalZone::Fixed(offset)
    }
}

impl TimeZone for LocalZone {
    type Offset = FixedOffset;

    fn from_offset(offset: &FixedOffset) -> Self {
        LocalZone::Fixed(*offset)
    }

    fn offset_from_local_date(&self, local: &NaiveDate) -> LocalResult<FixedOffset> {
        match self {
            LocalZone::System => Local.offset_from_local_date(local).map(|o| o.fix()),
            LocalZone::Fixed(o) => LocalResult::Single(*o),
        }
    }

    fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> LocalResult<FixedOffset> {
        match self {
            LocalZone::System => Local.offset_from_local_datetime(local).map(|o| o.fix()),
            LocalZone::Fixed(o) => LocalResult::Single(*o),
        }
    }

    fn offset_from_utc_date(&self, utc: &NaiveDate) -> FixedOffset {
        match self {
            LocalZone::System => Local.offset_from_utc_date(utc).fix(),
            LocalZone::Fixed(o) => *o,
        }
    }

    fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> FixedOffset {
        match self {
            LocalZone::System => Local.offset_from_utc_datetime(utc).fix(),
            LocalZone::Fixed(o) => *o,
        }
    }
}

/// The instant `day` begins in `tz`: its local midnight, in the offset in
/// force on that date. Where a zone skips midnight itself, the day begins at
/// the first local time that exists.
pub fn start_of_local_day<Tz: TimeZone>(tz: &Tz, day: NaiveDate) -> DateTime<Utc> {
    let midnight = day.and_hms_opt(0, 0, 0).expect("midnight is a valid time");
    (0..=2)
        .find_map(|h| {
            tz.from_local_datetime(&(midnight + chrono::Duration::hours(h)))
                .earliest()
        })
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(|| midnight.and_utc())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_across_a_dst_change_starts_at_its_own_midnight() {
        let berlin = chrono_tz::Europe::Berlin;
        let day = |m, d| NaiveDate::from_ymd_opt(2026, m, d).unwrap();
        // 25 October: CEST until 03:00, so midnight is 22:00 UTC the day before.
        assert_eq!(
            start_of_local_day(&berlin, day(10, 25)),
            Utc.with_ymd_and_hms(2026, 10, 24, 22, 0, 0).unwrap()
        );
        // The next midnight is already CET: the day was 25 hours long.
        assert_eq!(
            start_of_local_day(&berlin, day(10, 26)),
            Utc.with_ymd_and_hms(2026, 10, 25, 23, 0, 0).unwrap()
        );
    }

    #[test]
    fn a_fixed_zone_keeps_its_offset_all_year() {
        let plus_one = LocalZone::from(FixedOffset::east_opt(3600).unwrap());
        let day = NaiveDate::from_ymd_opt(2026, 7, 1).unwrap();
        assert_eq!(
            start_of_local_day(&plus_one, day),
            Utc.with_ymd_and_hms(2026, 6, 30, 23, 0, 0).unwrap()
        );
    }
}
