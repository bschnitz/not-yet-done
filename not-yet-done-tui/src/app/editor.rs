//! Editor orchestration: open external editors, process results, persist filters.

use std::sync::Arc;

use not_yet_done_core::entity::task::Model as Task;

use crate::edit_session::{CommitOutcome, EditSession, EditorSpawnContext, FollowUp};
use crate::query_filter;
use crate::tabs::TasksSubView;

/// Result of an asynchronously running commit. Sent from the spawned task
/// back to the main loop via `commit_tx`. `Reopen` carries the session back
/// so the App can re-attach it for the next round.
pub enum CommitMsg {
    Done { message: Option<String> },
    Cancelled { message: Option<String> },
    Reopen { session: Box<dyn EditSession>, content: String },
    FollowUp(FollowUp),
}

/// Decompose a Postgres table-node id (composite path produced by the
/// Postgres adapter, e.g. `<db>/schemas/<s>/tables/<t>`) into its three
/// addressing components. Returns `None` if the path doesn't match the
/// expected shape.
fn parse_postgres_table_path(id: &str) -> Option<(String, String, String)> {
    let parts: Vec<&str> = id.split('/').collect();
    if parts.len() == 5 && parts[1] == "schemas" && parts[3] == "tables" {
        Some((parts[0].to_string(), parts[2].to_string(), parts[4].to_string()))
    } else {
        None
    }
}

/// Directory for tracking scripts: <data_dir>/not_yet_done/tracking/scripts/
pub(crate) fn tracking_scripts_dir() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("not_yet_done")
        .join("tracking")
        .join("scripts")
}

use super::{App, is_in_subtree};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// What the main loop should do after handle_key returns.
///
/// `spawn_context` bundles every knob the session wants applied to the
/// editor child process (temp-file location/prefix, extra env vars).
/// See [`EditorSpawnContext`] for the full set.
pub enum EditorRequest {
    Inline {
        command: String,
        content: String,
        suffix: &'static str,
        spawn_context: EditorSpawnContext,
    },
    Launch {
        command: String,
        content: String,
        suffix: &'static str,
        spawn_context: EditorSpawnContext,
    },
    /// Run a script with full terminal control. TUI pauses, script runs,
    /// then optionally captures output for the editor.
    ///
    /// `child_env` mirrors the adapter-owned env shipped to editor
    /// children — same source ([`ContentAdapter::child_process_env`]),
    /// same lifecycle. Empty for non-content scripts (Tasks /
    /// Trackings).
    Script {
        script_path: String,
        stdin_json: String,
        capture: bool,
        child_env: std::collections::HashMap<String, String>,
    },
    None,
}

/// How a tracking script interacts with the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptMode {
    /// No terminal output. Stderr shown in notification bar.
    Background,
    /// Output captured and shown in editor afterwards.
    Capture,
    /// TUI yields terminal to script (interactive).
    Interactive,
    /// Interactive + output captured in editor.
    InteractiveCapture,
    /// Background-style execution that, after the script exits, reads
    /// `$NYD_OUTPUT_FILE` as JSON `{ "commands": [".."], … }` and feeds
    /// each entry to [`crate::app::App::execute_cmdline`]. Extra JSON
    /// keys are tolerated for forward-compatibility.
    Commands,
    /// Interactive variant of [`ScriptMode::Commands`] — TUI yields the
    /// terminal, and post-exit the output file is parsed as commands.
    InteractiveCommands,
}

impl ScriptMode {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim() {
            "background" => Some(Self::Background),
            "capture" => Some(Self::Capture),
            "interactive" => Some(Self::Interactive),
            "interactive+capture" => Some(Self::InteractiveCapture),
            "commands" => Some(Self::Commands),
            "interactive+commands" => Some(Self::InteractiveCommands),
            _ => None,
        }
    }

    pub fn is_interactive(self) -> bool {
        matches!(self, Self::Interactive | Self::InteractiveCapture | Self::InteractiveCommands)
    }

    pub fn captures_output(self) -> bool {
        matches!(self, Self::Capture | Self::InteractiveCapture)
    }

    /// True when the script's output file should be parsed as a JSON
    /// command list rather than displayed as text.
    pub fn emits_commands(self) -> bool {
        matches!(self, Self::Commands | Self::InteractiveCommands)
    }
}

