//! Root node of the mail tree — the instance above every account.
//!
//! It is only ever *seen* by a view that browses shape B (one tree over all
//! accounts). Shape A points each subtab straight at the folder level with
//! `query: "account:<id>"`, and then the root is just the thing those
//! listings hang off.

use async_trait::async_trait;

use not_yet_done_content::{Metadata, Node, NodeType};

use super::ROOT_ID;
use super::types::root_type;

pub(super) struct MailRoot {
    pub(super) name: String,
}

#[async_trait]
impl Node for MailRoot {
    fn id(&self) -> &str {
        ROOT_ID
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn node_type(&self) -> &NodeType {
        root_type()
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: Metadata = Metadata { fields: Vec::new() };
        &EMPTY
    }
}
