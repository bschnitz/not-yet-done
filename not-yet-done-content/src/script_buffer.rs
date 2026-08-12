//! The on-screen buffer format shared by every adapter-native script
//! editor (SQL editors today; nothing here is SQL-specific beyond the
//! comment syntax the markers use).
//!
//! A script buffer has two regions separated by [`QUERY_MARKER`]:
//!
//! - **above** the marker: a free scratch area — notes, half-finished
//!   helper statements, anything the user wants to keep next to the
//!   query but never execute;
//! - **below** the marker: the executable body.
//!
//! On a failed run the host prepends an error banner
//! ([`render_with_error`]) so the message is visible in the editor
//! itself, and strips it again on the next parse
//! ([`strip_error_banner`]) so banners never stack or reach the disk.
//!
//! This lives in the contract crate because *both* sides need the exact
//! same protocol and neither may own it: the TUI's edit sessions write
//! and parse the buffer, while an adapter's own "execute this script"
//! shortcut has to extract the same executable region without going
//! through the editor at all. Two copies would drift, and a copy in the
//! adapter would force the host to depend on that adapter — which is
//! precisely the coupling the [`ScriptStore`](crate::ScriptStore) seam
//! exists to prevent.

/// Marker line separating the scratch area from the executable body.
///
/// Written as a SQL line comment so a buffer stays valid SQL even when a
/// tool outside the TUI (an LSP, `psql -f`) reads the file whole.
pub const QUERY_MARKER: &str = "-- ▼ THIS SQL WILL BE EXECUTED ON SAVE ▼";

const ERROR_BANNER_START: &str = "-- ─── ERRORS ───";
const ERROR_BANNER_END: &str = "-- ─────────────────";

/// The executable portion of a script buffer: everything below the
/// [`QUERY_MARKER`] line, or the whole text when no marker is present.
///
/// A marker-less buffer executing in full is deliberate — it makes a
/// hand-written or externally generated `.sql` file work unchanged.
pub fn parse_query_area(text: &str) -> &str {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        offset += line.len();
        if line.trim_end_matches(['\r', '\n']).trim() == QUERY_MARKER {
            return &text[offset..];
        }
    }
    text
}

/// Strip a previously rendered error banner from the start of `text`.
///
/// Idempotent, and a no-op on a buffer that has none — reopening on a
/// still-broken query must not stack banners, and committing must not
/// persist one.
pub fn strip_error_banner(text: &str) -> &str {
    let Some(rest) = text.strip_prefix(ERROR_BANNER_START) else {
        return text;
    };
    let after_start = rest.strip_prefix('\n').unwrap_or(rest);
    let needle = format!("\n{ERROR_BANNER_END}");
    match after_start.find(&needle) {
        Some(pos) => {
            let after_end = &after_start[pos + needle.len()..];
            after_end.strip_prefix('\n').unwrap_or(after_end)
        }
        // Truncated banner (no terminator): drop what we recognised and
        // keep the rest, rather than returning the mangled original.
        None => after_start,
    }
}

/// Prepend an error banner to `text`, replacing any banner already
/// there. Each line of `error` becomes its own SQL comment so a
/// multi-line backend message stays readable.
pub fn render_with_error(text: &str, error: &str) -> String {
    let stripped = strip_error_banner(text);
    let mut out = String::new();
    out.push_str(ERROR_BANNER_START);
    out.push('\n');
    for line in error.lines() {
        out.push_str("-- • ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(ERROR_BANNER_END);
    out.push('\n');
    out.push_str(stripped);
    out
}

/// Assemble a fresh script buffer: a scratch-area hint, the marker, then
/// `body`. Adapters use this so every backend's default template has the
/// same shape and the host's parser always finds a marker.
pub fn default_buffer(body: &str) -> String {
    format!(
        "-- Scratch area: notes, helper SELECTs. Lines above the marker\n\
         -- below are ignored on every :w.\n\
         \n\
         {QUERY_MARKER}\n\
         \n\
         {body}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_extracts_body_below_marker() {
        let text = format!("-- scratch\n{QUERY_MARKER}\nSELECT 1;\n");
        assert_eq!(parse_query_area(&text), "SELECT 1;\n");
    }

    #[test]
    fn parse_without_marker_returns_full_text() {
        assert_eq!(parse_query_area("SELECT 1;\n"), "SELECT 1;\n");
    }

    #[test]
    fn parse_tolerates_trailing_whitespace_on_the_marker() {
        let text = format!("scratch\n{QUERY_MARKER}   \nSELECT 1;\n");
        assert_eq!(parse_query_area(&text), "SELECT 1;\n");
    }

    #[test]
    fn parse_keeps_multi_statement_bodies_intact() {
        let text = format!("{QUERY_MARKER}\nUPDATE t SET x = 1;\nSELECT * FROM t;\n");
        assert_eq!(
            parse_query_area(&text),
            "UPDATE t SET x = 1;\nSELECT * FROM t;\n"
        );
    }

    #[test]
    fn parse_returns_empty_when_marker_is_the_last_line() {
        let text = format!("scratch\n{QUERY_MARKER}\n");
        assert_eq!(parse_query_area(&text), "");
    }

    #[test]
    fn banner_round_trips() {
        let body = "SELECT 1;\n";
        let once = render_with_error(body, "syntax error");
        assert!(once.starts_with(ERROR_BANNER_START));
        assert_eq!(strip_error_banner(&once), body);
    }

    #[test]
    fn banner_does_not_stack() {
        let body = "SELECT 1;\n";
        let once = render_with_error(body, "syntax error");
        let twice = render_with_error(&once, "syntax error");
        assert_eq!(once, twice);
    }

    #[test]
    fn strip_is_a_noop_without_a_banner() {
        assert_eq!(strip_error_banner("no banner here\n"), "no banner here\n");
    }

    #[test]
    fn multi_line_errors_each_get_their_own_comment() {
        let out = render_with_error("SELECT 1;\n", "line one\nline two");
        assert!(out.contains("-- • line one\n"));
        assert!(out.contains("-- • line two\n"));
    }

    #[test]
    fn default_buffer_is_parseable_back_to_its_body() {
        let buf = default_buffer("SELECT * FROM t;\n");
        assert_eq!(parse_query_area(&buf), "\nSELECT * FROM t;\n");
    }
}