/// Parse `# mode: <mode>` from the first few lines of a script.
pub fn parse_script_mode(content: &str) -> ScriptMode {
    for line in content.lines().take(10) {
        let trimmed = line.trim();
        let after = trimmed.strip_prefix('#')
            .or_else(|| trimmed.strip_prefix("//"))
            .or_else(|| trimmed.strip_prefix("--"))
            .or_else(|| trimmed.strip_prefix(";;"));
        if let Some(rest) = after {
            if let Some(mode_str) = rest.trim().strip_prefix("mode:") {
                if let Some(mode) = ScriptMode::from_str(mode_str) {
                    return mode;
                }
            }
        }
    }
    ScriptMode::Background
}

// ---------------------------------------------------------------------------
// Opening editors — per-action entry points
// ---------------------------------------------------------------------------

impl App {
    /// Open the editor for a polymorphic [`EditSession`]. All editor flows
    /// route through here.
    pub fn open_session(&mut self, session: Box<dyn EditSession>) -> EditorRequest {
        if self.editor_busy() {
            let msg = if self.commit_in_flight {
                "Saving previous edit, please wait…"
            } else {
                "Editor is already open"
            };
            self.notify(msg.to_string());
            return EditorRequest::None;
        }

        // Resolve the profile the session asked for (None → `default`)
        // while `session` is still borrowed, before it moves into App.
        let editor = self.config.editors.resolve(session.editor_profile());
        let command = if editor.command.is_empty() {
            std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string())
        } else {
            editor.command.clone()
        };

        // Snapshot template + suffix + spawn context before the session
        // is moved into App. The spawn context is read exactly once per
        // open: refreshing it across reopens would risk staleness if the
        // adapter's connection state changed mid-edit, and the editor
        // child is already running by then anyway.
        let content = session.template().to_string();
        // The dispatch enum demands a `&'static str`; leak a per-session copy.
        let suffix: &'static str = Box::leak(session.suffix().to_string().into_boxed_str());
        let spawn_context = session.spawn_context();

        self.pending_session = Some(session);
        self.last_editor_buffer = Some(content.clone());

