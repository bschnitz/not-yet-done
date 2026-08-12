//! End-to-end edit flow for the `edit_full` action: parse → validate →
//! diff → write. Conflicts dispatch to `merge::handle_conflict`.

use not_yet_done_content::*;

use super::JiraIssueNode;
use super::edit_with_comments::{comments_canonical_to_md, comments_md_to_canonical};
use super::slugs::{canonicalize_labels_via_jira, resolve_slugs_inplace};
use super::template::{FieldError, edit_full_fields};

impl JiraIssueNode {
    /// `edit_markdown` flow: the buffer carries the header as two GFM tables,
    /// the body as Markdown, and every comment below a `## Comments` divider.
    /// Convert it back to the canonical `edit_with_comments` buffer and reuse
    /// that pipeline (parse, validate, diff, conflict-merge, comment PUT/DELETE/
    /// POST). On a recoverable `Reopen`, the canonical buffer is converted back
    /// to Markdown so the user stays in the Markdown editor.
    pub(super) async fn execute_edit_markdown(
        &mut self,
        text: &str,
        version: &str,
        original_text: Option<&str>,
    ) -> Result<ActionOutcome> {
        let canonical = comments_md_to_canonical(text);
        let canonical_original = original_text.map(comments_md_to_canonical);
        // `execute_edit_with_comments` needs the buffer the user opened with to
        // diff comments; the editor flow always supplies it, but fall back to
        // the current buffer defensively (no comment change detected).
        let original_ref = canonical_original.as_deref().unwrap_or(&canonical);
        let outcome = self
            .execute_edit_with_comments(&canonical, original_ref, version)
            .await?;
        Ok(match outcome {
            ActionOutcome::Reopen {
                content,
                new_version,
            } => ActionOutcome::Reopen {
                content: comments_canonical_to_md(&content),
                new_version,
            },
            other => other,
        })
    }

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

        // The opened-at version token is no longer used for concurrency: the
        // write path checks the actual body/field content against a baseline
        // snapshot instead (a bare version bump — e.g. our own just-added
        // comment — must not conflict). Kept in the signature so all three
        // edit entrypoints share one `(text, version, original)` shape.
        let _ = version;

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
        let detail_snapshot = self.detail().await?.clone();
        let summary_for_default = detail_snapshot.summary.clone();

        // (a2) Resolve `ll-…` / `uu-…` / `ss-…` slugs back to original Jira
        // values. First upgrade any label the cache doesn't know to its
        // canonical Jira casing (network), so `resolve_slugs_inplace` maps it
        // cleanly. The status table needs the async transition lookup, so build
        // the full tables here (labels/users from cache + reachable statuses).
        let tables = self.slug_tables(&detail_snapshot).await;
        canonicalize_labels_via_jira(&self.client, &self.cache, &tables, &mut parsed).await;
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
            let restored =
                self.restore_blanked_editable(text, original_text, &editable_fields, detail);
            return Ok(ActionOutcome::Reopen {
                content: self.render_with_errors(&restored, &v_errors),
                new_version: None,
            });
        }

        // (c) Diff — bail out if nothing actually changed. Keep an owned
        // snapshot of the detail we opened with: it is the baseline the write
        // path checks concurrency against (body content / per-field), so a
        // version bump from our own just-added comment isn't a phantom conflict.
        let baseline_detail = self.detail().await?.clone();
        let changes = self.diff_against_current(&parsed, &baseline_detail);
        let has_metadata = !changes.metadata_changes.is_empty();
        let has_content = changes.content.is_some();
        let has_status = changes.status_change.is_some();
        if !has_metadata && !has_content && !has_status {
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

        // (d) Write content body (if changed). Fold any pending summary
        // change in first so update_issue's PUT carries it too. Concurrency
        // is checked against `baseline_detail`, not the opened-at version
        // token, so the token argument is no longer threaded through.
        if let Some(new_content) = changes.content.as_ref() {
            match self
                .write_description(
                    new_content,
                    &changes.metadata_changes,
                    Some(&baseline_detail.description),
                    &summary_default,
                )
                .await
            {
                Ok(_) => {}
                Err(ContentError::Conflict(conflict)) => {
                    return self
                        .handle_conflict(text, original_text, &editable_fields, conflict)
                        .await;
                }
                Err(e) => return Err(e),
            }
        }

        // (e) Write metadata (if changed and not already covered by body PUT).
        if has_metadata && !has_content {
            match self
                .update_summary_field(&changes.metadata_changes, Some(&baseline_detail))
                .await
            {
                Ok(()) => {}
                Err(ContentError::Conflict(conflict)) => {
                    return self
                        .handle_conflict(text, original_text, &editable_fields, conflict)
                        .await;
                }
                Err(e) => return Err(e),
            }
        }

        // (f) Status change → workflow transition. Runs last: field/body PUTs
        // are done, so the transition sees the intended issue state, and it
        // re-fetches the detail afterwards. `apply_status_transition` no-ops
        // when the resolved target equals the current status.
        let mut parts: Vec<String> = Vec::new();
        if has_metadata || has_content {
            parts.push(format!("{} updated", self.key));
        }
        if let Some(target) = &changes.status_change {
            if let Some(status_msg) = self
                .apply_status_transition(target, &baseline_detail)
                .await?
            {
                parts.push(status_msg);
            }
        }
        let message = if parts.is_empty() {
            format!("{} updated", self.key)
        } else {
            parts.join("; ")
        };
        Ok(ActionOutcome::Done {
            message: Some(message),
        })
    }
}
