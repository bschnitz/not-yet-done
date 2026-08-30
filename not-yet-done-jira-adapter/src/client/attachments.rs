//! Attachments: list per issue + raw byte download via signed `content` URL.

use serde::Deserialize;

use super::{Assignee, JiraClient};

/// A Jira attachment.
#[derive(Debug, Clone)]
pub struct JiraAttachment {
    pub id: String,
    pub filename: String,
    pub author: String,
    pub created: String,
    pub size: u64,
    pub mime_type: String,
    pub content_url: String,
}

#[derive(Deserialize)]
struct AttachmentIssueResponse {
    fields: AttachmentIssueFields,
}

#[derive(Deserialize)]
struct AttachmentIssueFields {
    attachment: Option<Vec<RawAttachment>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAttachment {
    id: String,
    filename: Option<String>,
    author: Option<Assignee>,
    created: Option<String>,
    size: Option<u64>,
    mime_type: Option<String>,
    content: Option<String>,
}

impl JiraClient {
    /// Fetch attachments for an issue.
    pub async fn get_attachments(&self, key: &str) -> Result<Vec<JiraAttachment>, String> {
        let url = format!(
            "{}/rest/api/2/issue/{}?fields=attachment",
            self.base_url, key
        );

        let resp = self.send("GET", &url, self.http.get(&url)).await?;
        let resp = self.check_status("GET", &url, resp).await?;
        let body_text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        let data: AttachmentIssueResponse = serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse attachments: {e}"))?;

        let attachments = data.fields.attachment.unwrap_or_default();

        Ok(attachments
            .into_iter()
            .map(|a| JiraAttachment {
                id: a.id,
                filename: a.filename.unwrap_or_default(),
                author: a.author.and_then(|a| a.display_name).unwrap_or_default(),
                created: a.created.unwrap_or_default(),
                size: a.size.unwrap_or(0),
                mime_type: a.mime_type.unwrap_or_default(),
                content_url: a.content.unwrap_or_default(),
            })
            .collect())
    }

    /// Delete a single attachment by its id
    /// (`DELETE /rest/api/2/attachment/{id}`). Returns `Ok(())` on a
    /// success status; an error string otherwise.
    pub async fn delete_attachment(&self, id: &str) -> Result<(), String> {
        let url = format!("{}/rest/api/2/attachment/{}", self.base_url, id);
        let resp = self.send("DELETE", &url, self.http.delete(&url)).await?;
        self.check_status("DELETE", &url, resp).await?;
        Ok(())
    }

    /// Download the raw bytes of an attachment from its `content_url`
    /// (the `content` field on a Jira attachment object — already an
    /// absolute URL, hence no `base_url` prefix).
    pub async fn download_attachment(&self, content_url: &str) -> Result<Vec<u8>, String> {
        let resp = self
            .send("GET", content_url, self.http.get(content_url))
            .await?;
        let resp = self.check_status("GET", content_url, resp).await?;

        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| format!("Failed to read response body: {e}"))
    }
}
