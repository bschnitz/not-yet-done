//! The `mail:attachment` level — the files hanging off one message.
//!
//! This level is **free**. A `BODYSTRUCTURE` arrives with every envelope the
//! message listing fetches anyway, and it already describes every part: name,
//! type, size and the IMAP section path. Listing a message's attachments is
//! therefore a projection of a row the adapter is holding, not a round trip.
//! Only *opening* one costs a fetch — and then only that part, not the whole
//! mail.

use std::path::PathBuf;

use async_trait::async_trait;

use not_yet_done_content::{
    ActionArgs, ActionInput, ActionOutcome, ColumnSchema, ContentError, FormFieldSpec, InputSpec,
    ListResult, Metadata, Node, NodeAction, NodeSummary, NodeType, Result,
};

use super::files::{message_dir, safe_file_name, sanitize_component};
use super::types::attachment_type;
use super::{field, form_field, other_err};
use crate::ids::{MessageId, attachment_id};
use crate::imap::conn::Connection;
use crate::model::AttachmentInfo;

pub(super) fn columns() -> Vec<ColumnSchema> {
    vec![
        ColumnSchema::new("filename", "Name"),
        ColumnSchema::new("content_type", "Type"),
        ColumnSchema::new("size", "Size").typed("number"),
        // The IMAP section path. Rarely interesting, but it is the whole
        // address of the part — worth having when a mail refuses to open.
        ColumnSchema::new("part", "Part"),
    ]
}

/// What an attachment offers. `open` is the workhorse (fetch once into a
/// temp dir, then the OS viewer); `download all` saves every file of the
/// parent message into a directory the user names — the same pair the Jira
/// and Stoat attachment levels expose, so the keys carry over.
pub(super) fn actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("open", "open", InputSpec::None),
        NodeAction::new(
            "download_all",
            "download all",
            InputSpec::Form {
                fields: vec![FormFieldSpec::text("dir", "Target directory")],
            },
        ),
    ]
}

fn metadata_of(att: &AttachmentInfo) -> Metadata {
    Metadata {
        fields: vec![
            field("filename", att.filename.clone(), "Name"),
            field("content_type", att.content_type.clone(), "Type"),
            field("size", att.size.to_string(), "Size"),
            field("part", att.part.clone(), "Part"),
        ],
    }
}

/// A part with no filename still has to be aimable at, and its section path
/// is the one thing it always has.
fn label_of(att: &AttachmentInfo) -> String {
    if att.filename.trim().is_empty() {
        format!("part {}", att.part)
    } else {
        att.filename.clone()
    }
}

/// Every attachment of one message, as rows. No network: the parts came with
/// the envelope.
pub(super) fn list(msg: &MessageId, attachments: &[AttachmentInfo]) -> ListResult {
    let message_id = msg.encode();
    let items: Vec<NodeSummary> = attachments
        .iter()
        .map(|att| NodeSummary {
            id: attachment_id(&message_id, &att.part),
            label: label_of(att),
            node_type: attachment_type().clone(),
            metadata: metadata_of(att),
            // Files are leaves.
            has_children: Some(false),
        })
        .collect();
    ListResult {
        items,
        applied_sort: Vec::new(),
        page: None,
        batch_download_available: false,
        downloaded: Vec::new(),
    }
}

/// Expand a leading `~`, create the directory, and refuse an existing
/// non-directory *before* anything is fetched.
fn prepare_target_dir(dir_input: &str) -> Result<PathBuf> {
    let trimmed = dir_input.trim();
    if trimmed.is_empty() {
        return Err(other_err("no target directory given"));
    }
    let expanded = match trimmed.strip_prefix('~') {
        Some(rest) => {
            let home =
                dirs::home_dir().ok_or_else(|| other_err("cannot resolve home directory"))?;
            home.join(rest.trim_start_matches('/'))
        }
        None => PathBuf::from(trimmed),
    };
    if expanded.exists() && !expanded.is_dir() {
        return Err(other_err(format!(
            "{} is not a directory",
            expanded.display()
        )));
    }
    std::fs::create_dir_all(&expanded)
        .map_err(|e| other_err(format!("create {}: {e}", expanded.display())))?;
    Ok(expanded)
}

