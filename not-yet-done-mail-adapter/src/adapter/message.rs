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
    ActionInput, ActionOutcome, ColumnSchema, Content, ContentError, EditorPrep, InputSpec,
    Metadata, Node, NodeAction, NodeSummary, NodeType, Result,
};

use super::field;
use super::files::message_dir;
use super::other_err;
use super::types::message_type;
use crate::compose::buffer::Headers;
use crate::compose::outbox::Outbox;
use crate::compose::{quote, render};
use crate::ids::MessageId;
use crate::imap::conn::Connection;
use crate::mime::Original;
use crate::model::EnvelopeRow;

/// One slot of the flags column: the glyph, and the blank that stands in
/// for it when a message does not carry that flag.
///
/// The blank is written out rather than measured because the glyphs are not
/// all the same width — an emoji takes two terminal cells, `↩` takes one
/// — and spelling it out forces a glyph swap to state the width it needs.
#[derive(Clone, Copy)]
struct Flag {
    glyph: &'static str,
    blank: &'static str,
}

/// What the flags column shows, in the order a mail client reads them: is it
/// new, did I answer it, did I mark it, is it mine and unsent, does it carry
/// files.
const UNREAD: Flag = Flag {
    glyph: "📩",
    blank: "  ",
};
const ANSWERED: Flag = Flag {
    glyph: "↩",
    blank: " ",
};
const FLAGGED: Flag = Flag {
    glyph: "⭐",
    blank: "  ",
};
const DRAFT: Flag = Flag {
    glyph: "📝",
    blank: "  ",
};
const ATTACHED: Flag = Flag {
    glyph: "📎",
    blank: "  ",
};

/// Which slots a list of messages actually needs — the union of their flags.
///
/// A slot nobody on the page uses is not rendered at all, so a mailbox where
/// nothing is starred or drafted does not pay for a star and a pencil column.
/// The rows of one page all get the same mask, which is what keeps the gutter
/// readable downward: within a page a glyph never moves.
///
/// Computed over the PAGE, not the visible window. A mask that followed the
/// viewport would re-lay the gutter out under the cursor on every scrolled
/// line, which is the jitter this whole arrangement exists to prevent.
#[derive(Clone, Copy, Default)]
pub(super) struct FlagSlots {
    unread: bool,
    answered: bool,
    flagged: bool,
    draft: bool,
    attached: bool,
}

impl FlagSlots {
    pub(super) fn of(rows: &[EnvelopeRow]) -> Self {
        let mut slots = Self::default();
        for row in rows {
            slots.unread |= !row.seen;
            slots.answered |= row.answered;
            slots.flagged |= row.flagged;
            slots.draft |= row.draft;
            slots.attached |= !row.attachments.is_empty();
        }
        slots
    }

    /// The mask for a message looked at on its own — a detail view has no
    /// column to line up with, so it shows what this one message carries and
    /// nothing else.
    pub(super) fn just(row: &EnvelopeRow) -> Self {
        Self::of(std::slice::from_ref(row))
    }
}

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

/// What an exported message is called on disk. These names are the whole
/// contract with whoever renders the mail: a viewer looks for them, and
/// `message.json` is written **last**, so a half-finished export is never
/// mistaken for a complete one.
const HTML_FILE: &str = "message.html";
const TEXT_FILE: &str = "message.txt";
const META_FILE: &str = "message.json";
const INLINE_DIR: &str = "inline";

/// What a message offers.
///
/// *Reading* it is not an action — the body is [`Content`], which is what
/// lets the preview pane follow the cursor. But a mail is more than the text
/// that pane can show: an HTML mail is markup, and its images are parts of
/// the message rather than files anyone lists. Handing that to an external
/// viewer means writing files, and the adapter is the only place that can:
/// the markup never leaves it otherwise.
pub(super) fn actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("export_html", "export html", InputSpec::None),
        NodeAction::new("reply", "reply", InputSpec::Editor),
        NodeAction::new("compose", "new message", InputSpec::Editor),
    ]
}

/// The draft of a reply is named after the message it answers, so pressing
/// `reply` again after a failed send re-opens the text that was written and
/// not an empty buffer.
pub(super) fn reply_draft(id: &MessageId) -> String {
    format!("reply-{}", id.encode())
}

/// A new message has nothing to be named after — one unsent draft per
/// account, which is the one a second `compose` should continue.
pub(super) const COMPOSE_DRAFT: &str = "compose";

