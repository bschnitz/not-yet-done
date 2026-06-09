//! `GET /rest/api/content/{id}/child/attachment` — list attachments
//! attached to a page, plus raw byte download via the server-relative
//! `_links.download` URL.
//!
//! The list endpoint returns the same envelope shape as `/space` and
//! `/content/{id}/child/page` (see [`super::page`]); only the row
//! payload diverges. Confluence Server emits `extensions.fileSize`,
//! `extensions.mediaType`, `version.by.displayName`, `version.when`, and
//! `_links.download` for every attachment.
//!
//! The download URL is **server-relative** (e.g.
//! `/download/attachments/12345/foo.pdf?version=1&modificationDate=...`),
//! so the byte-fetch path prefixes it with the adapter base URL before
//! issuing the GET.

use serde::Deserialize;

use not_yet_done_content::http_log;

use super::ConfluenceClient;

/// Subset of one attachment record. Mirrors the fields the adapter
/// surfaces in `Node::metadata` plus the download URL fragment needed
/// to fetch the bytes. Flattened from the wire format up front so the
/// adapter doesn't have to thread `extensions` / `version` sub-objects
/// through every call site.
#[derive(Clone, Debug)]
pub struct AttachmentMeta {
    /// Confluence attachment id (numeric string, e.g. `"att56789"`).
    pub id: String,
    /// The attachment's filename (Confluence's `title` field on the
    /// attachment object — that's the user-visible filename, distinct
    /// from a page title).
    pub title: String,
    /// `"attachment"` for our endpoint — captured for symmetry with the
    /// page envelope handling, never assumed.
    pub attachment_type: String,
    /// Byte length of the attachment, from `extensions.fileSize`.
    /// `0` when the server omitted it.
    pub file_size: u64,
    /// Mime-type from `extensions.mediaType`. Empty when Confluence
    /// couldn't determine it (or didn't expose the field).
    pub media_type: String,
    /// Display name of the user who created the current version of
    /// the attachment, from `version.by.displayName`. Empty when
    /// permission-stripped.
    pub author: String,
    /// `version.when` timestamp. Empty when not provided. ISO-8601.
    pub created: String,
    /// `_links.download` — server-relative download URL, including query
    /// string. Empty when Confluence omitted the field.
    pub download_path: String,
}

/// Wire format mirroring the per-row JSON. The public surface is
/// [`AttachmentMeta`]; this struct exists so serde can decode the
/// nested `extensions` / `version` / `_links` objects without forcing
/// each consumer to walk through them.
#[derive(Deserialize, Debug)]
struct AttachmentWire {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default, rename = "type")]
    attachment_type: String,
    #[serde(default)]
    extensions: Option<AttachmentExtensionsWire>,
    #[serde(default)]
    version: Option<AttachmentVersionWire>,
    #[serde(default, rename = "_links")]
    links: Option<AttachmentLinksWire>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct AttachmentExtensionsWire {
    #[serde(default)]
    file_size: Option<u64>,
    #[serde(default)]
    media_type: Option<String>,
}

