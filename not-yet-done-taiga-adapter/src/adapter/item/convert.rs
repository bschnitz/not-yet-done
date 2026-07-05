//! `convert` action — change a Taiga item's type.
//!
//! Convert is a two-step flow. Triggering it opens a **target-type picker**
//! (an `InputSpec::Picker` listing every type the source can become — issue,
//! epic, user story; never task, and never the source's own type). Picking a
//! target returns [`ActionOutcome::OpenEditor`] for that target's dedicated
//! editor action `convert:<target>`, which the frontend then opens like any
//! other editor. Choosing the target *before* the editor lets each editor be
//! purpose-built for its destination — different conversion directions drop
//! different source fields, which no single editor could represent cleanly.
//!
//! Taiga has no symmetric "change type" endpoint, so a conversion means:
//! create the target item, migrate comments + attachments, then delete the
//! source. All directions use the normal `create_item` endpoint — Taiga's
//! native `promote_to_us` is missing on some deployments (plain-HTML 404), so
//! we don't rely on it. The editor opens with the target's fields pre-filled
//! plus a read-only infoblock ("what will be migrated / deleted") and an
//! editable conversion note listing source fields with no target equivalent.
//!
//! Ordering is strict: create → migrate → **only on success** delete the
//! source. A migration failure keeps the source and reports a warning; a
//! delete failure navigates to the new item and warns "delete manually".

use std::collections::HashSet;

use not_yet_done_content::*;

use crate::client::{
    self, CreateFields, EditFields, ItemPatch, ItemType, PatchOutcome, TaigaClient, TaigaMember,
    TaigaStatus, add_comment, create_item, delete_item, download_attachment, fetch_comments,
    list_attachments, patch_item, upload_attachment_bytes,
};

use super::TaigaItemNode;
use super::edit_full::{build_tables, edit_full_fields};
use super::node_type_for;
use super::slugs::TaigaSlugTables;
use super::template::{
    self, BODY_MARKER, EDITABLE_MARKER, FieldError, render_cache_section, render_with_errors,
    resolve_slugs_inplace, validate_3b,
};

/// The convert *menu* action id — an `InputSpec::Picker` whose options are the
/// per-target editor action ids. Selecting one yields
/// [`ActionOutcome::OpenEditor`] for that target's editor.
pub(super) const CONVERT_ACTION_ID: &str = "convert";

/// Prefix marking a per-target convert *editor* action id (`convert:<target>`).
const CONVERT_EDITOR_PREFIX: &str = "convert:";

/// The types an item of `from` can convert into: issue, epic and user story,
/// minus the source's own type. Task is never a target (only a source) — a
/// task can still convert *out* into any of the three.
pub(super) fn convert_targets(from: ItemType) -> Vec<ItemType> {
    [ItemType::Issue, ItemType::Epic, ItemType::UserStory]
        .into_iter()
        .filter(|&t| t != from)
        .collect()
}

/// The editor action id for converting into `target` (`convert:userstory` etc.).
pub(super) fn convert_editor_action_id(target: ItemType) -> String {
    format!("{CONVERT_EDITOR_PREFIX}{}", target.as_str())
}

/// Parse a per-target convert editor action id back into its target type.
/// Returns `None` for the bare menu id `"convert"` or any non-convert id.
pub(super) fn parse_convert_target(action_id: &str) -> Option<ItemType> {
    let rest = action_id.strip_prefix(CONVERT_EDITOR_PREFIX)?;
    ItemType::parse(rest).ok()
}

/// Human-readable target label for the picker menu.
fn target_menu_label(target: ItemType) -> &'static str {
    match target {
        ItemType::UserStory => "user story",
        ItemType::Issue => "issue",
        ItemType::Epic => "epic",
        ItemType::Task => "task",
    }
}

