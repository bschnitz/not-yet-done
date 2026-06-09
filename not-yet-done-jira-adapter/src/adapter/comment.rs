//! Single-comment node: edit own comments, read-only `Content` access.

use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::*;

use crate::client::{JiraClient, JiraComment};

use super::types::comment_node_type;
use super::util::other_err;

/// Single-marker separator used by the comment node template (no
/// metadata = no need for the 3b layout).
const COMMENT_SEPARATOR: &str = "# ─────────────────────────────────────────────────";

pub(super) fn comment_actions() -> Vec<NodeAction> {
    vec![NodeAction::new("edit_full", "edit", InputSpec::Editor)]
}

pub(super) struct JiraCommentNode {
    client: Arc<JiraClient>,
    issue_key: String,
    /// Composite ID: `{issue_key}/comment/{comment_id}` for use in `get_by_id`.
    composite_id: String,
    comment: JiraComment,
    cached_metadata: Metadata,
}

impl JiraCommentNode {
    pub(super) fn new(
        client: Arc<JiraClient>,
        comment: JiraComment,
        issue_key: String,
    ) -> Self {
        let cached_metadata = Metadata {
            fields: vec![
                MetadataField {
                    key: "author".into(),
                    value: comment.author.clone(),
                    display_label: "Author".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "created".into(),
                    value: comment.created.clone(),
                    display_label: "Created".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "updated".into(),
                    value: comment.updated.clone(),
                    display_label: "Updated".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "issue".into(),
                    value: issue_key.clone(),
                    display_label: "Issue".into(),
                    editable: false,
                    allowed_values: None,
                },
            ],
        };
        let composite_id = format!("{}/comment/{}", issue_key, comment.id);
        Self {
            client,
            issue_key,
            composite_id,
            comment,
            cached_metadata,
        }
    }

    async fn write_body(
        &mut self,
        data: &[u8],
        expected_version: Option<&str>,
    ) -> Result<String> {
        if let Some(expected) = expected_version {
            if self.comment.updated != expected {
                return Err(ContentError::Conflict(ConflictError {
                    remote_version: self.comment.updated.clone(),
                    remote_content: Some(self.comment.body.as_bytes().to_vec()),
                    message: format!(
                        "Comment {} was modified (expected {}, current {})",
                        self.comment.id, expected, self.comment.updated
                    ),
                }));
            }
        }

        let body =
            String::from_utf8(data.to_vec()).map_err(|e| ContentError::Other(Box::new(e)))?;

        let updated = self
            .client
            .update_comment(&self.issue_key, &self.comment.id, &body)
            .await
            .map_err(other_err)?;

        self.comment = updated;
        Ok(self.comment.updated.clone())
    }
}

#[async_trait]
impl Node for JiraCommentNode {
    fn id(&self) -> &str {
        &self.composite_id
    }

    fn label(&self) -> &str {
        &self.comment.author
    }

    fn node_type(&self) -> &NodeType {
        static COMMENT_TYPE: std::sync::LazyLock<NodeType> =
            std::sync::LazyLock::new(comment_node_type);
        &COMMENT_TYPE
    }

    fn metadata(&self) -> &Metadata {
        &self.cached_metadata
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![]
    }

    async fn list(&self, _params: ListParams) -> Result<ListResult> {
        Ok(ListResult {
            items: vec![],
            applied_sort: Vec::new(),
            page: None,
            batch_download_available: false,
            downloaded: vec![],
        })
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        Err(ContentError::NotFound(format!("No child: {id}")))
    }

    fn content(&self) -> Option<&dyn Content> {
        Some(self)
    }

    fn actions(&self) -> Vec<NodeAction> {
        // Always expose `edit`; the server rejects non-author edits and
        // the user gets a clean error. Per-instance filtering would
        // violate the deterministic-per-node_type contract that lets
        // the TUI resolve hints without a `get_by_id` chain walk.
        comment_actions()
    }

    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        match action_id {
            "edit_full" => {
                let c = &self.comment;
                let template = format!(
                    "# Comment on {}\n# Author: {} | Created: {}\n{}\n\n{}",
                    self.issue_key, c.author, c.created, COMMENT_SEPARATOR, c.body
                );
                Ok(EditorPrep {
                    template,
                    version: self.comment.updated.clone(),
                    suffix: ".jira".into(),
                })
            }
            other => Err(ContentError::NotSupported(format!(
                "prepare: unknown action {other}"
            ))),
        }
    }

    async fn execute(
        &mut self,
        action_id: &str,
        input: ActionInput,
    ) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("edit_full", ActionInput::Edited { text, version, .. }) => {
                let body = parse_comment_buffer(&text);
                if body == self.comment.body.trim() {
                    return Ok(ActionOutcome::NoChanges);
                }
                let expected = if version.is_empty() { None } else { Some(version.as_str()) };
                self.write_body(body.as_bytes(), expected).await?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Comment {} updated", self.comment.id)),
                })
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "execute: unknown action {other}"
            ))),
        }
    }
}

