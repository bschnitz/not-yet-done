//! 3-way merge against upstream when a 412/conflict comes back from the
//! save. Disjoint changes auto-apply; overlapping changes come back as a
//! `Reopen` buffer with git-style markers and a banner.

use std::sync::Arc;

use not_yet_done_content::*;

use crate::client::JiraIssueDetail;

use super::super::cache::fetch_issue;
use super::super::util::{ensure_trailing_newline, normalize_blank_lines, other_err};
use super::JiraIssueNode;
use super::markers::{CONFLICT_BANNER_END, CONFLICT_BANNER_START};
use super::template::{metadata_changes_to_fields, strip_banner};

/// Prepend the conflict banner to a buffer that already contains diffy's
/// `<<<<<<< ours` / `>>>>>>> theirs` markers. Banners are stripped first so
/// repeated reopens don't stack.
pub(super) fn render_conflict_banner(text: &str) -> String {
    let mut out = String::new();
    out.push_str(CONFLICT_BANNER_START);
    out.push('\n');
    out.push_str("# Issue was modified upstream while you were editing.\n");
    out.push_str("# Disjoint changes were merged automatically. Each remaining\n");
    out.push_str("# `<<<<<<< ours` … `>>>>>>> theirs` block marks an overlapping\n");
    out.push_str("# change — keep one side and remove the markers, then save.\n");
    out.push_str(CONFLICT_BANNER_END);
    out.push('\n');
    out.push_str(strip_banner(text));
    out
}

impl JiraIssueNode {
    /// Handle a 412/conflict via a line-level 3-way merge using the
    /// `diffy` crate. Inputs:
    /// - **ancestor**: the template the user opened with (`original_text`),
    ///   or the freshly-rendered template as a fallback.
    /// - **ours**: the user's current buffer.
    /// - **theirs**: the freshly-rendered template (current upstream state).
    ///
    /// Cleanly-merging changes (different lines, or one side untouched) are
    /// applied automatically. Overlapping changes come back as a `Reopen`
    /// buffer with git-style `<<<<<<< ours` / `>>>>>>> theirs` markers
    /// placed exactly at the conflicting line — read-only fields, hint
    /// comments, and disjoint body changes do *not* trigger conflicts.
    pub(super) async fn handle_conflict(
        &mut self,
        user_text: &str,
        original_text: Option<&str>,
        editable_fields: &[String],
        conflict: ConflictError,
    ) -> Result<ActionOutcome> {
        let fresh = match fetch_issue(&self.client, &self.cache, &self.key).await {
            Ok(f) => f,
            Err(_) => {
                // Can't re-fetch — surface a banner, return user's text as-is.
                let mut content = String::new();
                content.push_str(CONFLICT_BANNER_START);
                content.push('\n');
                content.push_str("# Issue was modified upstream and the fresh state could not be re-fetched.\n");
                content.push_str("# Save again to overwrite, or Esc to cancel.\n");
                content.push_str(CONFLICT_BANNER_END);
                content.push('\n');
                content.push_str(strip_banner(user_text));
                return Ok(ActionOutcome::Reopen {
                    content,
                    new_version: Some(conflict.remote_version),
                });
            }
        };

        // Refresh node so render_3b uses the upstream state.
        self.replace_detail(fresh.clone());
        let fresh_text   = ensure_trailing_newline(self.render_3b(editable_fields, &fresh, None, None));
        let user_text_n  = ensure_trailing_newline(strip_banner(user_text).to_string());
        let ancestor_raw = original_text
            .map(|t| ensure_trailing_newline(strip_banner(t).to_string()))
            .unwrap_or_else(|| fresh_text.clone());

        // Normalize blank-line runs across all three inputs before merging.
        // Without this, server-side body reformatting (e.g. Jira UI saves
        // adding blank lines between paragraphs) makes diffy treat
        // ancestor and theirs as completely different, which collapses
        // into a single whole-document conflict region.
        let ancestor_n = normalize_blank_lines(&ancestor_raw);
        let user_n     = normalize_blank_lines(&user_text_n);
        let fresh_n    = normalize_blank_lines(&fresh_text);

        let mut opts = diffy::MergeOptions::new();
        opts.set_conflict_style(diffy::ConflictStyle::Merge);
        match opts.merge(&ancestor_n, &user_n, &fresh_n) {
            Ok(clean) => {
                // Disjoint or one-sided edits — write the merged buffer through.
                self.auto_apply_clean(&clean, &fresh, editable_fields).await
            }
            Err(marked) => {
                let content = render_conflict_banner(&marked);
                Ok(ActionOutcome::Reopen {
                    content,
                    new_version: Some(fresh.updated.clone()),
                })
            }
        }
    }

