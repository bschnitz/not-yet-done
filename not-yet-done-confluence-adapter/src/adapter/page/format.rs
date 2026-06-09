//! XHTML pretty-printing for Confluence's `body.storage` format.
//!
//! Confluence stores page bodies as XHTML-flavoured XML with custom
//! Atlassian namespaces (`<ac:structured-macro>`, `<ri:user>`, …). Raw
//! storage values come back as a single long line — unreadable in
//! `$EDITOR`. We pipe the value through `xmllint --format` to get one
//! tag per line with sensible indentation.
//!
//! Direct invocation would fail because the body is a fragment, not a
//! full document. The trick (lifted from the standalone `conf-edit`
//! script): wrap the fragment in a synthetic `<root>…</root>` so xmllint
//! has a single root element to format, then strip the wrapper + the
//! XML declaration back off afterwards.
//!
//! Fallbacks: if `xmllint` isn't installed (or the body isn't parseable
//! XML), we hand the raw value back unchanged. The edit flow still
//! works — it just shows the body as one long line, same as the
//! preview pane already does today.

use tokio::io::AsyncWriteExt;

/// Pretty-print a `body.storage` fragment. Returns the input verbatim
/// if `xmllint` is missing or the content can't be parsed.
pub(in crate::adapter) async fn format_xhtml(raw: &str) -> String {
    let mut child = match tokio::process::Command::new("xmllint")
        .arg("--format")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return raw.to_string(),
    };

    let wrapped = format!("<root>{raw}</root>");
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(wrapped.as_bytes()).await.is_err() {
            return raw.to_string();
        }
        drop(stdin);
    }

    let output = match child.wait_with_output().await {
        Ok(o) if o.status.success() => o.stdout,
        _ => return raw.to_string(),
    };

    let formatted = match String::from_utf8(output) {
        Ok(s) => s,
        Err(_) => return raw.to_string(),
    };

    strip_root_wrapper(&formatted)
}

/// Strip the xmllint header + the synthetic `<root>` / `</root>` lines.
/// Returns the inner content with leading/trailing newlines trimmed, so
/// re-running the same input through `format_xhtml` is idempotent.
fn strip_root_wrapper(formatted: &str) -> String {
    let mut lines: Vec<&str> = formatted.lines().collect();
    if matches!(lines.first(), Some(l) if l.starts_with("<?xml")) {
        lines.remove(0);
    }
    // `<root>` and `</root>` land on their own lines after xmllint.
    if matches!(lines.first(), Some(l) if l.trim() == "<root>") {
        lines.remove(0);
    }
    if matches!(lines.last(), Some(l) if l.trim() == "</root>") {
        lines.pop();
    }
    // Idempotency: an empty fragment becomes `<root/>` after xmllint —
    // strip that too so re-formatting yields the same empty string.
    if matches!(lines.first(), Some(l) if l.trim() == "<root/>") {
        lines.remove(0);
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_handles_xmllint_output_shape() {
        let xmllint_out = "<?xml version=\"1.0\"?>\n<root>\n  <p>hello</p>\n  <p>world</p>\n</root>\n";
        let stripped = strip_root_wrapper(xmllint_out);
        assert_eq!(stripped, "  <p>hello</p>\n  <p>world</p>");
    }

    #[test]
    fn strip_handles_empty_root_self_close() {
        let xmllint_out = "<?xml version=\"1.0\"?>\n<root/>\n";
        let stripped = strip_root_wrapper(xmllint_out);
        assert_eq!(stripped, "");
    }

    #[test]
    fn strip_leaves_input_alone_when_no_wrapper() {
        let raw = "<p>x</p>";
        // No leading xml decl, no <root> wrapper — strip should keep it.
        let stripped = strip_root_wrapper(raw);
        assert_eq!(stripped, raw);
    }

    #[tokio::test]
    async fn format_returns_input_when_xmllint_missing_or_fails() {
        // Either xmllint is absent (returns raw) or it succeeds. Both
        // branches must yield something — we just lock in that the
        // helper never panics on weird input and never returns "".
        let formatted = format_xhtml("<p>hello</p>").await;
        assert!(
            formatted.contains("hello"),
            "format must preserve content; got: {formatted:?}"
        );
    }
}