/// Strip the comment editor's `# `-prefixed header and `# ───` separator,
/// returning the trimmed body text.
fn parse_comment_buffer(text: &str) -> String {
    let mut in_body = false;
    let mut body_lines = Vec::new();

    for line in text.lines() {
        if in_body {
            body_lines.push(line);
            continue;
        }
        if line.starts_with("# ───") {
            in_body = true;
            continue;
        }
        if line.starts_with("# ") || line == "#" {
            continue;
        }
    }

    if !in_body {
        body_lines.clear();
        for line in text.lines() {
            if !line.starts_with("# ") && line != "#" {
                body_lines.push(line);
            }
        }
    }

    body_lines.join("\n").trim().to_string()
}

#[async_trait]
impl Content for JiraCommentNode {
    fn node_type(&self) -> &NodeType {
        static COMMENT_TYPE: std::sync::LazyLock<NodeType> =
            std::sync::LazyLock::new(comment_node_type);
        &COMMENT_TYPE
    }

    fn version(&self) -> Option<&str> {
        Some(&self.comment.updated)
    }

    async fn read(&self) -> Result<Vec<u8>> {
        Ok(self.comment.body.as_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client() -> Arc<JiraClient> {
        Arc::new(
            JiraClient::new("http://localhost:0", None, None, Some("test"), false).unwrap(),
        )
    }

    fn sample_comment() -> JiraComment {
        JiraComment {
            id: "10042".into(),
            author: "bob".into(),
            author_key: "bob".into(),
            body: "This needs a fix ASAP.".into(),
            created: "2025-06-01T10:00:00.000+0000".into(),
            updated: "2025-06-01T10:00:00.000+0000".into(),
        }
    }

    #[test]
    fn comment_node_metadata() {
        let node = JiraCommentNode::new(test_client(), sample_comment(), "PROJ-42".into());
        assert_eq!(node.id(), "PROJ-42/comment/10042");
        assert_eq!(node.label(), "bob");

        let meta = node.metadata();
        assert_eq!(meta.fields.len(), 4);
        assert_eq!(meta.fields[0].key, "author");
        assert_eq!(meta.fields[0].value, "bob");
        assert_eq!(meta.fields[3].key, "issue");
        assert_eq!(meta.fields[3].value, "PROJ-42");
    }

    #[tokio::test]
    async fn comment_node_content_read() {
        let node = JiraCommentNode::new(test_client(), sample_comment(), "PROJ-42".into());
        let content = node.content().unwrap();
        let text = content.read_text().await.unwrap();
        assert_eq!(text, "This needs a fix ASAP.");
    }

    #[test]
    fn comment_node_has_no_children() {
        let node = JiraCommentNode::new(test_client(), sample_comment(), "PROJ-42".into());
        assert!(node.children_types().is_empty());
    }

    #[test]
    fn comment_node_declares_actions() {
        let node = JiraCommentNode::new(test_client(), sample_comment(), "PROJ-42".into());
        let actions = node.actions();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].id, "edit_full");
        assert!(matches!(actions[0].input, InputSpec::Editor));
    }

    #[test]
    fn comment_actions_are_static_per_type() {
        // Per the deterministic-per-node_type contract the action list
        // does not depend on the authenticated user or the comment's
        // author. Foreign-author edits are rejected by the server at
        // execute time instead.
        let actions = comment_actions();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].id, "edit_full");
    }

    #[tokio::test]
    async fn comment_prepare_contains_header_and_body() {
        let node = JiraCommentNode::new(test_client(), sample_comment(), "PROJ-42".into());
        let prep = node.prepare("edit_full").await.unwrap();

        assert!(prep.template.contains("# Comment on PROJ-42"));
        assert!(prep.template.contains("# Author: bob"));
        assert!(prep.template.contains(COMMENT_SEPARATOR));
        assert!(prep.template.contains("This needs a fix ASAP."));
        assert_eq!(prep.version, "2025-06-01T10:00:00.000+0000");
    }

    #[test]
    fn parse_comment_buffer_unchanged() {
        let text = format!(
            "# Comment on PROJ-42\n\
             # Author: bob | Created: 2025-06-01T10:00:00.000+0000\n\
             {COMMENT_SEPARATOR}\n\
             \n\
             This needs a fix ASAP."
        );
        let body = parse_comment_buffer(&text);
        assert_eq!(body, "This needs a fix ASAP.");
    }

    #[test]
    fn parse_comment_buffer_changed_body() {
        let text = format!(
            "# Comment on PROJ-42\n\
             {COMMENT_SEPARATOR}\n\
             \n\
             Updated comment body with more details."
        );
        let body = parse_comment_buffer(&text);
        assert_eq!(body, "Updated comment body with more details.");
    }

    #[tokio::test]
    async fn comment_editor_roundtrip() {
        let node = JiraCommentNode::new(test_client(), sample_comment(), "PROJ-42".into());
        let prep = node.prepare("edit_full").await.unwrap();
        let body = parse_comment_buffer(&prep.template);

        // Unchanged template parses back to the original body.
        assert_eq!(body, node.comment.body.trim());
    }
}
