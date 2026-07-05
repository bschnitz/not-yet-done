//! Per-item node (task / issue / epic / userstory). Detail fetch happens
//! at construction; navigation to comments uses the `taiga:comment` child
//! type.

use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::*;

use super::types::{attachment_type, comment_type, node_type_for};
use crate::client::{
    ItemType, TaigaAttachment, TaigaClient, fetch_comments, list_attachments, toggle_watch,
    upload_attachment,
};

mod clone;
mod convert;
mod edit_full;
mod edit_with_comments;
mod slugs;
mod template;

/// Detail fetched eagerly for the body/preview *and* for edit flows. Most
/// fields surface in the editable template; `version` is Taiga's optimistic
/// lock and round-trips through PATCH.
pub(super) struct ItemDetail {
    pub(super) item_type: ItemType,
    pub(super) id: u64,
    pub(super) r#ref: u64,
    pub(super) project_id: u64,
    pub(super) project_slug: Option<String>,
    pub(super) subject: String,
    pub(super) description: String,
    pub(super) status: String,
    /// Display names of all assignees, current-user-first then alphabetical.
    pub(super) assignees: Vec<String>,
    /// Canonical usernames parallel to `assignees`.
    pub(super) assignee_usernames: Vec<String>,
    pub(super) tags: Vec<String>,
    pub(super) modified: Option<String>,
    pub(super) version: u64,
    /// Tasks only: parent user story id from `user_story` in the detail
    /// payload. `None` for other item types or detached tasks.
    pub(super) parent_user_story_id: Option<u64>,
    /// Display label for the parent user story (subject from
    /// `user_story_extra_info`). May be empty even when the id is set.
    pub(super) parent_user_story_subject: Option<String>,
}

/// Actions available on an item. `item_type` gates the type-specific
/// conversion action (issue ↔ user story); pass `None` for the generic
/// `taiga:item` node type, which has no concrete type to convert.
pub(super) fn item_actions(item_type: Option<ItemType>) -> Vec<NodeAction> {
    let mut actions = vec![
        NodeAction::new("edit_full", "edit", InputSpec::Editor),
        NodeAction::new("edit_with_comments", "edit + comments", InputSpec::Editor),
        NodeAction::new("toggle_watch", "toggle watch", InputSpec::None),
        NodeAction::new("open_in_browser", "open in browser", InputSpec::None),
        NodeAction::new(
            "upload_attachment",
            "upload attachment",
            InputSpec::FilePicker { multi: true },
        ),
        NodeAction::new("clone", "clone", InputSpec::Editor),
    ];
    if let Some(item_type) = item_type {
        // The convert menu (Picker) plus its hidden per-target editor actions.
        // The editor actions are never key-bound; they exist so the edit
        // session's `actions()` membership check passes when `OpenEditor`
        // opens `convert:<target>` after the user picks from the menu.
        if let Some(action) = convert::convert_action(item_type) {
            actions.push(action);
            actions.extend(convert::convert_editor_actions(item_type));
        }
    }
    actions
}

pub(super) struct TaigaItemNode {
    pub(super) client: Arc<TaigaClient>,
    pub(super) detail: ItemDetail,
    pub(super) composite_id: String,
}

impl TaigaItemNode {
    pub(super) async fn new(
        client: Arc<TaigaClient>,
        item_type: ItemType,
        id: u64,
    ) -> Result<Self> {
        let detail = fetch_detail(&client, item_type, id).await?;
        let composite_id = format!("{}:{}", item_type.as_str(), id);
        Ok(Self { client, detail, composite_id })
    }
}

/// Look up display names + usernames for every assignee id and order
/// them current-user-first, alphabetical otherwise. Members missing from
/// the project's roster fall back to `user-<id>`.
pub(super) async fn resolve_detail_assignees(
    client: &TaigaClient,
    project_id: u64,
    assignee_ids: &[u64],
) -> (Vec<String>, Vec<String>) {
    if assignee_ids.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let members = client.ensure_members(project_id).await.unwrap_or_default();
    let current_user_id = client.current_user_id().await.unwrap_or(0);
    let mut named: Vec<(u64, String, String)> = assignee_ids
        .iter()
        .map(|id| {
            let m = members.iter().find(|m| m.id == *id);
            let display = m
                .map(|m| {
                    if m.full_name.is_empty() {
                        m.username.clone()
                    } else {
                        m.full_name.clone()
                    }
                })
                .unwrap_or_else(|| format!("user-{id}"));
            let username = m
                .map(|m| m.username.clone())
                .unwrap_or_else(|| format!("user-{id}"));
            (*id, display, username)
        })
        .collect();
    named.sort_by(|a, b| {
        let a_cur = a.0 == current_user_id;
        let b_cur = b.0 == current_user_id;
        match (a_cur, b_cur) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.1.to_lowercase().cmp(&b.1.to_lowercase()),
        }
    });
    let assignees: Vec<String> = named.iter().map(|(_, d, _)| d.clone()).collect();
    let usernames: Vec<String> = named.into_iter().map(|(_, _, u)| u).collect();
    (assignees, usernames)
}