#[derive(Deserialize, Debug)]
struct AttachmentVersionWire {
    #[serde(default)]
    by: Option<AttachmentVersionByWire>,
    #[serde(default)]
    when: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct AttachmentVersionByWire {
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct AttachmentLinksWire {
    #[serde(default)]
    download: Option<String>,
}

impl From<AttachmentWire> for AttachmentMeta {
    fn from(w: AttachmentWire) -> Self {
        let (file_size, media_type) = w
            .extensions
            .map(|e| (e.file_size.unwrap_or(0), e.media_type.unwrap_or_default()))
            .unwrap_or((0, String::new()));
        let (author, created) = w
            .version
            .map(|v| {
                let author = v
                    .by
                    .and_then(|b| b.display_name)
                    .unwrap_or_default();
                (author, v.when.unwrap_or_default())
            })
            .unwrap_or((String::new(), String::new()));
        let download_path = w
            .links
            .and_then(|l| l.download)
            .unwrap_or_default();
        AttachmentMeta {
            id: w.id,
            title: w.title,
            attachment_type: w.attachment_type,
            file_size,
            media_type,
            author,
            created,
            download_path,
        }
    }
}

/// Wire envelope shape for attachment listings. Same layout as the
/// page envelope — separate type so the per-row deserializer can be
/// `AttachmentWire` instead of `PageMeta`.
#[derive(Deserialize, Debug)]
struct AttachmentEnvelope {
    #[serde(default)]
    results: Vec<AttachmentWire>,
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

/// One page of [`AttachmentMeta`] rows plus pagination state.
#[derive(Debug)]
pub struct AttachmentList {
    pub attachments: Vec<AttachmentMeta>,
    pub start: u32,
    pub limit: u32,
    /// Number of records on this page (`< limit` only on the last page).
    pub size: u32,
    /// True iff Confluence included a `_links.next` field on the envelope.
    pub has_next: bool,
}

impl ConfluenceClient {
    /// `GET /rest/api/content/{id}/child/attachment?start={start}&limit={limit}`
    /// — returns the attachments hanging off `page_id`. The `version` and
    /// `extensions` expand sub-objects are included by default on this
    /// endpoint, so the request URL doesn't need an explicit `expand=`
    /// param.
    pub async fn list_attachments(
        &self,
        page_id: &str,
        start: u32,
        limit: u32,
    ) -> Result<AttachmentList, String> {
        let url = format!(
            "{}/rest/api/content/{page_id}/child/attachment?start={start}&limit={limit}",
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
        let env: AttachmentEnvelope = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse attachments response: {e}"))?;
        Ok(AttachmentList {
            attachments: env.results.into_iter().map(AttachmentMeta::from).collect(),
            start: env.start,
            limit: env.limit,
            size: env.size,
            has_next: env.links.next.is_some(),
        })
    }

    /// `POST /rest/api/content/{page_id}/child/attachment` — multipart
    /// upload of a single file as a new attachment on the page. The
    /// adapter's `X-Atlassian-Token: no-check` default header satisfies
    /// the XSRF gate; reqwest sets the `multipart/form-data` boundary
    /// Content-Type itself, overriding the client default `application/
    /// json`. Confluence returns a `{results: [...]}` envelope even for
    /// single-file uploads — we surface the first row (multi-file uploads
    /// loop at the adapter level, one POST per file).
    pub async fn upload_attachment(
        &self,
        page_id: &str,
        file_path: &std::path::Path,
    ) -> Result<AttachmentMeta, String> {
        let bytes = tokio::fs::read(file_path)
            .await
            .map_err(|e| format!("read {}: {e}", file_path.display()))?;
        let filename = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("non-UTF-8 filename: {}", file_path.display()))?
            .to_string();
        let part = reqwest::multipart::Part::bytes(bytes).file_name(filename.clone());
        let form = reqwest::multipart::Form::new().part("file", part);

        let url = format!(
            "{}/rest/api/content/{page_id}/child/attachment",
            self.base_url()
        );
        http_log::log_request("POST", &url);
        let resp = self
            .inner_http()
            .post(&url)
            .multipart(form)
            .send()
            .await
            .map_err(|e| http_log::network_error("POST", &url, e))?;
        let resp = http_log::check_status("POST", &url, resp).await?;
        let body = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;
        let env: AttachmentEnvelope = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse upload response: {e}"))?;
        env.results
            .into_iter()
            .next()
            .map(AttachmentMeta::from)
            .ok_or_else(|| format!("Upload response had no results row for {filename}"))
    }

    /// Download the raw bytes of an attachment. `download_path` is a
    /// server-relative path (`/download/attachments/...`) — the method
    /// joins it onto the adapter's `base_url` itself. Empty paths are
    /// rejected before the request goes out so callers get a clean
    /// error instead of a malformed URL.
    pub async fn download_attachment(&self, download_path: &str) -> Result<Vec<u8>, String> {
        if download_path.is_empty() {
            return Err("Attachment has no download link".to_string());
        }
        let url = format!("{}{}", self.base_url(), download_path);
        http_log::log_request("GET", &url);
        let resp = self
            .inner_http()
            .get(&url)
            .send()
            .await
            .map_err(|e| http_log::network_error("GET", &url, e))?;
        let resp = http_log::check_status("GET", &url, resp).await?;
        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| format!("Failed to read attachment bytes: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_attachment_envelope() {
        let body = r#"{
            "results": [
                {
                    "id": "att56789",
                    "type": "attachment",
                    "title": "design.pdf",
                    "extensions": {
                        "mediaType": "application/pdf",
                        "fileSize": 524288
                    },
                    "version": {
                        "by": { "displayName": "Alice Example" },
                        "when": "2026-05-01T10:00:00.000Z",
                        "number": 1
                    },
                    "_links": {
                        "download": "/download/attachments/12345/design.pdf?version=1",
                        "webui": "/pages/viewpageattachments.action?pageId=12345"
                    }
                }
            ],
            "start": 0,
            "limit": 25,
            "size": 1,
            "_links": { "next": "/rest/api/content/12345/child/attachment?start=25&limit=25" }
        }"#;
        let env: AttachmentEnvelope = serde_json::from_str(body).expect("parses");
        assert_eq!(env.results.len(), 1);
        let att: AttachmentMeta = env.results.into_iter().next().unwrap().into();
        assert_eq!(att.id, "att56789");
        assert_eq!(att.title, "design.pdf");
        assert_eq!(att.attachment_type, "attachment");
        assert_eq!(att.file_size, 524288);
        assert_eq!(att.media_type, "application/pdf");
        assert_eq!(att.author, "Alice Example");
        assert_eq!(att.created, "2026-05-01T10:00:00.000Z");
        assert_eq!(
            att.download_path,
            "/download/attachments/12345/design.pdf?version=1"
        );
        assert!(env.links.next.is_some());
    }

    #[test]
    fn parses_minimal_attachment_without_extensions_or_version() {
        // Server may strip extensions/version sub-objects when the cookie
        // user lacks read-permission on metadata — hydration must still
        // succeed with empty defaults.
        let body = r#"{
            "results": [
                {
                    "id": "att1",
                    "type": "attachment",
                    "title": "bare.txt"
                }
            ],
            "start": 0,
            "limit": 25,
            "size": 1,
            "_links": {}
        }"#;
        let env: AttachmentEnvelope = serde_json::from_str(body).expect("parses");
        let att: AttachmentMeta = env.results.into_iter().next().unwrap().into();
        assert_eq!(att.id, "att1");
        assert_eq!(att.title, "bare.txt");
        assert_eq!(att.file_size, 0);
        assert!(att.media_type.is_empty());
        assert!(att.author.is_empty());
        assert!(att.created.is_empty());
        assert!(att.download_path.is_empty());
        assert!(env.links.next.is_none());
    }

    #[test]
    fn parses_single_row_upload_envelope() {
        // Confluence returns `{results: [...]}` even for single-file
        // uploads — lock in that the first row hydrates into a usable
        // AttachmentMeta with download path intact.
        let body = r#"{
            "results": [
                {
                    "id": "att90001",
                    "type": "attachment",
                    "title": "report.txt",
                    "extensions": { "mediaType": "text/plain", "fileSize": 42 },
                    "version": {
                        "by": { "displayName": "Synthetic Author" },
                        "when": "2026-06-02T11:22:33.000Z"
                    },
                    "_links": {
                        "download": "/download/attachments/12345/report.txt?version=1"
                    }
                }
            ],
            "start": 0,
            "limit": 25,
            "size": 1,
            "_links": {}
        }"#;
        let env: AttachmentEnvelope = serde_json::from_str(body).expect("parses");
        let att: AttachmentMeta = env.results.into_iter().next().unwrap().into();
        assert_eq!(att.id, "att90001");
        assert_eq!(att.title, "report.txt");
        assert_eq!(att.media_type, "text/plain");
        assert_eq!(att.file_size, 42);
        assert_eq!(
            att.download_path,
            "/download/attachments/12345/report.txt?version=1"
        );
    }

    #[tokio::test]
    async fn upload_errors_when_file_missing() {
        // tokio::fs::read on a missing path surfaces a fs error verbatim;
        // the helper wraps it with the path so the user can see which
        // file failed. No network call should be issued.
        let client = ConfluenceClient::new(
            "https://wiki.example.invalid",
            "JSESSIONID=synthetic",
            false,
        )
        .expect("client");
        let missing = std::path::Path::new("/definitely/does/not/exist/nyd-upload-test.bin");
        let err = client
            .upload_attachment("12345", missing)
            .await
            .expect_err("missing file must error");
        assert!(err.contains("read"), "error mentions read: {err}");
        assert!(
            err.contains("nyd-upload-test.bin"),
            "error names file: {err}"
        );
    }

    #[tokio::test]
    async fn download_rejects_empty_path() {
        let client = ConfluenceClient::new(
            "https://wiki.example.invalid",
            "JSESSIONID=synthetic",
            false,
        )
        .expect("client");
        let err = client.download_attachment("").await.expect_err("must error");
        assert!(err.contains("download"), "error mentions download: {err}");
    }
}
