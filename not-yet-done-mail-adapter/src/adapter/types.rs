//! Static `NodeType` definitions for the mail tree.
//!
//! One `LazyLock` per type, handed out by reference: the descriptors are
//! constants of the adapter, and a view's `node_type:` names them verbatim.

use std::sync::LazyLock;

use not_yet_done_content::NodeType;

/// The instance above all accounts. A view only meets it when it browses
/// shape B (one tree over every account); shape A pins a subtab straight to
/// one account's folders.
pub(super) fn root_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "mail:root".into(),
        mime_type: String::new(),
        syntax: None,
        file_extension: String::new(),
        display_name: "Mail".into(),
    });
    &T
}

pub(super) fn account_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "mail:account".into(),
        mime_type: String::new(),
        syntax: None,
        file_extension: String::new(),
        display_name: "Account".into(),
    });
    &T
}

pub(super) fn folder_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "mail:folder".into(),
        mime_type: String::new(),
        syntax: None,
        file_extension: String::new(),
        display_name: "Folder".into(),
    });
    &T
}

pub(super) fn message_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "mail:message".into(),
        mime_type: "text/plain".into(),
        syntax: None,
        file_extension: ".md".into(),
        display_name: "Message".into(),
    });
    &T
}

/// A file hanging off a message. `file_extension` stays empty: the extension
/// of an attachment is the one its own filename carries, and inventing one
/// here would rename every part to the same thing.
pub(super) fn attachment_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "mail:attachment".into(),
        mime_type: String::new(),
        syntax: None,
        file_extension: String::new(),
        display_name: "Attachment".into(),
    });
    &T
}
