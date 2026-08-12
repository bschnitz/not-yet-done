//! `edit_full` action — edit subject, status, assignee, tags, body in
//! one buffer. Slugs (`ss-`/`uu-`/`tt-`) keep the buffer autocompletable;
//! the trailing COMPLETIONS section enumerates the allowed values.

use not_yet_done_content::*;

use crate::client::{
    EditFields, ItemPatch, ItemType, PatchOutcome, TaigaClient, TaigaMember, TaigaStatus,
    patch_item,
};

use super::TaigaItemNode;
use super::slugs::{TaigaSlugTables, build_status_table, build_tag_table, build_user_table};
use super::template::{
    self, FieldError, Parsed3b, edit_full_fields as fields_list, render_3b, render_with_errors,
    resolve_slugs_inplace, validate_3b,
};

pub(super) fn edit_full_fields() -> Vec<String> {
    fields_list()
}

/// Pull statuses + members + tags from the project_meta cache (lazily
/// hydrated). Errors propagate as `Other` — these endpoints all share the
/// same auth, so a partial failure is unusual and worth surfacing.
async fn fetch_project_meta(
    client: &TaigaClient,
    project_id: u64,
    item_type: ItemType,
) -> Result<(Vec<TaigaStatus>, Vec<TaigaMember>, Vec<String>)> {
    let statuses = client
        .ensure_statuses(project_id, item_type)
        .await
        .map_err(|e| ContentError::Other(e.into()))?;
    let members = client
        .ensure_members(project_id)
        .await
        .map_err(|e| ContentError::Other(e.into()))?;
    let tags = client
        .ensure_tags(project_id)
        .await
        .map_err(|e| ContentError::Other(e.into()))?;
    Ok((statuses, members, tags))
}

pub(super) fn build_tables(
    statuses: &[TaigaStatus],
    members: &[TaigaMember],
    tags: &[String],
) -> TaigaSlugTables {
    TaigaSlugTables {
        statuses: build_status_table(statuses),
        users: build_user_table(members),
        tags: build_tag_table(tags),
    }
}

impl TaigaItemNode {
    pub(super) async fn prepare_edit_full(&self) -> Result<EditorPrep> {
        let (statuses, members, tags) =
            fetch_project_meta(&self.client, self.detail.project_id, self.detail.item_type).await?;
        let tables = build_tables(&statuses, &members, &tags);
        let template = render_3b(&edit_full_fields(), &self.detail, &tables, None, None, true);
        Ok(EditorPrep {
            template,
            version: self.detail.version.to_string(),
            suffix: ".md".into(),
            file_path: None,
        })
    }

    pub(super) async fn execute_edit_full(
        &mut self,
        text: &str,
        original_text: &str,
        version: &str,
    ) -> Result<ActionOutcome> {
        let outcome = self
            .execute_edit_full_inner(text, original_text, version, None)
            .await?;
        Ok(outcome)
    }

