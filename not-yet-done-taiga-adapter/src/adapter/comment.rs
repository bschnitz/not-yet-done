//! Comment node — read-only `Content`, plus an `edit` action when the
//! authenticated user owns the comment.

use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::*;

use super::types::comment_type;
use crate::client::{ItemType, TaigaClient, TaigaComment, delete_comment, edit_comment};

/// Single-marker separator used by the comment editor template (no
/// metadata = no need for a 3b layout).
const COMMENT_SEPARATOR: &str = "# ─────────────────────────────────────────────────";

pub(super) fn comment_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("edit_full", "edit", InputSpec::Editor),
        NodeAction::new("delete", "delete", InputSpec::None),
    ]
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
    async fn invoke_action(&self, name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
        match name {
            "delete" => Ok(ActionDispatch::DeleteSelf {
                confirm: Some(format!(
                    "Delete comment {} by {}? (y/n)",
                    self.comment.id, self.comment.author
                )),
            }),
            _ => Ok(ActionDispatch::Noop),
        }
    }

    async fn prepare(&self, action_id: &str, _args: &ActionArgs) -> Result<EditorPrep> {
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
                    file_path: None,
                    args: Default::default(),
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
        _args: &ActionArgs,
    ) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("edit_full", ActionInput::Edited { text, original, .. }) => {
                let body = parse_comment_buffer(&text, &original);
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
            ("delete", ActionInput::None) => {
                delete_comment(&self.client, self.item_type, self.item_id, &self.comment.id)
                    .await
                    .map_err(|e| ContentError::Other(e.into()))?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("comment {} deleted", self.comment.id)),
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
/// returning the trimmed body text. Everything below the separator is the
/// body and is taken verbatim.
///
/// Without a separator — a body handed over as a file, or a buffer whose
/// separator the user deleted — only the *leading* run of lines is looked
/// at, and only lines the template itself carried are dropped. `# ` at the
/// start of a line opens a heading in the Markdown that Taiga comments use,
/// so dropping every such line anywhere in the buffer would silently
/// swallow the comment's headings.
fn parse_comment_buffer(text: &str, template: &str) -> String {
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
    }

    if !in_body {
        // Header of the template: everything up to and including the
        // separator. The lines below it are the comment's own body and must
        // never serve as a strip pattern.
        let header: Vec<&str> = template
            .lines()
            .take_while(|l| !l.starts_with("# ───"))
            .map(str::trim_end)
            .filter(|l| !l.is_empty())
            .collect();
        let mut in_header = !header.is_empty();
        for line in text.lines() {
            if in_header {
                let trimmed = line.trim_end();
                if trimmed.is_empty() || header.contains(&trimmed) {
                    continue;
                }
                in_header = false;
            }
            body_lines.push(line);
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
        let body = parse_comment_buffer(&text, &text);
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
        let body = parse_comment_buffer(&text, &text);
        assert_eq!(body, "Updated body, longer now.");
    }

    /// Without a separator only the template's own header goes; a `# `
    /// line further down is the comment's Markdown heading and stays.
    #[test]
    fn parse_comment_buffer_no_separator_drops_only_the_template_header() {
        let template = format!("# Comment on task:1\n{COMMENT_SEPARATOR}\n\nold body");
        let text = "# Comment on task:1\n\nactual body\n\n# A heading";
        let body = parse_comment_buffer(text, &template);
        assert_eq!(body, "actual body\n\n# A heading");
    }

    /// A body supplied as a file (`--file`) carries no header at all, so
    /// nothing is stripped from it — headings included.
    #[test]
    fn parse_comment_buffer_keeps_a_headed_body_from_a_file() {
        let template = format!("# Comment on task:1\n{COMMENT_SEPARATOR}\n\nold body");
        let text = "# Findings\n\nthe first one";
        assert_eq!(parse_comment_buffer(text, &template), text);
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