/// The name one part is written under. Prefixed with its section path so two
/// parts sharing a filename — which happens, `image001.png` twice in a
/// forwarded thread — never overwrite each other.
fn file_name_for(att: &AttachmentInfo) -> String {
    format!(
        "{}_{}",
        sanitize_component(&att.part),
        safe_file_name(&att.filename)
    )
}

pub(super) struct MailAttachmentNode {
    id: String,
    label: String,
    metadata: Metadata,
    msg: MessageId,
    att: AttachmentInfo,
    /// Every attachment of the parent message, when it is known. Empty for a
    /// node resolved straight from an id (a restored cursor); `download all`
    /// then asks the server rather than refusing.
    siblings: Vec<AttachmentInfo>,
    conn: Connection,
}

impl MailAttachmentNode {
    pub(super) fn new(
        msg: &MessageId,
        att: AttachmentInfo,
        siblings: Vec<AttachmentInfo>,
        conn: Connection,
    ) -> Self {
        Self {
            id: attachment_id(&msg.encode(), &att.part),
            label: label_of(&att),
            metadata: metadata_of(&att),
            msg: msg.clone(),
            att,
            siblings,
            conn,
        }
    }

    /// Fetch one part's decoded bytes.
    async fn bytes(&self, part: &str) -> Result<Vec<u8>> {
        self.conn
            .part(&self.msg.folder, self.msg.uid_validity, self.msg.uid, part)
            .await
            .map_err(super::mail_err)
    }

    /// Write the file into the message's temp dir (reusing a copy that is
    /// already there) and hand the path to the frontend, which opens it with
    /// the system viewer. The adapter says *what* to open, the frontend
    /// *how*.
    async fn open_external(&self) -> Result<ActionOutcome> {
        let dir = message_dir(&self.msg.encode()).map_err(|e| other_err(e.to_string()))?;
        let path = dir.join(file_name_for(&self.att));
        if !path.exists() {
            let bytes = self.bytes(&self.att.part).await?;
            tokio::fs::write(&path, &bytes)
                .await
                .map_err(|e| other_err(format!("write {}: {e}", path.display())))?;
        }
        Ok(ActionOutcome::OpenExternal {
            target: path.to_string_lossy().into_owned(),
            message: Some(format!("Opening {}", self.label)),
        })
    }

    /// Every attachment of the parent message. Known already when the node
    /// came from a listing; asked for once when it did not.
    async fn all(&self) -> Result<Vec<AttachmentInfo>> {
        if !self.siblings.is_empty() {
            return Ok(self.siblings.clone());
        }
        let row = self
            .conn
            .envelope(&self.msg.folder, self.msg.uid_validity, self.msg.uid)
            .await
            .map_err(super::mail_err)?;
        Ok(row.attachments)
    }

    /// Save every attachment of the message into `dir_input`. A part that
    /// fails is collected, not fatal: nine files out of ten is a better
    /// outcome than none.
    async fn download_all(&self, dir_input: &str) -> Result<ActionOutcome> {
        let dir = prepare_target_dir(dir_input)?;
        let all = self.all().await?;
        if all.is_empty() {
            return Ok(ActionOutcome::Done {
                message: Some("this message has no attachments".into()),
            });
        }
        let total = all.len();
        let mut saved = 0usize;
        let mut failures: Vec<String> = Vec::new();
        for att in &all {
            let path = dir.join(file_name_for(att));
            match self.bytes(&att.part).await {
                Ok(bytes) => match tokio::fs::write(&path, &bytes).await {
                    Ok(()) => saved += 1,
                    Err(e) => failures.push(format!("{}: {e}", label_of(att))),
                },
                Err(e) => failures.push(format!("{}: {e}", label_of(att))),
            }
        }
        Ok(ActionOutcome::Done {
            message: Some(summary(&dir, saved, total, &failures)),
        })
    }
}

fn summary(dir: &std::path::Path, saved: usize, total: usize, failures: &[String]) -> String {
    let mut message = format!("Saved {saved}/{total} attachment(s) to {}", dir.display());
    if !failures.is_empty() {
        message.push_str(&format!(
            " — {} failed ({})",
            failures.len(),
            failures.join("; ")
        ));
    }
    message
}

