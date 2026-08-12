//! File uploads to the autumn (file) server.
//!
//! Revolt/Stoat splits posting a file in two: the bytes go to autumn's
//! `POST {autumn}/attachments` (multipart, field name `file`), which
//! answers with an opaque file id; that id is then referenced by a normal
//! `POST …/messages` in its `attachments` array. There is no endpoint that
//! adds a file to an *existing* message — `PATCH …/messages/{id}` only
//! carries `content`/`embeds` — so attaching always means sending a new
//! message.
//!
//! The upload carries the session token: a self-hosted autumn may run with
//! uploads gated behind the API session (downloads stay public by id, see
//! [`StoatClient::download_bytes`](super::StoatClient::download_bytes)).

use std::path::Path;

use serde::Deserialize;

use not_yet_done_content::http_log;

use super::StoatClient;

/// Autumn's answer to an upload: the id to reference in a message.
#[derive(Deserialize)]
struct UploadResponse {
    id: String,
}

impl StoatClient {
    /// Upload one local file to autumn's `attachments` bucket and return
    /// its file id. Fails early (without touching the network) when the
    /// file can't be read or when autumn hasn't been discovered yet — the
    /// latter happens only before the first successful connect, since
    /// `GET /api/` captures the file-server URL as a side effect.
    pub async fn upload_attachment(&self, path: &Path) -> Result<String, String> {
        let autumn = self
            .autumn_url()
            .ok_or_else(|| "file server (autumn) not discovered yet".to_string())?;

        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".to_string());

        // Autumn sniffs the content type itself, so the part carries only
        // the bytes and the display filename.
        let part = reqwest::multipart::Part::bytes(bytes).file_name(filename);
        let form = reqwest::multipart::Form::new().part("file", part);

        let url = format!("{autumn}/attachments");
        http_log::log_request("POST", &url);
        let resp = self
            .http
            .post(&url)
            .headers(self.auth_headers()?)
            .multipart(form)
            .send()
            .await
            .map_err(|e| http_log::network_error("POST", &url, e))?;
        let resp = http_log::check_status("POST", &url, resp).await?;
        let uploaded = resp
            .json::<UploadResponse>()
            .await
            .map_err(|e| format!("parse upload response: {e}"))?;
        Ok(uploaded.id)
    }
}
