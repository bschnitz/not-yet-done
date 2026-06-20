//! `:script` orchestration — the App-level menu lifecycle and the
//! script-run pipeline for both Trackings (legacy JSON shape) and
//! content-view nodes (`{ "node": { ref, id, label, node_type, tab,
//! instance, fields } }`).
//!
//! Anatomy:
//!   - [`ScriptContext`] carries everything needed to decide *where*
//!     scripts live on disk and *what* JSON gets handed to them. Built
//!     by [`App::open_script_menu_for_trackings`] /
//!     [`App::open_script_menu_for_content`] and stashed on
//!     `App::script_menu_ctx` for the duration the menu is open.
//!   - [`App::handle_script_menu_key`] dispatches the menu's message
//!     to run / edit / delete / create. Run paths fork on script
//!     `# mode:` header (`background` / `capture` / `interactive` /
//!     `interactive+capture`).
//!   - The interactive-detached path uses `interactive_command` from
//!     `tui.yaml::script:` with placeholders `{script}` / `{json_file}`
//!     / `{output_file}`.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use not_yet_done_content::NodeRef;

use crate::app::editor::{parse_script_mode, ScriptMode};
use crate::app::{App, ContentSlot, DetachedScript, EditorRequest};
use crate::components::script_menu::{ScriptMenuEntry, ScriptMenuMessage};
use crate::edit_session::{ScriptOutputSession, ScriptSession, SessionScope};
use crate::tabs::Tab;
use crate::views::content_view::PaneId;

/// What the open script menu is operating on. Drives the on-disk
/// scripts directory, the JSON layout handed to executed scripts, the
/// session scope used for the action-bar slot, and the scaffold
/// inserted when the user creates a new script via the menu.
#[derive(Debug, Clone)]
pub enum ScriptContext {
    /// Content-view node. JSON shape (new generic schema):
    /// `{"node": {"ref": .., "id": .., "label": .., "node_type": ..,
    /// "tab": .., "instance": .., "fields": {<key>: <value>, …}}}`.
    ContentNode {
        view_index: usize,
        pane_id: PaneId,
        tab: String,
        instance: String,
        /// View-hierarchy path (root ViewDef.node_type followed by every
        /// drilled-into ChildDef.node_type). Drives the scripts directory
        /// — stable across item selections within the same pane, so the
        /// menu doesn't shuffle when a multi-type pane (e.g. Taiga items
        /// mixing issues / userstories) cycles selection.
        view_path: Vec<String>,
        /// Selected item's node_type — surfaced in the JSON payload as
        /// `node.node_type`, *not* used to scope the scripts directory.
        node_type: String,
        node_id: String,
        node_ref: String,
        /// The selected item's display label (e.g. a task's description,
        /// a ticket's summary) — the one row value that is *not* a
        /// metadata field (columns pull it via `source: label`), so the
        /// payload carries it explicitly.
        label: String,
        fields: Vec<(String, String)>,
        /// Scaffold for create-new, pre-resolved at menu-open time
        /// (per-view override, else global fallback).
        new_script_template: String,
    },
    /// Content-view **batch** script (action `scope: filtered_set`). Carries
    /// the whole currently-filtered row set + the active query's date bounds,
    /// reusing the legacy Trackings JSON shape verbatim so the historical
    /// aggregate scripts (daily reports, period equalizers) run unchanged:
    /// `{"tracking_ids": [..], "filter_min_date": .., "filter_max_date": ..}`.
    /// Scripts live in the same per-view directory as [`ContentNode`]
    /// (`<data>/not_yet_done/scripts/<tab>/<view…>/`).
    ContentBatch {
        view_index: usize,
        pane_id: PaneId,
        tab: String,
        view_path: Vec<String>,
        /// Ids of every currently-visible (filtered) row in the pane.
        node_ids: Vec<String>,
        /// Date bounds extracted from the active query (resolved relative
        /// dates included), mirroring the legacy filter's bounds.
        min_date: Option<DateTime<Utc>>,
        max_date: Option<DateTime<Utc>>,
        /// Scaffold for create-new, pre-resolved at menu-open time.
        new_script_template: String,
    },
}