    /// Parse, validate and write the cleanly-merged buffer. If a second
    /// concurrent change races our PUT we return `Reopen` with the merged
    /// text + conflict banner instead of recursing — the user can retry
    /// manually.
    async fn auto_apply_clean(
        &mut self,
        clean: &str,
        fresh: &JiraIssueDetail,
        editable_fields: &[String],
    ) -> Result<ActionOutcome> {
        let parsed = match self.parse_3b(clean) {
            Ok(p) => p,
            Err(errs) => {
                return Ok(ActionOutcome::Reopen {
                    content: self.render_with_errors(clean, &errs),
                    new_version: Some(fresh.updated.clone()),
                });
            }
        };
        let v_errors = self.validate_3b(&parsed, editable_fields);
        if !v_errors.is_empty() {
            return Ok(ActionOutcome::Reopen {
                content: self.render_with_errors(clean, &v_errors),
                new_version: Some(fresh.updated.clone()),
            });
        }

        // handle_conflict has already replaced self.detail with `fresh`
        // — diff against `fresh` directly, no extra fetch needed.
        let changes = self.diff_against_current(&parsed, fresh);
        if changes.metadata_changes.is_empty() && changes.content.is_none() {
            return Ok(ActionOutcome::Done {
                message: Some(format!(
                    "{} updated (auto-merged with upstream changes)",
                    self.key
                )),
            });
        }

        // The body PUT also rewrites `summary`; pass through any merged
        // summary change so `write_description`'s default isn't the stale
        // upstream value.
        let summary_default = changes
            .metadata_changes
            .iter()
            .find(|(k, _)| k == "summary")
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| fresh.summary.clone());

        let mut new_version = fresh.updated.clone();
        if let Some(new_content) = changes.content.as_ref() {
            match self
                .write_description(
                    new_content,
                    &changes.metadata_changes,
                    Some(&new_version),
                    &summary_default,
                )
                .await
            {
                Ok(v) => new_version = v,
                Err(ContentError::Conflict(_)) => {
                    return Ok(ActionOutcome::Reopen {
                        content: render_conflict_banner(clean),
                        new_version: None,
                    });
                }
                Err(e) => return Err(e),
            }
        } else if !changes.metadata_changes.is_empty() {
            // Body unchanged → body PUT skipped → metadata fields need their own PUT.
            match self.update_summary_field(&changes.metadata_changes, Some(&new_version)).await {
                Ok(()) => {}
                Err(ContentError::Conflict(_)) => {
                    return Ok(ActionOutcome::Reopen {
                        content: render_conflict_banner(clean),
                        new_version: None,
                    });
                }
                Err(e) => return Err(e),
            }
        }

        let _ = new_version;
        Ok(ActionOutcome::Done {
            message: Some(format!(
                "{} updated (auto-merged with upstream changes)",
                self.key
            )),
        })
    }

    /// Write the issue description body. Returns the new version token.
    /// Conflict detection re-fetches and compares the `updated` timestamp.
    /// `summary_default` is what gets set on the wire when the
    /// `metadata_changes` list doesn't already include a summary update —
    /// passing it explicitly avoids any dependency on `self.detail`'s
    /// summary, which may be stale or unloaded.
    pub(super) async fn write_description(
        &mut self,
        data: &[u8],
        metadata_changes: &[(String, String)],
        expected_version: Option<&str>,
        summary_default: &str,
    ) -> Result<String> {
        if let Some(expected) = expected_version {
            let current = fetch_issue(&self.client, &self.cache, &self.key)
                .await
                .map_err(other_err)?;
            if current.updated != expected {
                return Err(ContentError::Conflict(ConflictError {
                    remote_version: current.updated.clone(),
                    remote_content: Some(current.description.as_bytes().to_vec()),
                    message: format!(
                        "Issue {} was modified (expected version {}, remote version {})",
                        self.key, expected, current.updated
                    ),
                }));
            }
        }

        let description =
            String::from_utf8(data.to_vec()).map_err(|e| ContentError::Other(Box::new(e)))?;

        let mut fields = metadata_changes_to_fields(metadata_changes)?;
        fields.insert(
            "description".into(),
            serde_json::Value::String(description),
        );
        // Always re-include summary so a description-only PUT doesn't drop it.
        fields
            .entry("summary".to_string())
            .or_insert_with(|| serde_json::Value::String(summary_default.to_string()));

        self.client
            .update_fields(&self.key, fields)
            .await
            .map_err(other_err)?;

        let refreshed = fetch_issue(&self.client, &self.cache, &self.key)
            .await
            .map_err(other_err)?;
        let new_version = refreshed.updated.clone();
        *self = JiraIssueNode::from_detail(Arc::clone(&self.client), Arc::clone(&self.cache), refreshed);
        Ok(new_version)
    }

    /// Update metadata fields that aren't covered by the body PUT.
    /// Handles `summary`, `labels`, and `assignee`.
    pub(super) async fn update_summary_field(
        &self,
        changes: &[(String, String)],
        expected_version: Option<&str>,
    ) -> Result<()> {
        if let Some(expected) = expected_version {
            let current = fetch_issue(&self.client, &self.cache, &self.key)
                .await
                .map_err(other_err)?;
            if current.updated != expected {
                return Err(ContentError::Conflict(ConflictError {
                    remote_version: current.updated.clone(),
                    remote_content: None,
                    message: format!(
                        "Issue {} was modified (expected {}, remote {})",
                        self.key, expected, current.updated
                    ),
                }));
            }
        }

        let fields = metadata_changes_to_fields(changes)?;
        self.client
            .update_fields(&self.key, fields)
            .await
            .map_err(other_err)
    }
}