        if editor.inline {
            EditorRequest::Inline { command, content, suffix, spawn_context }
        } else if editor.pause_tui {
            EditorRequest::Launch { command, content, suffix, spawn_context }
        } else {
            match not_yet_done_ratatui::open_editor_detached_in(
                Some(&command),
                &content,
                Some(suffix),
                spawn_context.tempfile_dir.as_deref(),
                spawn_context.tempfile_prefix,
                &spawn_context.child_env,
            ) {
                Ok(handle) => { self.detached_editor = Some(handle); }
                Err(e) => { self.notify_error(format!("Editor error: {e}")); }
            }
            EditorRequest::None
        }
    }

    pub fn open_editor_for_add(&mut self) -> EditorRequest {
        let parent_id = if self.tasks_view.sub_view() == TasksSubView::Tree {
            self.selected_task_id()
        } else {
            None
        };
        let session = crate::edit_session::TaskEditSession::create(
            Arc::clone(&self.task_service),
            Arc::clone(&self.tracking_repo),
            self.config.tracking.allow_parallel,
            self.tasks_view.state.task_rows.clone(),
            parent_id,
        );
        self.open_session(Box::new(session))
    }

    pub fn open_editor_for_edit(&mut self) -> EditorRequest {
        let Some(task) = self.selected_task() else {
            self.notify("No task selected".to_string());
            return EditorRequest::None;
        };
        let tracked_ids = self.get_tracked_task_ids();
        let is_tracked = tracked_ids.contains(&task.id);
        let session = crate::edit_session::TaskEditSession::edit(
            Arc::clone(&self.task_service),
            Arc::clone(&self.tracking_repo),
            self.config.tracking.allow_parallel,
            self.tasks_view.state.task_rows.clone(),
            task,
            is_tracked,
        );
        self.open_session(Box::new(session))
    }

    pub fn open_editor_for_restructure(&mut self) -> EditorRequest {
        let Some(task) = self.selected_task() else {
            self.notify("No task selected".to_string());
            return EditorRequest::None;
        };
        let subtree: Vec<Task> = self.tasks_view.state.task_rows.iter()
            .filter(|t| is_in_subtree(t, task.id, &self.tasks_view.state.task_rows))
            .cloned()
            .collect();
        let indent = self.config.editors.default.indent;
        let tracked_ids = self.get_tracked_task_ids();
        let content = crate::tree_edit::serialize_with_indent(&task, &subtree, indent, &tracked_ids);
        let session = crate::edit_session::RestructureSession::new(
            Arc::clone(&self.task_service),
            Arc::clone(&self.tracking_repo),
            self.config.tracking.allow_parallel,
            subtree,
            task.id,
            content,
        );
        self.open_session(Box::new(session))
    }

    /// Open the YAML query editor with a specific starting content. Used by the
    /// new query menu when editing an existing entry or creating a new one.
    /// For new entries the editor always starts from the empty template,
    /// never from the currently applied filter.
    pub fn open_editor_for_saved_query(
        &mut self,
        scope: &str,
        name: String,
        content: Option<String>,
        is_new: bool,
    ) -> EditorRequest {
        match scope {
            "tracking" => {
                let content = if is_new {
                    query_filter::tracking_template()
                } else {
                    content
                        .or_else(|| self.trackings_view.active_filter_json.clone())
                        .unwrap_or_else(query_filter::tracking_template)
                };
                let session = crate::edit_session::TrackingQueryFilterSession::new(name, is_new, content);
                self.open_session(Box::new(session))
            }
            _ => {
                let content = if is_new {
                    query_filter::template()
                } else {
                    content
                        .or_else(|| self.tasks_view.active_filter_json.clone())
                        .unwrap_or_else(query_filter::template)
                };
                let session = crate::edit_session::TaskQueryFilterSession::new(name, is_new, content);
                self.open_session(Box::new(session))
            }
        }
    }


    /// Open the Postgres scripts menu (the new `q` keybind on the
    /// `tables` subtab). Parses the selected table-node id, lists
    /// scripts under `<instance_data_dir>/queries/<db>/<schema>/<table>/`,
    /// and pushes the resulting entries into the content view's popup.
    pub fn open_postgres_scripts_menu(
        &mut self,
        view_index: usize,
        _pane_id: crate::views::content_view::PaneId,
        table_node_id: String,
    ) -> EditorRequest {
        let Some(adapter) = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone)
        else {
            self.notify("No adapter for this view".to_string());
            return EditorRequest::None;
        };
        if adapter.adapter_type() != "postgres" {
            self.notify(format!(
                "Scripts menu not implemented for adapter '{}'",
                adapter.adapter_type()
            ));
            return EditorRequest::None;
        }
        let Some((database, schema, table)) = parse_postgres_table_path(&table_node_id) else {
            self.notify(format!(
                "Cannot derive (database, schema, table) from '{table_node_id}'"
            ));
            return EditorRequest::None;
        };
        let instance_dir = adapter.instance_data_dir();
        let scope = crate::app::node_actions::postgres_table_scope(
            adapter.instance_id(),
            &database,
            &schema,
            &table,
        );
        let shortcut_repo = Arc::clone(&self.query_shortcut_repo);
        let (scripts, shortcut_map) = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let scripts = not_yet_done_postgres_adapter::query::list_scripts_in_table(
                    &instance_dir,
                    &database,
                    &schema,
                    &table,
                )
                .await;
                let shortcuts: std::collections::HashMap<String, String> = shortcut_repo
                    .list_by_scope(&scope)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|m| (m.name, m.shortcut))
                    .collect();
                (scripts, shortcuts)
            })
        });
        let scripts = match scripts {
            Ok(v) => v,
            Err(e) => {
                self.notify_error(format!("Failed to list scripts: {e}"));
                return EditorRequest::None;
            }
        };
        let entries: Vec<crate::components::query_menu::QueryMenuEntry> = scripts
            .into_iter()
            .map(|name| crate::components::query_menu::QueryMenuEntry {
                shortcut: shortcut_map.get(&name).cloned(),
                name,
                // Value field is unused for postgres scripts; keep
                // non-empty so the popup widget treats the row as
                // selectable.
                query: "<file>".to_string(),
                is_default: false,
            })
            .collect();
        if let Some(cv) = self.content_view_mut(view_index) {
            cv.open_postgres_scripts_popup(database, schema, table, entries);
        }
        EditorRequest::None
    }

    /// Run a Postgres script for `(db, schema, table, script)`. Reads
    /// the file, executes the query area via the adapter, and replaces
    /// the focused pane's items with the result.
    pub fn run_postgres_script(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        database: String,
        schema: String,
        table: String,
        script: String,
    ) -> EditorRequest {
        let Some(adapter) = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone)
        else {
            self.notify("No adapter for this view".to_string());
            return EditorRequest::None;
        };
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let session = crate::edit_session::PostgresQuerySession::open_named(
                    Arc::clone(&adapter),
                    database.clone(),
                    schema.clone(),
                    table.clone(),
                    script.clone(),
                    view_index,
                    pane_id,
                )
                .await;
                let text = session.template().to_string();
                let sql = crate::edit_session::postgres_parse_query_area(&text)
                    .trim()
                    .to_string();
                if sql.is_empty() {
                    return Err("query is empty".to_string());
                }
                let ctx = not_yet_done_content::CustomQueryContext::new()
                    .with("database", database.clone())
                    .with_page(not_yet_done_content::PageRequest {
                        offset: 0,
                        limit: crate::edit_session::POSTGRES_QUERY_DEFAULT_PAGE_SIZE,
                    });
                adapter
                    .execute_custom_query(&sql, &ctx)
                    .await
                    .map(|out| (out, sql))
                    .map_err(|e| e.to_string())
            })
        });
        match result {
            Ok((out, sql)) => {
                let items = out.items;
                let status = out.status;
                let page = out.page;
                let custom_query = Some(crate::views::content_view::CustomQueryRunState {
                    query: sql,
                    database,
                    // Placeholder — `apply_custom_query_result` patches
                    // `mode` from the pane's live view-config below.
                    mode: crate::config::view_config::PaginationMode::Server,
                    cursor_id: out.cursor_id,
                });
                if let Some(cv) = self.content_view_mut(view_index) {
                    cv.apply_custom_query_result(
                        pane_id,
                        items,
                        page,
                        custom_query,
                    );
                }
                self.set_query_error(None);
                if let Some(s) = status {
                    self.notify(s);
                } else {
                    self.notify(format!("Ran '{script}'"));
                }
            }
            Err(msg) => {
                self.notify_error(format!("Script '{script}' failed: {msg}"));
            }
        }
        EditorRequest::None
    }

    /// Open the Postgres SQL editor for a named per-table script.
    /// `is_new` is currently informational — the editor writes to the
    /// path either way; missing files yield the default template.
    pub fn edit_postgres_script(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        database: String,
        schema: String,
        table: String,
        script: String,
        _is_new: bool,
    ) -> EditorRequest {
        let Some(adapter) = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone)
        else {
            self.notify("No adapter for this view".to_string());
            return EditorRequest::None;
        };
        let session = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                crate::edit_session::PostgresQuerySession::open_named(
                    adapter, database, schema, table, script, view_index, pane_id,
                )
                .await
            })
        });
        self.open_session(Box::new(session))
    }

    /// Remove a Postgres script `.sql` file and its sidecar shortcut.
    pub fn delete_postgres_script(
        &mut self,
        view_index: usize,
        _pane_id: crate::views::content_view::PaneId,
        database: String,
        schema: String,
        table: String,
        script: String,
    ) {
        let Some(adapter) = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone)
        else {
            self.notify("No adapter for this view".to_string());
            return;
        };
        let instance_dir = adapter.instance_data_dir();
        let scope = crate::app::node_actions::postgres_table_scope(
            adapter.instance_id(),
            &database,
            &schema,
            &table,
        );
        let shortcut_repo = Arc::clone(&self.query_shortcut_repo);
        let result: std::io::Result<()> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let path = not_yet_done_postgres_adapter::query::query_file_path(
                    &instance_dir, &database, &schema, &table, &script,
                );
                match tokio::fs::remove_file(&path).await {
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
                // Best-effort: missing/failed shortcut row is not a
                // delete failure (idempotent unset already).
                let _ = shortcut_repo.unset(&scope, &script).await;
                Ok(())
            })
        });
        match result {
            Ok(_) => self.notify(format!("Deleted script '{script}'")),
            Err(e) => self.notify_error(format!("Delete failed: {e}")),
        }
        // Drop the cached chord-claim entry so the next keypress on
        // this table refetches from `query_shortcut` (SQ-8d).
        let table_node_id = format!("{database}/schemas/{schema}/tables/{table}");
        if let Some(cv) = self.content_view_mut(view_index) {
            cv.postgres_table_shortcuts.remove(&table_node_id);
        }
    }

    /// Set up shortcut-capture state for a Postgres script. The next
    /// non-Esc keypress is bound by [`App::handle_key`]'s capture branch.
    pub fn prompt_postgres_script_shortcut(
        &mut self,
        view_index: usize,
        database: String,
        schema: String,
        table: String,
        script: String,
    ) {
        self.modal_message = Some(format!(
            "Press a shortcut key for script '{}'\n\nEsc to cancel",
            script
        ));
        self.awaiting_postgres_script_shortcut = Some(crate::app::PostgresScriptCoords {
            view_index,
            database,
            schema,
            table,
            script,
        });
    }

    /// Persist a captured key chord into the `query_shortcut` DB table
    /// for the script identified by `coords`. Called after the user
    /// presses a non-Esc, non-conflicting key while
    /// `awaiting_postgres_script_shortcut` is set.
    pub fn bind_postgres_script_shortcut(
        &mut self,
        coords: crate::app::PostgresScriptCoords,
        chord: &str,
    ) {
        let Some(adapter) = self
            .content_view(coords.view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone)
        else {
            self.notify("No adapter for this view".to_string());
            return;
        };
        let scope = crate::app::node_actions::postgres_table_scope(
            adapter.instance_id(),
            &coords.database,
            &coords.schema,
            &coords.table,
        );
        let shortcut_repo = Arc::clone(&self.query_shortcut_repo);
        let chord_owned = chord.to_string();
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                shortcut_repo
                    .set(&scope, &coords.script, &chord_owned)
                    .await
            })
        });
        if let Err(e) = result {
            self.notify_error(format!("Failed to persist shortcut: {e}"));
        }
        // Drop the cached chord-claim entry so the next keypress on
        // this table refetches from `query_shortcut` (SQ-8d).
        let table_node_id = format!(
            "{}/schemas/{}/tables/{}",
            coords.database, coords.schema, coords.table,
        );
        if let Some(cv) = self.content_view_mut(coords.view_index) {
            cv.postgres_table_shortcuts.remove(&table_node_id);
        }
    }

    /// Open the adapter-native query editor (Postgres SQL editor today).
    /// Parses the active drill-down's parent node id to derive the
    /// adapter-specific addressing data, builds a [`PostgresQuerySession`]
    /// pre-populated with the persisted buffer (or the default
    /// `SELECT * FROM <schema>.<table>` template), and routes it through
    /// the standard editor lifecycle.
    pub fn open_adapter_query_editor(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        parent_node_id: String,
    ) -> EditorRequest {
        let Some(adapter) = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone)
        else {
            self.notify("No adapter for this view".to_string());
            return EditorRequest::None;
        };
        if adapter.adapter_type() != "postgres" {
            self.notify(format!(
                "Custom-query editor not yet implemented for adapter '{}'",
                adapter.adapter_type()
            ));
            return EditorRequest::None;
        }
        let Some((database, schema, table)) = parse_postgres_table_path(&parent_node_id) else {
            self.notify(format!(
                "Cannot derive (database, schema, table) from '{parent_node_id}'"
            ));
            return EditorRequest::None;
        };

        let session = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                crate::edit_session::PostgresQuerySession::open(
                    adapter, database, schema, table, view_index, pane_id,
                )
                .await
            })
        });
        self.open_session(Box::new(session))
    }

    // -----------------------------------------------------------------------
    // Processing editor results
    // -----------------------------------------------------------------------

    pub async fn process_editor_content(&mut self, content: &str) -> Option<String> {
        // Cancel detection: if the buffer comes back byte-identical to what
        // we last wrote, the user closed without saving (`:q` / `:q!`).
        // Critical for breaking out of validation-error reopen loops where
        // the disk content stays the same on `:q!`.
        if self.last_editor_buffer.as_deref() == Some(content) {
            self.cancel_pending_edit();
            return None;
        }

        let result = self.process_session_commit(content).await;
        self.track_buffer_after_process(&result);
        result
    }

    /// Mark the pending edit as cancelled and notify. Called when `:q!`
    /// returns the same buffer we just wrote to the editor.
    fn cancel_pending_edit(&mut self) {
        let was_pending = self.pending_session.is_some();
        self.pending_session = None;
        self.last_editor_buffer = None;
        if was_pending {
            self.notify("Edit cancelled".to_string());
        }
    }

    /// Update `last_editor_buffer` after a process step:
    /// - `Some(content)` means a reopen — remember it so a `:q!` on the
    ///   reopened editor (which leaves disk content unchanged) is detected
    ///   as a cancel instead of looping.
    /// - `None` means we're done — clear the snapshot.
    fn track_buffer_after_process(&mut self, result: &Option<String>) {
        match result {
            Some(buf) => self.last_editor_buffer = Some(buf.clone()),
            None      => self.last_editor_buffer = None,
        }
    }

    /// Drive the session through `commit` and translate the outcome into the
    /// existing return contract: `None` = done / cancelled, `Some(content)` =
    /// reopen the editor with `content`. Keeps the pending session in place
    /// only on `Reopen`.
    async fn process_session_commit(&mut self, content: &str) -> Option<String> {
        let mut session = self.pending_session.take()?;
        let outcome = session.commit(content).await;
        match outcome {
            CommitOutcome::Done { message } => {
                if let Some(m) = message { self.notify(m); }
                None
            }
            CommitOutcome::Cancelled { message } => {
                if let Some(m) = message { self.notify(m); }
                None
            }
            CommitOutcome::Reopen { content } => {
                self.pending_session = Some(session);
                Some(content)
            }
            CommitOutcome::FollowUp(follow_up) => {
                self.handle_follow_up(follow_up).await;
                None
            }
        }
    }

    pub(crate) async fn handle_follow_up(&mut self, follow_up: FollowUp) {
        match follow_up {
            FollowUp::ReloadTasks { focus_id, tracking_changed, message } => {
                if let Some(id) = focus_id {
                    self.tasks_view.set_pending_focus(id);
                }
                self.set_query_error(None);
                self.notify(message);
                self.spawn_load();
                if tracking_changed {
                    self.refresh_tracked_ids();
                }
            }
            FollowUp::ReloadContentDrillDown { view_index, parent_node_id, child_node_type, message } => {
                self.notify(message);
                let pane_id = self
                    .content_view(view_index)
                    .map(|cv| cv.active_pane_id())
                    .unwrap_or_default();
                self.spawn_content_drill_down(view_index, pane_id, parent_node_id, child_node_type);
            }
            FollowUp::ReloadContentPane { view_index, pane_id, message } => {
                self.notify(message);
                self.reload_content_pane_current_level(view_index, pane_id);
            }
            FollowUp::ApplyTaskFilter { content } => {
                self.apply_query_filter(&content);
            }
            FollowUp::ApplyTrackingFilter { content } => {
                self.apply_tracking_query_filter(&content);
            }
            FollowUp::ApplyContentFilter { view_index, content, save_name } => {
                self.apply_content_query_live(&content, view_index, save_name.as_deref());
            }
            FollowUp::CloseTaskFilter { content, name, is_new } => {
                self.apply_query_filter(&content);
                self.process_query_filter_close(&name, is_new).await;
            }
            FollowUp::CloseTrackingFilter { content, name, is_new } => {
                self.apply_tracking_query_filter(&content);
                self.process_tracking_query_filter_close(&name, is_new).await;
            }
            FollowUp::CloseContentQuery { view_index, content, save_name, is_new } => {
                self.process_content_query_edit(&content, view_index, save_name.as_deref(), is_new);
            }
            FollowUp::SetQueryError(msg) => {
                self.set_query_error(Some(msg));
            }
            FollowUp::ReplaceContentItems { view_index, pane_id, items, status, page, custom_query } => {
                if let Some(cv) = self.content_view_mut(view_index) {
                    cv.apply_custom_query_result(pane_id, items, page, custom_query);
                }
                self.set_query_error(None);
                if let Some(s) = status {
                    self.notify(s);
                }
            }
            FollowUp::ReloadConfig { path } => {
                match self.reload_config(&path) {
                    Ok(msg) => self.notify(msg),
                    Err(e) => {
                        let err_msg = e.to_string();
                        self.notify_error(format!(
                            "Reload {} failed: {err_msg}",
                            path.display()
                        ));
                        self.reopen_config_with_error(path, err_msg);
                    }
                }
            }
            FollowUp::ReloadContentSavedQueries { view_index, message } => {
                self.reload_content_saved_queries(view_index);
                self.notify(message);
            }
        }
    }

    pub fn poll_detached_editor(&mut self) -> Option<String> {
        let editor = self.detached_editor.as_ref()?;
        if !editor.is_done() { return None; }
        let content = editor.read_content().ok();
        editor.cleanup();
        self.detached_editor = None;
        content
    }

    /// Poll for live-reload file changes during a detached editor session.
    /// Called each tick from the main loop.
    /// Returns `true` when a live `:w` buffer was applied to the active
    /// session (visible state may have changed); `false` on every
    /// early-out (no detached editor, no change yet, etc.).
    pub async fn poll_live_editor(&mut self) -> bool {
        if self.detached_editor.is_none() { return false; }
        if !self.has_pending_edit() { return false; }

        let changed = self.detached_editor.as_mut()
            .map(|e| e.has_changed())
            .unwrap_or(false);
        if !changed { return false; }

        let content = match self.detached_editor.as_ref().and_then(|e| e.read_live_content().ok()) {
            Some(c) => c,
            None => return false,
        };

        // Hand the buffer to the active session; each session opts into live
        // behaviour by overriding `live_apply`.
        if let Some(session) = self.pending_session.as_mut() {
            let follow_up = session.live_apply(&content).await;
            if let Some(fu) = follow_up {
                self.handle_follow_up(fu).await;
            }
        }
        true
    }

    /// Poll for editor close (`.done` marker). When present, hand the buffer
    /// off to a background commit task so the main loop stays responsive
    /// even on slow backends. Result is consumed later via
    /// [`Self::poll_commit_result`]; nothing is returned here.
    /// Returns `true` when a closed editor's buffer was handed off to a
    /// background commit (which flips `commit_in_flight` and thus changes
    /// the action bar); `false` when no editor closed this tick.
    pub fn poll_editor_close(&mut self) -> bool {
        let Some(content) = self.poll_detached_editor() else { return false };
        self.spawn_session_commit(&content);
        true
    }

    /// Spawn the active session's `commit` on a background tokio task.
    /// Sets `commit_in_flight = true` until the result is drained from
    /// `commit_rx`. Cancel detection (`:q!` returns the same buffer)
    /// happens here, before any work is dispatched.
    pub fn spawn_session_commit(&mut self, content: &str) {
        if self.last_editor_buffer.as_deref() == Some(content) {
            self.cancel_pending_edit();
            return;
        }
        let Some(mut session) = self.pending_session.take() else {
            return;
        };
        self.commit_in_flight = true;
        let tx = self.commit_tx.clone();
        let content = content.to_string();
        tokio::spawn(async move {
            let outcome = session.commit(&content).await;
            let msg = match outcome {
                CommitOutcome::Done { message } => CommitMsg::Done { message },
                CommitOutcome::Cancelled { message } => CommitMsg::Cancelled { message },
                CommitOutcome::Reopen { content: c } => CommitMsg::Reopen { session, content: c },
                CommitOutcome::FollowUp(fu) => CommitMsg::FollowUp(fu),
            };
            let _ = tx.send(msg);
        });
    }

    /// Apply a single [`CommitMsg`] drained from `commit_rx`. Returns
    /// `Some(error_content)` when the session asked for a `Reopen` — the
    /// main loop relaunches the editor with that buffer; all other
    /// outcomes are handled inline (notify / follow-up). Called from the
    /// event-driven (1b) `select!` loop with the one message its
    /// `commit_rx.recv()` consumed.
    /// See docs/decisions/0001-render-loop-dirty-gating.md.
    pub async fn handle_commit_msg(&mut self, msg: CommitMsg) -> Option<String> {
        self.commit_in_flight = false;
        match msg {
            CommitMsg::Done { message } => {
                if let Some(m) = message { self.notify(m); }
                self.last_editor_buffer = None;
                None
            }
            CommitMsg::Cancelled { message } => {
                if let Some(m) = message { self.notify(m); }
                self.last_editor_buffer = None;
                None
            }
            CommitMsg::Reopen { session, content } => {
                self.pending_session = Some(session);
                self.last_editor_buffer = Some(content.clone());
                Some(content)
            }
            CommitMsg::FollowUp(fu) => {
                self.handle_follow_up(fu).await;
                self.last_editor_buffer = None;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_postgres_table_path_extracts_three_parts() {
        let got = parse_postgres_table_path("mydb/schemas/public/tables/users").unwrap();
        assert_eq!(got, ("mydb".into(), "public".into(), "users".into()));
    }

    #[test]
    fn parse_postgres_table_path_rejects_partial_paths() {
        assert!(parse_postgres_table_path("mydb").is_none());
        assert!(parse_postgres_table_path("mydb/schemas/public").is_none());
        assert!(parse_postgres_table_path("mydb/schemas/public/tables").is_none());
    }

    #[test]
    fn parse_postgres_table_path_rejects_wrong_sentinels() {
        assert!(parse_postgres_table_path("mydb/foo/public/tables/users").is_none());
        assert!(parse_postgres_table_path("mydb/schemas/public/bar/users").is_none());
    }
}
