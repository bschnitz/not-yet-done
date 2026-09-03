//! Modified UTF-7, the encoding IMAP mailbox names arrive in (RFC 3501 §5.1.3).
//!
//! `&` opens a shifted run that ends at `-`; inside it, UTF-16BE is base64ed
//! with `,` standing in for `/`, and `&-` is a literal ampersand. So the
//! folder a user calls *Entwürfe* travels as `Entw&APw-rfe`, and a folder
//! tree that skips this step shows mojibake on every account that is not
//! English.
//!
//! Decoding only, deliberately. The *path* we keep is the wire form exactly
//! as the server sent it, and that is what goes back in `SELECT`/`STATUS`;
//! the decoded form is for the label. Re-encoding a name we never encoded
//! could only introduce a mismatch.

/// Decode a mailbox name for display. Anything malformed is passed through
/// verbatim: a name we cannot read is still a name the user can recognise,
/// and losing it would be worse than showing it raw.
pub(crate) fn decode(input: &str) -> String {
    let mut out = String::new();
    let mut rest = input;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let Some(end) = after.find('-') else {
            // Unterminated run — not our business to repair.
            out.push_str(&rest[amp..]);
            return out;
        };
        let run = &after[..end];
        if run.is_empty() {
            out.push('&');
        } else {
            match decode_run(run) {
                Some(text) => out.push_str(&text),
                None => out.push_str(&rest[amp..amp + 1 + end + 1]),
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// One shifted run: modified base64 of UTF-16BE.
fn decode_run(run: &str) -> Option<String> {
    let mut bits: u32 = 0;
    let mut have: u32 = 0;
    let mut units: Vec<u16> = Vec::new();
    for c in run.chars() {
        let v = sextet(c)?;
        bits = (bits << 6) | u32::from(v);
        have += 6;
        if have >= 16 {
            have -= 16;
            units.push(((bits >> have) & 0xFFFF) as u16);
        }
    }
    // Whatever is left must be zero padding; anything else is a truncated
    // character, not a name we should guess at.
    if have >= 6 || (bits & ((1 << have) - 1)) != 0 {
        return None;
    }
    String::from_utf16(&units).ok()
}

fn sextet(c: char) -> Option<u8> {
    Some(match c {
        'A'..='Z' => c as u8 - b'A',
        'a'..='z' => c as u8 - b'a' + 26,
        '0'..='9' => c as u8 - b'0' + 52,
        '+' => 62,
        // The one deviation from base64: `/` would collide with the
        // hierarchy delimiter some servers use.
        ',' => 63,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_ascii_is_untouched() {
        assert_eq!(decode("INBOX"), "INBOX");
        assert_eq!(decode("INBOX/Projects"), "INBOX/Projects");
        assert_eq!(decode(""), "");
    }

    #[test]
    fn a_shifted_run_becomes_the_name_the_user_gave_it() {
        assert_eq!(decode("Entw&APw-rfe"), "Entwürfe");
        assert_eq!(decode("Gel&APY-schte Elemente"), "Gelöschte Elemente");
        assert_eq!(decode("&APw-"), "ü");
        assert_eq!(decode("INBOX/&APw-ber"), "INBOX/über");
    }

    /// `&-` is how the encoding writes an ampersand — a folder actually
    /// named `R&D` depends on it.
    #[test]
    fn an_escaped_ampersand_comes_back_as_one() {
        assert_eq!(decode("R&-D"), "R&D");
        assert_eq!(decode("&-"), "&");
        assert_eq!(decode("&-&-"), "&&");
    }

    /// Characters outside the BMP travel as a surrogate pair in one run.
    #[test]
    fn a_surrogate_pair_survives() {
        assert_eq!(decode("&2DzfiQ-"), "🎉");
    }

    /// A name we cannot read is shown as it arrived, never dropped and never
    /// half-decoded.
    #[test]
    fn malformed_input_is_passed_through() {
        assert_eq!(decode("Entw&APw"), "Entw&APw");
        assert_eq!(decode("&!!!-x"), "&!!!-x");
        assert_eq!(decode("a&"), "a&");
    }
}
