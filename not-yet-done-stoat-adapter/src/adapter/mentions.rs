//! Revolt `<@USERID>` mention rendering — the Stoat analogue of the
//! Jira/Taiga `uu-…` slug mechanism (built on the same
//! [`not_yet_done_content::slug::SlugTable`]).
//!
//! Two directions, because display and editing want different forms:
//!
//! - **Display** (read-only chat rendering): `<@01ABC…>` → `@username`.
//!   The user sees a readable handle instead of the internal code.
//! - **Edit** (round-trip): `<@01ABC…>` ↔ `@uu-username`. The editor
//!   buffer carries autocomplete-friendly slugs plus a trailing CACHE
//!   section listing every available `@uu-…`; on save the slugs are
//!   translated back to the wire `<@ID>` form.
//!
//! Unknown ids / slugs are kept verbatim on render and surfaced as an
//! error on parse, mirroring the Jira behaviour.

use std::collections::HashMap;

use not_yet_done_content::slug::SlugTable;

/// Slug prefix for user mentions (shared convention with Jira/Taiga).
pub(super) const USER_PREFIX: &str = "uu-";

/// Marker introducing the trailing CACHE section in an editor buffer.
/// Everything from this line on is stripped before parsing.
pub(super) const CACHE_MARKER: &str = "#### CACHE / available @mentions (do not edit) ####";

/// Build the mention slug table from an `id → username` map.
///
/// `slug_source` is the username (what the slug body is normalized from),
/// `original` is the **user id** — the value that goes inside `<@ID>` on
/// the wire.
pub(super) fn user_table(users: &HashMap<String, String>) -> SlugTable {
    SlugTable::build(
        users.iter().map(|(id, name)| (name.clone(), id.clone())),
        USER_PREFIX,
    )
}

/// Walk `<@ID>` tokens, replacing each via `f(id)`. When `f` returns
/// `None` the raw `<@ID>` is kept verbatim (so it round-trips unchanged).
fn replace_mentions<F>(text: &str, mut f: F) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest.find("<@") {
        out.push_str(&rest[..idx]);
        let after = &rest[idx + 2..];
        if let Some(end) = after.find('>') {
            let id = &after[..end];
            match f(id) {
                Some(rep) => out.push_str(&rep),
                None => out.push_str(&rest[idx..idx + 2 + end + 1]),
            }
            rest = &after[end + 1..];
        } else {
            // Unterminated `<@` — emit the remainder untouched.
            out.push_str(&rest[idx..]);
            return out;
        }
    }
    out.push_str(rest);
    out
}

/// `<@ID>` → `@username` for read-only display. Unknown ids kept raw.
pub(super) fn render_display(text: &str, users: &HashMap<String, String>) -> String {
    replace_mentions(text, |id| users.get(id).map(|name| format!("@{name}")))
}

/// `<@ID>` → `@uu-slug` for the editor buffer. Unknown ids kept raw
/// (they round-trip back unchanged through [`parse_slugs`]).
pub(super) fn render_slugs(text: &str, users: &SlugTable) -> String {
    replace_mentions(text, |id| users.slug_for(id).map(|slug| format!("@{slug}")))
}

/// Reverse of [`render_slugs`]: rewrite `@uu-slug` back to `<@ID>`.
/// Only matches at word boundaries so `mail@uu-x.com` is preserved.
/// Returns the offending slug if any `@uu-…` doesn't resolve.
pub(super) fn parse_slugs(text: &str, users: &SlugTable) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut last = 0usize;
    let mut i = 0usize;
    while i + 4 <= bytes.len() {
        if bytes[i] == b'@' && &bytes[i + 1..i + 4] == b"uu-" {
            let prev_ok = i == 0 || !is_word_byte(bytes[i - 1]);
            if prev_ok {
                let mut end = i + 4;
                while end < bytes.len() && is_slug_byte(bytes[end]) {
                    end += 1;
                }
                if end > i + 4 {
                    let slug = &text[i + 1..end];
                    match users.original_for(slug) {
                        Some(id) => {
                            out.push_str(&text[last..i]);
                            out.push_str("<@");
                            out.push_str(id);
                            out.push('>');
                            last = end;
                            i = end;
                            continue;
                        }
                        None => return Err(slug.to_string()),
                    }
                }
            }
        }
        i += 1;
    }
    out.push_str(&text[last..]);
    Ok(out)
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_slug_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-'
}

