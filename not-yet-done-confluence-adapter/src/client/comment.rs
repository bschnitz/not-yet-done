//! `GET /rest/api/content/{id}/child/comment?expand=body.storage,version`
//! — list comments attached to a page, plus CF-12 single-comment CRUD
//! (`get_comment` / `create_comment` / `update_comment` / `delete_comment`).
//!
//! The list endpoint returns the same envelope shape as `/space`,
//! `/content/{id}/child/page`, and `/content/{id}/child/attachment` (see
//! sibling modules); only the row payload diverges. Per-comment we
//! consume `body.storage.value` (rendered XHTML), `version.by.displayName`,
//! `version.when`, and `version.number` — the latter is required as the
//! optimistic-lock token on the PUT path (CF-12).
//!
//! Unlike `body.storage` on a page (CF-5), the `expand=body.storage,version`
//! parameter rides on the *list* endpoint — so listings never need a
//! per-comment detail GET. The single-comment GET ([`get_comment`]) only
//! kicks in when a comment node was synthesized via `get_child` (no
//! list context — e.g. coming from a link) and an edit needs the latest
//! body + version.

use serde::Deserialize;

use not_yet_done_content::http_log;

use super::{ConfluenceClient, UpdatePageError};

/// Subset of one comment record. Flattened from the wire format up
/// front so the adapter doesn't have to walk through `body.storage` /
/// `version` sub-objects at every call site.
#[derive(Clone, Debug)]
pub struct CommentMeta {
    /// Confluence comment id (numeric string).
    pub id: String,
    /// Auto-generated title (`Re: <page title>` for most cases). Kept
    /// for completeness; the adapter uses `body` as the user-visible
    /// label-source.
    pub title: String,
    /// `body.storage.value` — XHTML body of the comment. Empty when
    /// Confluence omitted the expand (permission-stripped or empty body).
    pub body_storage: String,
    /// `version.by.displayName`. Empty when permission-stripped.
    pub author: String,
    /// `version.when` timestamp. Empty when not provided. ISO-8601.
    pub created: String,
    /// `version.number` — required as the optimistic-lock token on the
    /// PUT path (CF-12 edit-comment). 0 when permission-stripped or when
    /// the comment was synthesized via `get_child` without a list context;
    /// adapters detect that case by checking `body_storage.is_empty() &&
    /// version_number == 0` and refetch via [`get_comment`] if so.
    pub version_number: i64,
}

/// Wire format mirroring the per-row JSON. The public surface is
/// [`CommentMeta`]; this struct exists so serde can decode the nested
/// `body.storage` / `version` objects without forcing each consumer to
/// walk through them.
#[derive(Deserialize, Debug)]
struct CommentWire {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: Option<CommentBodyWire>,
    #[serde(default)]
    version: Option<CommentVersionWire>,
}

#[derive(Deserialize, Debug)]
struct CommentBodyWire {
    #[serde(default)]
    storage: Option<CommentBodyStorageWire>,
}

#[derive(Deserialize, Debug)]
struct CommentBodyStorageWire {
    #[serde(default)]
    value: Option<String>,
}

