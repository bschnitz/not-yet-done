//! The `mail:folder` level — the mailbox tree of one account.
//!
//! Rows carry the counts a mail client is read for: how much is in the folder
//! and how much of it is unread. Those come from `STATUS`, one round trip per
//! folder, so they are fetched for the folders of the level being listed and
//! not for the whole tree (see [`crate::imap::conn::Connection::folder_status`]).

use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::{ActionArgs, 
    ActionInput, ActionOutcome, ColumnSchema, ContentError, EditorPrep, InputSpec, Metadata, Node,
    NodeAction, NodeSummary, NodeType, Result,
};

use super::field;
use super::message::{compose_prep, compose_send};
use super::types::folder_type;
use crate::compose::outbox::Outbox;
use crate::ids::folder_id;
use crate::model::FolderInfo;

/// The folder columns, in the order a mail client shows them. Also what
/// `describe_columns("mail:folder")` answers, so the view config and the rows
/// cannot drift apart.
pub(super) fn columns() -> Vec<ColumnSchema> {
    vec![
        ColumnSchema::new("name", "Folder"),
        ColumnSchema::new("unread_count", "Unread").typed("number"),
        ColumnSchema::new("total", "Total").typed("number"),
        ColumnSchema::new("path", "Path"),
    ]
}

/// A count as a cell: unknown (`\Noselect`, or not asked for) is empty rather
/// than `0` — "cannot be counted" and "counted zero" are different answers.
fn count(value: Option<u32>) -> String {
    value.map(|v| v.to_string()).unwrap_or_default()
}

pub(super) fn metadata_of(account: &str, info: &FolderInfo) -> Metadata {
    Metadata {
        fields: vec![
            field("name", info.name.clone(), "Folder"),
            field("unread_count", count(info.unread), "Unread"),
            field("total", count(info.total), "Total"),
            field("path", info.path.clone(), "Path"),
            field("account", account.to_string(), "Account"),
            // The flag the styling layer paints from — `unread` is the key
            // the frontend looks under, the same one the Stoat rows carry,
            // which is why the COUNT above had to be called something else.
            // "true" means "highlight this row"; empty means it is read.
            field(
                "unread",
                match info.unread {
                    Some(n) if n > 0 => "true".to_string(),
                    _ => String::new(),
                },
                "Unread",
            ),
        ],
    }
}

pub(super) fn folder_row(account: &str, info: &FolderInfo, has_children: bool) -> NodeSummary {
    NodeSummary {
        id: folder_id(account, &info.path),
        label: info.name.clone(),
        node_type: folder_type().clone(),
        metadata: metadata_of(account, info),
        has_children: Some(has_children),
    }
}

/// What a folder offers.
///
/// Only writing a new message — everything else on this level is a listing.
/// It sits here as well as on the message below it for one reason: an empty
/// folder has no row to stand on, and `target: parent` then finds the folder.
pub(super) fn actions() -> Vec<NodeAction> {
    vec![NodeAction::new("compose", "new message", InputSpec::Editor)]
}

/// One mailbox, as a node.
pub(super) struct MailFolderNode {
    id: String,
    label: String,
    metadata: Metadata,
    outbox: Arc<Outbox>,
}

impl MailFolderNode {
    pub(super) fn new(account: &str, info: &FolderInfo, outbox: Arc<Outbox>) -> Self {
        Self {
            id: folder_id(account, &info.path),
            label: info.name.clone(),
            metadata: metadata_of(account, info),
            outbox,
        }
    }
}

#[async_trait]
impl Node for MailFolderNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn node_type(&self) -> &NodeType {
        folder_type()
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    async fn prepare(&self, action_id: &str, _args: &ActionArgs) -> Result<EditorPrep> {
        match action_id {
            "compose" => compose_prep(&self.outbox).await,
            other => Err(ContentError::NotSupported(format!(
                "`{other}` does not open an editor on a mail folder"
            ))),
        }
    }

    async fn execute(&mut self, action_id: &str, input: ActionInput, _args: &ActionArgs) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("compose", ActionInput::Edited { text, .. }) => {
                compose_send(&self.outbox, &text).await
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "`{other}` is not an action of a mail folder"
            ))),
        }
    }
}