/// Render the trailing CACHE section listing every available `@uu-…`
/// slug. Empty string when there are no users to advertise.
pub(super) fn cache_section(users: &SlugTable) -> String {
    if users.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push('\n');
    out.push_str(CACHE_MARKER);
    out.push('\n');
    out.push_str("# mentions: ");
    let slugs: Vec<String> = users.slugs().iter().map(|s| format!("@{s}")).collect();
    out.push_str(&slugs.join(", "));
    out.push('\n');
    out
}

/// Strip the trailing CACHE section before parsing. The marker line and
/// everything after it are dropped. Idempotent on input without it.
pub(super) fn strip_cache_section(text: &str) -> &str {
    if let Some(pos) = text.find(CACHE_MARKER) {
        text[..pos].trim_end_matches(|c: char| c.is_whitespace())
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_users() -> HashMap<String, String> {
        // Fully invented ids/names — no real instance data.
        let mut m = HashMap::new();
        m.insert("01AAA".to_string(), "alice".to_string());
        m.insert("01BBB".to_string(), "bob".to_string());
        m
    }

    #[test]
    fn display_resolves_known_keeps_unknown() {
        let users = sample_users();
        assert_eq!(
            render_display("hi <@01AAA> and <@01ZZZ>", &users),
            "hi @alice and <@01ZZZ>"
        );
    }

    #[test]
    fn display_handles_unterminated_mention() {
        let users = sample_users();
        assert_eq!(render_display("oops <@01AAA", &users), "oops <@01AAA");
    }

    #[test]
    fn slug_render_then_parse_roundtrips() {
        let users = sample_users();
        let table = user_table(&users);
        let body = "ping <@01AAA> and <@01BBB>!";
        let rendered = render_slugs(body, &table);
        assert_eq!(rendered, "ping @uu-alice and @uu-bob!");
        // …and back to the wire form.
        assert_eq!(parse_slugs(&rendered, &table).unwrap(), body);
    }

    #[test]
    fn parse_keeps_unknown_id_verbatim_on_render() {
        let users = sample_users();
        let table = user_table(&users);
        // Unknown id has no slug → kept raw → parses back identically.
        let body = "see <@01ZZZ>";
        let rendered = render_slugs(body, &table);
        assert_eq!(rendered, body);
        assert_eq!(parse_slugs(&rendered, &table).unwrap(), body);
    }

    #[test]
    fn parse_rejects_unknown_slug() {
        let users = sample_users();
        let table = user_table(&users);
        assert_eq!(parse_slugs("hi @uu-nobody", &table), Err("uu-nobody".into()));
    }

    #[test]
    fn parse_preserves_email_like_at_word() {
        let users = sample_users();
        let table = user_table(&users);
        // `x@uu-...` is mid-word (preceded by a word byte) → untouched.
        let s = "mail x@uu-alice stays";
        assert_eq!(parse_slugs(s, &table).unwrap(), s);
    }

    #[test]
    fn cache_section_lists_slugs_and_strips_clean() {
        let users = sample_users();
        let table = user_table(&users);
        let section = cache_section(&table);
        assert!(section.contains(CACHE_MARKER));
        assert!(section.contains("@uu-alice"));
        assert!(section.contains("@uu-bob"));

        let buffer = format!("hello @uu-alice\n{section}");
        assert_eq!(strip_cache_section(&buffer), "hello @uu-alice");
    }

    #[test]
    fn cache_section_empty_when_no_users() {
        let table = user_table(&HashMap::new());
        assert_eq!(cache_section(&table), "");
        assert_eq!(strip_cache_section("no marker here"), "no marker here");
    }
}