#[async_trait]
impl Node for MailAttachmentNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn node_type(&self) -> &NodeType {
        attachment_type()
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    async fn execute(
        &mut self,
        action_id: &str,
        input: ActionInput,
        _args: &ActionArgs,
    ) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("open", ActionInput::None) => self.open_external().await,
            ("download_all", input) => {
                let dir = form_field(&input, "dir")?;
                self.download_all(&dir).await
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "`{other}` is not an action of a mail attachment"
            ))),
        }
    }
}

/// The `AttachmentInfo` a node is built from when nothing listed it: the id
/// carries the part path, which is all `open` actually needs. The name is
/// unknown, so the row says the part rather than inventing a filename.
pub(super) fn placeholder(part: &str) -> AttachmentInfo {
    AttachmentInfo {
        part: part.to_string(),
        filename: String::new(),
        content_type: String::new(),
        size: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(part: &str, filename: &str) -> AttachmentInfo {
        AttachmentInfo {
            part: part.to_string(),
            filename: filename.to_string(),
            content_type: "application/pdf".into(),
            size: 4096,
        }
    }

    fn msg() -> MessageId {
        MessageId {
            account: "work".into(),
            folder: "INBOX".into(),
            uid_validity: 42,
            uid: 7,
        }
    }

    /// The rows are a projection of the envelope — ids that parse back, and
    /// leaves, because a file has nothing under it.
    #[test]
    fn attachments_list_from_the_envelope_alone() {
        let res = list(&msg(), &[att("2", "invoice.pdf"), att("3.1", "")]);
        let ids: Vec<&str> = res.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["work/INBOX#42.7/part/2", "work/INBOX#42.7/part/3.1"]);
        assert_eq!(res.items[0].label, "invoice.pdf");
        assert_eq!(
            res.items[1].label, "part 3.1",
            "a nameless part is still aimable at"
        );
        assert_eq!(res.items[0].has_children, Some(false));

        let (parsed, part) =
            crate::ids::parse_attachment_id(&res.items[0].id).expect("the id parses back");
        assert_eq!(parsed, msg());
        assert_eq!(part, "2");
    }

    /// Every column the level declares must exist in every row, or the table
    /// shows a blank where a value was promised.
    #[test]
    fn every_declared_column_has_a_cell() {
        let res = list(&msg(), &[att("2", "invoice.pdf")]);
        for column in columns() {
            assert!(
                res.items[0]
                    .metadata
                    .fields
                    .iter()
                    .any(|f| f.key == column.key),
                "no cell for `{}`",
                column.key
            );
        }
    }

    /// Two parts of one message regularly share a filename — `image001.png`
    /// twice in a forwarded thread — so the section path prefixes it.
    /// (That the name itself cannot be a path is `files`' own test.)
    #[test]
    fn a_part_is_named_after_its_section_and_its_file() {
        assert_eq!(file_name_for(&att("2.1", "invoice.pdf")), "2.1_invoice.pdf");
        assert_eq!(file_name_for(&att("2", "../../etc/passwd")), "2_passwd");
    }

    #[test]
    fn the_level_offers_open_and_download_all() {
        let ids: Vec<String> = actions().into_iter().map(|a| a.id).collect();
        assert_eq!(ids, ["open", "download_all"]);
    }

    /// The directory is validated before a single byte is fetched — the
    /// cheap failure has to happen first.
    #[tokio::test]
    async fn download_all_refuses_a_non_directory_before_any_fetch() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let node = MailAttachmentNode {
            id: String::new(),
            label: String::new(),
            metadata: Metadata { fields: Vec::new() },
            msg: msg(),
            att: att("2", "invoice.pdf"),
            siblings: Vec::new(),
            conn: Connection::from_sender(tx),
        };
        let mut file = std::env::temp_dir();
        file.push("nyd_mail_dl_not_a_dir");
        std::fs::write(&file, b"x").unwrap();
        let outcome = node.download_all(file.to_str().unwrap()).await;
        std::fs::remove_file(&file).unwrap();
        match outcome {
            Err(e) => assert!(e.to_string().contains("not a directory"), "{e}"),
            Ok(_) => panic!("expected a refusal for a non-directory path"),
        }
    }
}
