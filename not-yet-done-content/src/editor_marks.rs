//! Vocabulary the adapters write into an editor buffer for the user to
//! read. Shared so the same situation reads the same word whatever
//! tracker the ticket came from — a Jira and a Taiga buffer that mark
//! the same thing differently are two things to learn, not one.

/// Ends the header line of a comment written by somebody else. Both Jira
/// and Taiga only let a comment's author edit it, so an edit of such a
/// block is refused before a request goes out — the marker is what makes
/// that visible while editing rather than after saving.
pub const NOT_YOURS_MARKER: &str = "[not yours]";