/// Picker options for the convert menu: one per available target, whose value
/// is the target's editor action id (so the picked value drives `OpenEditor`).
pub(super) fn convert_target_options(from: ItemType) -> Vec<ActionOption> {
    convert_targets(from)
        .into_iter()
        .map(|t| ActionOption {
            label: format!("convert to {}", target_menu_label(t)),
            value: convert_editor_action_id(t),
        })
        .collect()
}

/// The convert *menu* action offered on an item of type `from` — a single
/// `Picker`, or `None` when there are no targets. The picked value is a
/// `convert:<target>` editor action id.
pub(super) fn convert_action(from: ItemType) -> Option<NodeAction> {
    if convert_targets(from).is_empty() {
        return None;
    }
    Some(NodeAction::new(CONVERT_ACTION_ID, "convert", InputSpec::Picker))
}

/// The per-target convert *editor* actions for an item of type `from`. These
/// are never key-bound or shown in the action bar — they exist only so
/// `actions()` membership passes when `OpenEditor` opens one (the edit session
/// validates the action id against `node.actions()` before calling `prepare`).
pub(super) fn convert_editor_actions(from: ItemType) -> Vec<NodeAction> {
    convert_targets(from)
        .into_iter()
        .map(|t| {
            NodeAction::new(
                convert_editor_action_id(t),
                format!("convert to {}", target_menu_label(t)),
                InputSpec::Editor,
            )
        })
        .collect()
}

