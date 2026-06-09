//! `GET /rest/api/space/{KEY}/content/page` and
//! `GET /rest/api/content/{id}/child/page` — both return the same Confluence
//! pagination envelope as `/space` (see [`super::space`]). The shape diverges
//! only in the row payload, so the envelope + pagination handling is shared
//! between the two calls in this module.
//!
//! CF-5 adds the single-page detail endpoint
//! `GET /rest/api/content/{id}?expand=body.storage,version,ancestors,metadata.labels`
//! which returns a richer record with the page body, version stash, and
//! ancestor/label metadata.
//!
//! Pages live in a single global namespace by id — there is no `space` prefix
//! needed to disambiguate them, so the `id` field is sufficient to identify a
//! page across calls. The `_links.webui` value is captured at list-time so
//! the open-in-browser action can spawn `xdg-open` without a second REST
//! round-trip.

use serde::{Deserialize, Serialize};

use not_yet_done_content::http_log;

use super::ConfluenceClient;

/// Outcome of [`ConfluenceClient::update_page`]. `version` is the
/// `version.number` Confluence assigned to the new revision — the next
/// edit must PUT with `version + 1` again.
#[derive(Debug)]
pub struct UpdatedPage {
    pub version: i64,
}

/// Outcome of [`ConfluenceClient::create_page`]. `id` is the new page's
/// Confluence id (numeric string); `webui` is the server-relative web
/// URL (`/spaces/.../pages/.../Title`), empty when Confluence omitted
/// the `_links.webui` field on the create response.
#[derive(Debug)]
pub struct CreatedPage {
    pub id: String,
    pub webui: String,
}

/// Why a PUT to `/rest/api/content/{id}` failed. The two-variant split
/// lets the adapter route 409 to its conflict-merge path while every
/// other failure mode (network, 4xx, 5xx) collapses into a single
/// transport error.
#[derive(Debug)]
pub enum UpdatePageError {
    /// HTTP 409 — page was modified upstream between fetch and PUT.
    /// Confluence's body is included for diagnostic logging; the adapter
    /// re-fetches the latest state for the actual merge.
    Conflict(String),
    /// Any other failure (network, non-200, parse). Already formatted
    /// for surfacing via `ContentError::Other`.
    Other(String),
}

impl std::fmt::Display for UpdatePageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict(msg) => write!(f, "conflict: {msg}"),
            Self::Other(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for UpdatePageError {}

/// Subset of the JSON record returned by the page-listing endpoints.
/// Confluence emits more fields (extensions, status, restrictions, …); we
/// only consume what the adapter needs for the tree rendering.
#[derive(Deserialize, Clone, Debug)]
pub struct PageMeta {
    /// Confluence Server returns page ids as numeric strings (e.g. `"12345"`).
    /// Kept as `String` so the node id can be passed straight to subsequent
    /// `/content/{id}` calls without re-stringifying.
    pub id: String,
    pub title: String,
    /// `"page"` for our two endpoints — captured for downstream filtering /
    /// debug-rendering, never assumed.
    #[serde(rename = "type", default)]
    pub page_type: String,
    /// `_links.webui` value — server-relative path like `/spaces/DEMO/pages/12345/Title`.
    /// Empty when Confluence omitted the field.
    #[serde(default, rename = "_links", deserialize_with = "deserialize_webui")]
    pub webui: String,
    /// `Some(true)` when the listing carried `children.page.size > 0`,
    /// `Some(false)` when `children.page.size == 0`, `None` when the
    /// listing didn't request `?expand=children.page.size` or the
    /// server returned only the `_expandable` placeholder. Confluence
    /// Server silently drops `childTypes.page` against the
    /// `/content/{id}/child/page` listing endpoint (verified on
    /// 2026-06-02 against a live Server install), so `children.page.size`
    /// is the only listing-side hook that reliably reports per-row
    /// child counts. The TUI uses this to fill `NodeSummary.has_children`
    /// so the tree renderer can pick the leaf vs. expand glyph even
    /// inside a `recursive: true` ChildDef where the static config
    /// check would always say "expandable".
    #[serde(
        default,
        rename = "children",
        deserialize_with = "deserialize_children_page_size"
    )]
    pub has_children: Option<bool>,
}

