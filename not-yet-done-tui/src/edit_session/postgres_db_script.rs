//! Edit session for a DB-level Postgres script (the `e` shortcut on a
//! `postgres:db_script` row).
//!
//! On disk under `<instance_data_dir>/db_scripts/<database>/<script>.sql`.
//! The buffer layout is identical to the table-scoped query editor
//! (scratch area, then [`QUERY_MARKER`], then SQL), but this session
//! does NOT auto-execute on `:w` — it just persists. The user re-runs
//! the script via the `x` shortcut, which goes through
//! [`Node::invoke_action`] → [`ActionDispatch::ExecuteQuery`] →
//! `RunAdapterDbScript`. That keeps the edit-then-save path off the hot
//! cursor lifecycle: re-executing into a result pane requires knowing
//! which pane is the script's "current" result, which the edit session
//! doesn't have when it's spawned from the db_scripts list view.
//!
//! Error handling mirrors [`super::postgres_query`]: a SQL-comment
//! banner is prepended on reopen and stripped on the next parse. The
//! marker stays the same so the user gets the same scratch/query split
//! they're used to from the per-table editor.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::{ContentAdapter, NodeRef};
use not_yet_done_postgres_adapter::query::{
    db_script_file_path, default_db_script_file, is_sql_extension,
};
use not_yet_done_postgres_adapter::script_completions::{
    append_completions_line, build_completions_line, strip_completions_line,
};
use not_yet_done_postgres_adapter::PostgresAdapter;

use super::{CommitOutcome, EditSession, EditorSpawnContext, SessionScope};

pub struct PostgresDbScriptSession {
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

impl PostgresDbScriptSession {
    /// Open the script file under
    /// `<instance_data_dir>/db_scripts/<database>/<script>.sql`. Missing
    /// files yield the default template (scratch area + marker +
    /// placeholder SELECT).
    ///
    /// If the adapter is a `PostgresAdapter`, the buffer is augmented
    /// with a trailing `-- table completions: tt_<schema>__<table>, …`
    /// comment listing every base table in `database`. The line is
    /// purely an editor convenience — it is stripped again on commit
    /// so the on-disk file never grows it. Failure to enumerate the
    /// tables is swallowed: the editor still opens, just without the
    /// completion hint.
    pub async fn open(
        adapter: Arc<dyn ContentAdapter>,
        database: String,
        script: String,
        in_place: bool,
    ) -> Self {
        let path = db_script_file_path(&adapter.instance_data_dir(), &database, &script);
        let on_disk = match tokio::fs::read_to_string(&path).await {
            Ok(text) => text,
            Err(_) => default_db_script_file(&database, &script),
        };
        // Strip defensively: a previous version of the editor might
        // have persisted a completion line that we now want to refresh.
        let stripped = strip_completions_line(&on_disk);
        // The completions line is a SQL comment, so it only makes
        // sense for SQL-flavored scripts. Other extensions (e.g. `.py`,
        // `.md`) would treat `-- table completions: …` as syntax noise.
        let template = if is_sql_extension(&script) {
            if let Some(pg) = adapter.as_any().downcast_ref::<PostgresAdapter>() {
                let tables = pg.list_completion_tables(&database).await;
                match build_completions_line(&tables) {
                    Some(line) => append_completions_line(&stripped, &line),
                    None => stripped,
                }
            } else {
                stripped
            }
        } else {
            stripped
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
            path.parent().map(|p| p.to_path_buf())
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

    fn script_path(&self) -> PathBuf {
        db_script_file_path(&self.adapter.instance_data_dir(), &self.database, &self.script)
    }

    async fn persist(&self, content: &str) -> std::io::Result<()> {
        let path = self.script_path();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, content).await
    }
}

#[async_trait]
impl EditSession for PostgresDbScriptSession {
    fn template(&self) -> &str {
        &self.template
    }

    fn suffix(&self) -> &str {
        &self.suffix
    }

    fn spawn_context(&self) -> EditorSpawnContext {
        // The completions-line trick gives the editor a static hint, but
        // an LSP needs real DB access. Pull whatever env the adapter
        // wants its children to see — for `PostgresAdapter` that's the
        // libpq `PG*` vars derived from the live tunnel + resolved
        // password. We pass a synthetic NodeRef carrying the database so
        // the adapter can fill `PGDATABASE`; the head segment is just a
        // routing-shape placeholder (the adapter only cares about
        // segment[1]).
        let nref_str = format!(
            "{}/{}/db_scripts/{}",
            self.adapter.adapter_type(),
            self.database,
            self.script
        );
        let parsed = NodeRef::parse(&nref_str);
        let child_env = parsed
            .as_ref()
            .map(|nref| self.adapter.child_process_env(nref))
            .unwrap_or_default();
        not_yet_done_content::http_log::log_debug(
            "pg_db_script.spawn_context",
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
        let without_completions = strip_completions_line(text);
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

// ---------------------------------------------------------------------------
// Error banner — same SQL-comment block syntax as `postgres_query`
// ---------------------------------------------------------------------------

const ERROR_BANNER_START: &str = "-- ─── ERRORS ───";
const ERROR_BANNER_END: &str = "-- ─────────────────";

fn strip_error_banner(text: &str) -> &str {
    if let Some(rest) = text.strip_prefix(ERROR_BANNER_START) {
        let after_start = rest.strip_prefix('\n').unwrap_or(rest);
        let needle = format!("\n{ERROR_BANNER_END}");
        if let Some(pos) = after_start.find(&needle) {
            let after_end = &after_start[pos + needle.len()..];
            return after_end.strip_prefix('\n').unwrap_or(after_end);
        }
        return after_start;
    }
    text
}

fn render_with_error(stripped: &str, error: &str) -> String {
    let mut out = String::new();
    out.push_str(ERROR_BANNER_START);
    out.push('\n');
    for line in error.lines() {
        out.push_str(&format!("-- • {line}\n"));
    }
    out.push_str(ERROR_BANNER_END);
    out.push('\n');
    out.push_str(stripped);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_round_trip() {
        let body = "SELECT 1;\n";
        let once = render_with_error(body, "boom");
        assert!(once.starts_with(ERROR_BANNER_START));
        assert_eq!(strip_error_banner(&once), body);
    }

    #[test]
    fn strip_idempotent_without_banner() {
        let text = "no banner here\n";
        assert_eq!(strip_error_banner(text), text);
    }
}
