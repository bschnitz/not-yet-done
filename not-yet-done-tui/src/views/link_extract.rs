//! Extract openable links from rendered item text for the link-hop feature.
//!
//! Link-hop (`f`) labels every link visible in the focused pane and opens the
//! picked one in the browser. Two link shapes are recognised:
//!
//! * **bare URLs** — `https://example.com/x` → `(url, url)`
//! * **markdown links** — `[text](url)` → `(text, url)`
//!
//! The returned pairs are `(needle, url)`: the *needle* is the substring the
//! table widget searches for on screen, the *url* is what gets opened. For a
//! markdown link the needle is the link **text**, because that is what the
//! markdown renderer paints (the pinned `ratatui-markdown` fork discards the
//! URL, so it can only be recovered here from the raw source). In a plainly
//! rendered pane the same `[text](url)` shows up literally, and searching for
//! `text` still matches it as a substring — so the needle works in both the
//! markdown-rendered (stoat) and literal (every other tab) case.
//!
//! Markdown-link spans are stripped before the bare-URL scan so the URL inside
//! a `[text](url)` does not also register as a bare needle that would never
//! render in markdown mode.

use std::sync::LazyLock;

use regex::Regex;

static MARKDOWN_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").unwrap());

static BARE_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?://[^\s<>()\[\]{}'\x22]+").unwrap());

/// Trailing punctuation that is almost never part of a URL when it sits at the
/// very end (sentence terminators, closing brackets left over from prose).
const TRAILING_TRIM: &[char] = &['.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '"', '\''];

/// Extract `(needle, url)` link targets from one rendered text fragment.
///
/// Markdown links are collected first and their spans blanked out; bare URLs
/// are then scanned over the remaining text. Duplicate `(needle, url)` pairs
/// are removed while preserving first-seen order.
pub fn extract_links(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();

    // Markdown links: needle = link text, payload = URL.
    let mut masked = text.to_string();
    for caps in MARKDOWN_LINK.captures_iter(text) {
        let whole = caps.get(0).unwrap();
        let label = caps.get(1).unwrap().as_str().trim();
        let url = caps.get(2).unwrap().as_str().trim();
        if !label.is_empty() && !url.is_empty() {
            push_unique(&mut out, label.to_string(), url.to_string());
        }
        // Blank the whole `[..](..)` span so the URL inside is not re-scanned
        // as a bare URL (it would never render in markdown mode).
        blank_span(&mut masked, whole.start(), whole.end());
    }

    // Bare URLs over the masked text.
    for m in BARE_URL.find_iter(&masked) {
        let url = m.as_str().trim_end_matches(TRAILING_TRIM);
        if !url.is_empty() {
            push_unique(&mut out, url.to_string(), url.to_string());
        }
    }

    out
}

/// Extract link targets from many fragments (e.g. every rendered cell of a
/// pane), de-duplicated across all of them.
pub fn extract_links_from<'a, I>(fragments: I) -> Vec<(String, String)>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut out: Vec<(String, String)> = Vec::new();
    for frag in fragments {
        for (needle, url) in extract_links(frag) {
            push_unique(&mut out, needle, url);
        }
    }
    out
}

fn push_unique(out: &mut Vec<(String, String)>, needle: String, url: String) {
    if !out.iter().any(|(n, u)| *n == needle && *u == url) {
        out.push((needle, url));
    }
}

/// Overwrite `[start, end)` (byte range) with spaces so later scans skip it,
/// without shifting any byte offsets.
fn blank_span(s: &mut str, start: usize, end: usize) {
    // Safe: we only replace whole UTF-8 code points from a regex match, which
    // always lands on char boundaries; ASCII space is one byte.
    // SAFETY: writing ASCII spaces over a char-boundary-aligned range keeps the
    // string valid UTF-8.
    let bytes = unsafe { s.as_bytes_mut() };
    for b in &mut bytes[start..end] {
        *b = b' ';
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_url_is_its_own_needle() {
        let links = extract_links("see https://example.com/page for details");
        assert_eq!(links, vec![("https://example.com/page".to_string(), "https://example.com/page".to_string())]);
    }

    #[test]
    fn trailing_sentence_punctuation_is_trimmed() {
        let links = extract_links("go to https://example.com/x.");
        assert_eq!(links[0].1, "https://example.com/x");
    }

    #[test]
    fn markdown_link_uses_text_as_needle() {
        let links = extract_links("check [the docs](https://example.com/docs) now");
        assert_eq!(links, vec![("the docs".to_string(), "https://example.com/docs".to_string())]);
    }

    #[test]
    fn markdown_url_is_not_also_a_bare_needle() {
        // The URL inside the markdown link must not leak out as a second target.
        let links = extract_links("[site](https://example.com)");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0], ("site".to_string(), "https://example.com".to_string()));
    }

    #[test]
    fn mixed_markdown_and_bare_in_one_fragment() {
        let links = extract_links("[a](https://a.test) and https://b.test/2");
        assert_eq!(
            links,
            vec![
                ("a".to_string(), "https://a.test".to_string()),
                ("https://b.test/2".to_string(), "https://b.test/2".to_string()),
            ]
        );
    }

    #[test]
    fn duplicates_are_deduped_across_fragments() {
        let links = extract_links_from(["https://x.test", "prefix https://x.test suffix"]);
        assert_eq!(links.len(), 1);
    }

    #[test]
    fn no_links_yields_empty() {
        assert!(extract_links("plain text, no links here").is_empty());
    }

    #[test]
    fn unicode_before_link_keeps_url_intact() {
        // A multi-byte prefix must not corrupt byte offsets used when masking.
        let links = extract_links("änderung [länk](https://ü.test/ä) ende");
        assert_eq!(links, vec![("länk".to_string(), "https://ü.test/ä".to_string())]);
    }
}