fn deserialize_webui<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct Links {
        #[serde(default)]
        webui: Option<String>,
    }
    let links = Links::deserialize(deserializer)?;
    Ok(links.webui.unwrap_or_default())
}

fn deserialize_children_page_size<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Expanded shape is
    // `{ "page": { "size": <int>, "results": [...], ... }, ... }`.
    // The unexpanded shape (server returned the field as `_expandable`
    // pointer only) is `{ "_expandable": {...}, "_links": {...} }`
    // without a `page` key — that collapses to `None` so we don't lie
    // about leaf-ness when the expand was silently dropped.
    let value: serde_json::Value = serde_json::Value::deserialize(deserializer)?;
    if value.is_null() {
        return Ok(None);
    }
    Ok(value
        .get("page")
        .and_then(|p| p.get("size"))
        .and_then(|v| v.as_u64())
        .map(|size| size > 0))
}

/// Full page record returned by `GET /content/{id}` with the standard CF-5
/// expand set. Captures the body, version, ancestor chain, and label list
/// in a single round-trip so the page node can hydrate lazily.
#[derive(Clone, Debug)]
pub struct PageDetail {
    pub id: String,
    pub title: String,
    pub page_type: String,
    /// Server-relative web URL (`/spaces/.../pages/.../Title`). Empty when
    /// Confluence omitted the `_links.webui` field.
    pub webui: String,
    /// `body.storage.value` — the canonical XHTML-ish storage format.
    /// Empty when the page is empty *or* when the expand parameter was
    /// silently ignored (the get_page helper always requests it, so the
    /// latter only happens against a misconfigured server).
    pub body_storage: String,
    /// `version.number`. Stashed for conflict-detection in CF-9 (`PUT`
    /// requires `version.number + 1`).
    pub version: i64,
    /// `ancestors[]` in root-to-parent order — the immediate parent is the
    /// last element. Each entry is `(id, title)`; empty for top-level pages.
    pub ancestors: Vec<PageAncestor>,
    /// `metadata.labels.results[].name` — flat list of label names. Empty
    /// when the page has none.
    pub labels: Vec<String>,
    /// `space.key` — the space this page lives in. Confluence always
    /// returns it on `GET /content/{id}` without explicit expand; CF-10
    /// needs it to address the `create-child` POST.
    pub space_key: String,
}

#[derive(Clone, Debug)]
pub struct PageAncestor {
    pub id: String,
    pub title: String,
}

/// Wire format for `GET /content/{id}?expand=...`. Internal — the public
/// surface is [`PageDetail`].
#[derive(Deserialize, Debug)]
struct PageDetailWire {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default, rename = "type")]
    page_type: String,
    #[serde(default)]
    body: Option<PageDetailBody>,
    #[serde(default)]
    version: Option<PageVersionWire>,
    #[serde(default)]
    ancestors: Vec<PageAncestorWire>,
    #[serde(default)]
    metadata: Option<PageMetadataWire>,
    #[serde(default, rename = "_links")]
    links: Option<PageLinksWire>,
    #[serde(default)]
    space: Option<PageSpaceWire>,
}

#[derive(Deserialize, Debug)]
struct PageSpaceWire {
    #[serde(default)]
    key: String,
}

#[derive(Deserialize, Debug)]
struct PageDetailBody {
    #[serde(default)]
    storage: Option<PageStorageWire>,
}

#[derive(Deserialize, Debug)]
struct PageStorageWire {
    #[serde(default)]
    value: String,
}

#[derive(Deserialize, Debug)]
struct PageVersionWire {
    #[serde(default)]
    number: i64,
}

#[derive(Deserialize, Debug)]
struct PageAncestorWire {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
}

#[derive(Deserialize, Debug, Default)]
struct PageMetadataWire {
    #[serde(default)]
    labels: Option<PageLabelsWire>,
}

#[derive(Deserialize, Debug, Default)]
struct PageLabelsWire {
    #[serde(default)]
    results: Vec<PageLabelWire>,
}

