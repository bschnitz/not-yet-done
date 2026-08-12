//! Shared conflict-banner constants + strip helper for content-edit
//! flows that may need to re-open the editor with a merge/retry hint
//! prepended above the user's buffer.
//!
//! Page-edit (CF-9) uses a 3-way merge: disjoint changes auto-merge,
//! overlapping changes come back as `<<<<<<< ours` / `>>>>>>> theirs`
//! markers under this banner. Comment-edit (CF-12) uses a simpler
//! flow: any 409 surfaces the banner alone — no merge attempt — and
//! the user manually re-edits.
//!
//! Both flows share the *banner markers* so the strip-on-resubmit
//! logic doesn't need to be duplicated. Wording is per-flow (page
//! merge text vs. comment retry hint) and stays local to each
//! caller's renderer.

pub(in crate::adapter) const CONFLICT_BANNER_START: &str =
    "<!-- ─── conflict ─────────────────────────────";
pub(in crate::adapter) const CONFLICT_BANNER_END: &str =
    "    ────────────────────────────────────── -->";

/// Strip a previously-rendered conflict banner from a buffer so repeated
/// reopens don't stack banners. Match is anchored at the start of the
/// string for safety — we never strip a banner that drifted into the
/// middle of the body. Returns the input verbatim when no banner is
/// present at the head.
pub(in crate::adapter) fn strip_banner(text: &str) -> &str {
    if !text.starts_with(CONFLICT_BANNER_START) {
        return text;
    }
    match text.find(CONFLICT_BANNER_END) {
        Some(end) => {
            let after = &text[end + CONFLICT_BANNER_END.len()..];
            after.strip_prefix('\n').unwrap_or(after)
        }
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_handles_full_banner_block() {
        let with_banner =
            format!("{CONFLICT_BANNER_START}\n    some text\n{CONFLICT_BANNER_END}\n<p>body</p>");
        assert_eq!(strip_banner(&with_banner), "<p>body</p>");
    }

    #[test]
    fn strip_leaves_text_alone_without_marker() {
        assert_eq!(strip_banner("<p>body</p>"), "<p>body</p>");
    }

    #[test]
    fn strip_returns_input_when_end_marker_missing() {
        // Truncated banner — leave text alone rather than chopping
        // unrelated content.
        let truncated = format!("{CONFLICT_BANNER_START}\n    incomplete\n<p>body</p>");
        assert_eq!(strip_banner(&truncated), truncated);
    }
}
