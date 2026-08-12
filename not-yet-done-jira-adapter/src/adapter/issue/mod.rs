//! `JiraIssueNode`: the central node type for Jira issues. Handles the
//! `edit_full` and `edit_with_comments` flows, conflict-resolution merge,
//! transition picker, and child listing (comments + attachments).
//!
//! Split into submodules by concern:
//! - [`markers`] — banner/marker constants
//! - [`template`] — 3b template render/parse/validate/diff
//! - [`slugs`] — `ll-…` / `uu-…` slug tables and mention rewriting
//! - [`merge`] — diffy-based 3-way merge for conflict handling
//! - [`edit_full`] — end-to-end `edit_full` action flow
//! - [`edit_with_comments`] — combined header+comments edit flow
//!
//! Tests for the entire issue surface live at the bottom of this file so
//! they can reference internals across all submodules via `super::*`.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::OnceCell;

use not_yet_done_content::*;

use crate::client::{JiraClient, JiraIssueDetail};

use super::attachment::{JiraAttachmentNode, download_summary, write_attachments};
use super::cache::{JiraCache, fetch_comments, fetch_issue};
use super::comment::JiraCommentNode;
use super::types::{attachment_node_type, comment_node_type, issue_node_type};
use super::util::{format_file_size, other_err, prepare_target_dir, truncate_body};

mod clone;
mod edit_full;
mod edit_with_comments;
mod export;
mod link;
mod markers;
mod merge;
mod slugs;
mod template;
mod transitions;
mod wiki_md;
mod workspace;

use template::{edit_full_fields, strip_template_comments};

/// Issue node with lazily-hydrated full detail. `from_key` constructs
/// without a network round-trip; `detail()` fetches on first access. This
/// avoids a 403 on the full-detail GET for users who can see the issue in
/// search results but lack issue-level read permission — child operations
/// (attachments, transitions, etc.) only need the key and may still
/// succeed.
pub(super) fn issue_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("edit_full", "edit", InputSpec::Editor),
        NodeAction::new("edit_markdown", "edit (markdown)", InputSpec::Editor),
        // CLI-oriented counterparts of `edit_markdown`, sharing its exact
        // buffer and write-back pipeline: `to_markdown` prints the Markdown to
        // stdout (`ActionOutcome::Done`), `from_markdown` takes that same
        // Markdown back (`-m`, `--file`, or stdin) and writes the ticket.
        NodeAction::new("to_markdown", "to markdown", InputSpec::None),
        NodeAction::new("from_markdown", "from markdown", InputSpec::Editor),
        NodeAction::new("edit_with_comments", "edit + comments", InputSpec::Editor),
        NodeAction::new("transition", "transition", InputSpec::Picker),
        NodeAction::new("create_comment", "add comment", InputSpec::Editor),
        NodeAction::new("toggle_watch", "toggle watch", InputSpec::None),
        NodeAction::new("toggle-bookmark", "bookmark", InputSpec::None),
        NodeAction::new("remove-bookmark", "remove bookmark", InputSpec::None),
        NodeAction::new("open_in_browser", "open in browser", InputSpec::None),
        NodeAction::new("clone", "clone", InputSpec::Editor),
        NodeAction::new(
            "link",
            "link issue",
            InputSpec::Form {
                fields: vec![
                    FormFieldSpec::text("target", "Target issue key (e.g. PROJ-123)"),
                    FormFieldSpec::text(
                        "relation",
                        "Relation (e.g. blocks, is blocked by, relates to)",
                    )
                    .with_default("relates to"),
                ],
            },
        ),
        NodeAction::new(
            "download-attachments",
            "download attachments",
            InputSpec::Form {
                fields: vec![FormFieldSpec::text("dir", "Target directory")],
            },
        ),
        NodeAction::new("export-bundle", "export bundle", InputSpec::None),
        NodeAction::new(
            "export_workspace",
            "export workspace",
            InputSpec::Form {
                fields: vec![FormFieldSpec::text("dir", "Workspace base directory")],
            },
        ),
    ]
}

pub(super) struct JiraIssueNode {
    pub(super) client: Arc<JiraClient>,
    pub(super) cache: Arc<Mutex<JiraCache>>,
    pub(super) key: String,
    pub(super) summary_hint: String,
    pub(super) detail: OnceCell<JiraIssueDetail>,
    pub(super) cached_metadata: Metadata,
    /// Bookmark store for the `toggle-bookmark` action. Present only on
    /// nodes built at the adapter boundary (`get_by_id`), which is the
    /// path every action dispatch takes; internal rebuilds (edit/merge)
    /// leave it `None` and never receive `toggle-bookmark`.
    pub(super) bookmarks: Option<Arc<dyn BookmarkStore>>,
    /// Base directory for the persistent ticket workspace, attached at the
    /// `get_by_id`/`get_child` boundary via [`with_workspace_base`]. `None`
    /// on internal rebuilds (edit/merge), which never dispatch the
    /// `edit_markdown` / `export_workspace` actions.
    ///
    /// [`with_workspace_base`]: JiraIssueNode::with_workspace_base
    pub(super) workspace_base: Option<Arc<std::path::PathBuf>>,
}

fn build_metadata_from_detail(detail: &JiraIssueDetail) -> Metadata {
    Metadata {
        fields: vec![
            MetadataField {
                key: "key".into(),
                value: detail.key.clone(),
                display_label: "Key".into(),
                editable: false,
                allowed_values: None,
            },
            MetadataField {
                key: "summary".into(),
                value: detail.summary.clone(),
                display_label: "Summary".into(),
                editable: true,
                allowed_values: None,
            },
            MetadataField {
                key: "type".into(),
                value: detail.issue_type.clone(),
                display_label: "Type".into(),
                editable: false,
                allowed_values: None,
            },
            MetadataField {
                key: "status".into(),
                value: detail.status.clone(),
                display_label: "Status".into(),
                editable: false,
                allowed_values: None,
            },
            MetadataField {
                key: "priority".into(),
                value: detail.priority.clone(),
                display_label: "Priority".into(),
                editable: false,
                allowed_values: None,
            },
            MetadataField {
                key: "assignee".into(),
                value: detail.assignee.clone(),
                display_label: "Assignee".into(),
                editable: false,
                allowed_values: None,
            },
            MetadataField {
                key: "creator".into(),
                value: detail.creator.clone(),
                display_label: "Creator".into(),
                editable: false,
                allowed_values: None,
            },
            MetadataField {
                key: "fix_versions".into(),
                value: detail.fix_versions.clone(),
                display_label: "Fix Versions".into(),
                editable: false,
                allowed_values: None,
            },
        ],
    }
}

/// The **list-row** metadata projection — must mirror the field keys
/// `JiraRoot::list_issues` emits (`key, type, status, priority, assignee,
/// creator, fix_versions, updated`), so the post-edit row patch refreshes the
/// same columns the list rendered. `attachments` is intentionally omitted: the
/// detail fetch doesn't carry an attachment count, so the patch keeps the row's
/// last-known value.
fn build_row_metadata_from_detail(detail: &JiraIssueDetail) -> Metadata {
    let f = |key: &str, value: String, label: &str| MetadataField {
        key: key.into(),
        value,
        display_label: label.into(),
        editable: false,
        allowed_values: None,
    };
    Metadata {
        fields: vec![
            f("key", detail.key.clone(), "Key"),
            f("type", detail.issue_type.clone(), "Type"),
            f("status", detail.status.clone(), "Status"),
            f("priority", detail.priority.clone(), "Priority"),
            f("assignee", detail.assignee.clone(), "Assignee"),
            f("creator", detail.creator.clone(), "Creator"),
            f("fix_versions", detail.fix_versions.clone(), "Fix Versions"),
            f("updated", detail.updated.clone(), "Updated"),
        ],
    }
}

fn build_metadata_sparse(key: &str, summary_hint: &str) -> Metadata {
    Metadata {
        fields: vec![
            MetadataField {
                key: "key".into(),
                value: key.to_string(),
                display_label: "Key".into(),
                editable: false,
                allowed_values: None,
            },
            MetadataField {
                key: "summary".into(),
                value: summary_hint.to_string(),
                display_label: "Summary".into(),
                editable: true,
                allowed_values: None,
            },
        ],
    }
}

impl JiraIssueNode {
    /// Construct without fetching the full detail. `summary_hint` populates
    /// `label()` until `detail()` is first awaited; pass empty string when
    /// no hint is available (label falls back to the key).
    pub(super) fn from_key(
        client: Arc<JiraClient>,
        cache: Arc<Mutex<JiraCache>>,
        key: String,
        summary_hint: String,
        bookmarks: Option<Arc<dyn BookmarkStore>>,
    ) -> Self {
        let cached_metadata = build_metadata_sparse(&key, &summary_hint);
        Self {
            client,
            cache,
            key,
            summary_hint,
            detail: OnceCell::new(),
            cached_metadata,
            bookmarks,
            workspace_base: None,
        }
    }

