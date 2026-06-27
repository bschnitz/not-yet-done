//! `export-bundle` action: emit a single JSON document with everything a
//! script needs to render a ticket to disk — metadata, reference→name maps,
//! the attachment manifest (with the exact on-disk name the
//! `download-attachments` action writes), and all comments (newest first).
//!
//! The body itself is intentionally **not** part of the bundle: it is fetched
//! separately via the generic `cat` verb (raw Jira wiki markup), which keeps
//! this JSON small and lets the script pipe the body straight into `pandoc`.

use std::collections::BTreeMap;

use not_yet_done_content::{ActionOutcome, Result};
use serde::Serialize;

use super::super::cache::{fetch_comments, resolve_unknown_mentions};
use super::super::util::{other_err, safe_attachment_name};
use super::JiraIssueNode;

#[derive(Serialize)]
struct Bundle {
    key: String,
    summary: String,
    #[serde(rename = "type")]
    issue_type: String,
    status: String,
    /// Mention key (`[~key]`) → display name, for rewriting user mentions.
    users: BTreeMap<String, String>,
    labels: Vec<String>,
    attachments: Vec<AttachmentEntry>,
    comments: Vec<CommentEntry>,
}

#[derive(Serialize)]
struct AttachmentEntry {
    id: String,
    filename: String,
    /// The exact filename the `download-attachments` action writes to disk
    /// (`<id>-<filename>`), so the script can rewrite body image links without
    /// duplicating the naming rule.
    written_name: String,
    mime_type: String,
    size: u64,
}

#[derive(Serialize)]
struct CommentEntry {
    id: String,
    author: String,
    author_key: String,
    created: String,
    updated: String,
    /// Raw Jira wiki markup — the script converts it with pandoc.
    body: String,
}

impl JiraIssueNode {
    /// Build the export bundle and return it as pretty-printed JSON in
    /// [`ActionOutcome::Done`] (which the CLI prints verbatim to stdout).
    pub(super) async fn export_bundle(&self) -> Result<ActionOutcome> {
        let detail = self.detail().await?.clone();

        let comments = fetch_comments(&self.client, &self.cache, &self.key)
            .await
            .map_err(other_err)?;

        // Resolve any `[~mention]` keys appearing in the body or comments so
        // the user map below carries their display names.
        let mut mention_sources: Vec<&str> = vec![detail.description.as_str()];
        mention_sources.extend(comments.iter().map(|c| c.body.as_str()));
        resolve_unknown_mentions(&self.client, &self.cache, &mention_sources).await;

        let attachments = self
            .client
            .get_attachments(&self.key)
            .await
            .map_err(other_err)?;

        // User map: the issue's own people, every comment author, plus the
        // resolved mention cache. Non-empty keys only; later inserts win, so
        // the cache snapshot (richest source) is applied last.
        let mut users: BTreeMap<String, String> = BTreeMap::new();
        let mut add = |key: &str, name: &str| {
            if !key.is_empty() && !name.is_empty() {
                users.insert(key.to_string(), name.to_string());
            }
        };
        add(&detail.assignee_key, &detail.assignee);
        add(&detail.reporter_key, &detail.reporter);
        add(&detail.creator_key, &detail.creator);
        for c in &comments {
            add(&c.author_key, &c.author);
        }
        for u in self.cache.lock().unwrap().users_snapshot() {
            add(&u.name, &u.display_name);
        }

        let attachments: Vec<AttachmentEntry> = attachments
            .into_iter()
            .map(|a| AttachmentEntry {
                written_name: format!("{}-{}", a.id, safe_attachment_name(&a.filename)),
                id: a.id,
                filename: a.filename,
                mime_type: a.mime_type,
                size: a.size,
            })
            .collect();

        // Comments are already newest-first from `fetch_comments`.
        let comments: Vec<CommentEntry> = comments
            .into_iter()
            .map(|c| CommentEntry {
                id: c.id,
                author: c.author,
                author_key: c.author_key,
                created: c.created,
                updated: c.updated,
                body: c.body,
            })
            .collect();

        let bundle = Bundle {
            key: detail.key,
            summary: detail.summary,
            issue_type: detail.issue_type,
            status: detail.status,
            users,
            labels: detail.labels,
            attachments,
            comments,
        };

        let json = serde_json::to_string_pretty(&bundle)
            .map_err(|e| other_err(format!("serialize export bundle: {e}")))?;
        Ok(ActionOutcome::Done { message: Some(json) })
    }
}