    /// Inner edit flow shared with `edit_with_comments`: takes an optional
    /// pre-resolved comment list to append after the field PATCH.
    pub(super) async fn execute_edit_full_inner(
        &mut self,
        text: &str,
        original_text: &str,
        version: &str,
        comments_to_add: Option<&[String]>,
    ) -> Result<ActionOutcome> {
        let editable_fields = edit_full_fields();
        let (statuses, members, tags) =
            fetch_project_meta(&self.client, self.detail.project_id, self.detail.item_type).await?;
        let tables = build_tables(&statuses, &members, &tags);

        // Parse → validate → resolve slugs.
        let mut parsed = match template::parse_3b(text) {
            Ok(p) => p,
            Err(errs) => {
                return Ok(ActionOutcome::Reopen {
                    content: render_with_errors(text, &errs),
                    new_version: None,
                });
            }
        };
        let mut errors = validate_3b(&parsed, &editable_fields);
        resolve_slugs_inplace(&mut parsed, &tables, &mut errors);
        if !errors.is_empty() {
            let restored = restore_blanked_subject(&parsed, original_text, &editable_fields);
            let buf = restored.unwrap_or_else(|| text.to_string());
            return Ok(ActionOutcome::Reopen {
                content: render_with_errors(&buf, &errors),
                new_version: None,
            });
        }

        // Diff.
        let changes = template::diff_against_current(&parsed, &self.detail);
        let has_field_changes = !changes.metadata_changes.is_empty() || changes.body.is_some();
        let has_comments = comments_to_add.map(|a| !a.is_empty()).unwrap_or(false);

        if !has_field_changes && !has_comments {
            return Ok(ActionOutcome::NoChanges);
        }

        // Map metadata changes → EditFields wire form.
        let mut fields = EditFields::default();
        for (key, value) in &changes.metadata_changes {
            match key.as_str() {
                "subject" => fields.subject = Some(value.clone()),
                "status" => {
                    let id = statuses
                        .iter()
                        .find(|s| s.name == *value)
                        .map(|s| s.id)
                        .ok_or_else(|| {
                            ContentError::Other(format!("status `{value}` not in project").into())
                        })?;
                    fields.status_id = Some(id);
                }
                "assignee" => {
                    let usernames: Vec<&str> = value
                        .split(',')
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .collect();
                    let mut ids: Vec<u64> = Vec::with_capacity(usernames.len());
                    for u in &usernames {
                        let id = members
                            .iter()
                            .find(|m| m.username == *u)
                            .map(|m| m.id)
                            .ok_or_else(|| {
                                ContentError::Other(format!("assignee `{u}` not in project").into())
                            })?;
                        ids.push(id);
                    }
                    fields.assigned_to = Some(ids.first().copied());
                    fields.assigned_users = Some(ids);
                }
                "tags" => {
                    let list: Vec<String> = value
                        .split(',')
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .collect();
                    fields.tags = Some(list);
                }
                other => {
                    return Err(ContentError::NotSupported(format!(
                        "unknown editable field `{other}`"
                    )));
                }
            }
        }
        if let Some(body) = &changes.body {
            // Resolve `@uu_slug` mentions in the description to `@username`
            // so Taiga turns them into real mentions (same slug system as the
            // assignee field).
            fields.description = Some(template::resolve_user_mentions(body, &tables.users));
        }

        let starting_version: u64 = version.parse().unwrap_or(self.detail.version);

        // First PATCH: fields + first comment (if any). If there are no
        // field changes but there ARE comments, we still need at least one
        // PATCH to send the first comment.
        let comments = comments_to_add.unwrap_or(&[]);
        let (first_comment, rest_comments) = comments
            .split_first()
            .map(|(c, r)| (Some(c.as_str()), r))
            .unwrap_or((None, &[][..]));

        let patch = ItemPatch {
            item_type: self.detail.item_type,
            item_id: self.detail.id,
            version: starting_version,
            fields: &fields,
            comment: first_comment,
        };
        let mut current_version = match patch_item(&self.client, patch).await {
            Ok(PatchOutcome::Updated { new_version }) => new_version,
            Ok(PatchOutcome::VersionConflict { server_message }) => {
                return self.reopen_with_fresh_state(&parsed, &server_message).await;
            }
            Err(e) => return Err(ContentError::Other(e.into())),
        };

        // Remaining comment-adds: each is its own PATCH (each consumes a version).
        for body in rest_comments {
            let outcome = crate::client::add_comment(
                &self.client,
                self.detail.item_type,
                self.detail.id,
                current_version,
                body,
            )
            .await
            .map_err(|e| ContentError::Other(e.into()))?;
            match outcome {
                PatchOutcome::Updated { new_version } => current_version = new_version,
                PatchOutcome::VersionConflict { server_message } => {
                    return Err(ContentError::Other(
                        format!("comment-add version conflict: {server_message}").into(),
                    ));
                }
            }
        }

        // Refresh self.detail so subsequent reads/preview reflect the new state.
        let fresh =
            super::fetch_detail(&self.client, self.detail.item_type, self.detail.id).await?;
        self.detail = fresh;

        let display_ref = match &self.detail.project_slug {
            Some(slug) if !slug.is_empty() => format!("{slug}#{}", self.detail.r#ref),
            _ => format!("#{}", self.detail.r#ref),
        };
        let n_changes = changes.metadata_changes.len() + usize::from(changes.body.is_some());
        let comment_part = if comments.is_empty() {
            String::new()
        } else {
            format!(", +{} comment(s)", comments.len())
        };
        Ok(ActionOutcome::Done {
            message: Some(format!(
                "{display_ref} updated ({n_changes} field(s){comment_part})"
            )),
        })
    }

    async fn reopen_with_fresh_state(
        &self,
        user_parsed: &Parsed3b,
        server_message: &str,
    ) -> Result<ActionOutcome> {
        let fresh =
            super::fetch_detail(&self.client, self.detail.item_type, self.detail.id).await?;
        let (statuses, members, tags) =
            fetch_project_meta(&self.client, fresh.project_id, fresh.item_type).await?;
        let tables = build_tables(&statuses, &members, &tags);
        let buf =
            template::render_3b_from_parsed(user_parsed, &edit_full_fields(), &fresh, &tables);
        let banner_err = FieldError {
            message: format!("upstream changed (version conflict): {server_message}"),
        };
        Ok(ActionOutcome::Reopen {
            content: render_with_errors(&buf, &[banner_err]),
            new_version: Some(fresh.version.to_string()),
        })
    }
}

/// If the user blanked out `subject` (a required field), pull the original
/// value back in so they don't lose their other edits on the Reopen.
fn restore_blanked_subject(
    parsed: &Parsed3b,
    original_text: &str,
    _editable_fields: &[String],
) -> Option<String> {
    let needs_restore = parsed
        .editable
        .get("subject")
        .map(|v| v.trim().is_empty())
        .unwrap_or(true);
    if !needs_restore {
        return None;
    }
    let original_parsed = template::parse_3b(original_text).ok()?;
    let orig = original_parsed.editable.get("subject")?;
    if orig.trim().is_empty() {
        return None;
    }
    // Cheap restore: textual replace of the `subject:` line. Avoids needing
    // the slug tables here.
    let mut out = String::with_capacity(original_text.len());
    let mut found = false;
    for (i, line) in original_text.lines().enumerate() {
        if !found && line.trim_start().starts_with("subject:") {
            out.push_str(&format!("subject: {orig}\n"));
            found = true;
            continue;
        }
        if i > 0 || !out.is_empty() {
            // already pushed something or non-first line
        }
        out.push_str(line);
        out.push('\n');
    }
    if found { Some(out) } else { None }
}
