//! One codec for a comment body on its way between Jira and the editor.
//!
//! Every path that shows or writes a comment goes through here: the ticket
//! buffer with all its comments (`edit_with_comments`, `edit_markdown`), the
//! single-comment node (`edit_full`) and a new comment (`create_comment`).
//! A comment must read and write the same way on each of them — a mention
//! or a heading that works in one editor and arrives raw from another is
//! a silent corruption of what goes out.
//!
//! The codec has two layers, applied in a fixed order:
//!
//! - **mentions** — `[~name]` ↔ `@uu_slug` ([`mentions_out`] / [`mentions_in`]),
//! - **markup** — Jira wiki markup ↔ Markdown ([`markup_out`] / [`markup_in`]).
//!
//! Out to the editor: mentions, then markup. Back from the editor: markup,
//! then mentions. The ticket buffer passes through its canonical (wiki)
//! form between the two layers, so it calls them one at a time; the other
//! paths call [`to_editor`] / [`from_editor`], which compose the same two.

use std::sync::Mutex;

use not_yet_done_content::slug::SlugTable;

use crate::client::JiraClient;

use super::super::cache::{JiraCache, fetch_comments, fetch_issue, resolve_unknown_mentions};
use super::slugs::{build_slug_tables, parse_user_mentions, render_user_mentions};
use super::wiki_md;

/// The dialect a comment is edited in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dialect {
    /// Markdown — the authoring dialect, as for the description.
    Markdown,
    /// Raw Jira wiki markup, for a body whose markup would not survive
    /// `wiki -> md -> wiki` unchanged.
    Jira,
}

impl Dialect {
    /// The dialect an existing body is handed out in: Markdown unless the
    /// round-trip would change it.
    pub(crate) fn for_body(wiki: &str) -> Self {
        if wiki_md::roundtrip_diff(wiki).is_none() {
            Dialect::Markdown
        } else {
            Dialect::Jira
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Dialect::Markdown => "markdown",
            Dialect::Jira => "jira",
        }
    }

    pub(crate) fn suffix(self) -> &'static str {
        match self {
            Dialect::Markdown => ".md",
            Dialect::Jira => ".jira",
        }
    }
}

/// The users a comment on one issue can mention, as `@uu_…` slugs.
pub(crate) struct Mentions {
    users: SlugTable,
}

impl Mentions {
    /// Built from the same cache the ticket buffer uses, after the issue
    /// and its comments were fetched (both mirror their users into it) and
    /// every `[~name]` in `bodies` was looked up. A failed fetch only
    /// narrows the table: an unknown slug is then refused on the way back,
    /// never sent.
    pub(crate) async fn load(
        client: &JiraClient,
        cache: &Mutex<JiraCache>,
        issue_key: &str,
        bodies: &[&str],
    ) -> Self {
        let _ = fetch_issue(client, cache, issue_key).await;
        let comments = fetch_comments(client, cache, issue_key)
            .await
            .unwrap_or_default();
        let mut sources: Vec<&str> = comments.iter().map(|c| c.body.as_str()).collect();
        sources.extend_from_slice(bodies);
        resolve_unknown_mentions(client, cache, &sources).await;
        Self::from_table(build_slug_tables(cache).users)
    }

    pub(super) fn from_table(users: SlugTable) -> Self {
        Self { users }
    }
}

/// Mention layer, out: `[~name]` → `@uu_slug`. A name without a slug stays
/// as it is.
pub(super) fn mentions_out(wiki: &str, users: &SlugTable) -> String {
    render_user_mentions(wiki.trim_end(), users)
}

/// Mention layer, back: `@uu_slug` → `[~name]`. `Err` carries the first
/// slug that names no known user.
pub(super) fn mentions_in(text: &str, users: &SlugTable) -> Result<String, String> {
    parse_user_mentions(text, users)
}

/// Markup layer, out: Jira wiki markup → Markdown.
pub(super) fn markup_out(wiki: &str) -> String {
    wiki_md::wiki_to_md(wiki)
}

/// Markup layer, back: Markdown → Jira wiki markup.
pub(super) fn markup_in(md: &str) -> String {
    wiki_md::md_to_wiki(md)
}

/// The stored body as the editor shows it.
pub(crate) fn to_editor(wiki: &str, dialect: Dialect, mentions: &Mentions) -> String {
    let text = mentions_out(wiki, &mentions.users);
    match dialect {
        Dialect::Markdown => markup_out(&text),
        Dialect::Jira => text,
    }
}

/// The edited text as Jira stores it. `Err` carries the message for an
/// unknown `@uu_…` mention — nothing may be sent then.
pub(crate) fn from_editor(
    text: &str,
    dialect: Dialect,
    mentions: &Mentions,
) -> Result<String, String> {
    let wiki = match dialect {
        Dialect::Markdown => markup_in(text),
        Dialect::Jira => text.to_string(),
    };
    mentions_in(&wiki, &mentions.users).map_err(|slug| unknown_mention(&slug))
}

/// The one wording for a mention that resolves to no user.
pub(super) fn unknown_mention(slug: &str) -> String {
    format!("unknown user mention `@{slug}` in comment")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mentions() -> Mentions {
        Mentions::from_table(SlugTable::build(
            [("Doe, Alice (ACME)".to_string(), "adoe".to_string())],
            super::super::slugs::USER_PREFIX,
        ))
    }

    fn slug() -> String {
        let m = mentions();
        mentions_out("[~adoe]", &m.users)
            .trim_start_matches('@')
            .to_string()
    }

    #[test]
    fn markdown_out_renders_mention_and_markup() {
        let out = to_editor(
            "h5. Titel\n\n[~adoe] ein *fetter* Hinweis",
            Dialect::Markdown,
            &mentions(),
        );
        assert_eq!(
            out,
            format!("##### Titel\n\n@{} ein **fetter** Hinweis", slug())
        );
    }

    #[test]
    fn markdown_back_resolves_mention_and_markup() {
        let text = format!(
            "##### Titel\n\n@{} ein **fetter** Hinweis\n\n1. erstens",
            slug()
        );
        let wiki = from_editor(&text, Dialect::Markdown, &mentions()).unwrap();
        assert_eq!(
            wiki,
            "h5. Titel\n\n[~adoe] ein *fetter* Hinweis\n\n# erstens"
        );
    }

    #[test]
    fn jira_dialect_resolves_mentions_but_keeps_markup() {
        let text = format!("@{} nicht ~~weg~~", slug());
        let wiki = from_editor(&text, Dialect::Jira, &mentions()).unwrap();
        assert_eq!(wiki, "[~adoe] nicht ~~weg~~");
    }

    #[test]
    fn unknown_mention_is_refused() {
        let err = from_editor("@uu_nobody hallo", Dialect::Markdown, &mentions()).unwrap_err();
        assert!(err.contains("@uu_nobody"), "{err}");
    }

    #[test]
    fn a_body_survives_the_round_trip() {
        let wiki = "h5. Titel\n\n[~adoe] ein *fetter* Hinweis";
        let m = mentions();
        let back = from_editor(
            &to_editor(wiki, Dialect::Markdown, &m),
            Dialect::Markdown,
            &m,
        );
        assert_eq!(back.unwrap(), wiki);
    }

    #[test]
    fn dialect_follows_the_round_trip() {
        assert_eq!(Dialect::for_body("h3. Titel"), Dialect::Markdown);
        assert_eq!(Dialect::for_body("nicht ~~weg~~ damit"), Dialect::Jira);
    }
}
