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

/// One of an issue's links — a row in the linked-tickets table under an
/// issue. The node is the *link*, not the ticket at its other end; that
/// ticket is reachable as an ordinary [`issue_node_type`] node via the row's
/// `key` field (view config: `node_id_from: key`).
pub(super) fn link_node_type() -> NodeType {
    NodeType {
        type_id: "jira:link".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Link".into(),
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

/// Root list type for the bookmarks view. The *rows* it produces are
/// ordinary [`issue_node_type`] issues — this type only selects the
/// bookmark-restricted root listing in `JiraRoot::list`.
pub(super) fn bookmark_node_type() -> NodeType {
    NodeType {
        type_id: "jira:bookmark".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Bookmark".into(),
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
