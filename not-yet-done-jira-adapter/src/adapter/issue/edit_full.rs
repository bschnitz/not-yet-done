//! End-to-end edit flow for the `edit_full` action: parse → validate →
//! diff → write. Conflicts dispatch to `merge::handle_conflict`.

use not_yet_done_content::*;

use super::JiraIssueNode;
use super::slugs::{build_slug_tables, resolve_slugs_inplace};
use super::template::{FieldError, edit_full_fields};

impl JiraIssueNode {
    /// End-to-end edit flow for the `edit_full` action. Parse + validation
    /// errors come back as `Reopen` with an inline `# ─── ERRORS ───`
    /// banner above the editable section. Conflicts trigger a 3-way merge
    /// against `original_text` (the buffer the user opened with): disjoint
    /// changes auto-merge and save; overlapping changes come back as
    /// `Reopen` with git-style `<<<<<<< ours` / `>>>>>>> theirs` markers.
    pub(super) async fn execute_edit_full(
        &mut self,
        text: &str,
        version: &str,
        original_text: Option<&str>,
    ) -> Result<ActionOutcome> {
        let editable_fields = edit_full_fields();

        // (a) Parse — structural errors are recoverable.
        let mut parsed = match self.parse_3b(text) {
            Ok(p) => p,
            Err(errs) => {
                return Ok(ActionOutcome::Reopen {
                    content: self.render_with_errors(text, &errs),
                    new_version: None,
                });
            }
        };

        // Detail must be loaded by here — the editor flow always opens
        // through `prepare()` which has already awaited it. We re-await
        // defensively (cheap when already cached).
        let summary_for_default = self.detail().await?.summary.clone();

        // (a2) Resolve `ll-…` / `uu-…` slugs back to original Jira values.
        let tables = build_slug_tables(&self.cache);
        let mut slug_errors: Vec<FieldError> = Vec::new();
        resolve_slugs_inplace(&mut parsed, &tables, &mut slug_errors);

        // (b) Validate — field-level errors are recoverable. If the user
        // blanked a required field, restore it from the original buffer
        // before re-rendering, so they don't lose context staring at an
        // empty `summary:` line.
        let mut v_errors = self.validate_3b(&parsed, &editable_fields);
        v_errors.append(&mut slug_errors);
        if !v_errors.is_empty() {
            let detail = self.detail().await?;
            let restored = self.restore_blanked_editable(text, original_text, &editable_fields, detail);
            return Ok(ActionOutcome::Reopen {
                content: self.render_with_errors(&restored, &v_errors),
                new_version: None,
            });
        }

        // (c) Diff — bail out if nothing actually changed.
        let changes = self.diff_against_current(&parsed, self.detail().await?);
        let has_metadata = !changes.metadata_changes.is_empty();
        let has_content  = changes.content.is_some();
        if !has_metadata && !has_content {
            return Ok(ActionOutcome::NoChanges);
        }

        // The body PUT also rewrites `summary`, so write_description needs
        // a default to fall back to when the user didn't change it. Pull
        // the value from any pending summary change first; otherwise from
        // the loaded detail.
        let summary_default = changes
            .metadata_changes
            .iter()
            .find(|(k, _)| k == "summary")
            .map(|(_, v)| v.clone())
            .unwrap_or(summary_for_default);

        let mut new_version = version.to_string();

        // (d) Write content body (if changed). Fold any pending summary
        // change in first so update_issue's PUT carries it too.
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
                Err(ContentError::Conflict(conflict)) => {
                    return self.handle_conflict(text, original_text, &editable_fields, conflict).await;
                }
                Err(e) => return Err(e),
            }
        }

        // (e) Write metadata (if changed and not already covered by body PUT).
        if has_metadata && !has_content {
            match self.update_summary_field(&changes.metadata_changes, Some(&new_version)).await {
                Ok(()) => {}
                Err(ContentError::Conflict(conflict)) => {
                    return self.handle_conflict(text, original_text, &editable_fields, conflict).await;
                }
                Err(e) => return Err(e),
            }
        }

        Ok(ActionOutcome::Done {
            message: Some(format!("{} updated", self.key)),
        })
    }
}