#[derive(Deserialize, Debug)]
struct PageLabelWire {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize, Debug, Default)]
struct PageLinksWire {
    #[serde(default)]
    webui: Option<String>,
}

/// Wire shape for the PUT body — mirrors what `conf-edit` produces and
/// what Confluence's docs spell out. All fields are required; omitting
/// `title` or `body.storage` would silently clear them on the server
/// side (Confluence treats this as a full content replacement).
#[derive(Serialize, Debug)]
struct UpdatePageBody<'a> {
    version: UpdateVersionBody,
    #[serde(rename = "type")]
    page_type: &'a str,
    title: &'a str,
    body: UpdateBodyOuter<'a>,
}

#[derive(Serialize, Debug)]
struct UpdateVersionBody {
    number: i64,
}

#[derive(Serialize, Debug)]
struct UpdateBodyOuter<'a> {
    storage: UpdateStorageBody<'a>,
}

#[derive(Serialize, Debug)]
struct UpdateStorageBody<'a> {
    value: &'a str,
    representation: &'a str,
}

/// Wire shape for the create POST. `space.key` is required even when
/// `ancestors` is set; Confluence rejects the call with a 400 if it's
/// missing. `ancestors` is omitted (rather than sent as an empty list)
/// for top-level pages — sending `[]` makes Confluence reject the call.
#[derive(Serialize, Debug)]
struct CreatePageBody<'a> {
    #[serde(rename = "type")]
    page_type: &'a str,
    title: &'a str,
    space: CreateSpaceBody<'a>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ancestors: Vec<CreateAncestorBody<'a>>,
    body: UpdateBodyOuter<'a>,
}

#[derive(Serialize, Debug)]
struct CreateSpaceBody<'a> {
    key: &'a str,
}

#[derive(Serialize, Debug)]
struct CreateAncestorBody<'a> {
    id: &'a str,
}

#[derive(Deserialize, Debug)]
struct PageEnvelope {
    #[serde(default)]
    results: Vec<PageMeta>,
    #[serde(default)]
    start: u32,
    #[serde(default)]
    limit: u32,
    #[serde(default)]
    size: u32,
    #[serde(default, rename = "_links")]
    links: EnvelopeLinks,
}

#[derive(Deserialize, Debug, Default)]
struct EnvelopeLinks {
    #[serde(default)]
    next: Option<String>,
}

/// One page (heh) of [`PageMeta`] plus the pagination state needed to render
/// next/prev affordances. `total` is omitted because the Confluence Server
/// page-listing endpoints don't report it.
#[derive(Debug)]
pub struct PageList {
    pub pages: Vec<PageMeta>,
    pub start: u32,
    pub limit: u32,
    /// Number of records on this page (`< limit` only on the last page).
    pub size: u32,
    /// True iff Confluence included a `_links.next` field on the envelope.
    pub has_next: bool,
}

impl ConfluenceClient {
    /// `GET /rest/api/space/{KEY}/content/page?start={start}&limit={limit}` —
    /// returns the top-level pages of `space_key` (direct children of the
    /// space, not of any specific page).
    pub async fn list_top_pages(
        &self,
        space_key: &str,
        start: u32,
        limit: u32,
    ) -> Result<PageList, String> {
        // `expand=childTypes.page` so PageMeta.has_children is populated
        // per row — the tree renderer needs it to distinguish leaves
        // inside the `recursive: true` ChildDef. Confluence Server
        // documents the expand on `/content` endpoints; missing it
        // gracefully degrades to `has_children: None` (static fallback).
        let url = format!(
            "{}/rest/api/space/{space_key}/content/page?start={start}&limit={limit}&expand=children.page.size",
            self.base_url()
        );
        self.fetch_page_envelope(&url).await
    }

    /// `GET /rest/api/content/{id}/child/page?start={start}&limit={limit}` —
    /// returns the direct child pages of `parent_id`.
    pub async fn list_child_pages(
        &self,
        parent_id: &str,
        start: u32,
        limit: u32,
    ) -> Result<PageList, String> {
        let url = format!(
            "{}/rest/api/content/{parent_id}/child/page?start={start}&limit={limit}&expand=children.page.size",
            self.base_url()
        );
        self.fetch_page_envelope(&url).await
    }