impl ScriptContext {
    /// Template inserted when the user creates a new script through
    /// the menu. Resolved at menu-open time from
    /// `views[].script_template` (per-view), with `script.template` as
    /// the global fallback.
    pub fn new_script_template(&self) -> &str {
        match self {
            ScriptContext::ContentNode { new_script_template, .. }
            | ScriptContext::ContentBatch { new_script_template, .. } => new_script_template,
        }
    }
}

impl ScriptContext {
    /// Where the menu reads / writes script files for this context.
    /// Both batch and content-node scripts use the **view path** (root
    /// ViewDef + drill-down ChildDefs, *not* the item-type) under
    /// `<data>/not_yet_done/scripts/<tab>/<view…>/`. `/` and `:` in
    /// node_types are replaced with `_` to keep path segments safe.
    pub fn scripts_dir(&self) -> std::path::PathBuf {
        match self {
            ScriptContext::ContentNode { tab, view_path, .. }
            | ScriptContext::ContentBatch { tab, view_path, .. } => {
                let mut p = dirs::data_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                    .join("not_yet_done")
                    .join("scripts")
                    .join(tab);
                for seg in view_path {
                    p = p.join(seg.replace(['/', ':'], "_"));
                }
                p
            }
        }
    }

    /// Build the JSON string handed to the script (either via temp
    /// file or stdin). Format is context-specific by design — the
    /// `scope: filtered_set` batch stays on its legacy aggregate shape
    /// for backward compatibility with the user's existing scripts.
    pub fn build_json(&self) -> String {
        match self {
            ScriptContext::ContentBatch { node_ids, min_date, max_date, .. } => {
                // Legacy aggregate JSON shape (key stays `tracking_ids`) so the
                // migrated aggregate scripts run unchanged.
                let ids = node_ids
                    .iter()
                    .map(|id| json_string(id))
                    .collect::<Vec<_>>()
                    .join(", ");
                let min = min_date
                    .as_ref()
                    .map(|dt| format!("\"{}\"", dt.to_rfc3339()))
                    .unwrap_or_else(|| "null".to_string());
                let max = max_date
                    .as_ref()
                    .map(|dt| format!("\"{}\"", dt.to_rfc3339()))
                    .unwrap_or_else(|| "null".to_string());
                format!(
                    "{{\"tracking_ids\": [{ids}], \"filter_min_date\": {min}, \"filter_max_date\": {max}}}"
                )
            }
            ScriptContext::ContentNode {
                tab, instance, node_type, node_id, node_ref, label, fields, ..
            } => {
                let fields_inner = fields
                    .iter()
                    .map(|(k, v)| format!("    {}: {}", json_string(k), json_string(v)))
                    .collect::<Vec<_>>()
                    .join(",\n");
                format!(
                    "{{\n  \"node\": {{\n    \"ref\": {nref},\n    \"id\": {nid},\n    \"label\": {lbl},\n    \"node_type\": {nt},\n    \"tab\": {tabq},\n    \"instance\": {iq},\n    \"fields\": {{\n{fields}\n    }}\n  }}\n}}",
                    nref = json_string(node_ref),
                    nid = json_string(node_id),
                    lbl = json_string(label),
                    nt = json_string(node_type),
                    tabq = json_string(tab),
                    iq = json_string(instance),
                    fields = fields_inner,
                )
            }
        }
    }

    /// `SessionScope` for editor sessions (action-bar slot under the
    /// owning tab).
    pub fn session_scope(&self) -> SessionScope {
        match self {
            ScriptContext::ContentNode { .. } | ScriptContext::ContentBatch { .. } => {
                SessionScope::Content
            }
        }
    }
}

