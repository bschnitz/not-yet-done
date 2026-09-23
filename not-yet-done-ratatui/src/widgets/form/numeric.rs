//! Pure input rules and conversions behind the numeric form fields.
//!
//! Everything here is a free function over strings: the driver decides *when*
//! to ask (on a keystroke, on submit), this module decides *what the answer
//! is*. That keeps the rules — which characters a half-typed number may
//! contain, which separator a decimal is shown with — testable without a
//! widget, a terminal or a form. Durations are the exception: a span is the
//! same span on a command line as in a field, so its two conversions live in
//! `not_yet_done_content::duration`.

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
}