    /// `GET /rest/api/content/{id}?expand=body.storage,version,ancestors,metadata.labels` —
    /// fetches the full page detail. Used by the page node's lazy hydration
    /// on `read()` / preview-toggle. The expand set is hard-coded because
    /// CF-5+ unconditionally needs all four sub-objects:
    /// - `body.storage` for the preview pane (and CF-9 edit buffer)
    /// - `version` to stash for conflict detection on PUT (CF-9)
    /// - `ancestors` so future breadcrumb rendering doesn't need a second call
    /// - `metadata.labels` for the future labels column / search
    pub async fn get_page(&self, id: &str) -> Result<PageDetail, String> {
        let url = format!(
            "{}/rest/api/content/{id}?expand=body.storage,version,ancestors,metadata.labels",
            self.base_url()
        );
        http_log::log_request("GET", &url);
        let resp = self
            .inner_http()
            .get(&url)
            .send()
            .await
            .map_err(|e| http_log::network_error("GET", &url, e))?;
        let resp = http_log::check_status("GET", &url, resp).await?;
        let body = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;
        let wire: PageDetailWire = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse page detail: {e}"))?;
        Ok(PageDetail {
            id: wire.id,
            title: wire.title,
            page_type: wire.page_type,
            webui: wire.links.and_then(|l| l.webui).unwrap_or_default(),
            body_storage: wire
                .body
                .and_then(|b| b.storage)
                .map(|s| s.value)
                .unwrap_or_default(),
            version: wire.version.map(|v| v.number).unwrap_or_default(),
            ancestors: wire
                .ancestors
                .into_iter()
                .map(|a| PageAncestor {
                    id: a.id,
                    title: a.title,
                })
                .collect(),
            labels: wire
                .metadata
                .and_then(|m| m.labels)
                .map(|l| l.results.into_iter().map(|x| x.name).collect())
                .unwrap_or_default(),
            space_key: wire.space.map(|s| s.key).unwrap_or_default(),
        })
    }

