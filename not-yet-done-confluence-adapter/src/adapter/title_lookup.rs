//! `page_by_title` action — the page of one exact title in one space,
//! listed on the Confluence root.
//!
//! [`find`](super::root_actions) answers "which pages mention this",
//! matching title *or* text, word-wise, across every configured space.
//! That is the right shape for a person searching and the wrong one for
//! a caller asking "does this space already hold a page of exactly this
//! title?" — the question a create path has to answer before it posts,
//! because a space allows one page per title and the second one is
//! rejected by the server after the fact.
//!
//! The answer is a listing, not a node dispatch: the caller wants the
//! id and the URL of what it found (to point a person at the page that
//! is already there), and those are what a row carries.

use std::collections::HashMap;

use not_yet_done_content::*;

use crate::client::ConfluenceClient;

use super::other_err;

/// The root's `page_by_title` action: a space key and a title, both
/// required.
pub(super) fn page_by_title_action() -> NodeAction {
    NodeAction::new(
        "page_by_title",
        "Page of an exact title in a space",
        InputSpec::Form {
            fields: vec![
                FormFieldSpec::text("space", "Space key (e.g. DEMO)"),
                FormFieldSpec::text("title", "Exact page title"),
            ],
        },
    )
}

/// `execute("page_by_title")` — one line per hit, `id<TAB>title<TAB>url`,
/// and nothing but a `#` comment line when the title is free. Tab-separated
/// for the same reason as every other listing action: a title may contain
/// spaces, and the answer is read by scripts as often as by people.
///
/// The URL is absolute, because the caller that reports "this title is
/// taken, here it is" has no other way to build one.
pub(super) async fn execute_page_by_title(
    client: &ConfluenceClient,
    base_url: &str,
    values: &HashMap<String, String>,
) -> Result<ActionOutcome> {
    let space = required(values, "space")?;
    let title = required(values, "title")?;

    let pages = client
        .find_pages_by_title(space, title)
        .await
        .map_err(other_err)?;

    let listing = if pages.is_empty() {
        format!("# no page titled \"{title}\" in {space}")
    } else {
        pages
            .iter()
            .map(|p| {
                let url = if p.webui.is_empty() {
                    format!("{base_url}/pages/viewpage.action?pageId={}", p.id)
                } else {
                    format!("{base_url}{}", p.webui)
                };
                format!("{}\t{}\t{}", p.id, p.title, url)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    Ok(ActionOutcome::Done {
        message: Some(listing),
    })
}

fn required<'a>(values: &'a HashMap<String, String>, key: &str) -> Result<&'a str> {
    values
        .get(key)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| other_err(format!("{key} is required")))
}
