//! The `mail:message` level — one folder's messages.
//!
//! A row here is an *envelope*, never a body: subject, sender, date, size and
//! the flags, all of which arrive in the one `FETCH` the page costs. Opening
//! a message is a separate read, so scrolling a mailbox of a hundred thousand
//! mails never pulls a single body over the wire.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use not_yet_done_content::{
    ColumnSchema, Content, Metadata, Node, NodeSummary, NodeType, Result,
};

use super::field;
use super::types::message_type;
use crate::ids::MessageId;
use crate::imap::conn::Connection;
use crate::model::EnvelopeRow;

/// What the flags column shows. Three glyphs, in the order a mail client
/// reads them: is it new, did I answer it, did I mark it.
const UNREAD: &str = "●";
const ANSWERED: &str = "↩";
const FLAGGED: &str = "★";
const DRAFT: &str = "✎";
const ATTACHED: &str = "📎";

pub(super) fn columns() -> Vec<ColumnSchema> {
    vec![
        ColumnSchema::new("flags", "").typed("text"),
        ColumnSchema::new("from", "From"),
        ColumnSchema::new("subject", "Subject"),
        ColumnSchema::new("date", "Date").typed("datetime"),
        ColumnSchema::new("size", "Size").typed("number"),
        ColumnSchema::new("attachments", "Att").typed("number"),
        ColumnSchema::new("to", "To"),
        ColumnSchema::new("account", "Account"),
    ]
}

/// The flag glyphs of one message, in a fixed order so the column stays a
/// column and does not jitter from row to row.
fn glyphs(row: &EnvelopeRow) -> String {
    let mut out = String::new();
    if !row.seen {
        out.push_str(UNREAD);
    }
    if row.answered {
        out.push_str(ANSWERED);
    }
    if row.flagged {
        out.push_str(FLAGGED);
    }
    if row.draft {
        out.push_str(DRAFT);
    }
    if !row.attachments.is_empty() {
        out.push_str(ATTACHED);
    }
    out
}

/// The date as the table sorts and shows it. RFC 3339 rather than something
/// prettier: the column is typed `datetime`, and the sort compares the cell
/// text, so it has to be a form that orders correctly.
fn date_cell(row: &EnvelopeRow) -> String {
    row.date.map(|d| d.to_rfc3339()).unwrap_or_default()
}

pub(super) fn metadata_of(account: &str, row: &EnvelopeRow) -> Metadata {
    Metadata {
        fields: vec![
            field("flags", glyphs(row), ""),
            field("from", row.from.clone(), "From"),
            field("subject", row.subject.clone(), "Subject"),
            field("date", date_cell(row), "Date"),
            field("size", row.size.to_string(), "Size"),
            field("attachments", row.attachments.len().to_string(), "Att"),
            field("to", row.to.clone(), "To"),
            field("account", account.to_string(), "Account"),
            // The flag the styling layer paints from — the same `unread`
            // key the folder rows and the Stoat messages carry, so an
            // unread mail lights up with no frontend work.
            field(
                "unread",
                if row.seen { String::new() } else { "true".into() },
                "Unread",
            ),
        ],
    }
}

/// A subject line that is empty on the wire is empty on screen too — and an
/// empty row label is a row the user cannot aim at. Mail clients have said
/// this the same way for thirty years.
fn label_of(row: &EnvelopeRow) -> String {
    if row.subject.trim().is_empty() {
        "(no subject)".to_string()
    } else {
        row.subject.clone()
    }
}

pub(super) fn message_row(account: &str, folder: &str, row: &EnvelopeRow) -> NodeSummary {
    let id = MessageId {
        account: account.to_string(),
        folder: folder.to_string(),
        uid_validity: row.uid_validity,
        uid: row.uid,
    };
    NodeSummary {
        id: id.encode(),
        label: label_of(row),
        node_type: message_type().clone(),
        metadata: metadata_of(account, row),
        // Only a message that actually carries files has something below it.
        // Saying so per row is what keeps the drill arrow honest: a mail with
        // no attachment is a leaf, and Enter on it does nothing rather than
        // opening an empty level.
        has_children: Some(!row.attachments.is_empty()),
    }
}

/// How many message bodies are kept. A body is the one expensive thing on
/// this level, and moving the cursor down a thread and back up is the normal
/// way to read mail — without a cache that is a fetch per keypress. Small
/// because a body is a whole message: thirty-odd of them is a bounded amount
/// of memory, a mailbox's worth would not be.
const BODY_CACHE_LEN: usize = 32;

/// The last few rendered message bodies, keyed by message id.
///
/// Oldest-out rather than least-recently-used: the access pattern that
/// matters is walking a list, and for that the two behave the same while
/// this one is a `VecDeque` and a `Mutex` instead of a data structure.
#[derive(Default)]
pub(super) struct BodyCache {
    entries: Mutex<VecDeque<(String, Arc<String>)>>,
}

impl BodyCache {
    fn get(&self, id: &str) -> Option<Arc<String>> {
        let entries = self.entries.lock().ok()?;
        entries
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, text)| Arc::clone(text))
    }

    fn put(&self, id: &str, text: Arc<String>) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        entries.retain(|(key, _)| key != id);
        entries.push_back((id.to_string(), text));
        while entries.len() > BODY_CACHE_LEN {
            entries.pop_front();
        }
    }

    /// Forget everything. Called when the instance drops its sessions, so a
    /// reconnect cannot serve a body from a mailbox that has since been
    /// renumbered.
    pub(super) fn clear(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
    }
}

