//! Editor orchestration: open external editors, process results, persist filters.

use std::sync::Arc;

use crate::edit_session::{CommitOutcome, EditSession, EditorSpawnContext, FollowUp};

/// Result of an asynchronously running commit. Sent from the spawned task
/// back to the main loop via `commit_tx`. `Reopen` carries the session back
/// so the App can re-attach it for the next round.
pub enum CommitMsg {
    Done {
        message: Option<String>,
    },
    Cancelled {
        message: Option<String>,
    },
    Reopen {
        session: Box<dyn EditSession>,
        content: String,
    },
    FollowUp(FollowUp),
}

use super::App;

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
        /// Temp-buffer file extension for the captured-output viewer (from the
        /// script's `# output:` header; `.txt` by default).
        output_suffix: String,
        child_env: std::collections::HashMap<String, String>,
    },
    /// A `:w` in the builtin editor pane: the editor stays open and the
    /// active session's [`EditSession::live_apply`] must run. It is a
    /// request rather than inline work in `handle_key` because
    /// `live_apply` is async and can hit the network (a Stoat
    /// `commit_on_save` compose sends the message right there) — blocking
    /// the key handler on it would freeze the TUI.
    BuiltinLiveApply {
        content: String,
    },
    None,
}

impl EditorRequest {
    /// Whether dispatching this request hands the terminal to a child
    /// process. The main loop must tear its `EventStream` reader down
    /// around those — otherwise the reader thread steals the child's
    /// input — but not around in-process work.
    pub fn suspends_terminal(&self) -> bool {
        matches!(
            self,
            Self::Inline { .. } | Self::Launch { .. } | Self::Script { .. }
        )
    }
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
        matches!(
            self,
            Self::Interactive | Self::InteractiveCapture | Self::InteractiveCommands
        )
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
        let after = trimmed
            .strip_prefix('#')
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

/// Parse `# output: <ext>` from the script header — the file extension the
/// captured-output viewer should use for its temp buffer. The extension drives
/// syntax highlighting and Markdown preview, so a report-style script that
/// emits Markdown declares `# output: md` to be rendered as Markdown rather
/// than plain text. The leading dot is optional (`md` and `.md` both work).
/// Defaults to `.txt`. Only meaningful for capture-mode scripts.
pub fn parse_script_output_suffix(content: &str) -> String {
    for line in content.lines().take(10) {
        let trimmed = line.trim();
        let after = trimmed
            .strip_prefix('#')
            .or_else(|| trimmed.strip_prefix("//"))
            .or_else(|| trimmed.strip_prefix("--"))
            .or_else(|| trimmed.strip_prefix(";;"));
        if let Some(rest) = after {
            if let Some(ext) = rest.trim().strip_prefix("output:") {
                let ext = ext.trim().trim_start_matches('.');
                if !ext.is_empty() {
                    return format!(".{ext}");
                }
            }
        }
    }
    ".txt".to_string()
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
        // Snapshot the builtin geometry before the profile borrow ends.
        let builtin = editor.builtin;
        let height = editor.height.clone();
        let line_numbers = editor.line_numbers;
        let label = session.label().to_string();

        self.pending_session = Some(session);
        self.last_editor_buffer = Some(content.clone());

        if builtin {
            // No child process, no temp file: mount the pane and let the
            // key handler drive it. The session stays pending exactly as
            // for an external editor.
            self.mount_builtin_editor(&label, &content, &height, line_numbers);
            return EditorRequest::None;
        }

        if editor.inline {
            EditorRequest::Inline {
                command,
                content,
                suffix,
                spawn_context,
            }
        } else if editor.pause_tui {
            EditorRequest::Launch {
                command,
                content,
                suffix,
                spawn_context,
            }
        } else {
            match not_yet_done_ratatui::open_editor_detached_in(
                Some(&command),
                &content,
                Some(suffix),
                spawn_context.tempfile_dir.as_deref(),
                spawn_context.tempfile_prefix,
                &spawn_context.child_env,
                spawn_context.persistent_file.as_deref(),
            ) {
                Ok(handle) => {
                    self.detached_editor = Some(handle);
                }
                Err(e) => {
                    self.notify_error(format!("Editor error: {e}"));
                }
            }
            EditorRequest::None
        }
    }

    // -----------------------------------------------------------------------
    // The builtin editor pane
    // -----------------------------------------------------------------------