/// The one cell a slot contributes: its glyph when the flag is set, its
/// blank when it is not.
fn slot(flag: Flag, present: bool) -> &'static str {
    if present { flag.glyph } else { flag.blank }
}

/// The flag glyphs of one message: every slot `slots` asks for, occupied by
/// its glyph or by its blank.
///
/// Packing the glyphs to the left instead would keep the *order* fixed while
/// letting the *positions* move: a mail whose only flag is an attachment
/// would put its 📎 exactly where the row above carries its unread marker,
/// and the gutter could no longer be read as a column. The mask is what
/// keeps that from costing five slots on a mailbox that only ever uses two.
fn glyphs(row: &EnvelopeRow, slots: FlagSlots) -> String {
    [
        (UNREAD, slots.unread, !row.seen),
        (ANSWERED, slots.answered, row.answered),
        (FLAGGED, slots.flagged, row.flagged),
        (DRAFT, slots.draft, row.draft),
        (ATTACHED, slots.attached, !row.attachments.is_empty()),
    ]
    .into_iter()
    .filter(|(_, needed, _)| *needed)
    .map(|(flag, _, present)| slot(flag, present))
    .collect()
}

/// The date as the table sorts and shows it. RFC 3339 rather than something
/// prettier: the column is typed `datetime`, and the sort compares the cell
/// text, so it has to be a form that orders correctly.
fn date_cell(row: &EnvelopeRow) -> String {
    row.date.map(|d| d.to_rfc3339()).unwrap_or_default()
}

