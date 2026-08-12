//! Static `NodeType` definitions for the Stoat tree.

use std::sync::LazyLock;

use not_yet_done_content::NodeType;

pub(super) fn root_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "stoat:root".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Stoat Root".into(),
    });
    &T
}

pub(super) fn server_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "stoat:server".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Server".into(),
    });
    &T
}

pub(super) fn category_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "stoat:category".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Category".into(),
    });
    &T
}

pub(super) fn channel_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "stoat:channel".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Channel".into(),
    });
    &T
}

/// One uploaded file below a message. Leaf; the mime type is per-file, so
/// the static descriptor stays generic (the node's metadata carries the
/// real `content_type`).
pub(super) fn attachment_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "stoat:attachment".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Attachment".into(),
    });
    &T
}

pub(super) fn message_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "stoat:message".into(),
        mime_type: "text/markdown".into(),
        syntax: Some("markdown".into()),
        file_extension: ".md".into(),
        display_name: "Message".into(),
    });
    &T
}
