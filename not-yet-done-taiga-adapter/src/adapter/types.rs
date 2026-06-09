//! Static `NodeType` definitions.

use std::sync::LazyLock;

use not_yet_done_content::NodeType;

pub(super) fn task_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "taiga:task".into(),
        mime_type: "text/markdown".into(),
        syntax: Some("markdown".into()),
        file_extension: ".md".into(),
        display_name: "Task".into(),
    });
    &T
}

pub(super) fn issue_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "taiga:issue".into(),
        mime_type: "text/markdown".into(),
        syntax: Some("markdown".into()),
        file_extension: ".md".into(),
        display_name: "Issue".into(),
    });
    &T
}

pub(super) fn epic_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "taiga:epic".into(),
        mime_type: "text/markdown".into(),
        syntax: Some("markdown".into()),
        file_extension: ".md".into(),
        display_name: "Epic".into(),
    });
    &T
}

pub(super) fn userstory_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "taiga:userstory".into(),
        mime_type: "text/markdown".into(),
        syntax: Some("markdown".into()),
        file_extension: ".md".into(),
        display_name: "User Story".into(),
    });
    &T
}

pub(super) fn item_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "taiga:item".into(),
        mime_type: "text/markdown".into(),
        syntax: Some("markdown".into()),
        file_extension: ".md".into(),
        display_name: "Taiga Item".into(),
    });
    &T
}

pub(super) fn comment_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "taiga:comment".into(),
        mime_type: "text/markdown".into(),
        syntax: Some("markdown".into()),
        file_extension: ".md".into(),
        display_name: "Comment".into(),
    });
    &T
}

pub(super) fn attachment_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "taiga:attachment".into(),
        mime_type: "application/octet-stream".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Attachment".into(),
    });
    &T
}

pub(super) fn notification_type() -> &'static NodeType {
    static T: LazyLock<NodeType> = LazyLock::new(|| NodeType {
        type_id: "taiga:notification".into(),
        mime_type: "text/plain".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Notification".into(),
    });
    &T
}

/// Map the internal [`crate::client::ItemType`] to the right node type.
pub(super) fn node_type_for(it: crate::client::ItemType) -> &'static NodeType {
    use crate::client::ItemType;
    match it {
        ItemType::Task => task_type(),
        ItemType::Issue => issue_type(),
        ItemType::Epic => epic_type(),
        ItemType::UserStory => userstory_type(),
    }
}
