use chrono::{DateTime, Local, NaiveTime, Utc};

/// A parsed datetime, always stored as UTC internally.
/// User input without explicit timezone is interpreted as local time.
#[derive(Clone)]
pub struct LocalDateTime {
    pub utc: DateTime<Utc>,
    pub original: String,
}

impl std::str::FromStr for LocalDateTime {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // 1. chrono-english: relative expressions (yesterday, next friday 8pm, today 9am, ...)
        if let Ok(dt) = chrono_english::parse_date_string(
            s,
            Local::now(),
            chrono_english::Dialect::Us,
        ) {
            return Ok(LocalDateTime { utc: dt.with_timezone(&Utc), original: s.to_string() });
        }

        // 2. dateparser fallback: broad absolute formats (RFC3339, unix timestamps,
        //    "2026-03-22 09:15", "6:15pm", ...)
        //    Uses Local as default timezone, midnight as default time for date-only strings.
        dateparser::parse_with(
            s,
            &Local,
            NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
        )
            .map(|utc| LocalDateTime { utc, original: s.to_string() })
            .map_err(|_| format!(
                "Cannot parse '{}' as a date/time. \
                    Accepted formats include: '2026-03-22', '2026-03-22 09:15', \
                    'yesterday', 'today 9am', 'next friday 8pm'",
                s
            ))
    }
}