impl TaigaItemNode {
    fn display_ref(&self) -> String {
        match &self.detail.project_slug {
            Some(slug) if !slug.is_empty() => format!("{slug}#{}", self.detail.r#ref),
            _ => format!("#{}", self.detail.r#ref),
        }
    }

    pub(super) async fn prepare_convert(&self, target: ItemType) -> Result<EditorPrep> {
        let (statuses, members, tags) =
            fetch_project_meta(&self.client, self.detail.project_id, target).await?;
        let tables = build_tables(&statuses, &members, &tags);

        // Best-effort counts + dropped-field note — never fail the prepare.
        let comment_count = fetch_comments(&self.client, self.detail.item_type, self.detail.id)
            .await
            .map(|c| c.len())
            .unwrap_or(0);
        let attach_count = list_attachments(
            &self.client,
            self.detail.item_type,
            self.detail.id,
            self.detail.project_id,
        )
        .await
        .map(|a| a.len())
        .unwrap_or(0);
        let dropped = self.dropped_fields_note(target).await;

        // Prefill with the target's *initial* status (first in project order,
        // which is what a fresh create would land on), rendered as a slug.
        let default_status_slug = statuses
            .first()
            .and_then(|s| tables.statuses.slug_for(&s.name))
            .unwrap_or("")
            .to_string();

        let template = render_convert_template(
            &self.detail,
            target,
            &tables,
            &default_status_slug,
            &dropped,
            comment_count,
            attach_count,
        );
        Ok(EditorPrep {
            // Create/promote has no optimistic-lock token, like `clone`.
            template,
            version: String::new(),
            suffix: ".md".into(),
        })
    }

    pub(super) async fn execute_convert(
        &mut self,
        target: ItemType,
        text: &str,
    ) -> Result<ActionOutcome> {
        let editable_fields = edit_full_fields();
        let (statuses, members, tags) =
            fetch_project_meta(&self.client, self.detail.project_id, target).await?;
        let tables = build_tables(&statuses, &members, &tags);

        // Parse → validate → resolve slugs against the TARGET type's tables.
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
            return Ok(ActionOutcome::Reopen {
                content: render_with_errors(text, &errors),
                new_version: None,
            });
        }

        // Extract target fields from the resolved buffer.
        let subject = parsed.editable.get("subject").cloned().unwrap_or_default();
        let status_name = parsed.editable.get("status").cloned().unwrap_or_default();
        let status_id = statuses.iter().find(|s| s.name == status_name).map(|s| s.id);
        let assignee_usernames = split_csv(parsed.editable.get("assignee"));
        let mut assigned_users: Vec<u64> = Vec::new();
        for name in &assignee_usernames {
            if let Some(m) = members.iter().find(|m| m.username == *name) {
                assigned_users.push(m.id);
            }
        }
        let assigned_to = assigned_users.first().copied();
        let tag_list = split_csv(parsed.editable.get("tags"));
        let description = template::resolve_user_mentions(parsed.body.trim(), &tables.users);

        // 1. Create the target item. Both directions go through the normal
        //    create endpoint — Taiga's native `promote_to_us` is missing on
        //    some deployments (plain-HTML 404). Provenance is recorded in the
        //    editable conversion note in the body, not via a
        //    `generated_from_issue` link, which would couple the new item to
        //    the source's deletion.
        let create_fields = CreateFields {
            project_id: self.detail.project_id,
            subject: subject.clone(),
            description: description.clone(),
            tags: tag_list.clone(),
            user_story_id: None,
            assigned_to,
            assigned_users: assigned_users.clone(),
        };
        let new_id = match create_item(&self.client, target, create_fields).await {
            Ok(created) => created.id,
            Err(e) => {
                return Ok(reopen_banner(
                    text,
                    format!("create {} failed: {e}", target.as_str()),
                ));
            }
        };

        // Everything past the create is best-effort: the target already exists,
        // so we must still return `Navigate` — which reloads the originating
        // pane so the new item shows up and the (deleted) source drops out.
        // Aborting here with `?` would strand the new item off-screen behind a
        // stale list. So reconcile/migrate/delete only record warnings.
        let mut warnings: Vec<String> = Vec::new();

        // 2. Re-fetch the fresh target for its version + ref. Comment
        //    migration needs the version, so a fetch failure skips migration
        //    (and keeps the source) rather than aborting.
        let dst_detail = match super::fetch_detail(&self.client, target, new_id).await {
            Ok(d) => Some(d),
            Err(e) => {
                warnings.push(format!("could not load new {}: {e}", target.as_str()));
                None
            }
        };

        // 3. Reconcile the status (create omits it → project default), then
        //    migrate comments (chronological) + attachments (dedup by
        //    name+size). All best-effort; the first migration failure keeps
        //    the source.
        let migration_ok = if let Some(dst) = &dst_detail {
            let mut current_version = dst.version;
            let fields = EditFields {
                status_id,
                ..EditFields::default()
            };
            if !fields.is_empty() {
                match self
                    .patch_target(target, new_id, current_version, &fields)
                    .await
                {
                    Ok(v) => current_version = v,
                    Err(e) => warnings.push(format!("status not set: {e}")),
                }
            }
            let migrated_comments = self
                .migrate_comments(target, new_id, &mut current_version, &mut warnings)
                .await;
            let migrated_attachments = if migrated_comments {
                self.migrate_attachments(target, new_id, &mut warnings).await
            } else {
                false
            };
            migrated_comments && migrated_attachments
        } else {
            false
        };

        // 4. Delete the source only when everything migrated.
        if migration_ok {
            if let Err(e) = delete_item(&self.client, self.detail.item_type, self.detail.id).await {
                warnings.push(format!("source not deleted (delete manually): {e}"));
            }
        } else {
            warnings.push(format!(
                "source {} {} kept — migration incomplete",
                self.detail.item_type.as_str(),
                self.display_ref(),
            ));
        }

        let dst_ref = match &dst_detail {
            Some(d) => match &d.project_slug {
                Some(slug) if !slug.is_empty() => format!("{slug}#{}", d.r#ref),
                _ => format!("#{}", d.r#ref),
            },
            None => format!("id {new_id}"),
        };
        let mut message = format!(
            "Converted {} {} → {} {dst_ref}",
            self.detail.item_type.as_str(),
            self.display_ref(),
            target.as_str(),
        );
        if !warnings.is_empty() {
            message.push_str(&format!(" — WARNING: {}", warnings.join("; ")));
        }
        Ok(ActionOutcome::Navigate {
            node_id: format!("{}:{}", target.as_str(), new_id),
            node_type: node_type_for(target).clone(),
            message: Some(message),
        })
    }

    /// PATCH the freshly-created target, retrying once on the (unlikely)
    /// version conflict against a just-created item.
    async fn patch_target(
        &self,
        target: ItemType,
        new_id: u64,
        version: u64,
        fields: &EditFields,
    ) -> Result<u64> {
        let patch = ItemPatch {
            item_type: target,
            item_id: new_id,
            version,
            fields,
            comment: None,
        };
        match patch_item(&self.client, patch).await {
            Ok(PatchOutcome::Updated { new_version }) => Ok(new_version),
            Ok(PatchOutcome::VersionConflict { .. }) => {
                let fresh = super::fetch_detail(&self.client, target, new_id).await?;
                let retry = ItemPatch {
                    item_type: target,
                    item_id: new_id,
                    version: fresh.version,
                    fields,
                    comment: None,
                };
                match patch_item(&self.client, retry).await {
                    Ok(PatchOutcome::Updated { new_version }) => Ok(new_version),
                    Ok(PatchOutcome::VersionConflict { server_message }) => Err(
                        ContentError::Other(format!("reconcile conflict: {server_message}").into()),
                    ),
                    Err(e) => Err(ContentError::Other(e.into())),
                }
            }
            Err(e) => Err(ContentError::Other(e.into())),
        }
    }

    /// Re-post the source's comments on the target in chronological order,
    /// each prefixed with its original author/date. Returns `false` (and
    /// records a warning) on the first failure so the caller aborts before
    /// deleting the source.
    async fn migrate_comments(
        &self,
        target: ItemType,
        new_id: u64,
        version: &mut u64,
        warnings: &mut Vec<String>,
    ) -> bool {
        let mut comments =
            match fetch_comments(&self.client, self.detail.item_type, self.detail.id).await {
                Ok(c) => c,
                Err(e) => {
                    warnings.push(format!("read source comments: {e}"));
                    return false;
                }
            };
        comments.sort_by(|a, b| a.created.cmp(&b.created));
        for c in &comments {
            let body = format!("> _Originally by {} on {}_\n\n{}", c.author, c.created, c.body);
            match add_comment(&self.client, target, new_id, *version, &body).await {
                Ok(PatchOutcome::Updated { new_version }) => *version = new_version,
                Ok(PatchOutcome::VersionConflict { .. }) => {
                    // Refresh the version once and retry.
                    let Ok(fresh) = super::fetch_detail(&self.client, target, new_id).await else {
                        warnings.push("comment migration: version refresh failed".into());
                        return false;
                    };
                    match add_comment(&self.client, target, new_id, fresh.version, &body).await {
                        Ok(PatchOutcome::Updated { new_version }) => *version = new_version,
                        _ => {
                            warnings.push("comment migration: version conflict".into());
                            return false;
                        }
                    }
                }
                Err(e) => {
                    warnings.push(format!("comment migration: {e}"));
                    return false;
                }
            }
        }
        true
    }

    /// Copy the source's attachments to the target, skipping any already
    /// present (matched by name + size). Returns `false` on any failure.
    async fn migrate_attachments(
        &self,
        target: ItemType,
        new_id: u64,
        warnings: &mut Vec<String>,
    ) -> bool {
        let existing =
            list_attachments(&self.client, target, new_id, self.detail.project_id)
                .await
                .unwrap_or_default();
        let existing_keys: HashSet<(String, u64)> =
            existing.iter().map(|a| (a.name.clone(), a.size)).collect();
        let src = match list_attachments(
            &self.client,
            self.detail.item_type,
            self.detail.id,
            self.detail.project_id,
        )
        .await
        {
            Ok(a) => a,
            Err(e) => {
                warnings.push(format!("read source attachments: {e}"));
                return false;
            }
        };
        let mut ok = true;
        for a in &src {
            if existing_keys.contains(&(a.name.clone(), a.size)) {
                continue;
            }
            match download_attachment(&self.client, &a.url).await {
                Ok(bytes) => {
                    if let Err(e) = upload_attachment_bytes(
                        &self.client,
                        target,
                        new_id,
                        self.detail.project_id,
                        &a.name,
                        bytes,
                    )
                    .await
                    {
                        warnings.push(format!("upload {}: {e}", a.name));
                        ok = false;
                    }
                }
                Err(e) => {
                    warnings.push(format!("download {}: {e}", a.name));
                    ok = false;
                }
            }
        }
        ok
    }

    /// Assemble the editable conversion note listing source fields with no
    /// equivalent on the target type. Best-effort: any failure yields an
    /// empty string so the conversion never blocks on it.
    async fn dropped_fields_note(&self, target: ItemType) -> String {
        let raw =
            match client::fetch_raw_detail(&self.client, self.detail.item_type, self.detail.id)
                .await
            {
                Ok(v) => v,
                Err(_) => return String::new(),
            };
        let mut parts: Vec<String> = Vec::new();
        match self.detail.item_type {
            ItemType::Issue => {
                for (field, endpoint) in [
                    ("type", "issue-types"),
                    ("severity", "severities"),
                    ("priority", "priorities"),
                ] {
                    if let Some(id) = raw.get(field).and_then(|x| x.as_u64()) {
                        let map = client::fetch_id_name_map(
                            &self.client,
                            endpoint,
                            self.detail.project_id,
                        )
                        .await;
                        if let Some(name) = map.get(&id) {
                            parts.push(format!("{field}={name}"));
                        }
                    }
                }
            }
            ItemType::UserStory => {
                let has_points = raw
                    .get("points")
                    .and_then(|x| x.as_object())
                    .map(|m| m.values().any(|v| !v.is_null()))
                    .unwrap_or(false);
                if has_points {
                    parts.push("points".into());
                }
                if let Some(ms) = raw
                    .get("milestone_extra_info")
                    .and_then(|m| m.get("name"))
                    .and_then(|x| x.as_str())
                    .filter(|s| !s.is_empty())
                {
                    parts.push(format!("milestone={ms}"));
                } else if raw.get("milestone").and_then(|x| x.as_u64()).is_some() {
                    parts.push("milestone".into());
                }
                for flag in ["is_iocaine", "team_requirement", "client_requirement"] {
                    if raw.get(flag).and_then(|x| x.as_bool()).unwrap_or(false) {
                        parts.push(flag.to_string());
                    }
                }
            }
            _ => {}
        }
        if parts.is_empty() {
            return String::new();
        }
        format!(
            "> **Converted from {} {}** (dropped: {}). These fields have no {} equivalent.",
            self.detail.item_type.as_str(),
            self.display_ref(),
            parts.join(", "),
            target.as_str(),
        )
    }
}

fn split_csv(raw: Option<&String>) -> Vec<String> {
    raw.map(|s| {
        s.split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    })
    .unwrap_or_default()
}

fn reopen_banner(text: &str, message: String) -> ActionOutcome {
    let banner = FieldError { message };
    ActionOutcome::Reopen {
        content: render_with_errors(text, &[banner]),
        new_version: None,
    }
}

/// Build the conversion editor buffer: editable header (subject/status/
/// assignee/tags in TARGET slugs), a read-only infoblock, the editable
/// conversion note + original description as the body, and the target's
/// COMPLETIONS cache.
fn render_convert_template(
    detail: &super::ItemDetail,
    target: ItemType,
    tables: &TaigaSlugTables,
    default_status_slug: &str,
    dropped_note: &str,
    comment_count: usize,
    attach_count: usize,
) -> String {
    let display_ref = match &detail.project_slug {
        Some(slug) if !slug.is_empty() => format!("{slug}#{}", detail.r#ref),
        _ => format!("#{}", detail.r#ref),
    };

    let mut out = String::new();
    out.push_str(&format!(
        "# Convert {} {display_ref} → {} — review the fields, then save to create the {} and delete the source\n",
        detail.item_type.as_str(),
        target.as_str(),
        target.as_str(),
    ));

    // Editable header, in the target type's slug vocabulary.
    out.push_str(&format!("subject: {}\n", detail.subject));
    out.push_str(&format!("status: {default_status_slug}\n"));
    let assignee_slugs: Vec<String> = detail
        .assignee_usernames
        .iter()
        .filter_map(|u| tables.users.slug_for(u).map(String::from))
        .collect();
    out.push_str(&format!("assignee: {}\n", assignee_slugs.join(", ")));
    let tag_slugs: Vec<String> = detail
        .tags
        .iter()
        .filter_map(|t| tables.tags.slug_for(t).map(String::from))
        .collect();
    out.push_str(&format!("tags: {}\n", tag_slugs.join(", ")));

    out.push_str(EDITABLE_MARKER);
    out.push('\n');

    // Read-only infoblock — the `---`…`===` region is ignored on parse.
    out.push_str(&format!(
        "converting: {} {display_ref} → {}\n",
        detail.item_type.as_str(),
        target.as_str(),
    ));
    out.push_str(&format!(
        "note: {comment_count} comment(s) + {attach_count} attachment(s) will be migrated; source {} {display_ref} will be DELETED on save\n",
        detail.item_type.as_str(),
    ));

    out.push_str(BODY_MARKER);
    out.push_str("\n\n");

    // Body: editable conversion note (if any) then the original description.
    if !dropped_note.is_empty() {
        out.push_str(dropped_note);
        out.push_str("\n\n");
    }
    out.push_str(&detail.description);

    out.push_str(&render_cache_section(tables));
    out
}

/// Statuses/members/tags for the given (target) type — mirrors the private
/// helpers in `edit_full`/`clone` to keep this module self-contained.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{TaigaMember, TaigaStatus};

    fn tables() -> TaigaSlugTables {
        // Invented target-type metadata.
        let statuses = vec![
            TaigaStatus { id: 10, name: "New".into() },
            TaigaStatus { id: 11, name: "In progress".into() },
        ];
        let members = vec![TaigaMember {
            id: 5,
            username: "ghopper".into(),
            full_name: "Grace Hopper".into(),
        }];
        let tags = vec!["frontend".to_string()];
        build_tables(&statuses, &members, &tags)
    }

    fn detail() -> super::super::ItemDetail {
        super::super::ItemDetail {
            item_type: ItemType::Issue,
            id: 42,
            r#ref: 7,
            project_id: 3,
            project_slug: Some("demo-board".into()),
            subject: "Broken export".into(),
            description: "Steps to reproduce…".into(),
            status: "Open".into(),
            assignees: vec!["Grace Hopper".into()],
            assignee_usernames: vec!["ghopper".into()],
            tags: vec!["frontend".into()],
            modified: None,
            version: 1,
            parent_user_story_id: None,
            parent_user_story_subject: None,
        }
    }

    #[test]
    fn convert_targets_exclude_task_and_self() {
        use std::collections::HashSet;
        // Issue → {epic, user story} (never task, never itself).
        let issue: HashSet<_> = convert_targets(ItemType::Issue).into_iter().collect();
        assert_eq!(
            issue,
            HashSet::from([ItemType::Epic, ItemType::UserStory])
        );
        // Task is only ever a source, so it can convert into all three.
        let task: HashSet<_> = convert_targets(ItemType::Task).into_iter().collect();
        assert_eq!(
            task,
            HashSet::from([ItemType::Issue, ItemType::Epic, ItemType::UserStory])
        );
        // Task is never a target for any source.
        for from in [ItemType::Issue, ItemType::Epic, ItemType::UserStory, ItemType::Task] {
            assert!(
                !convert_targets(from).contains(&ItemType::Task),
                "task must never be a convert target (from {from:?})"
            );
        }
    }

    #[test]
    fn convert_action_is_a_picker() {
        // convert is now a single Picker action; the target is chosen from its
        // options, not baked into the id.
        let a = convert_action(ItemType::Issue).unwrap();
        assert_eq!(a.id, CONVERT_ACTION_ID);
        assert_eq!(a.label, "convert");
        assert!(matches!(a.input, InputSpec::Picker));
    }

    #[test]
    fn convert_target_options_map_to_editor_action_ids() {
        let opts = convert_target_options(ItemType::Issue);
        // One option per target, value = the target's editor action id.
        assert_eq!(opts.len(), 2);
        assert!(opts.iter().any(|o| o.value == "convert:epic"
            && o.label == "convert to epic"));
        assert!(opts.iter().any(|o| o.value == "convert:userstory"
            && o.label == "convert to user story"));
    }

    #[test]
    fn parse_convert_target_round_trips_and_rejects_bare_id() {
        for t in [ItemType::Issue, ItemType::Epic, ItemType::UserStory] {
            let id = convert_editor_action_id(t);
            assert_eq!(parse_convert_target(&id), Some(t));
        }
        // The bare menu id and unrelated ids are not editor targets.
        assert_eq!(parse_convert_target(CONVERT_ACTION_ID), None);
        assert_eq!(parse_convert_target("edit_full"), None);
        assert_eq!(parse_convert_target("convert:bogus"), None);
    }

    #[test]
    fn convert_editor_actions_cover_every_target() {
        let actions = convert_editor_actions(ItemType::Task);
        let ids: Vec<_> = actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&"convert:issue"));
        assert!(ids.contains(&"convert:epic"));
        assert!(ids.contains(&"convert:userstory"));
        // All are editor actions (they open a type-specific buffer).
        assert!(actions.iter().all(|a| matches!(a.input, InputSpec::Editor)));
    }

    #[test]
    fn split_csv_trims_and_drops_empties() {
        assert_eq!(
            split_csv(Some(&"a, b ,,c".to_string())),
            vec!["a".to_string(), "b".into(), "c".into()]
        );
        assert!(split_csv(None).is_empty());
    }

    /// The rendered buffer must parse back cleanly: the read-only infoblock is
    /// dropped, the editable header carries the target's default status slug
    /// (and resolves to a real status name), and the body keeps the note.
    #[test]
    fn convert_template_round_trips_through_parse() {
        let tables = tables();
        let note = "> **Converted from issue demo-board#7** (dropped: severity=Critical). \
                    These fields have no userstory equivalent.";
        // `prepare_convert` computes the initial status slug from the first
        // project-ordered status; mirror that here.
        let statuses = [
            TaigaStatus { id: 10, name: "New".into() },
            TaigaStatus { id: 11, name: "In progress".into() },
        ];
        let default_slug = tables.statuses.slug_for(&statuses[0].name).unwrap();
        let buf = render_convert_template(
            &detail(),
            ItemType::UserStory,
            &tables,
            default_slug,
            note,
            2,
            1,
        );

        // Header carries the target's initial status slug.
        assert!(buf.contains("status: ss-new"));
        // Read-only note advertises the destructive action + counts.
        assert!(buf.contains("2 comment(s) + 1 attachment(s) will be migrated"));
        assert!(buf.contains("will be DELETED on save"));
        // Body keeps the editable conversion note + original description.
        assert!(buf.contains("dropped: severity=Critical"));
        assert!(buf.contains("Steps to reproduce"));

        let mut parsed = template::parse_3b(&buf).expect("parses");
        // Read-only `converting:`/`note:` lines must not leak into editable.
        assert!(!parsed.editable.contains_key("converting"));
        assert!(!parsed.editable.contains_key("note"));
        assert_eq!(parsed.editable.get("subject").unwrap(), "Broken export");
        assert_eq!(parsed.editable.get("assignee").unwrap(), "uu-grace-hopper");

        // Slugs resolve against the target tables.
        let mut errs = validate_3b(&parsed, &edit_full_fields());
        resolve_slugs_inplace(&mut parsed, &tables, &mut errs);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
        assert_eq!(parsed.editable.get("status").unwrap(), "New");
        assert_eq!(parsed.editable.get("assignee").unwrap(), "ghopper");
    }
}