pub(super) fn metadata_of(account: &str, row: &EnvelopeRow, slots: FlagSlots) -> Metadata {
    Metadata {
        fields: vec![
            field("flags", glyphs(row, slots), ""),
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
                if row.seen {
                    String::new()
                } else {
                    "true".into()
                },
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

pub(super) fn message_row(
    account: &str,
    folder: &str,
    row: &EnvelopeRow,
    slots: FlagSlots,
) -> NodeSummary {
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
        metadata: metadata_of(account, row, slots),
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
        let names: Vec<&str> = row
            .attachments
            .iter()
            .map(|a| a.filename.as_str())
            .collect();
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
    /// The account's sending side. Present on every message node, because
    /// answering a mail is a gesture of the mailbox it is in — an account
    /// that cannot send says so when the editor would have opened.
    outbox: Arc<Outbox>,
}

impl MailMessageNode {
    pub(super) fn new(
        id: &MessageId,
        row: &EnvelopeRow,
        conn: Connection,
        cache: Arc<BodyCache>,
        outbox: Arc<Outbox>,
    ) -> Self {
        Self {
            id: id.encode(),
            label: label_of(row),
            metadata: metadata_of(&id.account, row, FlagSlots::just(row)),
            body: MessageBody {
                id: id.clone(),
                header: header_block(row),
                conn,
                cache,
            },
            outbox,
        }
    }
}

impl MailMessageNode {
    /// Write this message into its own directory and say where it is.
    ///
    /// One directory per message, the same one an opened attachment lands in,
    /// holding at most four things: the sender's markup (`message.html`), the
    /// plain-text alternative (`message.txt`), the images that markup points
    /// at (`inline/`), and the headers a renderer needs for its own layout
    /// (`message.json`). A message carries what it carries — a plain mail
    /// gets no `message.html` — and the metadata says which of them exist, so
    /// nothing has to guess from a missing file.
    ///
    /// The markup is written **unsanitised**, exactly as it arrived. Deciding
    /// what may run in a renderer is the renderer's job and depends on what
    /// it is: a browser and a terminal are not in the same danger. Filtering
    /// here would give every viewer a false guarantee — that whatever comes
    /// out of this directory is safe to display — and quietly change the
    /// message on the way.
    async fn export_html(&self) -> Result<ActionOutcome> {
        let dir = message_dir(&self.id).map_err(|e| other_err(e.to_string()))?;
        let export = crate::mime::export(&self.body.raw().await?);

        if let Some(body) = &export.html {
            if !body.inline.is_empty() {
                let inline = dir.join(INLINE_DIR);
                tokio::fs::create_dir_all(&inline)
                    .await
                    .map_err(|e| other_err(format!("create {}: {e}", inline.display())))?;
                for part in &body.inline {
                    write(&inline.join(&part.file_name), &part.bytes).await?;
                }
            }
            let html = crate::mime::link_inline(&body.html, &body.inline, INLINE_DIR);
            write(&dir.join(HTML_FILE), html.as_bytes()).await?;
        }
        if let Some(text) = &export.text {
            write(&dir.join(TEXT_FILE), text.as_bytes()).await?;
        }

        let meta = serde_json::json!({
            "id": self.id,
            "subject": export.subject,
            "from": export.from,
            "to": export.to,
            "cc": export.cc,
            "date": export.date,
            "attachments": export.attachments,
            "html": export.html.as_ref().map(|_| HTML_FILE),
            "text": export.text.as_ref().map(|_| TEXT_FILE),
            "inline": export.html.as_ref().map(|b| b.inline.len()).unwrap_or(0),
        });
        let meta = serde_json::to_vec_pretty(&meta).map_err(|e| other_err(e.to_string()))?;
        write(&dir.join(META_FILE), &meta).await?;

        Ok(ActionOutcome::Done {
            message: Some(format!("exported message to {}", dir.display())),
        })
    }
}

impl MailMessageNode {
    /// The message this one answers, parsed out of its own source.
    ///
    /// Fetched twice over one reply — once to fill the editor, once to build
    /// what travels — and deliberately not cached: the second read is what
    /// makes the quote guard stateless, and a body the server has since
    /// changed is a body we should be quoting, not one we remember.
    async fn original(&self) -> Result<Original> {
        Ok(crate::mime::original(&self.body.raw().await?))
    }

    /// The buffer a reply opens on: the addresses filled in, the subject
    /// prefixed, and the original below the marker as readable text.
    async fn reply_prep(&self) -> Result<EditorPrep> {
        let original = self.original().await?;
        let headers = Headers {
            from: self.outbox.identity().map_err(super::mail_err)?,
            to: render::reply_recipients(&original, &self.outbox.own_addresses()),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: render::reply_subject(&original.subject),
        };
        let quoted = quote::quoted_text(&original, &self.outbox.attribution(&original));
        self.outbox
            .prep(&reply_draft(&self.body.id), &headers, Some(&quoted))
            .map_err(super::mail_err)
    }

    async fn reply_send(&self, text: &str) -> Result<ActionOutcome> {
        let original = self.original().await?;
        let report = self
            .outbox
            .deliver(
                &reply_draft(&self.body.id),
                text,
                Some(&original),
                Some(&self.body.id),
            )
            .await
            .map_err(super::mail_err)?;
        Ok(ActionOutcome::Done {
            message: Some(report),
        })
    }
}

/// The buffer a new message opens on, and what comes back from it.
///
/// Shared by the message level and the folder above it: `compose` needs an
/// account and nothing else, and both levels know theirs. Which is why a
/// folder with no messages in it can still start a mail.
pub(super) async fn compose_prep(outbox: &Outbox) -> Result<EditorPrep> {
    let headers = Headers {
        from: outbox.identity().map_err(super::mail_err)?,
        ..Headers::default()
    };
    outbox
        .prep(COMPOSE_DRAFT, &headers, None)
        .map_err(super::mail_err)
}

pub(super) async fn compose_send(outbox: &Outbox, text: &str) -> Result<ActionOutcome> {
    let report = outbox
        .deliver(COMPOSE_DRAFT, text, None, None)
        .await
        .map_err(super::mail_err)?;
    Ok(ActionOutcome::Done {
        message: Some(report),
    })
}

async fn write(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    tokio::fs::write(path, bytes)
        .await
        .map_err(|e| other_err(format!("write {}: {e}", path.display())))
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
        let text = Arc::new(format!(
            "{}{}",
            self.header,
            crate::mime::body_text(&source)
        ));
        self.cache.put(&key, Arc::clone(&text));
        Ok(text)
    }
}

impl MessageBody {
    /// The message exactly as it travelled. Not cached: the cache holds the
    /// *rendered* text, and the one caller that needs the source writes what
    /// it makes of it to disk, where a second look finds it without a fetch.
    async fn raw(&self) -> Result<Vec<u8>> {
        self.conn
            .body(&self.id.folder, self.id.uid_validity, self.id.uid)
            .await
            .map_err(super::mail_err)
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

    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        match action_id {
            "reply" => self.reply_prep().await,
            "compose" => compose_prep(&self.outbox).await,
            other => Err(ContentError::NotSupported(format!(
                "`{other}` does not open an editor on a mail message"
            ))),
        }
    }

    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("export_html", ActionInput::None) => self.export_html().await,
            ("reply", ActionInput::Edited { text, .. }) => self.reply_send(&text).await,
            ("compose", ActionInput::Edited { text, .. }) => {
                compose_send(&self.outbox, &text).await
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "`{other}` is not an action of a mail message"
            ))),
        }
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

    fn pdf() -> crate::model::AttachmentInfo {
        crate::model::AttachmentInfo {
            part: "2".into(),
            filename: "invoice.pdf".into(),
            content_type: "application/pdf".into(),
            size: 4096,
        }
    }

    /// A page needs every slot in it: with all five in use each flag keeps
    /// its own cell, so the same meaning always lands on the same column.
    /// The attachment case is the one that matters — packed to the left, its
    /// 📎 would sit where the unread marker sits, and scanning down the
    /// gutter could not tell the two apart.
    #[test]
    fn every_flag_keeps_its_own_slot() {
        let all = FlagSlots {
            unread: true,
            answered: true,
            flagged: true,
            draft: true,
            attached: true,
        };
        let mut r = row();
        assert_eq!(glyphs(&r, all), "📩       ", "unseen by default");
        r.seen = true;
        assert_eq!(
            glyphs(&r, all),
            "         ",
            "no flags is nine blanks, not none"
        );

        r.attachments = vec![pdf()];
        assert_eq!(
            glyphs(&r, all),
            "       📎",
            "an attachment stays in the last slot"
        );

        r.flagged = true;
        r.answered = true;
        assert_eq!(glyphs(&r, all), "  ↩⭐  📎");

        r.seen = false;
        r.draft = true;
        assert_eq!(glyphs(&r, all), "📩↩⭐📝📎", "all five, in reading order");
    }

    /// The gutter is only as wide as the page needs: a slot no message on the
    /// page uses is not rendered, so an ordinary mailbox pays for unread and
    /// attachment and not for the star and the pencil it never sets.
    #[test]
    fn the_gutter_costs_only_the_slots_the_page_uses() {
        let unread = row();
        let mut with_file = row();
        with_file.seen = true;
        with_file.attachments = vec![pdf()];

        let page = [unread.clone(), with_file.clone()];
        let slots = FlagSlots::of(&page);

        // Two slots, four cells — not the nine a full mask would cost.
        assert_eq!(glyphs(&unread, slots), "📩  ");
        assert_eq!(glyphs(&with_file, slots), "  📎");
    }

    /// A page with nothing flagged at all renders no gutter. The view file
    /// asks for `sizing: max`, so the column then takes no width either.
    #[test]
    fn a_page_without_flags_has_no_gutter() {
        let mut read = row();
        read.seen = true;
        let slots = FlagSlots::of(std::slice::from_ref(&read));
        assert_eq!(glyphs(&read, slots), "");
    }

    /// Looked at on its own a message has no column to line up with, so it
    /// shows its own flags and no blanks.
    #[test]
    fn a_single_message_shows_only_what_it_carries() {
        let mut r = row();
        r.seen = true;
        r.attachments = vec![pdf()];
        assert_eq!(glyphs(&r, FlagSlots::just(&r)), "📎");
    }

    /// The blank of a slot has to be exactly as wide as its glyph, or a row
    /// missing a flag that another row on the page carries shifts every slot
    /// after it. This is the invariant a glyph swap breaks silently — emoji
    /// are two terminal cells wide, most of the arrows and stars in the same
    /// neighbourhood are one.
    #[test]
    fn every_blank_matches_the_width_of_its_glyph() {
        use unicode_width::UnicodeWidthStr;
        for flag in [UNREAD, ANSWERED, FLAGGED, DRAFT, ATTACHED] {
            assert_eq!(
                flag.glyph.width(),
                flag.blank.width(),
                "`{}` and its blank disagree on width",
                flag.glyph
            );
            assert!(
                flag.blank.chars().all(|c| c == ' '),
                "a blank slot is spaces, not `{}`",
                flag.blank
            );
        }
    }

    /// Every column the schema names must exist on every row, or a table
    /// silently shows a blank where a value was promised.
    #[test]
    fn every_declared_column_has_a_cell() {
        let meta = metadata_of("work", &row(), FlagSlots::just(&row()));
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
            metadata_of("work", r, FlagSlots::just(r))
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
        let summary = message_row("work", "INBOX/Projects", &row(), FlagSlots::just(&row()));
        assert_eq!(summary.id, "work/INBOX/Projects#42.7");
        assert_eq!(
            summary.has_children,
            Some(false),
            "a mail without attachments is a leaf"
        );
    }
}