/// The header block a reader expects above the text. Written here rather
/// than left to the view because the body the server hands back has no
/// header the user would want to read — the raw ones are a screenful of
/// `Received:` lines — and a message without a visible sender is unreadable.
fn header_block(row: &EnvelopeRow) -> String {
    let mut out = String::new();
    for (label, value) in [
        ("From", row.from.as_str()),
        ("To", row.to.as_str()),
        ("Subject", row.subject.as_str()),
    ] {
        if !value.trim().is_empty() {
            out.push_str(&format!("{label}: {value}\n"));
        }
    }
    if let Some(date) = row.date {
        out.push_str(&format!("Date: {}\n", date.to_rfc2822()));
    }
    if !row.attachments.is_empty() {
        let names: Vec<&str> = row.attachments.iter().map(|a| a.filename.as_str()).collect();
        out.push_str(&format!("Attachments: {}\n", names.join(", ")));
    }
    out.push('\n');
    out
}

/// One message, as a node.
pub(super) struct MailMessageNode {
    id: String,
    label: String,
    metadata: Metadata,
    body: MessageBody,
}

impl MailMessageNode {
    pub(super) fn new(
        id: &MessageId,
        row: &EnvelopeRow,
        conn: Connection,
        cache: Arc<BodyCache>,
    ) -> Self {
        Self {
            id: id.encode(),
            label: label_of(row),
            metadata: metadata_of(&id.account, row),
            body: MessageBody {
                id: id.clone(),
                header: header_block(row),
                conn,
                cache,
            },
        }
    }
}

/// The readable form of one message: a header block over the text part.
///
/// A `Content` and not an action, so the frontend's preview pane can show it
/// while the cursor moves — which is the whole point of the level. What
/// keeps that affordable is [`BodyCache`] plus `BODY.PEEK`: reading never
/// costs a second fetch, and never marks the mail `\Seen` either.
struct MessageBody {
    id: MessageId,
    header: String,
    conn: Connection,
    cache: Arc<BodyCache>,
}

impl MessageBody {
    async fn text(&self) -> Result<Arc<String>> {
        let key = self.id.encode();
        if let Some(hit) = self.cache.get(&key) {
            return Ok(hit);
        }
        let source = self
            .conn
            .body(&self.id.folder, self.id.uid_validity, self.id.uid)
            .await
            .map_err(super::mail_err)?;
        let text = Arc::new(format!("{}{}", self.header, crate::mime::body_text(&source)));
        self.cache.put(&key, Arc::clone(&text));
        Ok(text)
    }
}

#[async_trait]
impl Content for MessageBody {
    fn node_type(&self) -> &NodeType {
        message_type()
    }

    /// Nothing to detect a conflict against: a message body is never
    /// written back, and the UID pair that would serve as a version is
    /// already in the node's id.
    fn version(&self) -> Option<&str> {
        None
    }

    async fn read(&self) -> Result<Vec<u8>> {
        Ok(self.text().await?.as_bytes().to_vec())
    }

    async fn read_text(&self) -> Result<String> {
        Ok(self.text().await?.to_string())
    }
}

#[async_trait]
impl Node for MailMessageNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn node_type(&self) -> &NodeType {
        message_type()
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn content(&self) -> Option<&dyn Content> {
        Some(&self.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> EnvelopeRow {
        EnvelopeRow {
            uid: 7,
            uid_validity: 42,
            subject: "Rechnung".into(),
            from: "Jürgen <j@example.org>".into(),
            ..Default::default()
        }
    }

    /// The glyph order is fixed so the column does not jitter, and an unread
    /// message is the one that has to be visible at a glance.
    #[test]
    fn the_flag_column_reads_in_a_fixed_order() {
        let mut r = row();
        assert_eq!(glyphs(&r), "●", "unseen by default");
        r.seen = true;
        assert_eq!(glyphs(&r), "");
        r.flagged = true;
        r.answered = true;
        r.attachments = vec![crate::model::AttachmentInfo {
            part: "2".into(),
            filename: "invoice.pdf".into(),
            content_type: "application/pdf".into(),
            size: 4096,
        }];
        assert_eq!(glyphs(&r), "↩★📎");
    }

    /// Every column the schema names must exist on every row, or a table
    /// silently shows a blank where a value was promised.
    #[test]
    fn every_declared_column_has_a_cell() {
        let meta = metadata_of("work", &row());
        for column in columns() {
            assert!(
                meta.fields.iter().any(|f| f.key == column.key),
                "no cell for `{}`",
                column.key
            );
        }
    }

    /// The highlight the frontend paints reads exactly one key — `unread`,
    /// with the value `"true"`. It is deliberately NOT a declared column
    /// (nothing renders it as text), so a test is the only thing that keeps
    /// it from disappearing unnoticed and taking the highlight with it.
    #[test]
    fn an_unseen_message_carries_the_unread_flag() {
        let flag = |r: &EnvelopeRow| {
            metadata_of("work", r)
                .fields
                .iter()
                .find(|f| f.key == "unread")
                .map(|f| f.value.clone())
                .unwrap_or_default()
        };
        let mut r = row();
        assert_eq!(flag(&r), "true", "unseen by default");
        r.seen = true;
        assert_eq!(flag(&r), "");
    }

    #[test]
    fn a_message_without_a_subject_still_has_a_label() {
        let mut r = row();
        r.subject = "   ".into();
        assert_eq!(label_of(&r), "(no subject)");
    }

    /// The id is what a restored cursor comes back with, so it has to carry
    /// account, folder and both message coordinates.
    #[test]
    fn the_row_id_addresses_the_message() {
        let summary = message_row("work", "INBOX/Projects", &row());
        assert_eq!(summary.id, "work/INBOX/Projects#42.7");
        assert_eq!(
            summary.has_children,
            Some(false),
            "a mail without attachments is a leaf"
        );
    }
}