    /// Attach the ticket-workspace base directory. Called at the
    /// `get_by_id`/`get_child` boundary so the `edit_markdown` /
    /// `export_workspace` actions can materialise `<base>/<KEY>-<slug>/`.
    pub(super) fn with_workspace_base(mut self, base: Arc<std::path::PathBuf>) -> Self {
        self.workspace_base = Some(base);
        self
    }

    /// Construct with detail already loaded. Used by `write_description`
    /// rebuilds and by tests; OnceCell is pre-filled so `detail()` won't
    /// hit the network.
    pub(super) fn from_detail(
        client: Arc<JiraClient>,
        cache: Arc<Mutex<JiraCache>>,
        detail: JiraIssueDetail,
    ) -> Self {
        let cached_metadata = build_metadata_from_detail(&detail);
        let key = detail.key.clone();
        let summary_hint = detail.summary.clone();
        let cell = OnceCell::new_with(Some(detail));
        Self {
            client,
            cache,
            key,
            summary_hint,
            detail: cell,
            cached_metadata,
            bookmarks: None,
            workspace_base: None,
        }
    }

    /// Compatibility alias for the older eager-construction call sites.
    pub(super) fn new(
        client: Arc<JiraClient>,
        cache: Arc<Mutex<JiraCache>>,
        detail: JiraIssueDetail,
    ) -> Self {
        Self::from_detail(client, cache, detail)
    }

    /// Lazily fetch the full issue detail on first access.
    pub(super) async fn detail(&self) -> Result<&JiraIssueDetail> {
        self.detail
            .get_or_try_init(|| async {
                fetch_issue(&self.client, &self.cache, &self.key)
                    .await
                    .map_err(other_err)
            })
            .await
    }

    /// Sync accessor for paths that have already awaited `detail()`. Panics
    /// when called before initialization — callers must guarantee that.
    #[cfg(test)]
    pub(super) fn detail_now(&self) -> &JiraIssueDetail {
        self.detail
            .get()
            .expect("detail must be loaded before detail_now()")
    }

    /// Replace the cached detail (e.g. after a successful PUT or refetch).
    /// Resets the OnceCell with the new value and rebuilds the dependent
    /// summary_hint / cached_metadata in lockstep.
    pub(super) fn replace_detail(&mut self, detail: JiraIssueDetail) {
        self.summary_hint = detail.summary.clone();
        self.cached_metadata = build_metadata_from_detail(&detail);
        self.detail = OnceCell::new_with(Some(detail));
    }
}

#[async_trait]
impl Node for JiraIssueNode {
    fn id(&self) -> &str {
        &self.key
    }

    fn label(&self) -> &str {
        if self.summary_hint.is_empty() {
            &self.key
        } else {
            &self.summary_hint
        }
    }

    fn node_type(&self) -> &NodeType {
        static ISSUE_TYPE: std::sync::LazyLock<NodeType> =
            std::sync::LazyLock::new(issue_node_type);
        &ISSUE_TYPE
    }

    fn metadata(&self) -> &Metadata {
        &self.cached_metadata
    }

    fn row_summary(&self) -> NodeSummary {
        // `metadata()` is the detail/edit-form projection (carries `summary`,
        // editable flags, no `updated`); the list row needs the `list_issues`
        // shape. Build it from the loaded detail; a stub that never hydrated
        // falls back to an empty field set, leaving the patch to keep the row's
        // base values.
        let metadata = match self.detail.get() {
            Some(detail) => build_row_metadata_from_detail(detail),
            None => Metadata { fields: vec![] },
        };
        NodeSummary {
            id: self.key.clone(),
            label: self.label().to_string(),
            node_type: Node::node_type(self).clone(),
            metadata,
            has_children: None,
        }
    }

    async fn hydrate(&mut self) {
        // A `from_key` stub carries a sparse label/metadata (just the key and
        // an empty summary). Fetch the full detail once and rebuild both
        // display fields in lockstep via `replace_detail`. Skip when detail is
        // already loaded; a failed fetch leaves the stub (degrades to the key).
        if self.detail.get().is_some() {
            return;
        }
        if let Ok(detail) = fetch_issue(&self.client, &self.cache, &self.key).await {
            self.replace_detail(detail);
        }
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        // Extract plain ID from composite format (e.g. "PROJ-1/comment/123" → "123")
        let plain_id = id.rsplit('/').next().unwrap_or(id);

        let comments = fetch_comments(&self.client, &self.cache, &self.key)
            .await
            .map_err(other_err)?;
        if let Some(comment) = comments.into_iter().find(|c| c.id == plain_id) {
            return Ok(Box::new(JiraCommentNode::new(
                Arc::clone(&self.client),
                comment,
                self.key.clone(),
            )));
        }

        let attachments = self
            .client
            .get_attachments(&self.key)
            .await
            .map_err(other_err)?;
        if let Some(attachment) = attachments.into_iter().find(|a| a.id == plain_id) {
            return Ok(Box::new(JiraAttachmentNode::new(
                Arc::clone(&self.client),
                attachment,
                self.key.clone(),
            )));
        }

        Err(ContentError::NotFound(format!("Child {id} not found")))
    }

