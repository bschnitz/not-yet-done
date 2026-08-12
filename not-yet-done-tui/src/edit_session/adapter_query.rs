//! Edit session for the adapter-native SQL editor (the `Q` keybind on a
//! level that declares `node_scripts: true`).
//!
//! Buffer layout: a free scratch area, then a marker line
//! (`-- ▼ THIS SQL WILL BE EXECUTED ON SAVE ▼`), then the SQL the
//! live-apply hook executes on every `:w`. On error a comment-banner
//! block is prepended on reopen and stripped on the next parse. The
//! layout itself is the shared protocol in
//! [`not_yet_done_content::script_buffer`], not this session's invention.
//!
//! The session knows the owning node only as an opaque `node_id`. Where
//! the buffer is persisted, what a fresh one contains, and which context
//! the query runs in are all asked of the adapter — via
//! [`ScriptStore::node_script_path`](not_yet_done_content::ScriptStore::node_script_path),
//! [`ScriptStore::default_node_script_body`](not_yet_done_content::ScriptStore::default_node_script_body)
//! and [`ContentAdapter::custom_query_context`]. That is what makes this
//! session work for any SQL-ish adapter instead of Postgres alone.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

pub use not_yet_done_content::script_buffer::parse_query_area;
use not_yet_done_content::script_buffer::{render_with_error, strip_error_banner};
use not_yet_done_content::{ContentAdapter, CustomQueryContext, PageRequest};

use crate::views::content_view::{CustomQueryRunState, PaneId};

use super::{CommitOutcome, EditSession, FollowUp, SessionScope};

/// Default page size used when the Q-editor runs a SELECT. Matches the
/// `page_size: 100` of the YAML Rows-child so paginated custom-query
/// results render at the same grain as the regular row drill-down.
pub const DEFAULT_PAGE_SIZE: u32 = 100;

pub struct AdapterQuerySession {
    adapter: Arc<dyn ContentAdapter>,
    /// Id of the node the scripts belong to, exactly as the adapter
    /// spelled it. Never parsed here.
    node_id: String,
    script: String,
    view_index: usize,
    pane_id: PaneId,
    template: String,
}

impl AdapterQuerySession {
    pub fn new(
        adapter: Arc<dyn ContentAdapter>,
        node_id: String,
        script: String,
        view_index: usize,
        pane_id: PaneId,
        initial_buffer: String,
    ) -> Self {
        Self {
            adapter,
            node_id,
            script,
            view_index,
            pane_id,
            template: initial_buffer,
        }
    }

    /// Convenience: read the adapter's persisted buffer for this node's
    /// implicit ("default") script, falling back to the adapter's
    /// template when nothing is stored yet.
    pub async fn open(
        adapter: Arc<dyn ContentAdapter>,
        node_id: String,
        view_index: usize,
        pane_id: PaneId,
    ) -> Self {
        let script = adapter
            .script_store()
            .map(|s| s.default_node_script_name().to_string())
            .unwrap_or_else(|| "default".to_string());
        Self::open_named(adapter, node_id, script, view_index, pane_id).await
    }

    /// Like [`open`] but for an explicitly named script in the same
    /// node namespace. Used by the Scripts subview.
    pub async fn open_named(
        adapter: Arc<dyn ContentAdapter>,
        node_id: String,
        script: String,
        view_index: usize,
        pane_id: PaneId,
    ) -> Self {
        let stored = match query_path(&adapter, &node_id, &script) {
            Some(path) => tokio::fs::read_to_string(&path).await.ok(),
            None => None,
        };
        let initial = stored.unwrap_or_else(|| default_body(&adapter, &node_id));
        Self::new(adapter, node_id, script, view_index, pane_id, initial)
    }

    /// Execution context for the query. The adapter derives whatever
    /// routing keys it needs (for Postgres: the target database) from the
    /// node id itself, so the host never has to know the id's shape.
    fn context(&self) -> CustomQueryContext {
        self.adapter
            .custom_query_context(&self.node_id)
            .with_page(PageRequest {
                offset: 0,
                limit: DEFAULT_PAGE_SIZE,
            })
    }

