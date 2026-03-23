use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};

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
            "january","february","march","april","may","june",
            "july","august","september","october","november","december",
        ];
        let has_month = months.iter().any(|m| s.contains(m));
        let has_day_number = regex::Regex::new(r"\b\d{1,2}\b").unwrap().is_match(&s);
        if has_month && !has_day_number {
            return Granularity::Month;
        }

        // Default: Day
        Granularity::Day
    }

    /// Snap to the start of the granularity period (for start-gravity).
    pub fn snap_start(&self, dt: DateTime<Utc>) -> DateTime<Utc> {
        match self {
            Granularity::Month => {
                Utc.with_ymd_and_hms(dt.year(), dt.month(), 1, 0, 0, 0).unwrap()
            }
            Granularity::Day => {
                Utc.with_ymd_and_hms(dt.year(), dt.month(), dt.day(), 0, 0, 0).unwrap()
            }
            Granularity::Hour => {
                Utc.with_ymd_and_hms(dt.year(), dt.month(), dt.day(), dt.hour(), 0, 0).unwrap()
            }
            Granularity::Minute => {
                Utc.with_ymd_and_hms(dt.year(), dt.month(), dt.day(), dt.hour(), dt.minute(), 0).unwrap()
            }
        }
    }

    /// Snap to the end of the granularity period (for end-gravity).
    pub fn snap_end(&self, dt: DateTime<Utc>) -> DateTime<Utc> {
        match self {
            Granularity::Month => {
                // Last day of month, 23:59:59
                let (year, month) = if dt.month() == 12 {
                    (dt.year() + 1, 1)
                } else {
                    (dt.year(), dt.month() + 1)
                };
                Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0).unwrap() - Duration::seconds(1)
            }
            Granularity::Day => {
                Utc.with_ymd_and_hms(dt.year(), dt.month(), dt.day(), 23, 59, 59).unwrap()
            }
            Granularity::Hour => {
                Utc.with_ymd_and_hms(dt.year(), dt.month(), dt.day(), dt.hour(), 59, 59).unwrap()
            }
            Granularity::Minute => {
                Utc.with_ymd_and_hms(dt.year(), dt.month(), dt.day(), dt.hour(), dt.minute(), 59).unwrap()
            }
        }
    }
}
