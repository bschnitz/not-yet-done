//! Signed time-offset grammar: `+1h`, `-30min`, `+2days`, `+90m`.
//!
//! Ported from the trackings `split`/`move` actions so every front-end shares
//! one implementation. A leading sign is **required** (that is what distinguishes
//! an offset like `+1h` from an absolute time like `1h`/`5pm`); callers that want
//! an unsigned value to mean "forward" prepend `+` themselves.

use chrono::Duration;

/// Parse a signed offset into a [`Duration`], or `None` if it is not one.
pub fn resolve_offset(s: &str) -> Option<Duration> {
    let s = s.trim();

    let (sign, rest) = match s.strip_prefix('+') {
        Some(r) => (1i64, r),
        None => (-1i64, s.strip_prefix('-')?),
    };

    // Split the leading number from the trailing unit.
    let split = rest.find(|c: char| c.is_alphabetic())?;
    let (num_str, unit) = rest.split_at(split);
    let num: i64 = num_str.trim().parse().ok()?;
    let n = sign * num;

    let duration = match unit.trim().to_lowercase().as_str() {
        "s" | "sec" | "secs" | "second" | "seconds" => Duration::seconds(n),
        "m" | "min" | "mins" | "minute" | "minutes" => Duration::minutes(n),
        "h" | "hr" | "hrs" | "hour" | "hours" => Duration::hours(n),
        "d" | "day" | "days" => Duration::days(n),
        "w" | "wk" | "week" | "weeks" => Duration::weeks(n),
        _ => return None,
    };
    Some(duration)
}