pub(super) async fn fetch_detail(
    client: &TaigaClient,
    item_type: ItemType,
    id: u64,
) -> Result<ItemDetail> {
    let endpoint = match item_type {
        ItemType::Task => "tasks",
        ItemType::Issue => "issues",
        ItemType::Epic => "epics",
        ItemType::UserStory => "userstories",
    };
    let url = format!("{}/api/v1/{endpoint}/{id}", client.base_url);
    http_log::log_request("GET", &url);
    let resp = client
        .http
        .get(&url)
        .headers(
            client
                .auth_headers()
                .map_err(|e| ContentError::Other(e.into()))?,
        )
        .send()
        .await
        .map_err(|e| ContentError::Other(http_log::network_error("GET", &url, e).into()))?;
    let resp = http_log::check_status("GET", &url, resp)
        .await
        .map_err(|e| ContentError::Other(e.into()))?;
    let raw: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ContentError::Other(format!("detail parse: {e}").into()))?;

    let s = |key: &str| {
        raw.get(key)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let u = |key: &str| raw.get(key).and_then(|x| x.as_u64()).unwrap_or(0);

    let project_slug = raw
        .get("project_extra_info")
        .and_then(|e| e.get("slug"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let project_id = u("project");
    let mut assignee_ids: Vec<u64> = raw
        .get("assigned_users")
        .and_then(|x| x.as_array())
        .map(|arr| arr.iter().filter_map(|x| x.as_u64()).collect())
        .unwrap_or_default();
    if assignee_ids.is_empty() {
        if let Some(id) = raw.get("assigned_to").and_then(|x| x.as_u64()) {
            assignee_ids.push(id);
        }
    }
    let (assignees, assignee_usernames) =
        resolve_detail_assignees(client, project_id, &assignee_ids).await;
    let status = raw
        .get("status_extra_info")
        .and_then(|e| e.get("name"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let tags: Vec<String> = match raw.get("tags") {
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|t| match t {
                serde_json::Value::Array(pair) => pair
                    .first()
                    .and_then(|x| x.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string()),
                serde_json::Value::String(s) => {
                    if s.is_empty() { None } else { Some(s.clone()) }
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };

    let parent_user_story_id = raw
        .get("user_story")
        .and_then(|x| x.as_u64());
    let parent_user_story_subject = raw
        .get("user_story_extra_info")
        .and_then(|e| e.get("subject"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());

    Ok(ItemDetail {
        item_type,
        id,
        r#ref: u("ref"),
        project_id,
        project_slug,
        subject: s("subject"),
        description: s("description"),
        status,
        assignees,
        assignee_usernames,
        tags,
        modified: raw
            .get("modified_date")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        version: u("version"),
        parent_user_story_id,
        parent_user_story_subject,
    })
}

/// The **list-row** projection of an item detail — mirrors the field keys
/// `item_summary_to_node_summary` emits (`ref, type, status, assignee,
/// modified, subject`) so the post-edit row patch refreshes the same columns
/// the list rendered. `attachments` is intentionally omitted: the detail fetch
/// carries no attachment count, so the patch keeps the row's last-known value.
fn item_detail_to_row_summary(d: &ItemDetail, composite_id: &str) -> NodeSummary {
    let display_ref = match &d.project_slug {
        Some(slug) if !slug.is_empty() => format!("{slug}#{}", d.r#ref),
        _ => format!("#{}", d.r#ref),
    };
    let f = |key: &str, value: String, label: &str| MetadataField {
        key: key.into(),
        value,
        display_label: label.into(),
        editable: false,
        allowed_values: None,
    };
    NodeSummary {
        id: composite_id.to_string(),
        label: d.subject.clone(),
        node_type: node_type_for(d.item_type).clone(),
        metadata: Metadata {
            fields: vec![
                f("ref", display_ref, "Ref"),
                f("type", d.item_type.as_str().to_string(), "Type"),
                f("status", d.status.clone(), "Status"),
                f("assignee", d.assignees.join(", "), "Assignee"),
                f("modified", d.modified.clone().unwrap_or_default(), "Modified"),
                f("subject", d.subject.clone(), "Subject"),
            ],
        },
        has_children: None,
    }
}

#[async_trait]
impl Node for TaigaItemNode {
    fn id(&self) -> &str {
        &self.composite_id
    }

    fn label(&self) -> &str {
        &self.detail.subject
    }

    fn node_type(&self) -> &NodeType {
        node_type_for(self.detail.item_type)
    }

    fn metadata(&self) -> &Metadata {
        // The list view supplies a freshly-built Metadata via NodeSummary;
        // direct-by-id navigation rarely renders a metadata table for now,
        // so we keep this lazy.
        static EMPTY: Metadata = Metadata { fields: vec![] };
        &EMPTY
    }

    fn row_summary(&self) -> NodeSummary {
        // `metadata()` is intentionally empty for by-id nodes; the post-edit
        // row patch needs the list-row shape instead. Delegated to a free
        // function so it stays testable without constructing a TaigaClient.
        item_detail_to_row_summary(&self.detail, &self.composite_id)
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![comment_type().clone(), attachment_type().clone()]
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        match params.node_type.type_id.as_str() {
            "taiga:comment" => self.list_comments().await,
            "taiga:attachment" => self.list_attachments().await,
            other => Err(ContentError::NotSupported(format!(
                "unsupported child type: {other}"
            ))),
        }
    }

    fn content(&self) -> Option<&dyn Content> {
        Some(self)
    }

    fn actions(&self) -> Vec<NodeAction> {
        item_actions(Some(self.detail.item_type))
    }

    async fn picker_options(&self, action_id: &str) -> Result<Vec<ActionOption>> {
        match action_id {
            convert::CONVERT_ACTION_ID => {
                Ok(convert::convert_target_options(self.detail.item_type))
            }
            other => Err(ContentError::NotSupported(format!(
                "picker_options: unknown action {other}"
            ))),
        }
    }

    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        // A `convert:<target>` id routes to the target-specific convert editor.
        if let Some(target) = convert::parse_convert_target(action_id) {
            return self.prepare_convert(target).await;
        }
        match action_id {
            "edit_full" => self.prepare_edit_full().await,
            "edit_with_comments" => self.prepare_edit_with_comments().await,
            "clone" => self.prepare_clone().await,
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
            ("edit_full", ActionInput::Edited { text, original, version }) => {
                self.execute_edit_full(&text, &original, &version).await
            }
            ("edit_with_comments", ActionInput::Edited { text, original, version }) => {
                self.execute_edit_with_comments(&text, &original, &version).await
            }
            ("toggle_watch", ActionInput::None) => {
                let now_watching =
                    toggle_watch(&self.client, self.detail.item_type, self.detail.id)
                        .await
                        .map_err(|e| ContentError::Other(e.into()))?;
                let label = if now_watching { "watching" } else { "no longer watching" };
                let display_ref = match &self.detail.project_slug {
                    Some(slug) if !slug.is_empty() => {
                        format!("{slug}#{}", self.detail.r#ref)
                    }
                    _ => format!("#{}", self.detail.r#ref),
                };
                Ok(ActionOutcome::Done {
                    message: Some(format!("{display_ref}: {label}")),
                })
            }
            ("open_in_browser", ActionInput::None) => self.open_in_browser().await,
            ("upload_attachment", ActionInput::Files(paths)) => {
                self.execute_upload_attachment(paths).await
            }
            ("clone", ActionInput::Edited { text, .. }) => {
                self.execute_clone(&text).await
            }
            (convert::CONVERT_ACTION_ID, ActionInput::Picked(value)) => {
                // Menu step: the picked value is a `convert:<target>` editor
                // action id. Hand it back so the frontend opens that editor on
                // this same node (prepare → edit → execute reuse the plumbing).
                if convert::parse_convert_target(&value).is_some() {
                    Ok(ActionOutcome::OpenEditor { action_id: value })
                } else {
                    Err(ContentError::NotSupported(format!(
                        "convert: unknown target selection {value}"
                    )))
                }
            }
            (id, ActionInput::Edited { text, .. })
                if convert::parse_convert_target(id).is_some() =>
            {
                let target = convert::parse_convert_target(id)
                    .expect("guard already checked this is a convert target");
                self.execute_convert(target, &text).await
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "execute: unknown action {other}"
            ))),
        }
    }
}

impl TaigaItemNode {
    /// Build the human-facing Taiga web URL and hand it to `xdg-open`.
    /// Requires `project_slug` from the detail payload — Taiga's UI
    /// routes use slugs, not project IDs.
    async fn open_in_browser(&self) -> Result<ActionOutcome> {
        let slug = self
            .detail
            .project_slug
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ContentError::Other(
                    "open_in_browser: project slug missing on item detail".into(),
                )
            })?;
        let base = self.client.base_url.trim_end_matches('/');
        let url = format!(
            "{base}/project/{slug}/{seg}/{ref_num}",
            seg = self.detail.item_type.web_segment(),
            ref_num = self.detail.r#ref,
        );

        std::process::Command::new("xdg-open")
            .arg(&url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| ContentError::Other(format!("spawn xdg-open: {e}").into()))?;

        Ok(ActionOutcome::Done { message: Some(format!("opened {url}")) })
    }

    async fn execute_upload_attachment(
        &self,
        paths: Vec<std::path::PathBuf>,
    ) -> Result<ActionOutcome> {
        if paths.is_empty() {
            return Ok(ActionOutcome::NoChanges);
        }
        let mut uploaded = 0usize;
        let mut failures: Vec<String> = Vec::new();
        for path in &paths {
            match upload_attachment(
                &self.client,
                self.detail.item_type,
                self.detail.id,
                self.detail.project_id,
                path,
            )
            .await
            {
                Ok(_) => uploaded += 1,
                Err(e) => failures.push(format!("{}: {e}", path.display())),
            }
        }
        if !failures.is_empty() {
            return Err(ContentError::Other(
                format!(
                    "uploaded {}/{}; failures: {}",
                    uploaded,
                    paths.len(),
                    failures.join("; "),
                )
                .into(),
            ));
        }
        let noun = if uploaded == 1 { "attachment" } else { "attachments" };
        Ok(ActionOutcome::Done {
            message: Some(format!("uploaded {uploaded} {noun}")),
        })
    }

    async fn list_comments(&self) -> Result<ListResult> {
        let comments = fetch_comments(&self.client, self.detail.item_type, self.detail.id)
            .await
            .map_err(|e| ContentError::Other(e.into()))?;
        let items = comments
            .into_iter()
            .map(|c| NodeSummary {
                id: format!("{}/comment/{}", self.composite_id, c.id),
                label: c.body.clone(),
                node_type: comment_type().clone(),
                metadata: Metadata {
                    fields: vec![
                        MetadataField {
                            key: "author".into(),
                            value: c.author,
                            display_label: "Author".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "created".into(),
                            value: c.created,
                            display_label: "Created".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "body".into(),
                            value: c.body,
                            display_label: "Body".into(),
                            editable: false,
                            allowed_values: None,
                        },
                    ],
                },
                has_children: None,
            })
            .collect();
        Ok(ListResult {
            items,
            applied_sort: Vec::new(),
            page: None,
            batch_download_available: false,
            downloaded: vec![],
        })
    }

    async fn list_attachments(&self) -> Result<ListResult> {
        let attachments = list_attachments(
            &self.client,
            self.detail.item_type,
            self.detail.id,
            self.detail.project_id,
        )
        .await
        .map_err(|e| ContentError::Other(e.into()))?;
        let items = attachments
            .into_iter()
            .map(|a| attachment_summary(&self.composite_id, &a))
            .collect();
        Ok(ListResult {
            items,
            applied_sort: Vec::new(),
            page: None,
            batch_download_available: false,
            downloaded: vec![],
        })
    }

    /// Re-fetch attachments and find the requested one by id. Used by
    /// `TaigaAdapter::get_by_id` for `task:1/attachment/42` paths.
    pub(super) async fn find_attachment(
        &self,
        attachment_id: u64,
    ) -> Result<TaigaAttachment> {
        let attachments = list_attachments(
            &self.client,
            self.detail.item_type,
            self.detail.id,
            self.detail.project_id,
        )
        .await
        .map_err(|e| ContentError::Other(e.into()))?;
        attachments
            .into_iter()
            .find(|a| a.id == attachment_id)
            .ok_or_else(|| ContentError::NotFound(format!("attachment {attachment_id}")))
    }
}

fn attachment_summary(parent_id: &str, a: &TaigaAttachment) -> NodeSummary {
    NodeSummary {
        id: format!("{parent_id}/attachment/{}", a.id),
        label: a.name.clone(),
        node_type: attachment_type().clone(),
        metadata: Metadata {
            fields: vec![
                MetadataField {
                    key: "filename".into(),
                    value: a.name.clone(),
                    display_label: "Filename".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "size".into(),
                    value: format_file_size(a.size),
                    display_label: "Size".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "created".into(),
                    value: a.created_date.clone(),
                    display_label: "Created".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "description".into(),
                    value: a.description.clone(),
                    display_label: "Description".into(),
                    editable: false,
                    allowed_values: None,
                },
            ],
        },
        has_children: None,
    }
}

fn format_file_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[async_trait]
impl Content for TaigaItemNode {
    fn node_type(&self) -> &NodeType {
        node_type_for(self.detail.item_type)
    }

    fn version(&self) -> Option<&str> {
        self.detail.modified.as_deref()
    }

    async fn read(&self) -> Result<Vec<u8>> {
        Ok(self.read_text().await?.into_bytes())
    }

    async fn read_text(&self) -> Result<String> {
        let d = &self.detail;
        let ref_num = d.r#ref;
        let display_ref = match &d.project_slug {
            Some(slug) if !slug.is_empty() => format!("{slug}#{ref_num}"),
            _ => format!("#{ref_num}"),
        };
        let assignees = if d.assignees.is_empty() {
            "(unassigned)".to_string()
        } else {
            d.assignees.join(", ")
        };
        let modified = d.modified.as_deref().unwrap_or("");
        let body = if d.description.is_empty() {
            "(no description)"
        } else {
            d.description.as_str()
        };
        Ok(format!(
            "# {}\n\n\
             - **Ref:** {display_ref}\n\
             - **Type:** {}\n\
             - **Status:** {}\n\
             - **Assignee:** {assignees}\n\
             - **Modified:** {modified}\n\n\
             ---\n\n{body}\n",
            d.subject,
            d.item_type.as_str(),
            d.status,
        ))
    }
}

#[cfg(test)]
mod row_summary_tests {
    use super::*;

    fn sample_detail() -> ItemDetail {
        ItemDetail {
            item_type: ItemType::UserStory,
            id: 1234,
            r#ref: 87,
            project_id: 9,
            project_slug: Some("demo-board".into()),
            subject: "Add export button".into(),
            description: "Lets users download the report.".into(),
            status: "In progress".into(),
            assignees: vec!["Dana Lee".into(), "Sam Ray".into()],
            assignee_usernames: vec!["dana".into(), "sam".into()],
            tags: vec![],
            modified: Some("2025-02-03T09:00:00+0000".into()),
            version: 4,
            parent_user_story_id: None,
            parent_user_story_subject: None,
        }
    }

    /// The row projection must mirror the keys `item_summary_to_node_summary`
    /// emits so the post-edit patch refreshes the right columns. `attachments`
    /// is deliberately absent — the detail fetch has no count — so the patch's
    /// key-merge keeps the row's last-known attachment value rather than
    /// blanking it. (Before the fix, `metadata()` was empty, blanking the
    /// whole row after an edit.)
    #[test]
    fn row_summary_mirrors_list_row_keys_and_values() {
        let d = sample_detail();
        let row = item_detail_to_row_summary(&d, "userstory:1234");

        assert_eq!(row.id, "userstory:1234");
        assert_eq!(row.label, "Add export button");

        let keys: Vec<&str> = row.metadata.fields.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, ["ref", "type", "status", "assignee", "modified", "subject"]);
        assert!(!keys.contains(&"attachments"));

        let value = |k: &str| {
            row.metadata
                .fields
                .iter()
                .find(|f| f.key == k)
                .map(|f| f.value.as_str())
                .unwrap_or("")
        };
        assert_eq!(value("ref"), "demo-board#87");
        assert_eq!(value("status"), "In progress");
        assert_eq!(value("assignee"), "Dana Lee, Sam Ray");
        assert_eq!(value("modified"), "2025-02-03T09:00:00+0000");
    }

    /// Without a project slug the ref falls back to a bare `#<ref>`.
    #[test]
    fn row_summary_ref_without_slug() {
        let mut d = sample_detail();
        d.project_slug = None;
        let row = item_detail_to_row_summary(&d, "userstory:1234");
        let ref_val = row
            .metadata
            .fields
            .iter()
            .find(|f| f.key == "ref")
            .map(|f| f.value.as_str())
            .unwrap_or("");
        assert_eq!(ref_val, "#87");
    }
}
