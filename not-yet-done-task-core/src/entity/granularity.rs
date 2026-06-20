use chrono::{DateTime, Datelike, Duration, FixedOffset, TimeZone, Timelike, Utc};

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
        if regex::Regex::new(r"\d{1,2}\s*(am|pm)").unwrap().is_match(&s) {
            return Granularity::Hour;
        }
        // Month names without day: "march", "next april", "last january"
        let months = [
            "january", "february", "march", "april", "may", "june",
            "july", "august", "september", "october", "november", "december",
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
    pub fn snap_start(&self, dt: DateTime<Utc>, tz: FixedOffset) -> DateTime<Utc> {
        let local = dt.with_timezone(&tz);
        let snapped_local = match self {
            Granularity::Month => {
                tz.with_ymd_and_hms(local.year(), local.month(), 1, 0, 0, 0).unwrap()
            }
            Granularity::Day => {
                tz.with_ymd_and_hms(local.year(), local.month(), local.day(), 0, 0, 0).unwrap()
            }
            Granularity::Hour => {
                tz.with_ymd_and_hms(
                    local.year(), local.month(), local.day(), local.hour(), 0, 0,
                ).unwrap()
            }
            Granularity::Minute => {
                tz.with_ymd_and_hms(
                    local.year(), local.month(), local.day(),
                    local.hour(), local.minute(), 0,
                ).unwrap()
            }
        };
        snapped_local.to_utc()
    }

    /// Snap to the end of the granularity period in the user's local timezone.
    pub fn snap_end(&self, dt: DateTime<Utc>, tz: FixedOffset) -> DateTime<Utc> {
        let local = dt.with_timezone(&tz);
        let snapped_local = match self {
            Granularity::Month => {
                let (year, month) = if local.month() == 12 {
                    (local.year() + 1, 1)
                } else {
                    (local.year(), local.month() + 1)
                };
                tz.with_ymd_and_hms(year, month, 1, 0, 0, 0).unwrap() - Duration::seconds(1)
            }
            Granularity::Day => {
                tz.with_ymd_and_hms(
                    local.year(), local.month(), local.day(), 23, 59, 59,
                ).unwrap()
            }
            Granularity::Hour => {
                tz.with_ymd_and_hms(
                    local.year(), local.month(), local.day(), local.hour(), 59, 59,
                ).unwrap()
            }
            Granularity::Minute => {
                tz.with_ymd_and_hms(
                    local.year(), local.month(), local.day(),
                    local.hour(), local.minute(), 59,
                ).unwrap()
            }
        };
        snapped_local.to_utc()
    }
}
