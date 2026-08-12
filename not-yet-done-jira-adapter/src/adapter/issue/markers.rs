//! Inline markers used by the 3b template, error/conflict banners, and
//! the comment-edit buffer. Concentrated here so render and parse code can
//! pull them in via a single import and tests can match on them by name.

/// 3b template layout: editable section / `---` / read-only section /
/// `===` / body. Both markers must appear as their own line. The parser
/// rejects unknown editable keys above `---`.
pub(super) const EDITABLE_MARKER: &str = "---";
pub(super) const BODY_MARKER: &str = "===";

/// Inline banner for parse / validation errors. Stripped before re-rendering
/// so reopens don't stack banners.
pub(super) const ERROR_BANNER_START: &str = "# ─── ERRORS ───";
pub(super) const ERROR_BANNER_END: &str = "# ──────────────";

/// Inline banner shown after a 412/conflict. Same stripping rule.
pub(super) const CONFLICT_BANNER_START: &str = "# ─── CONFLICT ───";
pub(super) const CONFLICT_BANNER_END: &str = "# ─────────────────";

/// Middle separator emitted by `diffy::MergeOptions { ConflictStyle::Merge }`
/// (default marker length 7). Surrounding `<<<<<<< ours` / `>>>>>>> theirs`
/// lines are matched by prefix in `parse_3b`.
pub(super) const CONFLICT_MARK_MIDDLE: &str = "=======";

/// Whole-line marker that opens a new-comment block in the
/// `edit_with_comments` buffer. Body runs until the next marker or EOF.
pub(super) const ADD_COMMENT_MARKER: &str = "--- add ---";

/// Sentinel line that introduces the read-only CACHE section appended at
/// the end of the edit template. Everything from this line onward is
/// stripped before parsing — it only exists for the user's reference.
pub(super) const CACHE_MARKER: &str =
    "#### CACHE / available labels, users & statuses (do not edit) ####";

/// Sentinel body content (case-insensitive, sole non-blank line) that
/// requests a comment deletion in `edit_with_comments`.
pub(super) const DELETE_KEYWORD_DEL: &str = "del";
pub(super) const DELETE_KEYWORD_DELETE: &str = "delete";

/// Banner shown when one or more comments were modified upstream while the
/// user was editing. Restored upstream bodies are inlined and the user's
/// would-be edit is reported in the banner so they can re-apply it.
pub(super) const FOREIGN_BANNER_START: &str = "# ─── COMMENTS CHANGED UPSTREAM ───";
pub(super) const FOREIGN_BANNER_END: &str = "# ─────────────────────────────────";
