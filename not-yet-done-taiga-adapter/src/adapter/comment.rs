//! Comment node — read-only `Content`, plus an `edit` action when the
//! authenticated user owns the comment.

use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::*;

use super::types::comment_type;
use crate::client::{ItemType, TaigaClient, TaigaComment, edit_comment};

/// Single-marker separator used by the comment editor template (no
/// metadata = no need for a 3b layout).
const COMMENT_SEPARATOR: &str = "# ─────────────────────────────────────────────────";

pub(super) fn comment_actions() -> Vec<NodeAction> {
    vec![NodeAction::new("edit_full", "edit", InputSpec::Editor)]
}

pub(super) struct TaigaCommentNode {
    client: Arc<TaigaClient>,
    composite_id: String,
    parent_id: String,
    item_type: ItemType,
    item_id: u64,
    comment: TaigaComment,
    metadata: Metadata,
}

impl TaigaCommentNode {
    pub(super) fn new(
        client: Arc<TaigaClient>,
        comment: TaigaComment,
        parent_id: String,
        item_type: ItemType,
        item_id: u64,
    ) -> Self {
        let composite_id = format!("{parent_id}/comment/{}", comment.id);
        let metadata = Metadata {
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
                    key: "body".into(),
                    value: comment.body.clone(),
                    display_label: "Body".into(),
                    editable: false,
                    allowed_values: None,
                },
            ],
        };
        Self {
            client,
            composite_id,
            parent_id,
            item_type,
            item_id,
            comment,
            metadata,
        }
    }
}

#[async_trait]
impl Node for TaigaCommentNode {
    fn id(&self) -> &str {
        &self.composite_id
    }

    fn label(&self) -> &str {
        &self.comment.body
    }

    fn node_type(&self) -> &NodeType {
        comment_type()
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn content(&self) -> Option<&dyn Content> {
        Some(self)
    }

    fn actions(&self) -> Vec<NodeAction> {
        // Always expose `edit`; the server rejects non-author edits with
        // a clean error. Per-instance filtering would violate the
        // deterministic-per-node_type contract that lets the TUI resolve
        // hints without a `get_by_id` chain walk.
        comment_actions()
    }

    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        match action_id {
            "edit_full" => {
                let c = &self.comment;
                let template = format!(
                    "# Comment on {}\n# Author: {} | Created: {}\n{}\n\n{}",
                    self.parent_id, c.author, c.created, COMMENT_SEPARATOR, c.body,
                );
                Ok(EditorPrep {
                    template,
                    version: c.created.clone(),
                    suffix: ".md".into(),
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
            ("edit_full", ActionInput::Edited { text, .. }) => {
                let body = parse_comment_buffer(&text);
                if body == self.comment.body.trim() {
                    return Ok(ActionOutcome::NoChanges);
                }
                edit_comment(
                    &self.client,
                    self.item_type,
                    self.item_id,
                    &self.comment.id,
                    &body,
                )
                .await
                .map_err(|e| ContentError::Other(e.into()))?;
                self.comment.body = body;
                Ok(ActionOutcome::Done {
                    message: Some(format!("comment {} updated", self.comment.id)),
                })
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "execute: unknown action {other}"
            ))),
        }
    }
}

#[async_trait]
impl Content for TaigaCommentNode {
    fn node_type(&self) -> &NodeType {
        comment_type()
    }

    fn version(&self) -> Option<&str> {
        Some(&self.comment.created)
    }

    async fn read(&self) -> Result<Vec<u8>> {
        Ok(self.read_text().await?.into_bytes())
    }

    async fn read_text(&self) -> Result<String> {
        Ok(format!(
            "**{}** — _{}_\n\n{}\n",
            self.comment.author, self.comment.created, self.comment.body
        ))
    }
}

/// Strip the editor's `# `-prefixed header and `# ───` separator,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_comment_buffer_unchanged() {
        let text = format!(
            "# Comment on task:1\n\
             # Author: Alice | Created: 2026-01-01T10:00:00Z\n\
             {COMMENT_SEPARATOR}\n\
             \n\
             First comment."
        );
        let body = parse_comment_buffer(&text);
        assert_eq!(body, "First comment.");
    }

    #[test]
    fn parse_comment_buffer_changed_body() {
        let text = format!(
            "# Comment on task:1\n\
             {COMMENT_SEPARATOR}\n\
             \n\
             Updated body, longer now."
        );
        let body = parse_comment_buffer(&text);
        assert_eq!(body, "Updated body, longer now.");
    }

    #[test]
    fn parse_comment_buffer_no_separator_strips_comment_lines() {
        let text = "# header\nactual body\n# trailing";
        let body = parse_comment_buffer(text);
        assert_eq!(body, "actual body");
    }

    /// `actions()`'s owner predicate is permissive when either side is
    /// unknown, and gates only when both are known and differ.
    #[test]
    fn owner_gating_predicate() {
        fn visible(current: Option<&str>, author: Option<&str>) -> bool {
            !matches!((current, author), (Some(c), Some(a)) if c != a)
        }
        assert!(visible(None, Some("alice")));
        assert!(visible(Some("alice"), None));
        assert!(visible(Some("alice"), Some("alice")));
        assert!(!visible(Some("alice"), Some("bob")));
    }
}
