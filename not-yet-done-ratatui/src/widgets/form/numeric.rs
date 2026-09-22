//! Pure input rules and conversions behind the numeric form fields.
//!
//! Everything here is a free function over strings: the driver decides *when*
//! to ask (on a keystroke, on submit), this module decides *what the answer
//! is*. That keeps the rules — which characters a half-typed number may
//! contain, how `1h30` becomes ninety minutes — testable without a widget, a
//! terminal or a form.

/// Whether `s` is something the user may still be in the middle of typing as a
/// whole number.
///
/// Deliberately generous: an empty field, a lone minus sign and a leading zero
/// all pass, because each is a prefix of a number that will be fine. What the
/// *finished* value has to satisfy — a range, actually being a number — is
/// checked on submit, where rejecting it does not take the keystroke away.
pub(super) fn number_in_progress(s: &str, allow_negative: bool) -> bool {
    let body = match s.strip_prefix('-') {
        Some(rest) if allow_negative => rest,
        Some(_) => return false,
        None => s,
    };
    body.chars().all(|c| c.is_ascii_digit())
}

/// Whether `s` is something the user may still be in the middle of typing as a
/// fractional number with `sep` between the parts.
///
/// Unlike the range, `precision` *is* enforced here: every prefix of a number
/// with too many decimals already has too many, so waiting until submit would
/// only let the user type digits that are going to be rejected anyway.
pub(super) fn decimal_in_progress(
    s: &str,
    sep: char,
    precision: Option<u8>,
    allow_negative: bool,
) -> bool {
    let body = match s.strip_prefix('-') {
        Some(rest) if allow_negative => rest,
        Some(_) => return false,
        None => s,
    };
    let mut parts = body.splitn(2, sep);
    let whole = parts.next().unwrap_or_default();
    if !whole.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let Some(frac) = parts.next() else {
        return true;
    };
    // A field that may carry no decimals at all has no use for the separator.
    if precision == Some(0) {
        return false;
    }
    // Catches a second separator too: it is not a digit.
    if !frac.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    match precision {
        Some(p) => frac.chars().count() <= p as usize,
        None => true,
    }
}

/// Rewrites a dot-canonical value for display with `sep`.
pub(super) fn to_display(value: &str, sep: char) -> String {
    if sep == '.' {
        value.to_string()
    } else {
        value.replace('.', &sep.to_string())
    }
}

/// Rewrites what the user typed back to the canonical dot form, dropping a
/// trailing separator: `2,` is a number the user is done with, and `2.` would
/// make a consumer parse a form it never asked for.
pub(super) fn to_canonical(value: &str, sep: char) -> String {
    let dotted = if sep == '.' {
        value.trim().to_string()
    } else {
        value.trim().replace(sep, ".")
    };
    dotted.strip_suffix('.').unwrap_or(&dotted).to_string()
}

/// Parses the usual spellings of a time span into whole minutes.
///
/// Accepted: a bare number (`90`), minutes (`90m`), hours (`2h`, `1.5h`,
/// `1,5h`), both (`1h30`, `1h30m`) and the clock form (`1:30`). Spaces and
/// case do not matter. A negative span, a clock form with sixty minutes or
/// more, and anything with junk in it are rejected — the field says so on
/// submit rather than guessing.
///
/// Fractions that do not land on a whole minute are rounded to the nearest
/// one, which is what the preview under the field shows, so the user sees the
/// rounding before submitting.
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

/// Renders whole minutes the way the field's preview line shows them back:
/// `45 min`, `2 h`, `1 h 30 min`.
///
/// The submitted value stays the minute count; this is only so the user can
/// see that `1h30` was understood as an hour and a half before submitting it.
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

    /// Every prefix of a number the user might be typing passes, including the
    /// empty field and a lone minus; anything that can never become a whole
    /// number does not.
    #[test]
    fn a_half_typed_whole_number_is_accepted() {
        for s in ["", "-", "0", "007", "-12", "42"] {
            assert!(number_in_progress(s, true), "{s} must be typeable");
        }
        for s in ["1.", "1,5", "a", "1 2", "--1", "+1"] {
            assert!(!number_in_progress(s, true), "{s} must be rejected");
        }
        // Without negatives the minus never gets in, not even as a prefix.
        assert!(!number_in_progress("-", false));
        assert!(number_in_progress("12", false));
    }

    /// The separator is the configured one and only that one; `precision`
    /// bites while typing, the range does not.
    #[test]
    fn a_half_typed_decimal_respects_separator_and_precision() {
        assert!(decimal_in_progress("1.", '.', None, false));
        assert!(decimal_in_progress("1.50", '.', None, false));
        assert!(!decimal_in_progress("1,5", '.', None, false));
        assert!(decimal_in_progress("1,5", ',', None, false));
        assert!(!decimal_in_progress("1.5", ',', None, false));

        assert!(decimal_in_progress("1.5", '.', Some(1), false));
        assert!(!decimal_in_progress("1.55", '.', Some(1), false));
        // Nothing after the separator means the separator itself is pointless.
        assert!(!decimal_in_progress("1.", '.', Some(0), false));
        assert!(decimal_in_progress("1", '.', Some(0), false));
        // A second separator is not a digit, so it falls out here.
        assert!(!decimal_in_progress("1.2.3", '.', None, false));
        // A number far outside its range is still typeable — submit judges it.
        assert!(decimal_in_progress("999999", '.', None, false));
    }

    /// Display and canonical form differ only in the separator, and a value
    /// the user left hanging on the separator loses it on the way out.
    #[test]
    fn the_separator_is_display_only() {
        assert_eq!(to_display("1.5", ','), "1,5");
        assert_eq!(to_display("1.5", '.'), "1.5");
        assert_eq!(to_canonical("1,5", ','), "1.5");
        assert_eq!(to_canonical("2,", ','), "2");
        assert_eq!(to_canonical(" 2. ", '.'), "2");
    }

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
