//! The `mail:account` level — one row per configured mailbox.
//!
//! Pure config, no network: the row exists whether or not the account has
//! ever been reached, which is what lets the tree render instantly and lets a
//! failing account show up as a row with a status instead of as a gap.

use async_trait::async_trait;

use not_yet_done_content::{
    ColumnSchema, ListParams, ListResult, Metadata, Node, NodeSummary, NodeType, apply_sort,
};

use super::field;
use super::types::account_type;
use crate::config::AccountConfig;

/// What an account row carries. `id` is in the rows as well as being the
/// node id: a view that wants to sort or filter by it should not have to
/// parse node ids to do so.
pub(super) fn columns() -> Vec<ColumnSchema> {
    vec![
        ColumnSchema::new("name", "Account"),
        ColumnSchema::new("address", "Address"),
        ColumnSchema::new("host", "Server"),
        ColumnSchema::new("id", "ID"),
    ]
}

pub(super) fn account_row(cfg: &AccountConfig) -> NodeSummary {
    NodeSummary {
        id: cfg.id.clone(),
        label: cfg.label().to_string(),
        node_type: account_type().clone(),
        metadata: metadata_of(cfg),
        // Every account has at least an INBOX; claiming otherwise would draw
        // the row as a leaf and make the folders unreachable.
        has_children: Some(true),
    }
}

fn metadata_of(cfg: &AccountConfig) -> Metadata {
    Metadata {
        fields: vec![
            field("name", cfg.label().to_string(), "Account"),
            field(
                "address",
                cfg.address.clone().unwrap_or_default(),
                "Address",
            ),
            field("host", cfg.host.clone(), "Server"),
            field("id", cfg.id.clone(), "ID"),
        ],
    }
}

/// List the configured accounts in configuration order — the order the user
/// wrote them in is the order they mean.
pub(super) fn list_accounts(
    accounts: &[std::sync::Arc<AccountConfig>],
    params: &ListParams,
) -> ListResult {
    let mut items: Vec<NodeSummary> = accounts.iter().map(|a| account_row(a)).collect();
    let applied = apply_sort(&mut items, &params.sort, &columns());
    ListResult {
        items,
        applied_sort: applied,
        page: None,
        batch_download_available: false,
        downloaded: Vec::new(),
    }
}

/// One configured mailbox, as a node.
pub(super) struct MailAccountNode {
    id: String,
    label: String,
    metadata: Metadata,
}

impl MailAccountNode {
    pub(super) fn new(cfg: &AccountConfig) -> Self {
        Self {
            id: cfg.id.clone(),
            label: cfg.label().to_string(),
            metadata: metadata_of(cfg),
        }
    }
}

#[async_trait]
impl Node for MailAccountNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn node_type(&self) -> &NodeType {
        account_type()
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}