    fn content(&self) -> Option<&dyn Content> {
        Some(self)
    }

    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        match action_id {
            "edit_full" => {
                let detail = self.detail().await?;
                let tables = self.slug_tables(detail).await;
                Ok(EditorPrep {
                    template: self.render_3b(&edit_full_fields(), detail, None, None, &tables),
                    version: detail.updated.clone(),
                    suffix: ".jira".into(),
                    file_path: None,
                })
            }
            "edit_markdown" => {
                // Guarded build: refuses if the description or any comment
                // wouldn't survive a lossless wiki→md→wiki round-trip.
                let template = self.ticket_markdown(true).await?;
                let detail = self.detail().await?;
                // When a workspace base is configured, open a persistent
                // `<base>/<KEY>-<slug>/ticket.md` and sync attachments on
                // demand so the image links resolve locally. Without one, fall
                // back to the classic throwaway temp file.
                let file_path = match &self.workspace_base {
                    Some(base) => {
                        let dir = workspace::ticket_dir(base, &self.key, &detail.summary);
                        workspace::sync_attachments(&self.client, &self.key, &dir).await?;
                        Some(dir.join(workspace::TICKET_FILE))
                    }
                    None => None,
                };
                Ok(EditorPrep {
                    template,
                    version: detail.updated.clone(),
                    suffix: ".md".into(),
                    file_path,
                })
            }
            "from_markdown" => {
                // Same guarded Markdown buffer as `edit_markdown`, but no
                // persistent workspace file — this is the CLI write-back entry
                // point (`do_editor` supplies the edited text from `-m`/`--file`
                // /stdin and never launches an interactive editor).
                let template = self.ticket_markdown(true).await?;
                let detail = self.detail().await?;
                Ok(EditorPrep {
                    template,
                    version: detail.updated.clone(),
                    suffix: ".md".into(),
                    file_path: None,
                })
            }
            "edit_with_comments" => {
                let detail = self.detail().await?;
                let comments = fetch_comments(&self.client, &self.cache, &self.key)
                    .await
                    .map_err(other_err)?;
                let mention_sources: Vec<&str> = comments.iter().map(|c| c.body.as_str()).collect();
                super::cache::resolve_unknown_mentions(&self.client, &self.cache, &mention_sources)
                    .await;
                let tables = self.slug_tables(detail).await;
                let template =
                    self.render_with_comments(&edit_full_fields(), detail, &comments, &tables);
                Ok(EditorPrep {
                    template,
                    version: detail.updated.clone(),
                    suffix: ".jira".into(),
                    file_path: None,
                })
            }
            "create_comment" => Ok(EditorPrep {
                template: format!("# New comment for {}\n\n", self.key),
                version: String::new(),
                suffix: ".jira".into(),
                file_path: None,
            }),
            "clone" => self.prepare_clone().await,
            other => Err(ContentError::NotSupported(format!(
                "prepare: unknown action {other}"
            ))),
        }
    }

    async fn picker_options(&self, action_id: &str) -> Result<Vec<ActionOption>> {
        match action_id {
            "transition" => {
                let detail = self.detail().await?.clone();
                let transitions = self
                    .client
                    .get_transitions(&self.key)
                    .await
                    .map_err(other_err)?;
                Ok(self.transition_options(&detail, &transitions).await)
            }
            other => Err(ContentError::NotSupported(format!(
                "picker_options: unknown action {other}"
            ))),
        }
    }

    /// Prefill the `export_workspace` form's `dir` field with the configured
    /// workspace base so the user only edits it when they want a one-off
    /// location. Other form actions carry no prefill.
    async fn form_prep(
        &self,
        action_id: &str,
    ) -> Result<std::collections::HashMap<String, String>> {
        let mut prefill = std::collections::HashMap::new();
        if action_id == "export_workspace" {
            if let Some(base) = &self.workspace_base {
                prefill.insert("dir".to_string(), base.display().to_string());
            }
        }
        Ok(prefill)
    }

    /// `remove-bookmark` opts into the generic confirm flow: the first
    /// invocation (`ctx.confirmed == false`) returns [`ActionDispatch::Confirm`]
    /// so the TUI shows a `(y/n)` prompt; on "y" it re-invokes with
    /// `confirmed: true` and we drop the bookmark, then reload so the row
    /// vanishes from the bookmarks subtab. The normal tickets subtab keeps
    /// using `toggle-bookmark` via [`Node::execute`] (no confirmation).
    async fn invoke_action(&self, name: &str, ctx: &ActionContext) -> Result<ActionDispatch> {
        match name {
            "remove-bookmark" => {
                let store = self.bookmarks.as_ref().ok_or_else(|| {
                    other_err("bookmark store unavailable for this node".to_string())
                })?;
                if !ctx.confirmed {
                    return Ok(ActionDispatch::Confirm {
                        prompt: format!("Remove bookmark for {}? (y/n)", self.key),
                    });
                }
                // Guarded toggle: only remove when actually bookmarked, so a
                // stale invocation can never accidentally *add* a bookmark.
                if store.contains(&self.key).await? {
                    store.toggle(&self.key).await?;
                }
                Ok(ActionDispatch::Reload)
            }
            _ => Ok(ActionDispatch::Noop),
        }
    }

    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match (action_id, input) {
            (
                "edit_full",
                ActionInput::Edited {
                    text,
                    original,
                    version,
                },
            ) => {
                self.execute_edit_full(&text, &version, Some(&original))
                    .await
            }
            (
                "edit_markdown",
                ActionInput::Edited {
                    text,
                    original,
                    version,
                },
            ) => {
                self.execute_edit_markdown(&text, &version, Some(&original))
                    .await
            }
            ("to_markdown", ActionInput::None) => {
                // Print the exact buffer `edit_markdown`/`from_markdown` open
                // with (guarded, so what round-trips out writes cleanly back).
                Ok(ActionOutcome::Done {
                    message: Some(self.ticket_markdown(true).await?),
                })
            }
            (
                "from_markdown",
                ActionInput::Edited {
                    text,
                    original,
                    version,
                },
            ) => {
                self.execute_edit_markdown(&text, &version, Some(&original))
                    .await
            }
            (
                "edit_with_comments",
                ActionInput::Edited {
                    text,
                    original,
                    version,
                },
            ) => {
                self.execute_edit_with_comments(&text, &original, &version)
                    .await
            }
            ("transition", ActionInput::Picked(transition_chain)) => {
                self.execute_transition_chain(&transition_chain).await
            }
            ("create_comment", ActionInput::Edited { text, .. }) => {
                let body = strip_template_comments(&text);
                if body.trim().is_empty() {
                    return Ok(ActionOutcome::NoChanges);
                }
                let comment = self
                    .client
                    .add_comment(&self.key, &body)
                    .await
                    .map_err(other_err)?;
                Ok(ActionOutcome::Navigate {
                    node_id: format!("{}/comment/{}", self.key, comment.id),
                    node_type: comment_node_type(),
                    message: None,
                })
            }
            ("toggle_watch", ActionInput::None) => {
                let now_watching = self
                    .client
                    .toggle_watch(&self.key)
                    .await
                    .map_err(other_err)?;
                let label = if now_watching {
                    "watching"
                } else {
                    "no longer watching"
                };
                Ok(ActionOutcome::Done {
                    message: Some(format!("{}: {label}", self.key)),
                })
            }
            ("toggle-bookmark", ActionInput::None) => {
                let store = self.bookmarks.as_ref().ok_or_else(|| {
                    other_err("bookmark store unavailable for this node".to_string())
                })?;
                let now_bookmarked = store.toggle(&self.key).await?;
                let label = if now_bookmarked {
                    "bookmarked"
                } else {
                    "bookmark removed"
                };
                Ok(ActionOutcome::Done {
                    message: Some(format!("{}: {label}", self.key)),
                })
            }
            ("open_in_browser", ActionInput::None) => self.open_in_browser(),
            ("clone", ActionInput::Edited { text, .. }) => self.execute_clone(&text).await,
            ("link", ActionInput::Form(values)) => self.execute_link(&values).await,
            ("download-attachments", ActionInput::Form(values)) => {
                let dir = values.get("dir").map(String::as_str).unwrap_or("");
                self.download_attachments(dir).await
            }
            ("export-bundle", ActionInput::None) => self.export_bundle().await,
            ("export_workspace", ActionInput::Form(values)) => {
                let dir = values.get("dir").map(String::as_str).unwrap_or("");
                self.export_workspace(dir).await
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "execute: unknown action {other}"
            ))),
        }
    }
}

impl JiraIssueNode {
    /// Hand `<base>/browse/<KEY>` to `xdg-open`. The base URL is the
    /// normalized REST root (see `client::normalize_base_url`), which
    /// already has any `/browse/`, `/rest/`, etc. suffix stripped.
    fn open_in_browser(&self) -> Result<ActionOutcome> {
        let base = self.client.base_url.trim_end_matches('/');
        let url = format!("{base}/browse/{}", self.key);
        std::process::Command::new("xdg-open")
            .arg(&url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| other_err(format!("spawn xdg-open: {e}")))?;
        Ok(ActionOutcome::Done {
            message: Some(format!("opened {url}")),
        })
    }

    /// Build the ticket body + comments as one Markdown buffer — the shared
    /// content of the `edit_markdown` editor and the `export_workspace` file.
    /// When `guarded`, refuse (with a descriptive error) if the description or
    /// any comment would not survive a lossless wiki→md→wiki round-trip; the
    /// unguarded path is for the read-only export, which never writes back.
    async fn ticket_markdown(&self, guarded: bool) -> Result<String> {
        let detail = self.detail().await?;
        let comments = fetch_comments(&self.client, &self.cache, &self.key)
            .await
            .map_err(other_err)?;
        if guarded {
            if let Some(diff) = wiki_md::roundtrip_diff(&detail.description) {
                return Err(other_err(format!(
                    "Markdown edit unavailable for {}: the description uses Jira \
                     markup that would not survive a lossless round-trip. Use the \
                     plain 'edit' action instead.\n\nFirst divergence:\n{diff}",
                    self.key
                )));
            }
            for c in &comments {
                if let Some(diff) = wiki_md::roundtrip_diff(&c.body) {
                    return Err(other_err(format!(
                        "Markdown edit unavailable for {}: comment {} uses Jira \
                         markup that would not survive a lossless round-trip. Use \
                         the plain 'edit + comments' action instead.\n\nFirst \
                         divergence:\n{diff}",
                        self.key, c.id
                    )));
                }
            }
        }
        let mention_sources: Vec<&str> = comments.iter().map(|c| c.body.as_str()).collect();
        super::cache::resolve_unknown_mentions(&self.client, &self.cache, &mention_sources).await;
        let tables = self.slug_tables(detail).await;
        let canonical = self.render_with_comments(&edit_full_fields(), detail, &comments, &tables);
        Ok(edit_with_comments::comments_canonical_to_md(&canonical))
    }

    /// Materialise the `export_workspace` folder: `<base>/<KEY>-<slug>/` with
    /// `ticket.md` (unguarded Markdown) and on-demand `attachments/`. No editor
    /// and no Jira write-back — a local, read-oriented snapshot.
    async fn export_workspace(&self, base_input: &str) -> Result<ActionOutcome> {
        let base = prepare_target_dir(base_input)?;
        let markdown = self.ticket_markdown(false).await?;
        let detail = self.detail().await?;
        let dir =
            workspace::materialize(&self.client, &self.key, &detail.summary, &markdown, &base)
                .await?;
        Ok(ActionOutcome::Done {
            message: Some(format!(
                "{}: exported workspace to {}",
                self.key,
                dir.display()
            )),
        })
    }

    /// Download **every** attachment of this issue into `dir_input` via the
    /// shared [`write_attachments`] helper — the issue-node counterpart to the
    /// attachment node's `download_all`, so a script can fetch all files with
    /// only the issue key in hand.
    async fn download_attachments(&self, dir_input: &str) -> Result<ActionOutcome> {
        let dir = prepare_target_dir(dir_input)?;

        let attachments = self
            .client
            .get_attachments(&self.key)
            .await
            .map_err(other_err)?;
        if attachments.is_empty() {
            return Ok(ActionOutcome::Done {
                message: Some(format!("{}: no attachments to download", self.key)),
            });
        }

        let (saved, total, failures) = write_attachments(&self.client, &attachments, &dir).await;
        Ok(ActionOutcome::Done {
            message: Some(download_summary(&self.key, &dir, saved, total, &failures)),
        })
    }
}

