use chrono::Duration;

#[derive(Clone)]
pub struct LocalOffset {
    pub duration: Duration,
}

impl std::str::FromStr for LocalOffset {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
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

        // Split number and unit
        let split = rest.find(|c: char| c.is_alphabetic())
            .ok_or_else(|| format!("Invalid offset '{}': missing unit", s))?;
        let (num_str, unit) = rest.split_at(split);

        let num: i64 = num_str.parse().map_err(|_| {
            format!("Invalid offset '{}': '{}' is not a number", s, num_str)
        })?;

        let duration = match unit.to_lowercase().as_str() {
            "s" | "sec" | "secs" | "second" | "seconds" =>
                Duration::seconds(sign * num),
            "m" | "min" | "mins" | "minute" | "minutes" =>
                Duration::minutes(sign * num),
            "h" | "hr" | "hrs" | "hour" | "hours" =>
                Duration::hours(sign * num),
            "d" | "day" | "days" =>
                Duration::days(sign * num),
            "w" | "week" | "weeks" =>
                Duration::weeks(sign * num),
            other => return Err(format!(
                "Unknown time unit '{}'. Use: s, min, h, d, w", other
            )),
        };

        Ok(LocalOffset { duration })
    }
}
