//! `GET /rest/api/content/search` — CQL-driven content lookup.
//!
//! CQL (Confluence Query Language) is the page-tree analogue to JQL.
//! The endpoint returns the same Confluence envelope shape as `/space`
//! and the page-listing calls; the row payload is `content`-shaped
//! (id, type, title, plus expand-selected sub-objects).
//!
//! CF-8 first cut requests `expand=space,version`:
//! - `space.key` lets us render which space a hit belongs to.
//! - `version.when` gives the last-modified timestamp for the row, so
//!   `ORDER BY lastModified DESC` results show up in the right order
//!   for the user even without a dedicated column.
//!
//! Result type is intentionally distinct from [`super::PageMeta`]:
//! `/content/search` may yield blogposts, attachments, or comments
//! mixed in with pages, and the adapter (CF-8c) makes the
//! page-only-by-convention assumption explicit at the boundary —
//! either by relying on the saved CQL containing `type = page`, or by
//! filtering server-side via the `type` discriminator on each row.

use serde::Deserialize;

use not_yet_done_content::http_log;

use super::ConfluenceClient;

/// One row of the `/content/search` envelope. Confluence emits more
/// (body, restrictions, …) on bigger expand sets; we only consume
/// what the adapter renders in the listing — plus `ancestors` so the
/// tree-find path (CT-3) can locate each hit inside the page tree
/// without an extra round-trip per row.
#[derive(Clone, Debug)]
pub struct SearchResultMeta {
    /// Numeric content id (page / blogpost / attachment / comment).
    /// Kept as `String` so it can flow straight back into
    /// `/content/{id}` follow-up calls.
    pub id: String,
    /// `"page"`, `"blogpost"`, `"attachment"`, or `"comment"`. Stored on
    /// the row so the listing path can either filter or render the
    /// distinction.
    pub content_type: String,
    /// Page title. Comments synthesise `"Re: <page>"`; attachments use
    /// their filename. Always present.
    pub title: String,
    /// Server-relative `_links.webui` path. Empty when Confluence
    /// omitted the field — e.g. unprivileged search results.
    pub webui: String,
    /// `space.key` if `expand=space` resolved. Empty for global content
    /// or when the user lacks space visibility.
    pub space_key: String,
    /// `version.when` ISO-8601 timestamp if `expand=version` resolved.
    /// Empty when missing — never relied on for sort (server already
    /// honours `ORDER BY lastModified` in the CQL itself).
    pub last_modified: String,
    /// Ancestor chain from space root → … → direct parent, top-down
    /// (Confluence delivers them in this order). Empty for top-level
    /// pages or when `expand=ancestors` returned no value (e.g.
    /// search-result is itself a space root or the user lacks
    /// visibility into intermediate parents).
    pub ancestors: Vec<AncestorMeta>,
}

/// One ancestor in a [`SearchResultMeta::ancestors`] chain. Confluence
/// emits more (`type`, `_links`, …); we only consume what the
/// tree-find driver needs to address each parent node.
#[derive(Clone, Debug)]
pub struct AncestorMeta {
    pub id: String,
    pub title: String,
}

/// One page of [`SearchResultMeta`] plus pagination state. The CQL
/// endpoint reports the same `start` / `limit` / `size` triple as
/// `/space` and the page-listing endpoints; `total` is omitted.
#[derive(Debug)]
pub struct SearchResults {
    pub items: Vec<SearchResultMeta>,
    pub start: u32,
    pub limit: u32,
    pub size: u32,
    pub has_next: bool,
}

// ---------------------------------------------------------------------------
// Wire layer — Optional everywhere + `From` to flatten.
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug)]
struct SearchResultWire {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default, rename = "type")]
    content_type: String,
    #[serde(default, rename = "_links")]
    links: Option<SearchLinksWire>,
    #[serde(default)]
    space: Option<SearchSpaceWire>,
    #[serde(default)]
    version: Option<SearchVersionWire>,
    /// `expand=ancestors` returns an array of ancestor content stubs.
    /// Each carries id+title (and a `type` we don't need). Empty when
    /// the row has no ancestors or when the server stripped the field.
    #[serde(default)]
    ancestors: Vec<AncestorWire>,
}

#[derive(Deserialize, Debug, Default)]
struct AncestorWire {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
}