    /// Mount the builtin editor pane for the pending session's buffer.
    fn mount_builtin_editor(
        &mut self,
        label: &str,
        content: &str,
        height: &str,
        line_numbers: bool,
    ) {
        self.builtin_editor = Some(crate::components::builtin_editor::BuiltinEditorPane::new(
            &self.theme,
            label,
            content,
            height,
            line_numbers,
        ));
    }

    /// Feed a key to the mounted builtin editor and act on what it reports.
    /// Called from [`App::handle_key`] while the pane is open; the pane owns
    /// every key, so this never falls through to global dispatch.
    pub(crate) fn handle_builtin_editor_key(&mut self, key: &str) -> EditorRequest {
        use crate::components::builtin_editor::BuiltinEditorOutcome as Outcome;
        let Some(pane) = self.builtin_editor.as_mut() else {
            return EditorRequest::None;
        };
        match pane.handle_key(key) {
            Outcome::Consumed => EditorRequest::None,
            // `:w` keeps the pane open; the async part happens in the main
            // loop (see [`EditorRequest::BuiltinLiveApply`]).
            Outcome::Save(content) => EditorRequest::BuiltinLiveApply { content },
            Outcome::SaveAndClose(content) => {
                self.builtin_editor = None;
                // Same background commit as a closing external editor —
                // including its cancel detection for an unchanged buffer.
                self.spawn_session_commit(&content);
                EditorRequest::None
            }
            Outcome::Cancel => {
                self.builtin_editor = None;
                self.cancel_pending_edit();
                EditorRequest::None
            }
        }
    }

    /// Run the active session's `live_apply` for a builtin `:w`, then report
    /// the outcome on the editor's own status line — the pane is still open
    /// and covers the notification bar's usual spot in the user's attention.
    pub async fn apply_builtin_live_save(&mut self, content: &str) {
        let Some(session) = self.pending_session.as_mut() else {
            return;
        };
        let follow_up = session.live_apply(content).await;
        if let Some(fu) = follow_up {
            self.handle_follow_up(fu).await;
        }
        if let Some(pane) = self.builtin_editor.as_mut() {
            pane.set_message("written");
        }
    }

    /// Re-mount the builtin pane with a session's validation-error buffer.
    /// The builtin counterpart of `main::reopen_editor_with_errors`; returns
    /// `false` when the pending session is not on a builtin profile, so the
    /// caller falls back to relaunching the external editor.
    fn reopen_builtin_with(&mut self, content: &str) -> bool {
        let profile = self.config.editors.resolve(
            self.pending_session
                .as_ref()
                .and_then(|s| s.editor_profile()),
        );
        if !profile.builtin {
            return false;
        }
        let height = profile.height.clone();
        let line_numbers = profile.line_numbers;
        let label = self
            .pending_session
            .as_ref()
            .map(|s| s.label().to_string())
            .unwrap_or_default();
        self.mount_builtin_editor(&label, content, &height, line_numbers);
        true
    }