#[derive(Deserialize, Debug)]
struct CommentVersionWire {
    #[serde(default)]
    by: Option<CommentVersionByWire>,
    #[serde(default)]
    when: Option<String>,
    #[serde(default)]
    number: i64,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct CommentVersionByWire {
    #[serde(default)]
    display_name: Option<String>,
}

impl From<CommentWire> for CommentMeta {
    fn from(w: CommentWire) -> Self {
        let body_storage = w
            .body
            .and_then(|b| b.storage)
            .and_then(|s| s.value)
            .unwrap_or_default();
        let (author, created, version_number) = w
            .version
            .map(|v| {
                let author = v.by.and_then(|b| b.display_name).unwrap_or_default();
                (author, v.when.unwrap_or_default(), v.number)
            })
            .unwrap_or_default();
        CommentMeta {
            id: w.id,
            title: w.title,
            body_storage,
            author,
            created,
            version_number,
        }
    }
}

/// Wire envelope shape for comment listings. Same layout as the page /
/// attachment envelopes — separate type so the per-row deserializer can
/// be `CommentWire`.
#[derive(Deserialize, Debug)]
struct CommentEnvelope {
    #[serde(default)]
    results: Vec<CommentWire>,
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

/// One page of [`CommentMeta`] rows plus pagination state.
#[derive(Debug)]
pub struct CommentList {
    pub comments: Vec<CommentMeta>,
    pub start: u32,
    pub limit: u32,
    pub size: u32,
    pub has_next: bool,
}

impl ConfluenceClient {
    /// `GET /rest/api/content/{id}/child/comment?expand=body.storage,version
    /// &start={start}&limit={limit}` — returns comments hanging off
    /// `page_id`. The `expand=body.storage,version` parameter pulls the
    /// full comment body in the same round-trip; without it the server
    /// returns stubs and we'd need an N+1 per-comment GET.
    pub async fn list_comments(
        &self,
        page_id: &str,
        start: u32,
        limit: u32,
    ) -> Result<CommentList, String> {
        let url = format!(
            "{}/rest/api/content/{page_id}/child/comment\
             ?expand=body.storage,version&start={start}&limit={limit}",
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
        let env: CommentEnvelope = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse comments response: {e}"))?;
        Ok(CommentList {
            comments: env.results.into_iter().map(CommentMeta::from).collect(),
            start: env.start,
            limit: env.limit,
            size: env.size,
            has_next: env.links.next.is_some(),
        })
    }

    /// `GET /rest/api/content/{comment_id}?expand=body.storage,version`
    /// — single-comment lookup with body + version. Used when a comment
    /// node was synthesized via `get_child` (no list context, e.g. via a
    /// stored link) and an edit needs the latest body + optimistic-lock
    /// token. Normal flows reuse the data from [`list_comments`] and
    /// never hit this endpoint.
    pub async fn get_comment(&self, comment_id: &str) -> Result<CommentMeta, String> {
        let url = format!(
            "{}/rest/api/content/{comment_id}?expand=body.storage,version",
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
        let wire: CommentWire = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse comment: {e}"))?;
        Ok(CommentMeta::from(wire))
    }

    /// `POST /rest/api/content` with `type=comment`. Confluence requires
    /// `container.{id,type}` so the new comment knows which page to hang
    /// off; `type:page` is hard-coded because we never let users attach a
    /// comment to a non-page container (Confluence does allow comments on
    /// blogposts, but that's out of scope here).
    ///
    /// The server response is parsed via [`CommentWire`] so the caller
    /// gets back a fully-populated [`CommentMeta`] — id, body, version,
    /// author, created. The TUI uses the new id for the success notification
    /// and lets `FollowUp::ReloadContentPane` re-fetch the comment list.
    pub async fn create_comment(
        &self,
        page_id: &str,
        body_storage: &str,
    ) -> Result<CommentMeta, String> {
        let url = format!("{}/rest/api/content", self.base_url());
        http_log::log_request("POST", &url);
        let body = serde_json::json!({
            "type": "comment",
            "container": { "id": page_id, "type": "page" },
            "body": {
                "storage": {
                    "value": body_storage,
                    "representation": "storage",
                }
            }
        });
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
        let wire: CommentWire = serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse create_comment response: {e}"))?;
        Ok(CommentMeta::from(wire))
    }

    /// `PUT /rest/api/content/{comment_id}` — write a new revision of a
    /// comment. The body shape mirrors [`super::ConfluenceClient::update_page`]
    /// but with `type:"comment"` instead of `"page"`; the title rides
    /// along unchanged because Confluence auto-generates it from the
    /// parent page (`Re: <page title>`) and omitting it on PUT clears it.
    ///
    /// On 409 the adapter routes to a simple Reopen-with-banner — no
    /// 3-way merge for comments (small bodies; manual re-edit is cheap).
    /// All other failures collapse into `UpdatePageError::Other` ready
    /// for `ContentError::Other` surfacing.
    pub async fn update_comment(
        &self,
        comment_id: &str,
        version_next: i64,
        title: &str,
        body_storage: &str,
    ) -> Result<i64, UpdatePageError> {
        let url = format!("{}/rest/api/content/{comment_id}", self.base_url());
        http_log::log_request("PUT", &url);
        let body = serde_json::json!({
            "version": { "number": version_next },
            "type": "comment",
            "title": title,
            "body": {
                "storage": {
                    "value": body_storage,
                    "representation": "storage",
                }
            }
        });
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
        let wire: CommentWire = serde_json::from_str(&body_text).map_err(|e| {
            UpdatePageError::Other(format!("Failed to parse update_comment response: {e}"))
        })?;
        Ok(wire.version.map(|v| v.number).unwrap_or_default())
    }

    /// `DELETE /rest/api/content/{comment_id}` — drop a comment. Unlike
    /// [`super::ConfluenceClient::delete_page`] this has no trash-vs-purge
    /// distinction: comments don't surface in Confluence's Trash UI, so
    /// the DELETE is final on the first call. Confirm-popup handling lives
    /// TUI-side (the same generic `ConfirmDeleteContentNode` path that
    /// CF-11 added for pages).
    pub async fn delete_comment(&self, comment_id: &str) -> Result<(), String> {
        let url = format!("{}/rest/api/content/{comment_id}", self.base_url());
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_comment_envelope() {
        let body = r#"{
            "results": [
                {
                    "id": "c1001",
                    "type": "comment",
                    "title": "Re: Design Doc",
                    "body": {
                        "storage": {
                            "value": "<p>Looks good to me.</p>",
                            "representation": "storage"
                        }
                    },
                    "version": {
                        "by": { "displayName": "Bob Example" },
                        "when": "2026-05-15T14:22:00.000Z",
                        "number": 3
                    }
                }
            ],
            "start": 0,
            "limit": 25,
            "size": 1,
            "_links": { "next": "/rest/api/content/12345/child/comment?start=25&limit=25" }
        }"#;
        let env: CommentEnvelope = serde_json::from_str(body).expect("parses");
        assert_eq!(env.results.len(), 1);
        let comment: CommentMeta = env.results.into_iter().next().unwrap().into();
        assert_eq!(comment.id, "c1001");
        assert_eq!(comment.title, "Re: Design Doc");
        assert_eq!(comment.body_storage, "<p>Looks good to me.</p>");
        assert_eq!(comment.author, "Bob Example");
        assert_eq!(comment.created, "2026-05-15T14:22:00.000Z");
        assert_eq!(comment.version_number, 3);
        assert!(env.links.next.is_some());
    }

    #[test]
    fn parses_minimal_comment_without_body_or_version() {
        // Server may strip body.storage / version sub-objects when the
        // cookie user lacks read-permission on the comment metadata
        // (or when `?expand=` was dropped) — hydration must still
        // succeed with empty defaults.
        let body = r#"{
            "results": [
                {
                    "id": "c1",
                    "type": "comment",
                    "title": "Re: Stub"
                }
            ],
            "start": 0,
            "limit": 25,
            "size": 1,
            "_links": {}
        }"#;
        let env: CommentEnvelope = serde_json::from_str(body).expect("parses");
        let comment: CommentMeta = env.results.into_iter().next().unwrap().into();
        assert_eq!(comment.id, "c1");
        assert_eq!(comment.title, "Re: Stub");
        assert!(comment.body_storage.is_empty());
        assert!(comment.author.is_empty());
        assert!(comment.created.is_empty());
        assert_eq!(comment.version_number, 0);
        assert!(env.links.next.is_none());
    }

    #[test]
    fn create_comment_body_shape_serializes_with_container() {
        // Lock in the JSON shape the POST endpoint expects. Container must
        // carry both `id` and `type:"page"`; omitting `type` makes
        // Confluence reject the call.
        let body = serde_json::json!({
            "type": "comment",
            "container": { "id": "12345", "type": "page" },
            "body": {
                "storage": {
                    "value": "<p>hi</p>",
                    "representation": "storage",
                }
            }
        });
        assert_eq!(body["type"], "comment");
        assert_eq!(body["container"]["id"], "12345");
        assert_eq!(body["container"]["type"], "page");
        assert_eq!(body["body"]["storage"]["value"], "<p>hi</p>");
        assert_eq!(body["body"]["storage"]["representation"], "storage");
    }

    #[test]
    fn update_comment_body_shape_carries_version_and_title() {
        // PUT requires `version.number + 1` (optimistic lock) and `title`
        // — Confluence treats omitted title as "clear", so we always
        // round-trip the auto-generated title.
        let body = serde_json::json!({
            "version": { "number": 4 },
            "type": "comment",
            "title": "Re: Design Doc",
            "body": {
                "storage": {
                    "value": "<p>edited</p>",
                    "representation": "storage",
                }
            }
        });
        assert_eq!(body["version"]["number"], 4);
        assert_eq!(body["type"], "comment");
        assert_eq!(body["title"], "Re: Design Doc");
        assert_eq!(body["body"]["storage"]["value"], "<p>edited</p>");
    }

    #[test]
    fn parses_empty_results_envelope() {
        // Pages without any comments still return a well-formed
        // envelope — make sure the list-path treats that as an empty
        // page, not an error.
        let body = r#"{
            "results": [],
            "start": 0,
            "limit": 25,
            "size": 0,
            "_links": {}
        }"#;
        let env: CommentEnvelope = serde_json::from_str(body).expect("parses");
        assert!(env.results.is_empty());
        assert_eq!(env.size, 0);
        assert!(env.links.next.is_none());
    }
}
