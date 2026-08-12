//! `clone` action — create a new item pre-filled from another item's
//! fields. The editor opens with the source's subject/description/tags
//! carried over; assignee starts empty; the item type and (for tasks)
//! the parent user story are read-only so the create call hits the
//! right endpoint with the right parent.
//!
//! Status is intentionally omitted from the payload — Taiga assigns the
//! project's default for the type, mirroring how a user-initiated
//! "create new" would behave in the web UI.

use not_yet_done_content::*;

use crate::client::{CreateFields, CreatedItem, ItemType, TaigaClient, create_item};

use super::TaigaItemNode;
use super::edit_full::build_tables;
use super::node_type_for;
use super::slugs::TaigaSlugTables;
use super::template::{
    self, BODY_MARKER, CACHE_MARKER, EDITABLE_MARKER, FieldError, Parsed3b, render_with_errors,
    resolve_slugs_inplace, validate_3b,
};

fn clone_fields() -> Vec<String> {
    vec!["subject".into(), "assignee".into(), "tags".into()]
}

impl TaigaItemNode {
    pub(super) async fn prepare_clone(&self) -> Result<EditorPrep> {
        let (statuses, members, tags) =
            fetch_project_meta(&self.client, self.detail.project_id, self.detail.item_type).await?;
        let tables = build_tables(&statuses, &members, &tags);
        let template = render_clone_template(&self.detail, &tables);
        Ok(EditorPrep {
            template,
            // No version semantics — create has no optimistic-lock token.
            version: String::new(),
            suffix: ".md".into(),
            file_path: None,
        })
    }

    pub(super) async fn execute_clone(&mut self, text: &str) -> Result<ActionOutcome> {
        let editable_fields = clone_fields();
        let (statuses, members, tags) =
            fetch_project_meta(&self.client, self.detail.project_id, self.detail.item_type).await?;
        let tables = build_tables(&statuses, &members, &tags);

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

        let (subject, assignee_usernames, tag_list) = extract_clone_inputs(&parsed);

        let mut assigned_users: Vec<u64> = Vec::with_capacity(assignee_usernames.len());
        for name in &assignee_usernames {
            if let Some(m) = members.iter().find(|m| m.username == *name) {
                assigned_users.push(m.id);
            }
        }
        let assigned_to = assigned_users.first().copied();

        let user_story_id = if matches!(self.detail.item_type, ItemType::Task) {
            self.detail.parent_user_story_id
        } else {
            None
        };

        let fields = CreateFields {
            project_id: self.detail.project_id,
            subject,
            description: parsed.body.trim().to_string(),
            tags: tag_list,
            user_story_id,
            assigned_to,
            assigned_users,
        };

        let CreatedItem {
            id: new_id,
            r#ref: new_ref,
        } = match create_item(&self.client, self.detail.item_type, fields).await {
            Ok(created) => created,
            Err(e) => {
                let banner = FieldError {
                    message: format!("create failed: {e}"),
                };
                return Ok(ActionOutcome::Reopen {
                    content: render_with_errors(text, &[banner]),
                    new_version: None,
                });
            }
        };

        let display_ref = match &self.detail.project_slug {
            Some(slug) if !slug.is_empty() => format!("{slug}#{new_ref}"),
            _ => format!("#{new_ref}"),
        };
        // `clone` is an `edit`-shaped action (it runs on the source item and
        // opens a pre-filled editor), yet it *creates* a sibling. Report that
        // as `Navigate` so the TUI reloads the list and the new item shows up
        // immediately — `Done` would only patch the source row.
        Ok(ActionOutcome::Navigate {
            node_id: format!("{}:{}", self.detail.item_type.as_str(), new_id),
            node_type: node_type_for(self.detail.item_type).clone(),
            message: Some(format!(
                "Created {} {display_ref}",
                self.detail.item_type.as_str(),
            )),
        })
    }
}

/// Pull just the inputs the clone POST cares about out of a parsed buffer.
/// `validate_3b` has already guaranteed `subject` is present + non-empty.
fn extract_clone_inputs(parsed: &Parsed3b) -> (String, Vec<String>, Vec<String>) {
    let subject = parsed.editable.get("subject").cloned().unwrap_or_default();
    let assignees: Vec<String> = parsed
        .editable
        .get("assignee")
        .map(|raw| {
            raw.split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();
    let tags = parsed
        .editable
        .get("tags")
        .map(|raw| {
            raw.split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    (subject, assignees, tags)
}

/// Build the clone-template buffer. Editable section starts with the
/// source's subject, blank assignee, and the source's tag slugs. The
/// read-only section advertises `type` (and `user_story` for tasks).
/// The completions block lists users + tags (no statuses — clone
/// always uses the project's default status).
fn render_clone_template(detail: &super::ItemDetail, tables: &TaigaSlugTables) -> String {
    let mut out = String::new();
    let display_ref = match &detail.project_slug {
        Some(slug) if !slug.is_empty() => format!("{slug}#{}", detail.r#ref),
        _ => format!("#{}", detail.r#ref),
    };
    out.push_str(&format!(
        "# Clone of {display_ref} — edit subject/assignee/tags below and save to create a new {}\n",
        detail.item_type.as_str(),
    ));

    out.push_str(&format!("subject: {}\n", detail.subject));
    out.push_str("assignee: \n");
    let tag_slugs: Vec<String> = detail
        .tags
        .iter()
        .filter_map(|t| tables.tags.slug_for(t).map(String::from))
        .collect();
    out.push_str(&format!("tags: {}\n", tag_slugs.join(", ")));

    out.push_str(EDITABLE_MARKER);
    out.push('\n');

    out.push_str(&format!("type: {}\n", detail.item_type.as_str()));
    if matches!(detail.item_type, ItemType::Task) {
        if let Some(us_id) = detail.parent_user_story_id {
            let subject = detail.parent_user_story_subject.as_deref().unwrap_or("");
            if subject.is_empty() {
                out.push_str(&format!("user_story: #{us_id}\n"));
            } else {
                out.push_str(&format!("user_story: #{us_id} — {subject}\n"));
            }
        }
    }

    out.push_str(BODY_MARKER);
    out.push_str("\n\n");
    out.push_str(&detail.description);

    out.push_str(&render_clone_cache_section(tables));
    out
}

fn render_clone_cache_section(tables: &TaigaSlugTables) -> String {
    if tables.users.is_empty() && tables.tags.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push('\n');
    out.push_str(CACHE_MARKER);
    out.push('\n');
    if !tables.users.is_empty() {
        out.push_str("# users: ");
        out.push_str(&tables.users.slugs().join(", "));
        out.push('\n');
    }
    if !tables.tags.is_empty() {
        out.push_str("# tags: ");
        out.push_str(&tables.tags.slugs().join(", "));
        out.push('\n');
    }
    out
}

/// Mirror of `edit_full::fetch_project_meta` — kept private to keep the
/// clone module self-contained. Returns statuses/members/tags lists.
async fn fetch_project_meta(
    client: &TaigaClient,
    project_id: u64,
    item_type: ItemType,
) -> Result<(
    Vec<crate::client::TaigaStatus>,
    Vec<crate::client::TaigaMember>,
    Vec<String>,
)> {
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
