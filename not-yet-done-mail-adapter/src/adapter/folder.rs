//! The `mail:folder` level — the mailbox tree of one account.
//!
//! Rows carry the counts a mail client is read for: how much is in the folder
//! and how much of it is unread. Those come from `STATUS`, one round trip per
//! folder, so they are fetched for the folders of the level being listed and
//! not for the whole tree (see [`crate::imap::conn::Connection::folder_status`]).

use async_trait::async_trait;

use not_yet_done_content::{ColumnSchema, Metadata, Node, NodeSummary, NodeType};

use super::field;
use super::types::folder_type;
use crate::ids::folder_id;
use crate::model::FolderInfo;

/// The folder columns, in the order a mail client shows them. Also what
/// `describe_columns("mail:folder")` answers, so the view config and the rows
/// cannot drift apart.
pub(super) fn columns() -> Vec<ColumnSchema> {
    vec![
        ColumnSchema::new("name", "Folder"),
        ColumnSchema::new("unread", "Unread").typed("number"),
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
            field("unread", count(info.unread), "Unread"),
            field("total", count(info.total), "Total"),
            field("path", info.path.clone(), "Path"),
            field("account", account.to_string(), "Account"),
            // The unread *marker* the styling layer paints from, mirroring
            // the Stoat rows: non-empty means "highlight this row".
            field(
                "unread_marker",
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

/// One mailbox, as a node.
pub(super) struct MailFolderNode {
    id: String,
    label: String,
    metadata: Metadata,
}

impl MailFolderNode {
    pub(super) fn new(account: &str, info: &FolderInfo) -> Self {
        Self {
            id: folder_id(account, &info.path),
            label: info.name.clone(),
            metadata: metadata_of(account, info),
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
}
