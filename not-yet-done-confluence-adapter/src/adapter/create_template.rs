//! Shared editor-buffer template for the page-create flows on
//! `confluence:space` (`create-page`) and `confluence:page`
//! (`create-child`).
//!
//! Format — three lines, hand-edited:
//!
//! ```text
//! title:
//!
//! <p></p>
//! ```
//!
//! Parsing rules:
//! - The first line must start with `title:`. The remainder of the line
//!   (after stripping the prefix and trimming) is the title.
//! - The title must be non-empty after trimming.
//! - Everything after the first newline is the body (XHTML storage
//!   format), with one optional leading blank line stripped so the empty
//!   `<p></p>` lands cleanly without `\n` prefixes.
//!
//! On parse failure the caller renders a banner above the buffer and
//! reopens the editor; [`strip_error_banner`] removes any previously-
//! rendered banner before re-parsing so banners don't stack on retry.

const TITLE_PREFIX: &str = "title:";

pub(in crate::adapter) const CREATE_ERROR_BANNER_START: &str =
    "<!-- ─── create error ─────────────────────────";
pub(in crate::adapter) const CREATE_ERROR_BANNER_END: &str =
    "    ────────────────────────────────────────── -->";

/// Parsed inputs from a create-buffer.
#[derive(Debug)]
pub(in crate::adapter) struct ParsedCreate {
    pub title: String,
    pub body: String,
}

/// Render the initial template a new editor session opens with.
pub(in crate::adapter) fn render_template() -> String {
    String::from("title: \n\n<p></p>\n")
}

/// Strip a previously-rendered error banner so repeated reopens don't
/// stack banners. Anchored at the start of the string for safety.
pub(in crate::adapter) fn strip_error_banner(text: &str) -> &str {
    if !text.starts_with(CREATE_ERROR_BANNER_START) {
        return text;
    }
    match text.find(CREATE_ERROR_BANNER_END) {
        Some(end) => {
            let after = &text[end + CREATE_ERROR_BANNER_END.len()..];
            after.strip_prefix('\n').unwrap_or(after)
        }
        None => text,
    }
}

/// Prepend an error banner to a buffer for re-display in the editor.
pub(in crate::adapter) fn render_with_error(text: &str, message: &str) -> String {
    let mut out = String::new();
    out.push_str(CREATE_ERROR_BANNER_START);
    out.push('\n');
    for line in message.lines() {
        out.push_str("    ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(CREATE_ERROR_BANNER_END);
    out.push('\n');
    out.push_str(strip_error_banner(text));
    out
}

/// Parse a create-buffer. Returns `Err(msg)` on malformed input — caller
/// renders an error banner and reopens the editor.
pub(in crate::adapter) fn parse_template(text: &str) -> Result<ParsedCreate, String> {
    let stripped = strip_error_banner(text);
    let (first_line, rest) = stripped.split_once('\n').unwrap_or((stripped, ""));
    let trimmed_first = first_line.trim_start();

    let title_raw = match trimmed_first.strip_prefix(TITLE_PREFIX) {
        Some(rest) => rest,
        None => {
            return Err(format!(
                "Buffer must start with `title: <page name>`.\nGot: {first_line}"
            ));
        }
    };
    let title = title_raw.trim().to_string();
    if title.is_empty() {
        return Err("Title must not be empty.".to_string());
    }

    // Drop one optional leading blank line so the user's `<p></p>` lands
    // at the start of the body buffer.
    let body = rest.strip_prefix('\n').unwrap_or(rest).to_string();

    Ok(ParsedCreate { title, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_template_extracts_title_and_body() {
        let buf = "title: Hello World\n\n<p>body</p>\n";
        let parsed = parse_template(buf).expect("parses");
        assert_eq!(parsed.title, "Hello World");
        assert_eq!(parsed.body, "<p>body</p>\n");
    }

    #[test]
    fn parse_template_rejects_missing_title_prefix() {
        let err = parse_template("Hello\n\n<p>x</p>\n").expect_err("must error");
        assert!(err.contains("title:"));
    }

    #[test]
    fn parse_template_rejects_empty_title() {
        let err = parse_template("title:  \n\n<p>x</p>\n").expect_err("must error");
        assert!(err.contains("empty"));
    }

    #[test]
    fn parse_template_strips_existing_error_banner_idempotently() {
        let buf = "title: Real Title\n\n<p>x</p>\n";
        let banner_buf = render_with_error(buf, "Something went wrong.");
        let parsed = parse_template(&banner_buf).expect("parses after banner");
        assert_eq!(parsed.title, "Real Title");
        assert_eq!(parsed.body, "<p>x</p>\n");
    }

    #[test]
    fn render_template_round_trips_through_parse() {
        let tpl = render_template();
        // Empty title fails parse — by design, forces user to fill it in.
        let err = parse_template(&tpl).expect_err("empty title must error");
        assert!(err.contains("empty"));
    }

    #[test]
    fn render_with_error_then_strip_is_identity() {
        let original = "title: X\n\n<p></p>\n";
        let with = render_with_error(original, "bad");
        let stripped = strip_error_banner(&with);
        assert_eq!(stripped, original);
    }

    #[test]
    fn parse_template_accepts_multiline_body() {
        let buf = "title: Multi\n\n<p>one</p>\n<p>two</p>\n";
        let parsed = parse_template(buf).expect("parses");
        assert_eq!(parsed.title, "Multi");
        assert_eq!(parsed.body, "<p>one</p>\n<p>two</p>\n");
    }
}
