//! Text helpers shared by adapters and the HTTP logger.
//!
//! Everything here is *character*-based on purpose. Rust's `&s[..n]` indexes
//! **bytes** and panics when `n` lands inside a multi-byte character, so any
//! "shorten this to n" written as a byte slice is a crash waiting for the
//! first umlaut — which is guaranteed to arrive in a German ticket comment or
//! error page.

/// Shorten `s` to at most `max_chars` characters, appending `ellipsis` when
/// anything was actually cut. Never panics, whatever the input encoding.
///
/// Counts `char`s, not grapheme clusters or display columns: a combining
/// accent or a wide CJK glyph still counts as one. That is deliberate — this
/// is a length *bound* for previews and log snippets, while column widths are
/// the table engine's job.
pub fn truncate_with_ellipsis(s: &str, max_chars: usize, ellipsis: &str) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_none() {
        head
    } else {
        format!("{head}{ellipsis}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorter_than_the_bound_is_returned_unchanged() {
        assert_eq!(truncate_with_ellipsis("hello", 10, "…"), "hello");
        // Exactly at the bound: nothing was cut, so no ellipsis.
        assert_eq!(truncate_with_ellipsis("hello", 5, "…"), "hello");
    }

    #[test]
    fn longer_input_is_cut_and_marked() {
        assert_eq!(truncate_with_ellipsis("abcdef", 3, "…"), "abc…");
        assert_eq!(
            truncate_with_ellipsis("abcdef", 3, "…(truncated)"),
            "abc…(truncated)"
        );
    }

    /// The regression this module exists for: a multi-byte character straddling
    /// the cut. `&s[..3]` would panic here ('ä' occupies bytes 2..4).
    #[test]
    fn cutting_inside_a_multibyte_character_does_not_panic() {
        let cut = truncate_with_ellipsis("abäcd", 3, "…");
        assert_eq!(cut, "abä…", "must cut on a character, not a byte");
    }

    /// A bound expressed in characters must hold for non-ASCII text too — the
    /// old byte-based version silently cut such a string to a third.
    #[test]
    fn the_bound_counts_characters_not_bytes() {
        let umlauts = "ä".repeat(100);
        let cut = truncate_with_ellipsis(&umlauts, 80, "…");
        assert_eq!(cut.chars().count(), 81, "80 chars plus the ellipsis");
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn a_zero_bound_yields_only_the_ellipsis() {
        assert_eq!(truncate_with_ellipsis("abc", 0, "…"), "…");
        assert_eq!(truncate_with_ellipsis("", 0, "…"), "");
    }
}
