//! Edit session for a database-level adapter script (the `e` shortcut on
//! a `db_script` row).
//!
//! Where the file lives and what a fresh one contains are the adapter's
//! business, asked for through [`ScriptStore`]; the buffer *format*
//! (scratch area, [`QUERY_MARKER`], executable body, error banner) is the
//! shared protocol in [`not_yet_done_content::script_buffer`]. This
//! session therefore holds no concrete-type knowledge of any backend and
//! works for every adapter that offers a script store.
//!
//! Unlike the per-node query editor this does NOT auto-execute on `:w` —
//! it just persists. The user re-runs the script via the `x` shortcut,
//! which goes through [`Node::invoke_action`] → [`ActionDispatch::ExecuteQuery`]
//! → `RunAdapterDbScript`. That keeps the edit-then-save path off the hot
//! cursor lifecycle: re-executing into a result pane requires knowing
//! which pane is the script's "current" result, which the edit session
//! doesn't have when it's spawned from the db_scripts list view.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::script_buffer::{render_with_error, strip_error_banner};
use not_yet_done_content::{ContentAdapter, NodeRef};

use super::{CommitOutcome, EditSession, EditorSpawnContext, SessionScope};

pub struct AdapterDbScriptSession {
    adapter: Arc<dyn ContentAdapter>,
    database: String,
    script: String,
    template: String,
    label: String,
    /// Suffix passed to the temp-file helper so the external editor
    /// picks up the right filetype (`.sql`, `.py`, `.md`, …). Derived
    /// from `script`'s extension; falls back to `.sql` for the
    /// corner case of a script with no extension (shouldn't happen
    /// since the create flow defaults to `.sql`).
    suffix: String,
    /// When `Some`, the editor's temp file is created in this
    /// directory (the real DB-Scripts dir for the database) with the
    /// [`TEMPFILE_PREFIX`] marker instead of `$TMPDIR`. Set via the
    /// `editor_in_place: true` ChildDef flag so LSPs (e.g.
    /// postgres-language-server) discover sibling config files.
    tempfile_dir: Option<PathBuf>,
}

/// Filename prefix for in-place edit temp files. Marks the file as ours so
/// stragglers from crashes are easy to identify and remove.
const TEMPFILE_PREFIX: &str = ".nyd_tmp_";

impl AdapterDbScriptSession {
    /// Open the database-level script `script` under `database`. Missing
    /// files yield the adapter's default template.
    ///
    /// The adapter is then given a chance to augment the buffer with
    /// editor-only completion hints (for Postgres: a trailing
    /// `-- table completions: tt_<schema>__<table>, …` comment listing
    /// every base table in `database`). Such hints are purely an editor
    /// convenience — they are stripped again on commit so the on-disk
    /// file never grows them — and adapters that have none return the
    /// buffer unchanged.
    pub async fn open(
        adapter: Arc<dyn ContentAdapter>,
        database: String,
        script: String,
        in_place: bool,
    ) -> Self {
        let path = script_path(&adapter, &database, &script);
        let on_disk = match &path {
            Some(p) => tokio::fs::read_to_string(p)
                .await
                .unwrap_or_else(|_| default_body(&adapter, &database, &script)),
            None => default_body(&adapter, &database, &script),
        };
        // Hand the raw buffer to the adapter for editor-only completion
        // hints. The adapter strips any stale hint first, so this is
        // idempotent across reopens; non-augmenting adapters return it
        // unchanged. We pass the item's canonical NodeRef
        // (`<type>/<db>/db_scripts/<script>`) so the adapter can scope
        // the hint to the right database/extension. If the ref can't be
        // built, skip augmentation and open the raw buffer.
        let template = match NodeRef::parse(&node_ref_string(&adapter, &database, &script)) {
            Ok(nref) => adapter.augment_editor_buffer(&nref, on_disk).await,
            Err(_) => on_disk,
        };
        let label = format!("edit {script}");
        let suffix = std::path::Path::new(&script)
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_else(|| ".sql".to_string());
        // In-place mode places the temp file in the script's real
        // directory so external tools (LSPs, formatters) discover the
        // same config files (e.g. postgres-language-server.jsonc) that
        // would apply to the persisted script.
        let tempfile_dir = if in_place {
            path.as_ref().and_then(|p| p.parent().map(PathBuf::from))
        } else {
            None
        };
        Self {
            adapter,
            database,
            script,
            template,
            label,
            suffix,
            tempfile_dir,
        }
    }

