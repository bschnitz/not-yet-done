use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, TimeZone, Timelike, Utc};

use crate::local_context::start_of_local_day;

#[derive(Clone, Debug, PartialEq)]
pub enum Granularity {
    Month,
    Day,
    Hour,
    Minute,
}

impl Granularity {
    /// Derive granularity from the original input string.
    pub fn from_original(s: &str) -> Self {
        let s = s.trim().to_lowercase();
        // Explicit time with minutes: "2026-03-22 09:15", "09:15", "9:15am"
        if regex::Regex::new(r"\d{1,2}:\d{2}").unwrap().is_match(&s) {
            return Granularity::Minute;
        }
        // Explicit hour only: "9am", "10pm", "today 9am", "yesterday 10pm"
        if regex::Regex::new(r"\d{1,2}\s*(am|pm)")
            .unwrap()
            .is_match(&s)
        {
            return Granularity::Hour;
        }
        // Month names without day: "march", "next april", "last january"
        let months = [
            "january",
            "february",
            "march",
            "april",
            "may",
            "june",
            "july",
            "august",
            "september",
            "october",
            "november",
            "december",
        ];
        let has_month = months.iter().any(|m| s.contains(m));
        let has_day_number = regex::Regex::new(r"\b\d{1,2}\b").unwrap().is_match(&s);
        if has_month && !has_day_number {
            return Granularity::Month;
        }
        // Default: Day
        Granularity::Day
    }

    /// Snap to the start of the granularity period in the user's local timezone.
    ///
    /// Day and month periods begin at the local midnight of their own date;
    /// hours and minutes are cut in the offset the instant itself has, so an
    /// hour repeated by a DST change still snaps within itself.
    pub fn snap_start<Tz: TimeZone>(&self, dt: DateTime<Utc>, tz: &Tz) -> DateTime<Utc> {
        let local = dt.with_timezone(tz);
        match self {
            Granularity::Month => start_of_local_day(tz, first_of_month(local.year(), local.month())),
            Granularity::Day => start_of_local_day(tz, local.date_naive()),
            Granularity::Hour => cut_in_own_offset(dt, tz, |l| (l.hour(), 0, 0)),
            Granularity::Minute => cut_in_own_offset(dt, tz, |l| (l.hour(), l.minute(), 0)),
        }
    }

    /// Snap to the end of the granularity period in the user's local timezone:
    /// its last second, one second before the next period begins.
    pub fn snap_end<Tz: TimeZone>(&self, dt: DateTime<Utc>, tz: &Tz) -> DateTime<Utc> {
        let local = dt.with_timezone(tz);
        match self {
            Granularity::Month => {
                let (year, month) = if local.month() == 12 {
                    (local.year() + 1, 1)
                } else {
                    (local.year(), local.month() + 1)
                };
                start_of_local_day(tz, first_of_month(year, month)) - Duration::seconds(1)
            }
            Granularity::Day => {
                let next = local.date_naive().succ_opt().expect("date in range");
                start_of_local_day(tz, next) - Duration::seconds(1)
            }
            Granularity::Hour => cut_in_own_offset(dt, tz, |l| (l.hour(), 59, 59)),
            Granularity::Minute => cut_in_own_offset(dt, tz, |l| (l.hour(), l.minute(), 59)),
        }
    }
}

fn first_of_month(year: i32, month: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, 1).expect("valid month")
}

/// `dt` with its local time replaced by `hms(local time)`, read and rebuilt in
/// the offset `dt` has in `tz` — which never is ambiguous.
fn cut_in_own_offset<Tz: TimeZone>(
    dt: DateTime<Utc>,
    tz: &Tz,
    hms: impl Fn(&DateTime<FixedOffset>) -> (u32, u32, u32),
) -> DateTime<Utc> {
    let local = dt.with_timezone(tz).fixed_offset();
    let (h, m, s) = hms(&local);
    local
        .timezone()
        .with_ymd_and_hms(local.year(), local.month(), local.day(), h, m, s)
        .single()
        .expect("a fixed offset has every local time once")
        .to_utc()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::Berlin;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().to_utc()
    }

    #[test]
    fn a_summer_day_snaps_to_its_own_midnight() {
        let noon = utc("2026-07-01T10:00:00Z");
        assert_eq!(Granularity::Day.snap_start(noon, &Berlin), utc("2026-06-30T22:00:00Z"));
        assert_eq!(Granularity::Day.snap_end(noon, &Berlin), utc("2026-07-01T21:59:59Z"));
    }

    #[test]
    fn the_day_the_clocks_go_back_is_25_hours_long() {
        let noon = utc("2026-10-25T11:00:00Z");
        assert_eq!(Granularity::Day.snap_start(noon, &Berlin), utc("2026-10-24T22:00:00Z"));
        assert_eq!(Granularity::Day.snap_end(noon, &Berlin), utc("2026-10-25T22:59:59Z"));
    }

    #[test]
    fn a_month_across_a_dst_change_ends_at_its_own_midnight() {
        let november = utc("2026-11-15T12:00:00Z");
        assert_eq!(Granularity::Month.snap_start(november, &Berlin), utc("2026-10-31T23:00:00Z"));
        let october = utc("2026-10-15T12:00:00Z");
        assert_eq!(Granularity::Month.snap_start(october, &Berlin), utc("2026-09-30T22:00:00Z"));
        assert_eq!(Granularity::Month.snap_end(october, &Berlin), utc("2026-10-31T22:59:59Z"));
    }

    #[test]
    fn a_repeated_hour_snaps_within_itself() {
        // 02:30 local happens twice on 2026-10-25: first in CEST, then in CET.
        let first = utc("2026-10-25T00:30:00Z");
        let second = utc("2026-10-25T01:30:00Z");
        assert_eq!(Granularity::Hour.snap_start(first, &Berlin), utc("2026-10-25T00:00:00Z"));
        assert_eq!(Granularity::Hour.snap_end(first, &Berlin), utc("2026-10-25T00:59:59Z"));
        assert_eq!(Granularity::Hour.snap_start(second, &Berlin), utc("2026-10-25T01:00:00Z"));
        assert_eq!(Granularity::Minute.snap_end(second, &Berlin), utc("2026-10-25T01:30:59Z"));
    }
}