#[derive(Deserialize, Debug, Default)]
struct SearchLinksWire {
    #[serde(default)]
    webui: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct SearchSpaceWire {
    #[serde(default)]
    key: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct SearchVersionWire {
    #[serde(default)]
    when: Option<String>,
}

impl From<SearchResultWire> for SearchResultMeta {
    fn from(w: SearchResultWire) -> Self {
        Self {
            id: w.id,
            content_type: w.content_type,
            title: w.title,
            webui: w.links.and_then(|l| l.webui).unwrap_or_default(),
            space_key: w.space.and_then(|s| s.key).unwrap_or_default(),
            last_modified: w.version.and_then(|v| v.when).unwrap_or_default(),
            ancestors: w
                .ancestors
                .into_iter()
                .map(|a| AncestorMeta {
                    id: a.id,
                    title: a.title,
                })
                .collect(),
        }
    }
}

#[derive(Deserialize, Debug)]
struct SearchEnvelope {
    #[serde(default)]
    results: Vec<SearchResultWire>,
    #[serde(default)]
    start: u32,
    #[serde(default)]
    limit: u32,
    #[serde(default)]
    size: u32,
    #[serde(default, rename = "_links")]
    links: SearchEnvelopeLinks,
}

#[derive(Deserialize, Debug, Default)]
struct SearchEnvelopeLinks {
    #[serde(default)]
    next: Option<String>,
}

impl ConfluenceClient {
    /// `GET /rest/api/content/search?cql=<urlencoded>&start={start}&limit={limit}&expand=space,version,ancestors`.
    /// Returns the matched content envelope as [`SearchResults`].
    ///
    /// URL-encoding of the CQL string is delegated to `reqwest`'s
    /// `query` builder — callers pass the raw CQL.
    ///
    /// `ancestors` is always expanded: the per-row payload is small
    /// (id+title per ancestor) and the tree-find path (CT-3) consumes
    /// it without an extra round-trip. The flat search subtab simply
    /// ignores the field.
    pub async fn cql_search(
        &self,
        cql: &str,
        start: u32,
        limit: u32,
    ) -> Result<SearchResults, String> {
        let url_for_log = format!("{}/rest/api/content/search", self.base_url());
        http_log::log_request("GET", &url_for_log);
        let resp = self
            .inner_http()
            .get(format!("{}/rest/api/content/search", self.base_url()))
            .query(&[
                ("cql", cql),
                ("start", &start.to_string()),
                ("limit", &limit.to_string()),
                ("expand", "space,version,ancestors"),
            ])
            .send()
            .await
            .map_err(|e| http_log::network_error("GET", &url_for_log, e))?;
        let resp = self.check_status("GET", &url_for_log, resp).await?;
        let body = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;
        let env: SearchEnvelope = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse /content/search response: {e}"))?;
        Ok(SearchResults {
            items: env
                .results
                .into_iter()
                .map(SearchResultMeta::from)
                .collect(),
            start: env.start,
            limit: env.limit,
            size: env.size,
            has_next: env.links.next.is_some(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_cql_envelope() {
        let body = r#"{
            "results": [
                {
                    "id": "12345",
                    "type": "page",
                    "title": "Design Doc",
                    "_links": { "webui": "/spaces/DEMO/pages/12345/Design+Doc" },
                    "space": { "key": "DEMO" },
                    "version": { "when": "2026-05-30T12:00:00.000Z" },
                    "ancestors": [
                        { "id": "1000", "title": "Architecture", "type": "page" },
                        { "id": "1100", "title": "Subsystem A", "type": "page" }
                    ]
                },
                {
                    "id": "67890",
                    "type": "blogpost",
                    "title": "Release Notes",
                    "_links": { "webui": "/spaces/DEMO/blog/2026/05/29/67890/Release+Notes" },
                    "space": { "key": "DEMO" },
                    "version": { "when": "2026-05-29T08:00:00.000Z" }
                }
            ],
            "start": 0,
            "limit": 25,
            "size": 2,
            "_links": { "next": "/rest/api/content/search?cql=...&start=25&limit=25" }
        }"#;
        let env: SearchEnvelope = serde_json::from_str(body).expect("parses");
        let items: Vec<SearchResultMeta> = env
            .results
            .into_iter()
            .map(SearchResultMeta::from)
            .collect();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "12345");
        assert_eq!(items[0].content_type, "page");
        assert_eq!(items[0].title, "Design Doc");
        assert_eq!(items[0].webui, "/spaces/DEMO/pages/12345/Design+Doc");
        assert_eq!(items[0].space_key, "DEMO");
        assert_eq!(items[0].last_modified, "2026-05-30T12:00:00.000Z");
        // Ancestor chain preserved in top-down order.
        assert_eq!(items[0].ancestors.len(), 2);
        assert_eq!(items[0].ancestors[0].id, "1000");
        assert_eq!(items[0].ancestors[0].title, "Architecture");
        assert_eq!(items[0].ancestors[1].id, "1100");
        // Row without ancestors → empty vec (default).
        assert!(items[1].ancestors.is_empty());
        assert_eq!(items[1].content_type, "blogpost");
        assert!(env.links.next.is_some());
    }

    #[test]
    fn parses_minimal_row_without_expand_subobjects() {
        // Server-side `expand` honoured only partially — the row carries id +
        // type + title, but no `_links`, no `space`, no `version`. All three
        // must default to empty strings via the Wire+From flatten.
        let body = r#"{
            "results": [
                { "id": "1", "type": "page", "title": "Bare" }
            ],
            "start": 0,
            "limit": 25,
            "size": 1,
            "_links": {}
        }"#;
        let env: SearchEnvelope = serde_json::from_str(body).expect("parses");
        let items: Vec<SearchResultMeta> = env
            .results
            .into_iter()
            .map(SearchResultMeta::from)
            .collect();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "1");
        assert!(items[0].webui.is_empty());
        assert!(items[0].space_key.is_empty());
        assert!(items[0].last_modified.is_empty());
        assert!(items[0].ancestors.is_empty());
        assert!(env.links.next.is_none());
    }

    #[test]
    fn parses_empty_results_envelope() {
        let body = r#"{
            "results": [],
            "start": 0,
            "limit": 25,
            "size": 0,
            "_links": {}
        }"#;
        let env: SearchEnvelope = serde_json::from_str(body).expect("parses");
        assert!(env.results.is_empty());
        assert!(env.links.next.is_none());
    }
}