/// JSON-escape `s` and wrap it in double-quotes. Hand-rolled to avoid
/// pulling in `serde_json` for a single literal-output use case.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl App {
    /// Dispatch `:script` based on the active tab.
    pub fn open_script_menu_from_current_tab(&mut self) {
        let Tab::Content(idx) = self.active_tab;
        let pane_id = self
            .content_view(idx)
            .map(|cv| cv.active_pane_id())
            .unwrap_or(0);
        self.open_script_menu_for_content(idx, pane_id);
    }

    /// Open the `:script` menu seeded with the focused row of the given
    /// content view pane. Notifies + bails out when nothing is selected
    /// or the slot is broken.
    pub fn open_script_menu_for_content(&mut self, view_index: usize, pane_id: PaneId) {
        let Some(slot) = self.content_views.get(view_index) else {
            self.notify("Content view out of range".to_string());
            return;
        };
        let ContentSlot::Working(cv) = slot else {
            self.notify("Content view is unavailable".to_string());
            return;
        };
        let Some(adapter) = cv.adapter.as_ref() else {
            self.notify("Content view has no adapter".to_string());
            return;
        };
        let kind = adapter.adapter_type().to_string();
        let instance = adapter.instance_id().to_string();
        let pane = cv.find_pane(pane_id);
        let Some(pane) = pane else {
            self.notify("Pane not found".to_string());
            return;
        };
        // Tree-aware: in tree mode the selected summary lives on the
        // tree entry, not in `pane.items` (depth-0 only) — an id lookup
        // there would miss every nested node.
        let Some(item) = pane.selected_item() else {
            self.notify("No row selected".to_string());
            return;
        };
        let node_id = item.id.clone();
        let node_type = item.node_type.type_id.clone();
        let label = item.label.clone();
        let fields: Vec<(String, String)> = item
            .metadata
            .fields
            .iter()
            .map(|f| (f.key.clone(), f.value.clone()))
            .collect();
        let node_ref = format!("{kind}/{instance}/{node_id}");
        let view_def_idx = pane.view_def_index();
        let view_path = pane.view_path_node_types(&cv.view_defs);
        let per_view_template = cv
            .view_defs
            .get(view_def_idx)
            .and_then(|vd| vd.script_template.clone());
        let new_script_template = per_view_template
            .unwrap_or_else(|| self.config.script.template.clone());

        let ctx = ScriptContext::ContentNode {
            view_index,
            pane_id,
            tab: kind,
            instance,
            view_path,
            node_type,
            node_id,
            node_ref,
            label,
            fields,
            new_script_template,
        };
        self.open_script_menu(ctx);
    }

    /// Open the `:script` menu in **batch** mode for a content pane
    /// (action `scope: filtered_set`). Hands the whole currently-filtered
    /// row set + the active query's date bounds to the script via the
    /// legacy batch payload, so the migrated aggregate Trackings scripts
    /// (daily report, period equalizer) run unchanged.
    pub fn open_script_menu_for_content_batch(&mut self, view_index: usize, pane_id: PaneId) {
        let Some(slot) = self.content_views.get(view_index) else {
            self.notify("Content view out of range".to_string());
            return;
        };
        let ContentSlot::Working(cv) = slot else {
            self.notify("Content view is unavailable".to_string());
            return;
        };
        let Some(adapter) = cv.adapter.as_ref() else {
            self.notify("Content view has no adapter".to_string());
            return;
        };
        let kind = adapter.adapter_type().to_string();
        let Some(pane) = cv.find_pane(pane_id) else {
            self.notify("Pane not found".to_string());
            return;
        };
        let node_ids = pane.filtered_item_ids();
        let view_def_idx = pane.view_def_index();
        let view_path = pane.view_path_node_types(&cv.view_defs);
        // Date bounds from the active query (relative dates already
        // resolved by `query_filter::parse`), mirroring the legacy
        // trackings filter's `extract_date_bounds`.
        let query_text = pane.current_query_text(&cv.view_defs);
        let bounds = crate::query_filter::parse(&query_text)
            .ok()
            .map(|pq| not_yet_done_core::filter::extract_date_bounds(&pq.expr));
        let per_view_template = cv
            .view_defs
            .get(view_def_idx)
            .and_then(|vd| vd.script_template.clone());
        let new_script_template =
            per_view_template.unwrap_or_else(|| self.config.script.template.clone());

        let ctx = ScriptContext::ContentBatch {
            view_index,
            pane_id,
            tab: kind,
            view_path,
            node_ids,
            min_date: bounds.as_ref().and_then(|b| b.min),
            max_date: bounds.as_ref().and_then(|b| b.max),
            new_script_template,
        };
        self.open_script_menu(ctx);
    }

    /// Internal: enumerate the context's scripts dir, populate the
    /// fuzzy menu and stash the context for the dispatch path.
    ///
    /// The directory is auto-created on first open so `+name<Enter>`
    /// (create-new) works without the user pre-mkdir'ing the per-tab/
    /// per-node-type tree. An empty dir surfaces a notification with
    /// the path — otherwise an empty popup is easy to mistake for "the
    /// menu didn't open at all".
    fn open_script_menu(&mut self, ctx: ScriptContext) {
        let dir = ctx.scripts_dir();
        let _ = std::fs::create_dir_all(&dir);
        let mut entries: Vec<ScriptMenuEntry> = match std::fs::read_dir(&dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
                .map(|e| ScriptMenuEntry {
                    path: e.path().to_string_lossy().to_string(),
                    label: e.file_name().to_string_lossy().to_string(),
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        entries.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));

        if entries.is_empty() {
            self.notify(format!(
                "No scripts in {}. Type `+name` then Enter to create one.",
                dir.display()
            ));
        }

        let title = match &ctx {
            ScriptContext::ContentNode { tab, view_path, .. }
            | ScriptContext::ContentBatch { tab, view_path, .. } => {
                if view_path.is_empty() {
                    format!("Scripts · {tab}")
                } else {
                    format!("Scripts · {tab} · {}", view_path.join(" · "))
                }
            }
        };
        let theme = Arc::clone(&self.shared_theme);
        self.script_menu = crate::components::script_menu::ScriptMenuComponent::new(theme, title)
            .with_popup_kb(
                self.keybindings.popup.clone(),
                self.keybindings.key_icons.clone(),
            );
        self.script_menu.open(&entries, &self.keybindings.script_menu);
        self.script_menu_ctx = Some(ctx);
    }

    /// Dispatch one keypress while the script menu is open. Returns an
    /// [`EditorRequest`] when the chosen action opens an external editor
    /// (Edit / CreateNew); otherwise [`EditorRequest::None`].
    pub fn handle_script_menu_key(&mut self, key: &str) -> EditorRequest {
        let msg = self.script_menu.handle_key(key, &self.keybindings.script_menu);
        match msg {
            ScriptMenuMessage::Unhandled | ScriptMenuMessage::Handled => EditorRequest::None,
            ScriptMenuMessage::Closed => {
                self.script_menu_ctx = None;
                EditorRequest::None
            }
            ScriptMenuMessage::Run { path, label: _ } => {
                let ctx = self.script_menu_ctx.take();
                match ctx {
                    Some(ctx) => self.run_script(&ctx, &path),
                    None => EditorRequest::None,
                }
            }
            ScriptMenuMessage::Edit { path, label } => {
                let ctx = self.script_menu_ctx.take();
                let Some(ctx) = ctx else { return EditorRequest::None };
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                let filename = std::path::Path::new(&path)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| label.clone());
                let session = ScriptSession::new(
                    ctx.scripts_dir(),
                    filename,
                    content,
                    ctx.session_scope(),
                    format!("edit {label}"),
                );
                self.open_session(Box::new(session))
            }
            ScriptMenuMessage::Delete { path, label } => {
                self.script_menu_ctx = None;
                match std::fs::remove_file(&path) {
                    Ok(_) => self.notify(format!("Deleted script {label}")),
                    Err(e) => self.notify_error(format!("Failed to delete {label}: {e}")),
                }
                EditorRequest::None
            }
            ScriptMenuMessage::CreateNew { name } => {
                let ctx = self.script_menu_ctx.take();
                let Some(ctx) = ctx else { return EditorRequest::None };
                let template = ctx.new_script_template().to_string();
                // Default suffix `.py` when the user types a bare name.
                let filename = if name.contains('.') {
                    name.clone()
                } else {
                    format!("{name}.py")
                };
                let session = ScriptSession::new(
                    ctx.scripts_dir(),
                    filename,
                    template,
                    ctx.session_scope(),
                    format!("new script {name}"),
                );
                self.open_session(Box::new(session))
            }
        }
    }

    /// Snapshot of the source adapter's child-process env for the
    /// script-run paths. Symmetric with the editor's
    /// [`crate::edit_session::EditorSpawnContext`]: same trait method
    /// (`ContentAdapter::child_process_env`), same opacity to the TUI.
    ///
    /// Only ContentNode contexts have an adapter to ask — Trackings /
    /// Task scripts always see an empty map (no change vs. pre-AE
    /// behaviour). When the node ref doesn't parse (impossible in
    /// practice but defensively cheap), fall back to empty too.
    pub(super) fn child_env_for_script(
        &self,
        ctx: &ScriptContext,
    ) -> std::collections::HashMap<String, String> {
        let ScriptContext::ContentNode { view_index, node_ref, .. } = ctx else {
            return std::collections::HashMap::new();
        };
        let Some(adapter) = self
            .content_view(*view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(std::sync::Arc::clone)
        else {
            return std::collections::HashMap::new();
        };
        match NodeRef::parse(node_ref) {
            Ok(nref) => adapter.child_process_env(&nref),
            Err(_) => std::collections::HashMap::new(),
        }
    }

    /// Execute a script for the given context. Forks on the script's
    /// `# mode:` header into background / capture / interactive paths;
    /// each path also chooses between detached (`interactive_command`
    /// template) and inline (TUI yields its terminal).
    fn run_script(&mut self, ctx: &ScriptContext, script_path: &str) -> EditorRequest {
        let path = std::path::Path::new(script_path);
        if !path.exists() {
            self.notify_error(format!("Script not found: {script_path}"));
            return EditorRequest::None;
        }

        let script_content = std::fs::read_to_string(path).unwrap_or_default();
        let mode = parse_script_mode(&script_content);
        let stdin_json = ctx.build_json();

        let child_env = self.child_env_for_script(ctx);
        if mode.is_interactive() {
            let interactive_cmd = self.config.script.interactive_command.clone();
            if !interactive_cmd.is_empty() {
                return self.launch_detached_script_ctx(
                    ctx,
                    script_path,
                    &stdin_json,
                    &interactive_cmd,
                    mode.captures_output(),
                    mode.emits_commands(),
                    &child_env,
                );
            }
            return EditorRequest::Script {
                script_path: script_path.to_string(),
                stdin_json,
                capture: mode.captures_output(),
                child_env,
            };
        }

        let result = self.run_script_background(ctx, script_path, &stdin_json, mode, &child_env);
        // Batch scripts may mutate the underlying data (e.g. a period
        // equalizer); reload the pane so the change is visible.
        if let ScriptContext::ContentBatch { view_index, pane_id, .. } = ctx {
            self.reload_content_pane_current_level(*view_index, *pane_id);
        }
        result
    }

    /// Detached interactive launch via the configured
    /// `interactive_command`. Placeholders: `{script}`, `{json_file}`,
    /// `{output_file}`. Used by the kitty-style template — the spawned
    /// terminal opens its own window so the TUI doesn't yield its own.
    fn launch_detached_script_ctx(
        &mut self,
        ctx: &ScriptContext,
        script_path: &str,
        stdin_json: &str,
        command_template: &str,
        capture: bool,
        emits_commands: bool,
        child_env: &std::collections::HashMap<String, String>,
    ) -> EditorRequest {
        let tmp = std::env::temp_dir();
        let pid = std::process::id();
        let output_path = tmp
            .join(format!("nyd-script-{pid}.marker"))
            .to_string_lossy()
            .to_string();
        let json_path = tmp
            .join(format!("nyd-script-{pid}.json"))
            .to_string_lossy()
            .to_string();
        let _ = std::fs::remove_file(&output_path);
        if let Err(e) = std::fs::write(&json_path, stdin_json) {
            self.notify_error(format!("Failed to write script JSON: {e}"));
            return EditorRequest::None;
        }
        // `{env}` expands to a shell-escaped assignment prefix —
        // mandatory for RPC-style launchers like `kitty @ launch` where
        // the daemon spawns the actual process with its own env (so
        // `cmd.envs()` below alone doesn't reach the script). See
        // `not_yet_done_ratatui::utils::open_editor` for the matching
        // editor-path implementation.
        let env_prefix = not_yet_done_ratatui::render_env_prefix(child_env);
        let cmd = command_template
            .replace("{env}", &env_prefix)
            .replace("{script}", script_path)
            .replace("{json_file}", &json_path)
            .replace("{output_file}", &output_path);
        let pause_tui = self.config.script.pause_tui;
        if pause_tui {
            let _ = crate::events::disable_kitty_protocol();
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::terminal::LeaveAlternateScreen
            );
        }
        // For shells that don't substitute the placeholder themselves
        // we also expose the output file as an env var — that's how
        // background-mode scripts already find it, and the
        // user-defined `interactive_command` may use either form.
        //
        // Adapter env (`PG*` for Postgres, etc.) is applied *first*
        // so the `NYD_*` keys we own can't be accidentally clobbered
        // by an adapter that decides to expose a key by the same name.
        let result = std::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .envs(child_env)
            .env("NYD_OUTPUT_FILE", &output_path)
            .status();
        if pause_tui {
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::terminal::EnterAlternateScreen
            );
            let _ = crate::events::enable_kitty_protocol();
        }
        match result {
            Ok(_) => {
                self.detached_script = Some(DetachedScript {
                    output_path: std::path::PathBuf::from(&output_path),
                    capture,
                    emits_commands,
                });
            }
            Err(e) => self.notify_error(format!("Failed to launch script: {e}")),
        }
        let _ = ctx; // explicitly unused — context already drove JSON + dir
        EditorRequest::None
    }

    /// Synchronous background run — writes JSON to a temp file, spawns
    /// the script with that path as its sole argument, and either
    /// surfaces the output in an editor (capture) or as a notification
    /// (background).
    ///
    /// When `mode` emits commands ([`ScriptMode::Commands`] /
    /// [`ScriptMode::InteractiveCommands`]), an additional temp output
    /// file is allocated and its path passed to the script via the
    /// `NYD_OUTPUT_FILE` environment variable. After the script exits,
    /// the file is parsed as JSON `{"commands": [...]}` and each entry
    /// is fed to [`App::execute_cmdline`].
    fn run_script_background(
        &mut self,
        ctx: &ScriptContext,
        script_path: &str,
        stdin_json: &str,
        mode: ScriptMode,
        child_env: &std::collections::HashMap<String, String>,
    ) -> EditorRequest {
        use std::process::{Command, Stdio};

        let tmp = std::env::temp_dir();
        let pid = std::process::id();
        let json_path = tmp.join(format!("nyd-bg-script-{pid}.json"));
        if let Err(e) = std::fs::write(&json_path, stdin_json) {
            self.notify_error(format!("Failed to write script JSON: {e}"));
            return EditorRequest::None;
        }
        let commands_output_path: Option<std::path::PathBuf> = if mode.emits_commands() {
            let p = tmp.join(format!("nyd-bg-script-{pid}-commands.json"));
            let _ = std::fs::remove_file(&p);
            Some(p)
        } else {
            None
        };
        let path = std::path::Path::new(script_path);
        let mut cmd = Command::new(script_path);
        cmd.arg(&json_path)
            .current_dir(path.parent().unwrap_or(std::path::Path::new(".")))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Adapter env applied before `NYD_OUTPUT_FILE` so we always
            // own the `NYD_*` namespace regardless of what the adapter
            // exposes.
            .envs(child_env);
        if let Some(ref op) = commands_output_path {
            cmd.env("NYD_OUTPUT_FILE", op);
        }
        let result = cmd.spawn();
        match result {
            Ok(child) => match child.wait_with_output() {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if mode == ScriptMode::Capture {
                        let mut combined = String::new();
                        if !stdout.trim().is_empty() {
                            combined.push_str(&stdout);
                        }
                        if !stderr.trim().is_empty() {
                            if !combined.is_empty() {
                                combined.push('\n');
                            }
                            combined.push_str(&stderr);
                        }
                        if !output.status.success() {
                            let msg = format!("Script exited with {}", output.status);
                            combined = if combined.is_empty() {
                                msg
                            } else {
                                format!("{msg}\n{combined}")
                            };
                        }
                        if !combined.trim().is_empty() {
                            let session = ScriptOutputSession::new(combined)
                                .with_scope(ctx.session_scope());
                            return self.open_session(Box::new(session));
                        }
                        self.notify("Script finished (no output)".to_string());
                    } else if mode.emits_commands() {
                        // Stderr still routes to a notification — but
                        // stdout is ignored here so scripts can freely
                        // use it for debug-printing without it ending
                        // up in the user's face.
                        if !stderr.trim().is_empty() {
                            self.notify(stderr.trim().to_string());
                        }
                        if !output.status.success() {
                            self.notify_error(format!(
                                "Script exited with {}",
                                output.status
                            ));
                        } else if let Some(ref op) = commands_output_path {
                            self.run_script_output_commands(op);
                        }
                    } else if !stderr.trim().is_empty() {
                        self.notify(stderr.trim().to_string());
                    } else if !output.status.success() {
                        self.notify_error(format!("Script exited with {}", output.status));
                    } else {
                        self.notify("Script finished".to_string());
                    }
                }
                Err(e) => self.notify_error(format!("Script wait error: {e}")),
            },
            Err(e) => self.notify_error(format!("Failed to run script: {e}")),
        }
        if let Some(p) = commands_output_path {
            let _ = std::fs::remove_file(&p);
        }
        EditorRequest::None
    }

    /// Read the script's commands output file and execute each entry
    /// through [`App::execute_cmdline`]. Tolerant of:
    ///   - missing file (script chose not to emit commands)
    ///   - extra top-level keys (forward-compat: `{ "commands": [..],
    ///     "version": 1, ... }`)
    ///   - leading `:` on individual command strings
    ///
    /// Surfaces a notification on parse failure but does *not* abort
    /// the run — the script's other side-effects (DB writes, etc.) are
    /// already committed by the time we get here.
    pub(super) fn run_script_output_commands(&mut self, output_path: &std::path::Path) {
        if !output_path.exists() {
            return;
        }
        let raw = match std::fs::read_to_string(output_path) {
            Ok(s) => s,
            Err(e) => {
                self.notify_error(format!(
                    "Failed to read script output file: {e}"
                ));
                return;
            }
        };
        if raw.trim().is_empty() {
            return;
        }
        let parsed: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                self.notify_error(format!("Script output is not valid JSON: {e}"));
                return;
            }
        };
        let Some(cmds) = parsed.get("commands").and_then(|v| v.as_array()) else {
            self.notify_error(
                "Script output JSON missing `commands` array".to_string(),
            );
            return;
        };
        for entry in cmds {
            let Some(s) = entry.as_str() else {
                self.notify_error(
                    "Script command entry is not a string".to_string(),
                );
                continue;
            };
            let stripped = s.trim().strip_prefix(':').unwrap_or(s.trim());
            if stripped.is_empty() {
                continue;
            }
            self.execute_cmdline(stripped);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn content_batch_json_matches_legacy_trackings_shape() {
        // The batch payload must be byte-identical to the legacy Trackings
        // payload (`tracking_ids` array + RFC3339 date bounds) so the
        // migrated aggregate scripts run unchanged.
        let batch = ScriptContext::ContentBatch {
            view_index: 0,
            pane_id: 0,
            tab: "trackings".into(),
            view_path: vec!["tracking:entry".into()],
            node_ids: vec!["a".into(), "b".into()],
            min_date: Some(dt("2026-01-01T00:00:00Z")),
            max_date: Some(dt("2026-02-01T00:00:00Z")),
            new_script_template: String::new(),
        };
        assert_eq!(
            batch.build_json(),
            "{\"tracking_ids\": [\"a\", \"b\"], \"filter_min_date\": \"2026-01-01T00:00:00+00:00\", \"filter_max_date\": \"2026-02-01T00:00:00+00:00\"}"
        );
    }

    #[test]
    fn content_batch_json_unbounded_dates_serialize_null() {
        let batch = ScriptContext::ContentBatch {
            view_index: 0,
            pane_id: 0,
            tab: "trackings".into(),
            view_path: vec![],
            node_ids: vec![],
            min_date: None,
            max_date: None,
            new_script_template: String::new(),
        };
        assert_eq!(
            batch.build_json(),
            "{\"tracking_ids\": [], \"filter_min_date\": null, \"filter_max_date\": null}"
        );
    }

    #[test]
    fn content_batch_reuses_content_scripts_dir() {
        // Batch scripts share the per-view directory with single-node
        // content scripts (tab + view_path), not the legacy tracking dir.
        let batch = ScriptContext::ContentBatch {
            view_index: 0,
            pane_id: 0,
            tab: "trackings".into(),
            view_path: vec!["tracking:entry".into()],
            node_ids: vec![],
            min_date: None,
            max_date: None,
            new_script_template: String::new(),
        };
        let dir = batch.scripts_dir();
        assert!(dir.ends_with("scripts/trackings/tracking_entry"), "{dir:?}");
    }
}