    async fn persist(&self, content: &str) -> std::io::Result<()> {
        let path = script_path(&self.adapter, &self.database, &self.script)
            .ok_or_else(|| std::io::Error::other("adapter offers no script store for this node"))?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, content).await
    }
}

#[async_trait]
impl EditSession for AdapterDbScriptSession {
    fn template(&self) -> &str {
        &self.template
    }

    fn suffix(&self) -> &str {
        &self.suffix
    }

    fn spawn_context(&self) -> EditorSpawnContext {
        // The completions-line trick gives the editor a static hint, but
        // an LSP needs real DB access. Pull whatever env the adapter
        // wants its children to see — for a Postgres adapter that's the
        // libpq `PG*` vars derived from the live tunnel + resolved
        // password.
        let nref_str = node_ref_string(&self.adapter, &self.database, &self.script);
        let parsed = NodeRef::parse(&nref_str);
        let child_env = parsed
            .as_ref()
            .map(|nref| self.adapter.child_process_env(nref))
            .unwrap_or_default();
        not_yet_done_content::http_log::log_debug(
            "db_script.spawn_context",
            &format!(
                "nref={:?} parse_ok={} child_env.len={} tempfile_dir={:?}",
                nref_str,
                parsed.is_ok(),
                child_env.len(),
                self.tempfile_dir,
            ),
        );
        EditorSpawnContext {
            tempfile_dir: self.tempfile_dir.clone(),
            // Only emit a prefix when we're actually in-place: a prefix
            // without a `tempfile_dir` would clutter `$TMPDIR` with
            // suspicious-looking dotfiles.
            tempfile_prefix: self.tempfile_dir.as_ref().map(|_| TEMPFILE_PREFIX),
            child_env,
            persistent_file: None,
        }
    }

    fn scope(&self) -> SessionScope {
        SessionScope::Content
    }

    fn label(&self) -> &str {
        &self.label
    }

    async fn commit(&mut self, text: &str) -> CommitOutcome {
        // Two-stage strip: the completions line (editor-only) goes
        // first so it never makes it onto disk; then the error banner
        // (also editor-only, prepended on Reopen) is removed. Order
        // doesn't actually matter since the two markers can't overlap,
        // but doing completions first keeps the on-disk diff against
        // `disk_body` purely on user-authored content.
        let without_completions = self.adapter.strip_editor_hints(text);
        let stripped = strip_error_banner(&without_completions).to_string();
        match self.persist(&stripped).await {
            Ok(()) => CommitOutcome::Done {
                message: Some(format!("Saved db script '{}'", self.script)),
            },
            Err(e) => CommitOutcome::Reopen {
                content: render_with_error(&stripped, &format!("Write failed: {e}")),
            },
        }
    }
}

/// Canonical `NodeRef` string for a database-level script:
/// `<adapter_type>/<database>/db_scripts/<script>`. Matches the node id
/// the adapter's own db-script nodes carry, so `augment_editor_buffer`
/// and `child_process_env` resolve it the same way they would for a row
/// the user selected in the list.
fn node_ref_string(adapter: &Arc<dyn ContentAdapter>, database: &str, script: &str) -> String {
    format!(
        "{}/{}/db_scripts/{}",
        adapter.adapter_type(),
        database,
        script
    )
}

fn script_path(adapter: &Arc<dyn ContentAdapter>, database: &str, script: &str) -> Option<PathBuf> {
    Some(adapter.script_store()?.db_script_path(database, script))
}

fn default_body(adapter: &Arc<dyn ContentAdapter>, database: &str, script: &str) -> String {
    adapter
        .script_store()
        .map(|s| s.default_db_script_body(database, script))
        .unwrap_or_default()
}