/// List an issue's comments — the single fetch source behind both the issue
/// node's legacy `list` (for `jira:comment`) and the adapter's
/// [`ContentAdapter::childs`] declaration. Reconstructs from adapter state
/// (`client`, `cache`) plus the issue key (= the node's `id()`).
pub(super) async fn list_comments(
    client: &Arc<JiraClient>,
    cache: &Arc<Mutex<JiraCache>>,
    key: &str,
) -> Result<ListResult> {
    let comments = fetch_comments(client, cache, key)
        .await
        .map_err(other_err)?;

    let issue_key = key;
    let items = comments
        .into_iter()
        .map(|c| {
            let preview = truncate_body(&c.body, 80);
            NodeSummary {
                id: format!("{issue_key}/comment/{}", c.id),
                label: preview,
                node_type: comment_node_type(),
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
                    ],
                },
                has_children: None,
            }
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

/// List an issue's attachments — the single fetch source behind both the
/// issue node's legacy `list` (for `jira:attachment`) and the adapter's
/// [`ContentAdapter::childs`] declaration. Reconstructs from adapter state
/// (`client`) plus the issue key (= the node's `id()`).
pub(super) async fn list_attachments(client: &Arc<JiraClient>, key: &str) -> Result<ListResult> {
    let attachments = client.get_attachments(key).await.map_err(other_err)?;

    let issue_key = key;
    let items = attachments
        .into_iter()
        .map(|a| {
            let size_display = format_file_size(a.size);
            NodeSummary {
                id: format!("{issue_key}/attachment/{}", a.id),
                label: a.filename.clone(),
                node_type: attachment_node_type(),
                metadata: Metadata {
                    fields: vec![
                        MetadataField {
                            key: "filename".into(),
                            value: a.filename,
                            display_label: "Filename".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "author".into(),
                            value: a.author,
                            display_label: "Author".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "size".into(),
                            value: size_display,
                            display_label: "Size".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "mime_type".into(),
                            value: a.mime_type,
                            display_label: "Type".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "created".into(),
                            value: a.created,
                            display_label: "Created".into(),
                            editable: false,
                            allowed_values: None,
                        },
                    ],
                },
                has_children: None,
            }
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

#[async_trait]
impl Content for JiraIssueNode {
    fn node_type(&self) -> &NodeType {
        static ISSUE_TYPE: std::sync::LazyLock<NodeType> =
            std::sync::LazyLock::new(issue_node_type);
        &ISSUE_TYPE
    }

    fn version(&self) -> Option<&str> {
        // Sync trait method — only return the version when detail is
        // already loaded; lazy callers either load it via `detail()` first
        // or accept None until then.
        self.detail.get().map(|d| d.updated.as_str())
    }

    async fn read(&self) -> Result<Vec<u8>> {
        Ok(self.detail().await?.description.as_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::super::cache::JiraCache as CacheAlias;
    use super::super::util::normalize_blank_lines;
    use super::edit_with_comments::{
        CommentBlockKind, comments_canonical_to_md, comments_md_to_canonical, is_delete_keyword,
        parse_comment_header_id, render_comment_header,
    };
    use super::markers::{ADD_COMMENT_MARKER, CACHE_MARKER, ERROR_BANNER_START};
    use super::slugs::{build_slug_tables, build_status_table, resolve_slugs_inplace};
    use super::template::{ChangeSet, FieldError};
    use super::*;
    use crate::cache_store;
    use crate::client::{JiraAttachment, JiraComment, JiraUser};
    use std::sync::Mutex;

    fn test_client() -> Arc<JiraClient> {
        Arc::new(JiraClient::new("http://localhost:0", None, None, Some("test"), false).unwrap())
    }

    /// Project an issue's user references (assignee, reporter, creator) into
    /// `JiraUser` records suitable for cache merging in tests. Keeps the
    /// production `cache::issue_users` private to the cache module.
    fn issue_users_for_test(detail: &JiraIssueDetail) -> Vec<JiraUser> {
        let mut users = Vec::new();
        for (key, display) in [
            (&detail.assignee_key, &detail.assignee),
            (&detail.reporter_key, &detail.reporter),
            (&detail.creator_key, &detail.creator),
        ] {
            if key.is_empty() {
                continue;
            }
            let display_name = if display.is_empty() {
                key.clone()
            } else {
                display.clone()
            };
            users.push(JiraUser {
                name: key.clone(),
                display_name,
                email_address: None,
            });
        }
        users
    }

    /// Create a test JiraIssueNode without network. The JiraClient is
    /// constructed with dummy params (never used for template/parse tests).
    ///
    /// Mirrors the production invariant that `fetch_issue` populates the
    /// cache with the issue's user references before any render/parse
    /// happens — without that seeding, `build_slug_tables` returns an
    /// empty users table and editable-`assignee` rendering can't produce
    /// a `uu-…` slug.
    fn test_node(detail: JiraIssueDetail) -> JiraIssueNode {
        let client = test_client();
        let scope_id = cache_store::scope_id_for_url("http://localhost:0");
        let cache = Arc::new(Mutex::new(CacheAlias::new(None, scope_id)));
        cache
            .lock()
            .unwrap()
            .merge_users(issue_users_for_test(&detail));
        cache
            .lock()
            .unwrap()
            .merge_labels(detail.labels.iter().cloned());
        JiraIssueNode::new(client, cache, detail)
    }

    fn sample_detail() -> JiraIssueDetail {
        JiraIssueDetail {
            key: "PROJ-42".into(),
            summary: "Fix login bug".into(),
            description: "The login form crashes on submit.".into(),
            status: "In Progress".into(),
            status_id: "3".into(),
            priority: "High".into(),
            issue_type: "Bug".into(),
            issue_type_id: "10001".into(),
            assignee: "alice".into(),
            assignee_key: "alice".into(),
            reporter: String::new(),
            reporter_key: String::new(),
            creator: "bob".into(),
            creator_key: "bob".into(),
            labels: Vec::new(),
            fix_versions: "1.2.0, 1.3.0".into(),
            updated: "2025-01-01T00:00:00.000+0000".into(),
        }
    }

    /// A `from_key` stub shows the key as its label and carries only a
    /// key+summary metadata pair (summary blank). `replace_detail` — the
    /// synchronous field-rewrite at the core of `Node::hydrate` — must swap in
    /// the real summary and the full field set. This is exactly what the
    /// post-edit row patch relies on: without it the patched row keeps the key
    /// in the Summary column and drops the rest until a full reload.
    #[test]
    fn replace_detail_rewrites_stub_label_and_metadata() {
        let client = test_client();
        let scope_id = cache_store::scope_id_for_url("http://localhost:0");
        let cache = Arc::new(Mutex::new(CacheAlias::new(None, scope_id)));
        let mut node =
            JiraIssueNode::from_key(client, cache, "PROJ-42".into(), String::new(), None);

        // Sparse stub: label falls back to the key; metadata is key + summary.
        assert_eq!(node.label(), "PROJ-42");
        assert_eq!(node.metadata().fields.len(), 2);
        let summary_before = node
            .metadata()
            .fields
            .iter()
            .find(|f| f.key == "summary")
            .map(|f| f.value.as_str());
        assert_eq!(summary_before, Some(""));

        // Hydration's field-rewrite: real summary as label + full metadata.
        node.replace_detail(sample_detail());
        assert_eq!(node.label(), "Fix login bug");
        assert!(node.metadata().fields.len() > 2);
        let summary_after = node
            .metadata()
            .fields
            .iter()
            .find(|f| f.key == "summary")
            .map(|f| f.value.as_str());
        assert_eq!(summary_after, Some("Fix login bug"));
    }

    /// The post-edit row patch overlays `row_summary()` onto the visible list
    /// row, merging by key. For that to refresh the right columns, the row
    /// projection's keys must mirror what `JiraRoot::list_issues` emits
    /// (`key, type, status, priority, assignee, creator, fix_versions, updated`)
    /// and carry the fresh detail values. `attachments` is deliberately absent —
    /// the detail fetch has no count — so the patch keeps the row's last-known
    /// value there.
    #[test]
    fn row_summary_mirrors_list_row_keys_and_values() {
        let node = test_node(sample_detail());
        let row = node.row_summary();

        assert_eq!(row.id, "PROJ-42");
        assert_eq!(row.label, "Fix login bug");

        let keys: Vec<&str> = row.metadata.fields.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "key",
                "type",
                "status",
                "priority",
                "assignee",
                "creator",
                "fix_versions",
                "updated"
            ]
        );
        // No `summary` field (that's the detail/edit-form projection) and no
        // `attachments` (the detail can't supply a count).
        assert!(!keys.contains(&"summary"));
        assert!(!keys.contains(&"attachments"));

        let value = |k: &str| {
            row.metadata
                .fields
                .iter()
                .find(|f| f.key == k)
                .map(|f| f.value.as_str())
                .unwrap_or("")
        };
        assert_eq!(value("status"), "In Progress");
        assert_eq!(value("priority"), "High");
        assert_eq!(value("assignee"), "alice");
        assert_eq!(value("creator"), "bob");
        assert_eq!(value("fix_versions"), "1.2.0, 1.3.0");
        assert_eq!(value("type"), "Bug");
    }

    /// A stub that never hydrated has no detail; `row_summary()` then yields an
    /// empty field set so the patch's merge keeps every base column rather than
    /// blanking them.
    #[test]
    fn row_summary_of_unhydrated_stub_is_empty() {
        let client = test_client();
        let scope_id = cache_store::scope_id_for_url("http://localhost:0");
        let cache = Arc::new(Mutex::new(CacheAlias::new(None, scope_id)));
        let node = JiraIssueNode::from_key(client, cache, "PROJ-42".into(), "hint".into(), None);

        let row = node.row_summary();
        assert!(row.metadata.fields.is_empty());
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

    fn sample_attachment() -> JiraAttachment {
        JiraAttachment {
            id: "20001".into(),
            filename: "screenshot.png".into(),
            author: "alice".into(),
            created: "2025-07-01T12:00:00.000+0000".into(),
            size: 1_048_576,
            mime_type: "image/png".into(),
            content_url: "https://jira.example.com/attachment/20001/screenshot.png".into(),
        }
    }

    fn make_comment(id: &str, author: &str, created: &str, body: &str) -> JiraComment {
        JiraComment {
            id: id.into(),
            author: author.into(),
            author_key: author.into(),
            body: body.into(),
            created: created.into(),
            updated: created.into(),
        }
    }

    /// Helper: parse + diff against current upstream, mirroring what the
    /// removed `parse_editor_output` returned.
    fn diff_buffer(
        node: &JiraIssueNode,
        text: &str,
    ) -> std::result::Result<ChangeSet, Vec<FieldError>> {
        let mut parsed = node.parse_3b(text)?;
        let tables = build_slug_tables(&node.cache);
        let mut errors: Vec<FieldError> = Vec::new();
        resolve_slugs_inplace(&mut parsed, &tables, &mut errors);
        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(node.diff_against_current(&parsed, node.detail_now()))
    }

    #[test]
    fn template_3b_layout() {
        let node = test_node(sample_detail());
        let template = node.render_3b(
            &["summary".into()],
            node.detail_now(),
            None,
            None,
            &build_slug_tables(&node.cache),
        );

        // Editable section comes first.
        assert!(template.contains("summary: Fix login bug"));

        // 3b markers, in order.
        let dash_pos = template.find("\n---\n").expect("missing --- marker");
        let eq_pos = template.find("\n===\n").expect("missing === marker");
        assert!(dash_pos < eq_pos, "--- must come before ===");

        assert!(template.contains("number: PROJ-42"));
        assert!(template.contains("type: Bug"));
        assert!(template.contains("status: In Progress"));
        assert!(template.contains("priority: High"));
        assert!(template.contains("assignee: alice"));
        // Several fix versions render as one comma-separated read-only line.
        assert!(template.contains("fix_versions: 1.2.0, 1.3.0"));

        // Body after `===`.
        assert!(template.contains("The login form crashes on submit."));
    }

    /// The read-only block is informational: re-saving a rendered buffer
    /// unchanged must not diff. Guards the newest read-only key specifically —
    /// a parser that took `fix_versions` for an editable field would turn every
    /// plain save into a phantom field change.
    #[test]
    fn read_only_fix_versions_line_does_not_diff() {
        let node = test_node(sample_detail());
        let template = node.render_3b(
            &["summary".into()],
            node.detail_now(),
            None,
            None,
            &build_slug_tables(&node.cache),
        );
        assert!(template.contains("fix_versions: "), "line must be rendered");

        let output = diff_buffer(&node, &template).unwrap();
        assert_eq!(
            output.metadata_changes.len(),
            0,
            "{:?}",
            output.metadata_changes
        );
    }

    #[test]
    fn template_multiple_editable_fields() {
        let node = test_node(sample_detail());
        let template = node.render_3b(
            &["summary".into(), "assignee".into()],
            node.detail_now(),
            None,
            None,
            &build_slug_tables(&node.cache),
        );

        assert!(template.contains("summary: Fix login bug"));
        // Assignee renders as a `uu_…` slug derived from display_name.
        assert!(template.contains("assignee: uu_alice"));
        // Editable fields move *out* of the read-only section.
        let after_dash = template.split("\n---\n").nth(1).unwrap_or("");
        assert!(!after_dash.contains("assignee:"));
    }

    #[test]
    fn diff_unchanged() {
        let node = test_node(sample_detail());
        let text = "summary: Fix login bug\n\
                    ---\n\
                    number: PROJ-42\n\
                    ===\n\
                    \n\
                    The login form crashes on submit.";

        let output = diff_buffer(&node, text).unwrap();
        assert_eq!(output.metadata_changes.len(), 0);
        assert!(output.content.is_none());
    }

    #[test]
    fn diff_changed_summary() {
        let node = test_node(sample_detail());
        let text = "summary: Updated title\n\
                    ---\n\
                    ===\n\
                    \n\
                    The login form crashes on submit.";

        let output = diff_buffer(&node, text).unwrap();
        assert_eq!(output.metadata_changes[0].0, "summary");
        assert_eq!(output.metadata_changes[0].1, "Updated title");
        assert!(output.content.is_none());
    }

    #[test]
    fn diff_changed_body() {
        let node = test_node(sample_detail());
        let text = "summary: Fix login bug\n\
                    ---\n\
                    ===\n\
                    \n\
                    New description with more details.";

        let output = diff_buffer(&node, text).unwrap();
        let body = String::from_utf8(output.content.unwrap()).unwrap();
        assert_eq!(body, "New description with more details.");
    }

    /// With a populated status table the editable `status` field renders as an
    /// `ss_…` slug and the CACHE legend lists the reachable statuses — the same
    /// slug treatment labels and users already get.
    #[test]
    fn status_renders_as_slug_with_cache_legend() {
        let node = test_node(sample_detail());
        let mut tables = build_slug_tables(&node.cache);
        tables.statuses = build_status_table(["Done", "In Review"], &node.detail_now().status);
        let template = node.render_3b(
            &["summary".into(), "status".into()],
            node.detail_now(),
            None,
            None,
            &tables,
        );

        // Current status "In Progress" → normalized slug in the editable section.
        assert!(
            template.contains("status: ss_in_progress"),
            "got: {template}"
        );
        // Reachable statuses listed in the CACHE legend.
        assert!(template.contains("# statuses: "), "got: {template}");
        assert!(template.contains("ss_done"), "got: {template}");
        assert!(template.contains("ss_in_review"), "got: {template}");
    }

    /// A changed status routes to `ChangeSet::status_change` (→ workflow
    /// transition), never to `metadata_changes` (which feeds the field PUT that
    /// rejects `status`).
    #[test]
    fn diff_status_change_routes_to_status_change() {
        let node = test_node(sample_detail());
        let text = "summary: Fix login bug\n\
                    status: Done\n\
                    ---\n\
                    ===\n\
                    \n\
                    The login form crashes on submit.";

        let output = diff_buffer(&node, text).unwrap();
        assert!(
            output.metadata_changes.iter().all(|(k, _)| k != "status"),
            "status must not be a metadata change: {:?}",
            output.metadata_changes
        );
        assert_eq!(output.status_change.as_deref(), Some("Done"));
    }

    /// An unchanged status leaves `status_change` empty — no spurious transition.
    #[test]
    fn diff_unchanged_status_no_transition() {
        let node = test_node(sample_detail());
        let text = "summary: Fix login bug\n\
                    status: In Progress\n\
                    ---\n\
                    ===\n\
                    \n\
                    The login form crashes on submit.";

        let output = diff_buffer(&node, text).unwrap();
        assert!(output.status_change.is_none());
    }

    #[test]
    fn diff_strips_inline_comments() {
        let node = test_node(sample_detail());
        // Trailing `# …` is dropped by the parser.
        let text = "summary: Updated title  # tweaked the wording\n\
                    ---\n\
                    ===\n\
                    \n\
                    The login form crashes on submit.";

        let output = diff_buffer(&node, text).unwrap();
        assert_eq!(output.metadata_changes[0].0, "summary");
        assert_eq!(output.metadata_changes[0].1, "Updated title");
    }

    #[test]
    fn parse_3b_missing_markers_errors() {
        let node = test_node(sample_detail());
        let text = "summary: Fix\nThe login form...";
        match node.parse_3b(text) {
            Ok(_) => panic!("expected parse error"),
            Err(errs) => {
                let msg = errs
                    .iter()
                    .map(|e| e.message.clone())
                    .collect::<Vec<_>>()
                    .join(" | ");
                assert!(msg.contains("---") || msg.contains("==="), "got: {msg}");
            }
        }
    }

    #[test]
    fn validate_3b_rejects_unknown_keys() {
        let node = test_node(sample_detail());
        let text = "summary: ok\nbogus: surprise\n---\n===\n\nbody";
        let parsed = node.parse_3b(text).unwrap();
        let errs = node.validate_3b(&parsed, &["summary".into()]);
        assert!(
            errs.iter().any(|e| e.message.contains("bogus")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn validate_3b_requires_summary_when_editable() {
        let node = test_node(sample_detail());
        let text = "summary:   \n---\n===\n\nbody";
        let parsed = node.parse_3b(text).unwrap();
        let errs = node.validate_3b(&parsed, &["summary".into()]);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("summary must not be empty"))
        );
    }

    #[test]
    fn restore_blanked_editable_recovers_summary() {
        let node = test_node(sample_detail());
        let original = "summary: Fix login bug\n---\nstatus: Open\n===\n\nBody";
        let user = "summary:   \n---\nstatus: Open\n===\n\nBody";
        let out = node.restore_blanked_editable(
            user,
            Some(original),
            &["summary".into()],
            node.detail_now(),
        );
        assert!(out.contains("summary: Fix login bug"));
        assert!(out.contains("Body"));
    }

    #[test]
    fn restore_blanked_editable_is_noop_when_user_kept_value() {
        let node = test_node(sample_detail());
        let original = "summary: Fix login bug\n---\n===\n\nOld body";
        let user = "summary: Updated title\n---\n===\n\nNew body";
        let out = node.restore_blanked_editable(
            user,
            Some(original),
            &["summary".into()],
            node.detail_now(),
        );
        assert_eq!(out, user);
    }

    #[test]
    fn restore_blanked_editable_is_noop_without_original() {
        let node = test_node(sample_detail());
        let user = "summary:   \n---\n===\n\nbody";
        let out = node.restore_blanked_editable(user, None, &["summary".into()], node.detail_now());
        assert_eq!(out, user);
    }

    #[test]
    fn render_with_errors_prepends_banner() {
        let node = test_node(sample_detail());
        let original = "summary: x\n---\n===\n\nbody";
        let errors = vec![FieldError {
            message: "summary must not be empty".into(),
        }];
        let out = node.render_with_errors(original, &errors);
        assert!(out.starts_with(ERROR_BANNER_START));
        assert!(out.contains("# • summary must not be empty"));
        assert!(out.contains("summary: x"));
    }

    #[test]
    fn render_with_errors_does_not_stack_banner() {
        let node = test_node(sample_detail());
        let original = "summary: x\n---\n===\n\nbody";
        let errors = vec![FieldError {
            message: "first".into(),
        }];
        let once = node.render_with_errors(original, &errors);
        let twice = node.render_with_errors(&once, &errors);
        assert_eq!(once, twice, "banners should not stack on repeated reopens");
    }

    /// Sanity check on the diffy-based merge driving `handle_conflict`:
    /// disjoint changes (different lines) merge cleanly without markers.
    #[test]
    fn diffy_merge_disjoint_lines_clean() {
        let ancestor = "summary: Original\n---\nstatus: Open\n===\n\nOriginal body\n";
        let ours = "summary: User-changed\n---\nstatus: Open\n===\n\nOriginal body\n";
        let theirs = "summary: Original\n---\nstatus: Closed\n===\n\nOriginal body\n";
        let mut opts = diffy::MergeOptions::new();
        opts.set_conflict_style(diffy::ConflictStyle::Merge);
        let merged = opts
            .merge(ancestor, ours, theirs)
            .expect("disjoint merge should succeed");
        assert!(merged.contains("summary: User-changed"));
        assert!(merged.contains("status: Closed"));
        assert!(!merged.contains("<<<<<<<"));
    }

    /// Both sides changing the same line produces a `<<<<<<< ours` conflict
    /// region — that's the "real conflict" path the user must resolve.
    #[test]
    fn diffy_merge_same_line_conflict() {
        let ancestor = "summary: Original\n---\n===\n\nbody\n";
        let ours = "summary: User\n---\n===\n\nbody\n";
        let theirs = "summary: Upstream\n---\n===\n\nbody\n";
        let mut opts = diffy::MergeOptions::new();
        opts.set_conflict_style(diffy::ConflictStyle::Merge);
        let conflict = opts
            .merge(ancestor, ours, theirs)
            .expect_err("should conflict");
        assert!(conflict.contains("<<<<<<<"));
        assert!(conflict.contains("======="));
        assert!(conflict.contains(">>>>>>>"));
        assert!(conflict.contains("summary: User"));
        assert!(conflict.contains("summary: Upstream"));
    }

    /// Body changes on different lines should auto-merge — this is the
    /// scenario that motivated swapping in diffy (the previous custom
    /// per-field merge treated the body as a single atom).
    #[test]
    fn diffy_merge_body_disjoint_lines_clean() {
        let ancestor = "summary: x\n---\n===\n\nline1\nline2\nline3\n";
        let ours = "summary: x\n---\n===\n\nline1-edited\nline2\nline3\n";
        let theirs = "summary: x\n---\n===\n\nline1\nline2\nline3-edited\n";
        let mut opts = diffy::MergeOptions::new();
        opts.set_conflict_style(diffy::ConflictStyle::Merge);
        let merged = opts
            .merge(ancestor, ours, theirs)
            .expect("disjoint body lines should merge");
        assert!(merged.contains("line1-edited"));
        assert!(merged.contains("line3-edited"));
        assert!(!merged.contains("<<<<<<<"));
    }

    /// Regression: when the upstream body has been reformatted (e.g. blank
    /// line added between every paragraph) and the user only changes the
    /// last line, diffy used to collapse the whole document into a single
    /// conflict region. Normalizing blank-line runs before merging yields
    /// the expected localized last-line conflict.
    #[test]
    fn diffy_merge_tolerates_server_reformatted_body() {
        let ancestor = "summary: x\n---\n===\n\npara1\npara2\npara3\nTest\n";
        let ours = "summary: x\n---\n===\n\npara1\npara2\npara3\nToast\n";
        // theirs: every paragraph followed by a blank line, plus the same
        // last-line edit.
        let theirs = "summary: x\n---\n===\n\npara1\n\npara2\n\npara3\n\nUpstream\n";

        let ancestor_n = normalize_blank_lines(ancestor);
        let ours_n = normalize_blank_lines(ours);
        let theirs_n = normalize_blank_lines(theirs);

        let mut opts = diffy::MergeOptions::new();
        opts.set_conflict_style(diffy::ConflictStyle::Merge);
        let conflict = opts
            .merge(&ancestor_n, &ours_n, &theirs_n)
            .expect_err("last line should conflict");

        // The conflict region should be small — only the last-line change,
        // not the entire body.
        assert!(conflict.contains("Toast"));
        assert!(conflict.contains("Upstream"));
        // Earlier paragraphs are unchanged in all three after normalization,
        // so they shouldn't appear inside any conflict block.
        let conflict_block_lines: Vec<&str> = conflict
            .lines()
            .skip_while(|l| !l.starts_with("<<<<<<<"))
            .take_while(|l| !l.starts_with(">>>>>>>"))
            .collect();
        let block = conflict_block_lines.join("\n");
        assert!(!block.contains("para1"));
        assert!(!block.contains("para2"));
    }

    #[test]
    fn parse_3b_rejects_unresolved_conflict_markers() {
        let node = test_node(sample_detail());
        // Use diffy's default labels (ours / theirs) to mirror what
        // `handle_conflict` actually emits.
        let text = "<<<<<<< ours\nsummary: User\n=======\nsummary: Upstream\n>>>>>>> theirs\n---\n===\n\nbody";
        let err = node.parse_3b(text).unwrap_err();
        assert!(
            err.iter()
                .any(|e| e.message.contains("unresolved conflict marker")),
            "expected conflict-marker rejection, got: {err:?}",
        );
    }

    #[test]
    fn editor_roundtrip() {
        let node = test_node(sample_detail());
        let fields = vec!["summary".into(), "assignee".into()];

        let template = node.render_3b(
            &fields,
            node.detail_now(),
            None,
            None,
            &build_slug_tables(&node.cache),
        );
        let output = diff_buffer(&node, &template).unwrap();

        // Unchanged template → no content change, no metadata changes
        assert!(output.content.is_none());
        assert_eq!(output.metadata_changes.len(), 0);
    }

    #[tokio::test]
    async fn issue_node_has_children_types() {
        let adapter = crate::adapter::test_adapter().await;
        let node = test_node(sample_detail());
        let types = not_yet_done_content::children::child_types(&adapter, &node);
        assert_eq!(types.len(), 2);
        assert_eq!(types[0].type_id, "jira:comment");
        assert_eq!(types[1].type_id, "jira:attachment");
    }

    #[test]
    fn issue_node_declares_actions() {
        let actions = issue_actions();
        let ids: Vec<&str> = actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "edit_full",
                "edit_markdown",
                "to_markdown",
                "from_markdown",
                "edit_with_comments",
                "transition",
                "create_comment",
                "toggle_watch",
                "toggle-bookmark",
                "remove-bookmark",
                "open_in_browser",
                "clone",
                "link",
                "download-attachments",
                "export-bundle",
                "export_workspace",
            ]
        );
        assert!(matches!(actions[0].input, InputSpec::Editor)); // edit_full
        assert!(matches!(actions[1].input, InputSpec::Editor)); // edit_markdown
        assert!(matches!(actions[2].input, InputSpec::None)); // to_markdown
        assert!(matches!(actions[3].input, InputSpec::Editor)); // from_markdown
        assert!(matches!(actions[4].input, InputSpec::Editor)); // edit_with_comments
        assert!(matches!(actions[5].input, InputSpec::Picker)); // transition
        assert!(matches!(actions[6].input, InputSpec::Editor)); // create_comment
        assert!(matches!(actions[7].input, InputSpec::None)); // toggle_watch
        assert!(matches!(actions[8].input, InputSpec::None)); // toggle-bookmark
        assert!(matches!(actions[9].input, InputSpec::None)); // remove-bookmark
        assert!(matches!(actions[10].input, InputSpec::None)); // open_in_browser
        assert!(matches!(actions[11].input, InputSpec::Editor)); // clone
        assert!(matches!(actions[12].input, InputSpec::Form { .. })); // link
        assert!(matches!(actions[13].input, InputSpec::Form { .. })); // download-attachments
        assert!(matches!(actions[14].input, InputSpec::None)); // export-bundle
        assert!(matches!(actions[15].input, InputSpec::Form { .. })); // export_workspace
    }

    #[test]
    fn comment_header_round_trip() {
        let c = make_comment("10042", "bob", "2025-06-01T10:00:00.000+0000", "x");
        let header = render_comment_header(&c);
        assert_eq!(header, "--- @bob 2025-06-01T10:00 (id=10042) ---");
        assert_eq!(parse_comment_header_id(&header), Some("10042"));
    }

    #[test]
    fn parse_comment_header_id_rejects_non_headers() {
        assert_eq!(parse_comment_header_id("---"), None);
        assert_eq!(parse_comment_header_id("--- add ---"), None);
        assert_eq!(parse_comment_header_id("just a line"), None);
        assert_eq!(parse_comment_header_id("--- bob (id=1) ---"), None); // missing @
    }

    #[test]
    fn is_delete_keyword_recognises_variants() {
        assert!(is_delete_keyword("del"));
        assert!(is_delete_keyword("DELETE"));
        assert!(is_delete_keyword("  del  "));
        assert!(is_delete_keyword("\n\ndel\n\n"));
        assert!(!is_delete_keyword("del me"));
        assert!(!is_delete_keyword("del\nbody"));
        assert!(!is_delete_keyword(""));
        assert!(!is_delete_keyword("delete the thing"));
    }

    #[test]
    fn render_with_comments_includes_each_comment_newest_first() {
        let node = test_node(sample_detail());
        let comments = vec![
            make_comment("1", "bob", "2025-05-01T10:00:00.000+0000", "old comment"),
            make_comment(
                "2",
                "alice",
                "2025-06-01T10:00:00.000+0000",
                "newer comment",
            ),
        ];
        let buf = node.render_with_comments(
            &edit_full_fields(),
            node.detail_now(),
            &comments,
            &build_slug_tables(&node.cache),
        );

        let pos2 = buf.find("(id=2)").expect("id=2 marker");
        let pos1 = buf.find("(id=1)").expect("id=1 marker");
        assert!(pos2 < pos1, "newest comment must come first");
        assert!(buf.contains("old comment"));
        assert!(buf.contains("newer comment"));
    }

    #[test]
    fn parse_with_comments_splits_header_and_blocks() {
        let node = test_node(sample_detail());
        let comments = vec![
            make_comment("10", "alice", "2025-06-01T10:00:00.000+0000", "first body"),
            make_comment("20", "bob", "2025-06-02T10:00:00.000+0000", "second body"),
        ];
        let buf = node.render_with_comments(
            &edit_full_fields(),
            node.detail_now(),
            &comments,
            &build_slug_tables(&node.cache),
        );

        let parsed = node
            .parse_with_comments(&buf)
            .expect("parse should succeed");
        assert_eq!(parsed.blocks.len(), 2);
        assert!(matches!(&parsed.blocks[0].kind, CommentBlockKind::Existing(id) if id == "20"));
        assert_eq!(parsed.blocks[0].body, "second body");
        assert!(matches!(&parsed.blocks[1].kind, CommentBlockKind::Existing(id) if id == "10"));
        assert_eq!(parsed.blocks[1].body, "first body");

        // Header survives.
        assert_eq!(
            parsed.header.editable.get("summary").map(String::as_str),
            Some("Fix login bug")
        );
        assert_eq!(parsed.header.body, "The login form crashes on submit.");
    }

    #[test]
    fn parse_with_comments_picks_up_add_blocks() {
        let node = test_node(sample_detail());
        let buf = node.render_with_comments(
            &edit_full_fields(),
            node.detail_now(),
            &[],
            &build_slug_tables(&node.cache),
        );
        // Insert the new-comment block before the trailing CACHE section.
        let buf = match buf.find(CACHE_MARKER) {
            Some(pos) => format!(
                "{}--- add ---\n\nthis is a brand new comment\n\n{}",
                &buf[..pos],
                &buf[pos..],
            ),
            None => format!("{buf}--- add ---\n\nthis is a brand new comment\n"),
        };

        let parsed = node
            .parse_with_comments(&buf)
            .expect("parse should succeed");
        assert_eq!(parsed.blocks.len(), 1);
        assert!(matches!(parsed.blocks[0].kind, CommentBlockKind::Add));
        assert_eq!(parsed.blocks[0].body, "this is a brand new comment");
    }

    #[test]
    fn render_with_comments_includes_empty_add_placeholder() {
        let node = test_node(sample_detail());
        let buf = node.render_with_comments(
            &edit_full_fields(),
            node.detail_now(),
            &[],
            &build_slug_tables(&node.cache),
        );
        assert!(
            buf.contains(ADD_COMMENT_MARKER),
            "render must include an `--- add ---` placeholder"
        );
    }

    #[test]
    fn parse_with_comments_drops_empty_add_placeholder() {
        let node = test_node(sample_detail());
        // Default render now includes an empty `--- add ---` placeholder —
        // it must not appear in the parsed blocks.
        let buf = node.render_with_comments(
            &edit_full_fields(),
            node.detail_now(),
            &[],
            &build_slug_tables(&node.cache),
        );
        let parsed = node
            .parse_with_comments(&buf)
            .expect("parse should succeed");
        assert!(
            parsed.blocks.is_empty(),
            "empty add placeholder must be dropped, got {:?}",
            parsed.blocks
        );
    }

    #[test]
    fn parse_with_comments_drops_whitespace_only_add_block() {
        let node = test_node(sample_detail());
        let buf = node.render_with_comments(
            &edit_full_fields(),
            node.detail_now(),
            &[],
            &build_slug_tables(&node.cache),
        );
        // Insert a second add block that contains only whitespace.
        let buf = match buf.find(CACHE_MARKER) {
            Some(pos) => format!("{}--- add ---\n\n   \n\t\n\n{}", &buf[..pos], &buf[pos..]),
            None => format!("{buf}--- add ---\n\n   \n"),
        };
        let parsed = node
            .parse_with_comments(&buf)
            .expect("parse should succeed");
        assert!(parsed.blocks.is_empty());
    }

    #[test]
    fn parse_with_comments_rejects_malformed_header() {
        let node = test_node(sample_detail());
        // No --- marker between editable and read-only sections.
        let buf = "summary: x\nnumber: PROJ-42\n=== \n\nbody\n";
        assert!(node.parse_with_comments(buf).is_err());
    }

    // ───────────── edit_markdown: canonical ⇄ markdown comments ─────────────

    #[test]
    fn edit_markdown_renders_comment_section_and_headings() {
        let node = test_node(sample_detail());
        let comments = vec![
            make_comment("10", "alice", "2025-06-01T10:00:00.000+0000", "first body"),
            make_comment("20", "bob", "2025-06-02T10:00:00.000+0000", "second body"),
        ];
        let canonical = node.render_with_comments(
            &edit_full_fields(),
            node.detail_now(),
            &comments,
            &build_slug_tables(&node.cache),
        );
        let md = comments_canonical_to_md(&canonical);

        // Header rendered as GFM tables, body/comments below the divider.
        assert!(
            md.contains("| Field | Value |"),
            "header should be a GFM table"
        );
        assert!(md.contains("## Comments <!-- jira comments section -->"));
        assert!(md.contains("### Add Comment <!-- jira comment add -->"));
        assert!(md.contains("<!-- jira comment id=10 -->"));
        assert!(md.contains("<!-- jira comment id=20 -->"));
        assert!(md.contains("first body"));
        assert!(md.contains("second body"));

        // Newest comment (id=20) still comes first.
        let pos20 = md.find("id=20").unwrap();
        let pos10 = md.find("id=10").unwrap();
        assert!(pos20 < pos10, "newest comment must come first");
    }

    #[test]
    fn edit_markdown_round_trip_preserves_header_and_comments() {
        let node = test_node(sample_detail());
        let comments = vec![
            make_comment("10", "alice", "2025-06-01T10:00:00.000+0000", "first body"),
            make_comment("20", "bob", "2025-06-02T10:00:00.000+0000", "second body"),
        ];
        let canonical = node.render_with_comments(
            &edit_full_fields(),
            node.detail_now(),
            &comments,
            &build_slug_tables(&node.cache),
        );
        let md = comments_canonical_to_md(&canonical);
        let back = comments_md_to_canonical(&md);
        let parsed = node
            .parse_with_comments(&back)
            .expect("re-parse should succeed");

        // Header survives.
        assert_eq!(
            parsed.header.editable.get("summary").map(String::as_str),
            Some("Fix login bug")
        );
        assert_eq!(parsed.header.body, "The login form crashes on submit.");

        // Both comments survive, newest first, bodies intact.
        assert_eq!(parsed.blocks.len(), 2);
        assert!(matches!(&parsed.blocks[0].kind, CommentBlockKind::Existing(id) if id == "20"));
        assert_eq!(parsed.blocks[0].body, "second body");
        assert!(matches!(&parsed.blocks[1].kind, CommentBlockKind::Existing(id) if id == "10"));
        assert_eq!(parsed.blocks[1].body, "first body");
    }

    #[test]
    fn edit_markdown_add_block_round_trips() {
        let node = test_node(sample_detail());
        let canonical = node.render_with_comments(
            &edit_full_fields(),
            node.detail_now(),
            &[],
            &build_slug_tables(&node.cache),
        );
        let md = comments_canonical_to_md(&canonical);
        // User fills in the empty add placeholder.
        let md = md.replace(
            "### Add Comment <!-- jira comment add -->",
            "### Add Comment <!-- jira comment add -->\n\nbrand new md comment",
        );
        let back = comments_md_to_canonical(&md);
        let parsed = node
            .parse_with_comments(&back)
            .expect("re-parse should succeed");

        assert_eq!(parsed.blocks.len(), 1);
        assert!(matches!(parsed.blocks[0].kind, CommentBlockKind::Add));
        assert_eq!(parsed.blocks[0].body, "brand new md comment");
    }

    #[test]
    fn edit_markdown_del_keyword_survives_round_trip() {
        let node = test_node(sample_detail());
        let comments = vec![make_comment(
            "10",
            "alice",
            "2025-06-01T10:00:00.000+0000",
            "first body",
        )];
        let canonical = node.render_with_comments(
            &edit_full_fields(),
            node.detail_now(),
            &comments,
            &build_slug_tables(&node.cache),
        );
        let md = comments_canonical_to_md(&canonical);
        // User replaces the comment body with the sole-body delete keyword.
        let md = md.replace("first body", "del");
        let back = comments_md_to_canonical(&md);
        let parsed = node
            .parse_with_comments(&back)
            .expect("re-parse should succeed");

        assert_eq!(parsed.blocks.len(), 1);
        assert!(matches!(&parsed.blocks[0].kind, CommentBlockKind::Existing(id) if id == "10"));
        assert!(is_delete_keyword(&parsed.blocks[0].body));
    }

    #[test]
    fn edit_markdown_without_comments_degrades_to_body_only() {
        // A plain 3b buffer (no comment markers) must round-trip through the
        // md transforms unchanged modulo markup, exercising the fallback paths.
        let node = test_node(sample_detail());
        let canonical = node.render_3b(
            &edit_full_fields(),
            node.detail_now(),
            None,
            None,
            &build_slug_tables(&node.cache),
        );
        let md = comments_canonical_to_md(&canonical);
        assert!(!md.contains("<!-- jira comments section -->"));
        assert!(md.contains("| Field | Value |"), "header still tabled");

        let back = comments_md_to_canonical(&md);
        let parsed = node.parse_3b(&back).expect("re-parse should succeed");
        assert_eq!(parsed.body, "The login form crashes on submit.");
    }

    #[test]
    fn edit_markdown_add_comment_before_paneled_foreign_comment() {
        // Repro: user writes a NEW comment under the Add heading, and the next
        // existing (foreign) comment's body contains {panel} blocks. The typed
        // text must stay in the Add block and must NOT leak into the existing
        // comment (which would surface as an illegal "editing someone else's
        // comment" on save).
        let node = test_node(sample_detail());
        let foreign_body = "{panel:title=Constraints}\nfirst constraint\nsecond constraint\n{panel}\n\n{panel:title=Next Steps}\nship it\n{panel}";
        let comments = vec![make_comment(
            "555",
            "someone-else",
            "2025-06-01T10:00:00.000+0000",
            foreign_body,
        )];
        let canonical = node.render_with_comments(
            &edit_full_fields(),
            node.detail_now(),
            &comments,
            &build_slug_tables(&node.cache),
        );
        let md = comments_canonical_to_md(&canonical);

        // User types a reply into the empty Add placeholder.
        let typed = "@some_user thanks, this is my brand new reply\nwith a second line";
        let md = md.replace(
            "### Add Comment <!-- jira comment add -->",
            &format!("### Add Comment <!-- jira comment add -->\n\n{typed}"),
        );

        let back = comments_md_to_canonical(&md);
        let parsed = node
            .parse_with_comments(&back)
            .expect("re-parse should succeed");

        assert_eq!(
            parsed.blocks.len(),
            2,
            "expected one Add + one Existing block, got {:?}",
            parsed.blocks
        );
        let add = parsed
            .blocks
            .iter()
            .find(|b| matches!(b.kind, CommentBlockKind::Add))
            .expect("add block present");
        assert!(
            add.body.contains("brand new reply"),
            "add block must hold the typed reply, got {:?}",
            add.body
        );

        let existing = parsed
            .blocks
            .iter()
            .find(|b| matches!(&b.kind, CommentBlockKind::Existing(id) if id == "555"))
            .expect("existing comment 555 present");
        assert!(
            !existing.body.contains("brand new reply"),
            "typed reply leaked into the foreign comment body: {:?}",
            existing.body
        );
        // And the foreign comment's body must round-trip back to its wiki form.
        assert!(
            existing.body.contains("{panel:title=Constraints}")
                && existing.body.contains("first constraint"),
            "foreign panel body must survive the round-trip, got {:?}",
            existing.body
        );
    }

    #[test]
    fn edit_markdown_filling_add_does_not_perturb_other_comments() {
        // The real-world repro: a long comment history where several foreign
        // comments carry titled panels. The user only fills the Add block.
        // Parsing the "opened" buffer and the "saved" buffer must yield
        // byte-identical bodies for every EXISTING comment — otherwise the
        // save-time classifier flags an untouched foreign comment as edited
        // ("cannot edit comment N: not authored by you").
        let node = test_node(sample_detail());
        let paneled = "{panel:title=Constraints|titleBGColor=#f2f2f2}\nsome constraint text\n{panel}\n\n{panel:title=Approach|titleBGColor=#f2f2f2}\n- step one\n- step two\n{panel}\n\n{panel:title=Next Steps|titleBGColor=#f2f2f2}\n{panel}";
        let comments = vec![
            make_comment(
                "555",
                "someone-else",
                "2026-01-05T10:00:00.000+0000",
                paneled,
            ),
            make_comment("444", "me", "2026-01-04T10:00:00.000+0000", ""),
            make_comment(
                "333",
                "someone-else",
                "2026-01-03T10:00:00.000+0000",
                "*Ergebnis:* \nfollow-up line",
            ),
        ];
        let canonical = node.render_with_comments(
            &edit_full_fields(),
            node.detail_now(),
            &comments,
            &build_slug_tables(&node.cache),
        );
        let opened_md = comments_canonical_to_md(&canonical);
        let saved_md = opened_md.replace(
            "### Add Comment <!-- jira comment add -->",
            "### Add Comment <!-- jira comment add -->\n\n@some_user here is my reply\nspanning two lines",
        );

        let opened = node
            .parse_with_comments(&comments_md_to_canonical(&opened_md))
            .expect("opened parses");
        let saved = node
            .parse_with_comments(&comments_md_to_canonical(&saved_md))
            .expect("saved parses");

        for id in ["555", "444", "333"] {
            let ob = opened
                .blocks
                .iter()
                .find(|b| matches!(&b.kind, CommentBlockKind::Existing(x) if x == id))
                .map(|b| b.body.clone());
            let sb = saved
                .blocks
                .iter()
                .find(|b| matches!(&b.kind, CommentBlockKind::Existing(x) if x == id))
                .map(|b| b.body.clone());
            assert_eq!(
                ob, sb,
                "comment {id} body changed merely by filling the Add block:\n opened={ob:?}\n saved={sb:?}"
            );
        }
    }

    // Quick smoke: cross-module helpers / nodes still wire up correctly.
    #[test]
    fn comment_node_sample_round_trip() {
        // Sanity check: importing JiraCommentNode from the comment submodule
        // and constructing it from the same client/comment fixtures.
        let node = JiraCommentNode::new(test_client(), sample_comment(), "PROJ-42".into());
        assert_eq!(node.id(), "PROJ-42/comment/10042");
    }

    #[test]
    fn attachment_node_sample_round_trip() {
        let node = JiraAttachmentNode::new(test_client(), sample_attachment(), "PROJ-42".into());
        assert_eq!(node.id(), "PROJ-42/attachment/20001");
    }
}
