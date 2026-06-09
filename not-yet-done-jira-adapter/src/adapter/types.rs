//! Constructors for the `NodeType` values produced by the Jira adapter.
//! Hard-coded — the type IDs / mime / extensions are part of the adapter
//! contract and don't vary per connection.

use not_yet_done_content::NodeType;

pub(super) fn issue_node_type() -> NodeType {
    NodeType {
        type_id: "jira:issue".into(),
        mime_type: "text/x-jira-wiki".into(),
        syntax: Some("jira".into()),
        file_extension: ".jira".into(),
        display_name: "Issue".into(),
    }
}

pub(super) fn label_node_type() -> NodeType {
    NodeType {
        type_id: "jira:label".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Label".into(),
    }
}

pub(super) fn comment_node_type() -> NodeType {
    NodeType {
        type_id: "jira:comment".into(),
        mime_type: "text/x-jira-wiki".into(),
        syntax: Some("jira".into()),
        file_extension: ".jira".into(),
        display_name: "Comment".into(),
    }
}

pub(super) fn attachment_node_type() -> NodeType {
    NodeType {
        type_id: "jira:attachment".into(),
        mime_type: "application/octet-stream".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Attachment".into(),
    }
}

pub(super) fn user_node_type() -> NodeType {
    NodeType {
        type_id: "jira:user".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "User".into(),
    }
}
