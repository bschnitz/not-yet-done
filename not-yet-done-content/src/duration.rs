//! Spans of time as people write them, and as they read them back.
//!
//! A duration field submits whole minutes, but nobody types minutes: they
//! type `1h30`, `1:30` or `1,5h`. Both directions of that translation live
//! here, in the crate that has no front-end, because the same span must mean
//! the same number whether it was typed into a form, passed on a command line
//! or filled in by a script.

/// Parses the usual spellings of a time span into whole minutes.
///
/// Accepted: a bare number (`90`), minutes (`90m`), hours (`2h`, `1.5h`,
/// `1,5h`), both (`1h30`, `1h30m`) and the clock form (`1:30`). Spaces and
/// case do not matter. A negative span, a clock form with sixty minutes or
/// more, and anything with junk in it are rejected — the caller says so
/// rather than guessing.
///
/// Fractions that do not land on a whole minute are rounded to the nearest
/// one, which is what a form's preview shows, so the user sees the rounding
/// before submitting.
pub fn parse_duration_minutes(s: &str) -> Option<i64> {
    let mut s: String = s.trim().to_ascii_lowercase();
    s.retain(|c| !c.is_whitespace());
    s = s.replace(',', ".");
    if s.is_empty() {
        return None;
    }

    let total: f64 = if let Some((h, m)) = s.split_once(':') {
        let hours: i64 = parse_digits(h)?;
        if m.len() != 2 {
            return None;
        }
        let minutes: i64 = parse_digits(m)?;
        if minutes >= 60 {
            return None;
        }
        (hours * 60 + minutes) as f64
    } else if let Some((h, rest)) = s.split_once('h') {
        let hours: f64 = parse_number(h)?;
        let rest = rest.strip_suffix('m').unwrap_or(rest);
        let minutes: f64 = if rest.is_empty() {
            0.0
        } else {
            parse_number(rest)?
        };
        hours * 60.0 + minutes
    } else if let Some(m) = s.strip_suffix('m') {
        parse_number(m)?
    } else {
        parse_number(&s)?
    };

    // An upper bound keeps a typo like `999999999h` from silently becoming a
    // number no backend can use; roughly two thousand years is past any span
    // a form is asking about.
    if !(0.0..=1.0e9).contains(&total) {
        return None;
    }
    Some(total.round() as i64)
}

/// Digits only — no sign, no fraction. Used for the clock form, where `1:-5`
/// or `1:3.5` is a typo rather than a span.
fn parse_digits(s: &str) -> Option<i64> {
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// A non-negative decimal number — `f64::from_str` also takes `inf`, `nan` and
/// a leading sign, none of which is a duration.
fn parse_number(s: &str) -> Option<f64> {
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Renders whole minutes the way a form's preview line shows them back:
/// `45 min`, `2 h`, `1 h 30 min`.
///
/// The submitted value stays the minute count; this is only so the user can
/// see that `1h30` was understood as an hour and a half.
pub fn format_duration(minutes: i64) -> String {
    if minutes < 60 {
        return format!("{minutes} min");
    }
    let (h, m) = (minutes / 60, minutes % 60);
    if m == 0 {
        format!("{h} h")
    } else {
        format!("{h} h {m} min")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spellings a user actually types all land on the same minute count.
    #[test]
    fn every_spelling_of_ninety_minutes_parses_the_same() {
        for s in [
            "90", "90m", "1h30", "1h30m", "1:30", "1.5h", "1,5h", " 1 h 30 ", "1H30M",
        ] {
            assert_eq!(parse_duration_minutes(s), Some(90), "{s}");
        }
        assert_eq!(parse_duration_minutes("2h"), Some(120));
        assert_eq!(parse_duration_minutes("0"), Some(0));
        // Fractions of a minute round to the nearest one.
        assert_eq!(parse_duration_minutes("0.77h"), Some(46));
    }

    /// Junk, negatives and an impossible clock are rejected rather than
    /// guessed at.
    #[test]
    fn a_span_that_is_not_one_is_rejected() {
        for s in [
            "", "   ", "abc", "-5", "1:75", "1:5", "1:305", ":30", "h30", "1m30", "1h30x",
        ] {
            assert_eq!(parse_duration_minutes(s), None, "{s} must not parse");
        }
    }

    /// The preview says the span in the words the user thinks in, not in the
    /// minute count that gets submitted.
    #[test]
    fn a_span_is_shown_in_hours_and_minutes() {
        assert_eq!(format_duration(0), "0 min");
        assert_eq!(format_duration(45), "45 min");
        assert_eq!(format_duration(60), "1 h");
        assert_eq!(format_duration(120), "2 h");
        assert_eq!(format_duration(90), "1 h 30 min");
    }
}