    async fn persist(&self, content: &str) {
        let Some(path) = query_path(&self.adapter, &self.node_id, &self.script) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let _ = tokio::fs::write(&path, content).await;
    }

    /// Run the query area against the adapter. Returns the parsed SQL
    /// alongside the execute result so callers can persist the same
    /// buffer they handed us. The adapter wraps SELECTs with
    /// `LIMIT/OFFSET` based on the [`CustomQueryContext::page`] we set
    /// here — non-SELECT or multi-statement queries fall through
    /// un-paginated and `result.page` is `None`.
    async fn run(
        &self,
        text: &str,
    ) -> (
        String,
        String,
        Result<not_yet_done_content::CustomQueryResult, String>,
    ) {
        let stripped = strip_error_banner(text).to_string();
        let sql = parse_query_area(&stripped).trim().to_string();
        if sql.is_empty() {
            return (stripped, sql, Err("query is empty".to_string()));
        }
        let ctx = self.context();
        let result = self
            .adapter
            .execute_custom_query(&sql, &ctx)
            .await
            .map_err(|e| e.to_string());
        (stripped, sql, result)
    }

    fn custom_query_state(&self, sql: &str) -> CustomQueryRunState {
        // `mode` is a placeholder — the pane re-derives it from its own
        // view config in `apply_custom_query_result` once the result
        // lands. We don't have a view-config reference here.
        // `cursor_id: None` because the editor's initial run always
        // goes through the LIMIT/OFFSET path (see `context()` above).
        CustomQueryRunState {
            query: sql.to_string(),
            node_id: self.node_id.clone(),
            mode: crate::config::view_config::PaginationMode::Server,
            cursor_id: None,
        }
    }
}

#[async_trait]
impl EditSession for AdapterQuerySession {
    fn template(&self) -> &str {
        &self.template
    }

    fn suffix(&self) -> &str {
        ".sql"
    }

    fn scope(&self) -> SessionScope {
        SessionScope::Content
    }

    fn label(&self) -> &str {
        "edit query"
    }

    async fn live_apply(&mut self, text: &str) -> Option<FollowUp> {
        let (stripped, sql, result) = self.run(text).await;
        match result {
            Ok(out) => {
                self.persist(&stripped).await;
                Some(FollowUp::ReplaceContentItems {
                    view_index: self.view_index,
                    pane_id: self.pane_id,
                    items: out.items,
                    status: out.status,
                    page: out.page,
                    custom_query: Some(self.custom_query_state(&sql)),
                })
            }
            Err(msg) => Some(FollowUp::SetQueryError(msg)),
        }
    }

    async fn commit(&mut self, text: &str) -> CommitOutcome {
        let (stripped, sql, result) = self.run(text).await;
        match result {
            Ok(out) => {
                self.persist(&stripped).await;
                let msg = match out.status.as_ref() {
                    Some(s) => s.clone(),
                    None => format!("{} row(s)", out.items.len()),
                };
                CommitOutcome::FollowUp(FollowUp::ReplaceContentItems {
                    view_index: self.view_index,
                    pane_id: self.pane_id,
                    items: out.items,
                    status: Some(msg),
                    page: out.page,
                    custom_query: Some(self.custom_query_state(&sql)),
                })
            }
            Err(msg) => CommitOutcome::Reopen {
                content: render_with_error(&stripped, &msg),
            },
        }
    }
}

/// On-disk path of the node-scoped script, asked of the adapter's
/// [`ScriptStore`](not_yet_done_content::ScriptStore) rather than
/// computed here — the layout below `queries/` is the adapter's.
///
/// `None` when the adapter has no script store, or does not recognise
/// this node as owning scripts.
fn query_path(adapter: &Arc<dyn ContentAdapter>, node_id: &str, script: &str) -> Option<PathBuf> {
    adapter.script_store()?.node_script_path(node_id, script)
}

/// Starter buffer for a script that doesn't exist yet — the adapter's,
/// so the seeded query already addresses the right object.
fn default_body(adapter: &Arc<dyn ContentAdapter>, node_id: &str) -> String {
    adapter
        .script_store()
        .map(|s| s.default_node_script_body(node_id))
        .unwrap_or_default()
}