    /// Open the per-node scripts menu (the `q` keybind on a level with
    /// `node_scripts: true`). Lists the scripts the adapter's
    /// [`ScriptStore`](not_yet_done_content::ScriptStore) holds for
    /// `node_id`, pairs them with their bound chords, and pushes the
    /// entries into the content view's popup.
    pub fn open_node_scripts_menu(
        &mut self,
        view_index: usize,
        _pane_id: crate::views::content_view::PaneId,
        node_id: String,
    ) -> EditorRequest {
        let Some(adapter) = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone)
        else {
            self.notify("No adapter for this view".to_string());
            return EditorRequest::None;
        };
        if adapter.script_store().is_none() {
            self.notify(format!(
                "Scripts menu not implemented for adapter '{}'",
                adapter.adapter_type()
            ));
            return EditorRequest::None;
        }
        let scope = crate::app::node_actions::node_script_scope(
            adapter.adapter_type(),
            adapter.instance_id(),
            &node_id,
        );
        let shortcut_repo = Arc::clone(&self.query_shortcut_repo);
        let (scripts, shortcut_map) = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let scripts = match adapter.script_store() {
                    Some(store) => store.list_node_scripts(&node_id).await,
                    None => Ok(Vec::new()),
                };
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
                // Value field is unused for node scripts; keep
                // non-empty so the popup widget treats the row as
                // selectable.
                query: "<file>".to_string(),
                is_default: false,
            })
            .collect();
        if let Some(cv) = self.content_view_mut(view_index) {
            cv.open_node_scripts_popup(node_id, entries);
        }
        EditorRequest::None
    }

    /// Run the node script `script` belonging to `node_id`. Reads the
    /// file, executes the query area via the adapter, and replaces the
    /// focused pane's items with the result.
    pub fn run_node_script(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
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
                let session = crate::edit_session::AdapterQuerySession::open_named(
                    Arc::clone(&adapter),
                    node_id.clone(),
                    script.clone(),
                    view_index,
                    pane_id,
                )
                .await;
                let text = session.template().to_string();
                let sql = crate::edit_session::adapter_parse_query_area(&text)
                    .trim()
                    .to_string();
                if sql.is_empty() {
                    return Err("query is empty".to_string());
                }
                // Routing keys (for Postgres: the target database) come
                // from the adapter, which knows its own id shape.
                let ctx = adapter.custom_query_context(&node_id).with_page(
                    not_yet_done_content::PageRequest {
                        offset: 0,
                        limit: crate::edit_session::ADAPTER_QUERY_DEFAULT_PAGE_SIZE,
                    },
                );
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
                    node_id,
                    // Placeholder — `apply_custom_query_result` patches
                    // `mode` from the pane's live view-config below.
                    mode: crate::config::view_config::PaginationMode::Server,
                    cursor_id: out.cursor_id,
                });
                if let Some(cv) = self.content_view_mut(view_index) {
                    cv.apply_custom_query_result(pane_id, items, page, custom_query);
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

    /// Open the SQL editor for a named script of `node_id`.
    /// `is_new` is currently informational — the editor writes to the
    /// path either way; missing files yield the default template.
    pub fn edit_node_script(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
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
                crate::edit_session::AdapterQuerySession::open_named(
                    adapter, node_id, script, view_index, pane_id,
                )
                .await
            })
        });
        self.open_session(Box::new(session))
    }

    /// Remove a node script file and its sidecar shortcut.
    pub fn delete_node_script(
        &mut self,
        view_index: usize,
        _pane_id: crate::views::content_view::PaneId,
        node_id: String,
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
        let scope = crate::app::node_actions::node_script_scope(
            adapter.adapter_type(),
            adapter.instance_id(),
            &node_id,
        );
        let shortcut_repo = Arc::clone(&self.query_shortcut_repo);
        let result: not_yet_done_content::Result<()> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                if let Some(store) = adapter.script_store() {
                    store.delete_node_script(&node_id, &script).await?;
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
        // this node refetches from `query_shortcut` (SQ-8d).
        if let Some(cv) = self.content_view_mut(view_index) {
            cv.node_script_shortcuts.remove(&node_id);
        }
    }

    /// Set up shortcut-capture state for a node script. The next
    /// non-Esc keypress is bound by [`App::handle_key`]'s capture branch.
    pub fn prompt_node_script_shortcut(
        &mut self,
        view_index: usize,
        node_id: String,
        script: String,
    ) {
        self.modal_message = Some(format!(
            "Press a shortcut key for script '{}'\n\nEsc to cancel",
            script
        ));
        self.awaiting_node_script_shortcut = Some(crate::app::NodeScriptCoords {
            view_index,
            node_id,
            script,
        });
    }

    /// Persist a captured key chord into the `query_shortcut` DB table
    /// for the script identified by `coords`. Called after the user
    /// presses a non-Esc, non-conflicting key while
    /// `awaiting_node_script_shortcut` is set.
    pub fn bind_node_script_shortcut(&mut self, coords: crate::app::NodeScriptCoords, chord: &str) {
        let Some(adapter) = self
            .content_view(coords.view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone)
        else {
            self.notify("No adapter for this view".to_string());
            return;
        };
        let scope = crate::app::node_actions::node_script_scope(
            adapter.adapter_type(),
            adapter.instance_id(),
            &coords.node_id,
        );
        let shortcut_repo = Arc::clone(&self.query_shortcut_repo);
        let chord_owned = chord.to_string();
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                // Script row — see `App::bind_script_shortcut` on the kind.
                shortcut_repo
                    .set(
                        &scope,
                        &coords.script,
                        not_yet_done_content::QueryKind::Saved.as_str(),
                        &chord_owned,
                    )
                    .await
            })
        });
        if let Err(e) = result {
            self.notify_error(format!("Failed to persist shortcut: {e}"));
        }
        // Drop the cached chord-claim entry so the next keypress on
        // this node refetches from `query_shortcut` (SQ-8d).
        if let Some(cv) = self.content_view_mut(coords.view_index) {
            cv.node_script_shortcuts.remove(&coords.node_id);
        }
    }

    /// Remove the key chord bound to a node script, leaving the script
    /// file itself in place. Mirrors [`Self::bind_node_script_shortcut`]
    /// but calls `unset` instead of `set`.
    pub fn clear_node_script_shortcut(
        &mut self,
        view_index: usize,
        node_id: String,
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
        let scope = crate::app::node_actions::node_script_scope(
            adapter.adapter_type(),
            adapter.instance_id(),
            &node_id,
        );
        let shortcut_repo = Arc::clone(&self.query_shortcut_repo);
        let script_owned = script.clone();
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { shortcut_repo.unset(&scope, &script_owned).await })
        });
        if let Err(e) = result {
            self.notify_error(format!("Failed to clear shortcut: {e}"));
            return;
        }
        // Drop the cached chord-claim entry so the next keypress on
        // this node refetches from `query_shortcut` (SQ-8d).
        if let Some(cv) = self.content_view_mut(view_index) {
            cv.node_script_shortcuts.remove(&node_id);
        }
        self.notify(format!("Cleared shortcut for script '{script}'"));
    }

    /// Open the adapter-native query editor for the node the active
    /// drill-down hangs off. Builds an [`AdapterQuerySession`]
    /// pre-populated with the persisted buffer (or the adapter's default
    /// template) and routes it through the standard editor lifecycle.
    ///
    /// `parent_node_id` stays opaque: the adapter decides where the buffer
    /// lives and which context the query runs in.
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
        if !adapter.capabilities().supports_node_query_editor {
            self.notify(format!(
                "Custom-query editor not implemented for adapter '{}'",
                adapter.adapter_type()
            ));
            return EditorRequest::None;
        }

        let session = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                crate::edit_session::AdapterQuerySession::open(
                    adapter,
                    parent_node_id,
                    view_index,
                    pane_id,
                )
                .await
            })
        });
        self.open_session(Box::new(session))
    }

    /// Open the editor for a content view's query body in the language and
    /// store that `kind` names.
    ///
    /// A new extended document starts from the framework's passthrough
    /// template rather than the view's `query.template`: the latter is a
    /// single adapter-native query, which is precisely what an extended
    /// document is not. Creation refuses a name either store already holds —
    /// the two share one namespace and the menu shows no difference between
    /// them, so overwriting would destroy a body the user cannot even see.
    pub fn open_content_query_editor(
        &mut self,
        view_index: usize,
        save_name: Option<String>,
        is_new: bool,
        kind: not_yet_done_content::QueryKind,
    ) -> EditorRequest {
        use not_yet_done_content::QueryKind;

        if is_new && let Some(name) = save_name.clone() {
            match self.existing_query_kind(view_index, &name) {
                Ok(Some(existing)) => {
                    self.modal_message = Some(format!(
                        "A query named '{name}' already exists ({existing}).\n\n\
                         Pick another name, or edit that one from the menu."
                    ));
                    return EditorRequest::None;
                }
                Ok(None) => {}
                Err(e) => {
                    // Not "the name is free": an unreadable store would
                    // otherwise get a body written over something present.
                    self.notify_error(format!("Could not check query names: {e}"));
                    return EditorRequest::None;
                }
            }
        }

        let Some(cv) = self.content_view(view_index) else {
            return EditorRequest::None;
        };
        let language = cv
            .adapter
            .as_ref()
            .map(|a| a.query_language().to_string())
            .unwrap_or_else(|| "yaml".to_string());
        let query_text = match (is_new, kind) {
            (true, QueryKind::Extended) => not_yet_done_extended_query::default_template(&language),
            (true, QueryKind::Saved) => cv.default_query_text(),
            (false, _) => cv.current_query_text(),
        };
        let suffix = match kind {
            QueryKind::Extended => not_yet_done_content::EXTENDED_QUERY_SUFFIX.to_string(),
            QueryKind::Saved => cv.query_body_suffix(),
        };
        let session = crate::edit_session::ContentQueryFilterSession::new(
            view_index, save_name, is_new, query_text, suffix, kind,
        );
        self.open_session(Box::new(session))
    }

    /// Which store already holds `name` for this view's adapter, if any.
    /// `Ok(None)` when the name is free or the view has no adapter.
    pub(super) fn existing_query_kind(
        &self,
        view_index: usize,
        name: &str,
    ) -> Result<Option<not_yet_done_content::QueryKind>, String> {
        let Some(adapter) = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone)
        else {
            return Ok(None);
        };
        let name = name.to_string();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                not_yet_done_content::existing_query_kind(adapter.as_ref(), &name)
                    .await
                    .map_err(|e| e.to_string())
            })
        })
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
            None => self.last_editor_buffer = None,
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
                if let Some(m) = message {
                    self.notify(m);
                }
                None
            }
            CommitOutcome::Cancelled { message } => {
                if let Some(m) = message {
                    self.notify(m);
                }
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
            FollowUp::InsertContentChild {
                view_index,
                pane_id,
                parent_node_id,
                child_node_type,
                message,
            } => {
                self.notify(message);
                self.insert_content_child(view_index, pane_id, parent_node_id, child_node_type);
            }
            FollowUp::PatchContentRow {
                view_index,
                pane_id,
                node_id,
                message,
            } => {
                self.notify(message);
                self.patch_content_row(view_index, pane_id, node_id).await;
            }
            FollowUp::ReloadContentPane {
                view_index,
                pane_id,
                message,
            } => {
                self.notify(message);
                self.reload_content_pane_current_level(view_index, pane_id);
            }
            FollowUp::ApplyContentFilter {
                view_index,
                content,
                save_name,
                kind,
            } => {
                self.apply_content_query_live(&content, view_index, save_name.as_deref(), kind);
            }
            FollowUp::CloseContentQuery {
                view_index,
                content,
                save_name,
                is_new,
                kind,
            } => {
                self.process_content_query_edit(
                    &content,
                    view_index,
                    save_name.as_deref(),
                    is_new,
                    kind,
                );
            }
            FollowUp::SetQueryError(msg) => {
                self.set_query_error(Some(msg));
            }
            FollowUp::ReplaceContentItems {
                view_index,
                pane_id,
                items,
                status,
                page,
                custom_query,
            } => {
                if let Some(cv) = self.content_view_mut(view_index) {
                    cv.apply_custom_query_result(pane_id, items, page, custom_query);
                }
                self.set_query_error(None);
                if let Some(s) = status {
                    self.notify(s);
                }
            }
            FollowUp::ReloadConfig { path } => match self.reload_config(&path) {
                Ok(msg) => self.notify(msg),
                Err(e) => {
                    let err_msg = e.to_string();
                    self.notify_error(format!("Reload {} failed: {err_msg}", path.display()));
                    self.reopen_config_with_error(path, err_msg);
                }
            },
            FollowUp::ReloadContentSavedQueries {
                view_index,
                message,
            } => {
                self.reload_content_saved_queries(view_index);
                self.notify(message);
            }
        }
    }

    pub fn poll_detached_editor(&mut self) -> Option<String> {
        let editor = self.detached_editor.as_ref()?;
        if !editor.is_done() {
            return None;
        }
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
        if self.detached_editor.is_none() {
            return false;
        }
        if !self.has_pending_edit() {
            return false;
        }

        let changed = self
            .detached_editor
            .as_mut()
            .map(|e| e.has_changed())
            .unwrap_or(false);
        if !changed {
            return false;
        }

        let content = match self
            .detached_editor
            .as_ref()
            .and_then(|e| e.read_live_content().ok())
        {
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
        let Some(content) = self.poll_detached_editor() else {
            return false;
        };
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
                CommitOutcome::Reopen { content: c } => CommitMsg::Reopen {
                    session,
                    content: c,
                },
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
                if let Some(m) = message {
                    self.notify(m);
                }
                self.last_editor_buffer = None;
                None
            }
            CommitMsg::Cancelled { message } => {
                if let Some(m) = message {
                    self.notify(m);
                }
                self.last_editor_buffer = None;
                None
            }
            CommitMsg::Reopen { session, content } => {
                self.pending_session = Some(session);
                self.last_editor_buffer = Some(content.clone());
                // A builtin session reopens in-process; only an external
                // one needs the main loop to relaunch a child.
                if self.reopen_builtin_with(&content) {
                    return None;
                }
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