    /// `PUT /rest/api/content/{id}` — write a new revision of the page.
    /// `version_next` must be `current.version.number + 1`; the server
    /// returns 409 if the upstream version moved in the meantime.
    ///
    /// The body shape is fixed: `{version:{number}, type:"page", title,
    /// body:{storage:{value, representation:"storage"}}}`. Both `title`
    /// and `body.storage` must be present even when only one of them
    /// changed — Confluence treats omitted fields as cleared.
    pub async fn update_page(
        &self,
        id: &str,
        version_next: i64,
        title: &str,
        body_storage: &str,
    ) -> Result<UpdatedPage, UpdatePageError> {
        let url = format!("{}/rest/api/content/{id}", self.base_url());
        http_log::log_request("PUT", &url);
        let body = UpdatePageBody {
            version: UpdateVersionBody {
                number: version_next,
            },
            page_type: "page",
            title,
            body: UpdateBodyOuter {
                storage: UpdateStorageBody {
                    value: body_storage,
                    representation: "storage",
                },
            },
        };
        let resp = self
            .inner_http()
            .put(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| UpdatePageError::Other(http_log::network_error("PUT", &url, e)))?;
        let status = resp.status();
        http_log::log_response("PUT", &url, status.as_u16());
        if status.as_u16() == 409 {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(UpdatePageError::Conflict(body_text));
        }
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(UpdatePageError::Other(format!(
                "PUT {url} -> {} {}: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or(""),
                body_text.chars().take(500).collect::<String>(),
            )));
        }
        let body_text = resp
            .text()
            .await
            .map_err(|e| UpdatePageError::Other(format!("Failed to read response: {e}")))?;
        let wire: PageDetailWire = serde_json::from_str(&body_text).map_err(|e| {
            UpdatePageError::Other(format!("Failed to parse update_page response: {e}"))
        })?;
        Ok(UpdatedPage {
            version: wire.version.map(|v| v.number).unwrap_or_default(),
        })
    }

    /// `POST /rest/api/content` — create a new page in `space_key`. When
    /// `parent_id` is `Some`, the page becomes a child of that page;
    /// when `None`, it's a top-level page of the space.
    ///
    /// Body shape: `{type:"page", title, space:{key}, ancestors:[{id}]?,
    /// body:{storage:{value, representation:"storage"}}}`. The server
    /// returns the freshly-minted page including its assigned `id` and
    /// `_links.webui`; both are propagated back to the caller so the TUI
    /// can navigate / open it without a follow-up GET.
    pub async fn create_page(
        &self,
        space_key: &str,
        parent_id: Option<&str>,
        title: &str,
        body_storage: &str,
    ) -> Result<CreatedPage, String> {
        let url = format!("{}/rest/api/content", self.base_url());
        http_log::log_request("POST", &url);
        let ancestors = match parent_id {
            Some(id) => vec![CreateAncestorBody { id }],
            None => Vec::new(),
        };
        let body = CreatePageBody {
            page_type: "page",
            title,
            space: CreateSpaceBody { key: space_key },
            ancestors,
            body: UpdateBodyOuter {
                storage: UpdateStorageBody {
                    value: body_storage,
                    representation: "storage",
                },
            },
        };
        let resp = self
            .inner_http()
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| http_log::network_error("POST", &url, e))?;
        let resp = http_log::check_status("POST", &url, resp).await?;
        let body_text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;
        let wire: PageDetailWire = serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse create_page response: {e}"))?;
        Ok(CreatedPage {
            id: wire.id,
            webui: wire.links.and_then(|l| l.webui).unwrap_or_default(),
        })
    }

    /// `DELETE /rest/api/content/{id}` — move a page to the Trash. The
    /// `purge` flag, when true, sends `?status=trashed`, which Confluence
    /// interprets as "permanently delete an already-trashed page". The
    /// adapter currently only exposes the soft delete (`purge=false`) via
    /// the UI; the purge path is here for completeness and future use
    /// (e.g. a "Trash"-subtab → empty action). Callers must double-check
    /// this is what they want — purge is irreversible.
    ///
    /// Both paths return HTTP 204 on success. 4xx/5xx and network errors
    /// collapse into a single `Err(String)` ready for surfacing via
    /// `ContentError::Other`.
    pub async fn delete_page(&self, id: &str, purge: bool) -> Result<(), String> {
        let url = if purge {
            format!("{}/rest/api/content/{id}?status=trashed", self.base_url())
        } else {
            format!("{}/rest/api/content/{id}", self.base_url())
        };
        http_log::log_request("DELETE", &url);
        let resp = self
            .inner_http()
            .delete(&url)
            .send()
            .await
            .map_err(|e| http_log::network_error("DELETE", &url, e))?;
        let status = resp.status();
        http_log::log_response("DELETE", &url, status.as_u16());
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "DELETE {url} -> {} {}: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or(""),
                body_text.chars().take(500).collect::<String>(),
            ));
        }
        Ok(())
    }

    async fn fetch_page_envelope(&self, url: &str) -> Result<PageList, String> {
        http_log::log_request("GET", url);
        let resp = self
            .inner_http()
            .get(url)
            .send()
            .await
            .map_err(|e| http_log::network_error("GET", url, e))?;
        let resp = http_log::check_status("GET", url, resp).await?;
        let body = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;
        let env: PageEnvelope = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse pages response: {e}"))?;
        Ok(PageList {
            pages: env.results,
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
    fn parses_full_envelope_with_next_link() {
        let body = r#"{
            "results": [
                {
                    "id": "100",
                    "type": "page",
                    "title": "Top Page",
                    "_links": { "webui": "/spaces/DEMO/pages/100/Top+Page" }
                },
                {
                    "id": "101",
                    "type": "page",
                    "title": "Second"
                }
            ],
            "start": 0,
            "limit": 25,
            "size": 2,
            "_links": { "next": "/rest/api/space/DEMO/content/page?start=25&limit=25" }
        }"#;
        let env: PageEnvelope = serde_json::from_str(body).expect("parses");
        assert_eq!(env.results.len(), 2);
        assert_eq!(env.results[0].id, "100");
        assert_eq!(env.results[0].title, "Top Page");
        assert_eq!(env.results[0].page_type, "page");
        assert_eq!(env.results[0].webui, "/spaces/DEMO/pages/100/Top+Page");
        assert_eq!(env.results[1].webui, "", "missing webui defaults to empty");
        assert_eq!(env.start, 0);
        assert_eq!(env.size, 2);
        assert!(env.links.next.is_some());
    }

    #[test]
    fn page_meta_extracts_children_page_size() {
        // `?expand=children.page.size` expands the whole `children.page`
        // object — we only consume `.size` and derive
        // `has_children = size > 0`. The unexpanded shape (server
        // returned only `_expandable` placeholders) collapses to `None`
        // so the tree-renderer falls back to its static config check.
        let parent = r#"{
            "id": "1",
            "type": "page",
            "title": "Parent with two child pages",
            "children": {
                "page": {
                    "results": [{"id":"10"},{"id":"11"}],
                    "size": 2,
                    "start": 0,
                    "limit": 25
                },
                "_links": {}
            }
        }"#;
        let m: PageMeta = serde_json::from_str(parent).expect("parses");
        assert_eq!(m.has_children, Some(true));

        let leaf = r#"{
            "id": "2",
            "type": "page",
            "title": "Leaf",
            "children": {
                "page": {
                    "results": [],
                    "size": 0,
                    "start": 0,
                    "limit": 25
                },
                "_links": {}
            }
        }"#;
        let m: PageMeta = serde_json::from_str(leaf).expect("parses");
        assert_eq!(m.has_children, Some(false));

        let no_expand = r#"{
            "id": "3",
            "type": "page",
            "title": "Children only available as _expandable"
        }"#;
        let m: PageMeta = serde_json::from_str(no_expand).expect("parses");
        assert_eq!(m.has_children, None);

        // Server returned `children` but without expanding `page` —
        // must not panic, must not invent a `Some(_)`.
        let unexpanded_page = r#"{
            "id": "4",
            "type": "page",
            "title": "children present but page placeholder only",
            "children": {
                "_expandable": { "page": "/rest/api/content/4/child/page" },
                "_links": {}
            }
        }"#;
        let m: PageMeta = serde_json::from_str(unexpanded_page).expect("parses");
        assert_eq!(m.has_children, None);
    }

    #[test]
    fn parses_full_page_detail() {
        let body = r#"{
            "id": "12345",
            "type": "page",
            "title": "Sample",
            "body": {
                "storage": {
                    "value": "<p>hello</p>",
                    "representation": "storage"
                }
            },
            "version": { "number": 7, "when": "2026-01-01T00:00:00Z" },
            "ancestors": [
                { "id": "100", "title": "Root" },
                { "id": "200", "title": "Section" }
            ],
            "metadata": {
                "labels": {
                    "results": [ { "name": "foo" }, { "name": "bar" } ]
                }
            },
            "_links": { "webui": "/spaces/DEMO/pages/12345/Sample" }
        }"#;
        let wire: PageDetailWire = serde_json::from_str(body).expect("parses");
        assert_eq!(wire.id, "12345");
        assert_eq!(wire.title, "Sample");
        assert_eq!(wire.page_type, "page");
        assert_eq!(
            wire.body.and_then(|b| b.storage).map(|s| s.value).unwrap(),
            "<p>hello</p>"
        );
        assert_eq!(wire.version.unwrap().number, 7);
        assert_eq!(wire.ancestors.len(), 2);
        assert_eq!(wire.ancestors[1].title, "Section");
        let labels: Vec<_> = wire
            .metadata
            .and_then(|m| m.labels)
            .map(|l| l.results.into_iter().map(|x| x.name).collect())
            .unwrap();
        assert_eq!(labels, vec!["foo".to_string(), "bar".to_string()]);
    }

    #[test]
    fn parses_minimal_page_detail_without_body_or_metadata() {
        // Server may omit expand sub-objects when permission is reduced —
        // hydration must still succeed with empties.
        let body = r#"{
            "id": "12345",
            "type": "page",
            "title": "Bare"
        }"#;
        let wire: PageDetailWire = serde_json::from_str(body).expect("parses");
        assert_eq!(wire.id, "12345");
        assert!(wire.body.is_none());
        assert!(wire.version.is_none());
        assert!(wire.ancestors.is_empty());
        assert!(wire.metadata.is_none());
        assert!(wire.links.is_none());
    }

    #[test]
    fn update_page_body_serializes_to_expected_shape() {
        // Locks in the JSON shape that Confluence's PUT endpoint expects.
        // If this ever drifts (field renamed, sub-object missing), the
        // server returns 400 with an opaque message — better to catch it
        // at compile/test time.
        let body = UpdatePageBody {
            version: UpdateVersionBody { number: 8 },
            page_type: "page",
            title: "Hello",
            body: UpdateBodyOuter {
                storage: UpdateStorageBody {
                    value: "<p>x</p>",
                    representation: "storage",
                },
            },
        };
        let json: serde_json::Value = serde_json::to_value(&body).expect("serializes");
        assert_eq!(json["version"]["number"], 8);
        assert_eq!(json["type"], "page");
        assert_eq!(json["title"], "Hello");
        assert_eq!(json["body"]["storage"]["value"], "<p>x</p>");
        assert_eq!(json["body"]["storage"]["representation"], "storage");
        // `type` must be at the top level, not under `body`.
        assert!(json["body"].get("type").is_none());
    }

    #[test]
    fn create_page_body_top_level_serializes_to_expected_shape() {
        // Top-level page: `space.key` is required, `ancestors` must be
        // *omitted* (sending an empty array makes Confluence 400).
        let body = CreatePageBody {
            page_type: "page",
            title: "New Page",
            space: CreateSpaceBody { key: "DEMO" },
            ancestors: Vec::new(),
            body: UpdateBodyOuter {
                storage: UpdateStorageBody {
                    value: "<p>hello</p>",
                    representation: "storage",
                },
            },
        };
        let json: serde_json::Value = serde_json::to_value(&body).expect("serializes");
        assert_eq!(json["type"], "page");
        assert_eq!(json["title"], "New Page");
        assert_eq!(json["space"]["key"], "DEMO");
        assert_eq!(json["body"]["storage"]["value"], "<p>hello</p>");
        assert_eq!(json["body"]["storage"]["representation"], "storage");
        assert!(
            json.get("ancestors").is_none(),
            "top-level create must omit ancestors entirely, got {json}"
        );
    }

    #[test]
    fn create_page_body_with_parent_serializes_ancestors() {
        let body = CreatePageBody {
            page_type: "page",
            title: "Child",
            space: CreateSpaceBody { key: "DEMO" },
            ancestors: vec![CreateAncestorBody { id: "42" }],
            body: UpdateBodyOuter {
                storage: UpdateStorageBody {
                    value: "<p>x</p>",
                    representation: "storage",
                },
            },
        };
        let json: serde_json::Value = serde_json::to_value(&body).expect("serializes");
        let ancestors = json["ancestors"].as_array().expect("ancestors array");
        assert_eq!(ancestors.len(), 1);
        assert_eq!(ancestors[0]["id"], "42");
    }

    #[test]
    fn parses_page_detail_carries_space_key() {
        let body = r#"{
            "id": "12345",
            "type": "page",
            "title": "Sample",
            "space": { "key": "DEMO" },
            "_links": { "webui": "/spaces/DEMO/pages/12345/Sample" }
        }"#;
        let wire: PageDetailWire = serde_json::from_str(body).expect("parses");
        assert_eq!(wire.space.expect("space present").key, "DEMO");
    }

    #[test]
    fn parses_minimal_envelope_without_next() {
        let body = r#"{
            "results": [
                { "id": "200", "type": "page", "title": "Lonely" }
            ],
            "start": 25,
            "limit": 25,
            "size": 1,
            "_links": {}
        }"#;
        let env: PageEnvelope = serde_json::from_str(body).expect("parses");
        assert_eq!(env.results.len(), 1);
        assert_eq!(env.results[0].webui, "");
        assert!(env.links.next.is_none());
    }
}
