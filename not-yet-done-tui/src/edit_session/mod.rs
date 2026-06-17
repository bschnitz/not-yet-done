//! Polymorphic editor sessions.
//!
//! An `EditSession` is the App-side handle for one round of "open external
//! editor, get text back". The App knows nothing about what is being edited —
//! it asks the session for the initial template, hands the saved buffer back
//! on close, and acts on a small `CommitOutcome` enum.
//!
//! All format-specific work (parsing, validation, error rendering, conflict
//! handling, backend writes) lives behind the trait, on whatever type
//! implements it. There is intentionally no shared toolkit: each session is
//! free to design its own buffer format and error syntax.

use async_trait::async_trait;
use not_yet_done_content::{NodeSummary, PageInfo};

use crate::views::content_view::{CustomQueryRunState, PaneId};

mod content_query_filter;
mod error_view;
mod file_edit;
mod node_action;
mod postgres_db_script;
mod postgres_query;
mod saved_query;
mod tag_form;
mod tracking_query_filter;
mod tracking_script;
mod tracking_script_output;

pub use content_query_filter::ContentQueryFilterSession;
pub use error_view::ErrorViewSession;
pub use file_edit::FileEditSession;
pub use node_action::{NavContext, NodeActionEditSession, ReloadTarget};
pub use postgres_db_script::PostgresDbScriptSession;
pub use postgres_query::{
    parse_query_area as postgres_parse_query_area, PostgresQuerySession,
    DEFAULT_PAGE_SIZE as POSTGRES_QUERY_DEFAULT_PAGE_SIZE,
};
pub use saved_query::SavedQueryEditSession;
pub use tag_form::TagFormSession;
pub use tracking_query_filter::TrackingQueryFilterSession;
pub use tracking_script::ScriptSession;
pub use tracking_script_output::ScriptOutputSession;

/// Which tab owns this session — drives action-bar slot selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionScope {
    Tasks,
    Trackings,
    Content,
}

/// Knobs the App applies when spawning the external editor for a
/// session. Combined into one struct so sessions hand back a single
/// value and the dispatch layer threads exactly one parameter through.
///
/// All fields are independent and default to "don't override":
///
/// - `tempfile_dir = None` → use `$TMPDIR` (the normal case).
/// - `tempfile_prefix = None` → use `tempfile`'s default random name.
/// - `child_env` empty → inherit only the parent's environment.
///
/// See the trait method [`EditSession::spawn_context`] for the lifecycle.
#[derive(Debug, Clone, Default)]
pub struct EditorSpawnContext {
    /// Directory in which to create the editor's temp file. `Some(path)`
    /// opts into "in-place" editing: the temp file is created alongside
    /// the real persisted file so external tools (LSPs, formatters)
    /// discover sibling config files (e.g.
    /// `postgres-language-server.jsonc`) by walking up from the buffer's
    /// directory.
    pub tempfile_dir: Option<std::path::PathBuf>,
    /// Filename prefix for the editor's temp file. Combined with the
    /// random component from `tempfile::Builder` and the suffix to form
    /// the final name (e.g. `.nyd_tmp_` + `aBc123` + `.sql` →
    /// `.nyd_tmp_aBc123.sql`). Only meaningful when paired with
    /// [`Self::tempfile_dir`].
    pub tempfile_prefix: Option<&'static str>,
    /// Extra environment variables to set on the editor's child process.
    /// Adapters expose connection state here (e.g. libpq `PG*` vars from
    /// the Postgres adapter's live tunnel) so an editor-spawned LSP can
    /// talk to the same backend the TUI is connected to without the
    /// user duplicating credentials in a sidecar config.
    ///
    /// The map is a snapshot at spawn time — not refreshed if connection
    /// state changes later.
    pub child_env: std::collections::HashMap<String, String>,
}

/// One round-trip through `$EDITOR`.
///
/// Methods are called by the App's editor lifecycle:
/// 1. `template()` + `suffix()` are read once when the editor is opened.
/// 2. `live_apply()` is called for each intermediate save (`:w`) while the
///    detached editor is still open. Default impl is a no-op.
/// 3. `commit()` is called once when the editor closes (or after a `:w` in
///    inline mode). Its `CommitOutcome` tells the App what to do next.
#[async_trait]
pub trait EditSession: Send + Sync {
    /// Initial buffer that gets written to the temp file.
    fn template(&self) -> &str;

    /// File suffix for `$EDITOR` syntax highlighting (`.md`, `.yaml`, `.py`,
    /// …). Returned as a slice so the trait stays object-safe; the App leaks
    /// a `'static` copy when dispatching to `EditorRequest`.
    fn suffix(&self) -> &str;

    /// Adapter-/session-specific knobs the App applies when spawning the
    /// external editor: where the temp file lives, what to prefix it
    /// with, and which env vars to propagate to the child. See
    /// [`EditorSpawnContext`] for the full set. Default returns an
    /// empty context (`$TMPDIR` + no extra env), so sessions opt in
    /// only when they need it.
    ///
    /// Combining the three knobs into one struct means new spawn-time
    /// knobs (e.g. cwd, ulimit) can be added without touching every
    /// `EditSession` impl or every dispatch site.
    fn spawn_context(&self) -> EditorSpawnContext {
        EditorSpawnContext::default()
    }

    /// Name of the editor profile (a key under `editors:`) the App should
    /// resolve and spawn for this session. `None` → the `default` profile.
    /// Only content node-action sessions override this (from the view
    /// config's per-action `editor:` field); all others use `default`.
    fn editor_profile(&self) -> Option<&str> {
        None
    }

    /// Tab that owns this session.
    fn scope(&self) -> SessionScope;

    /// Short label shown in the active editor slot of the action bar
    /// (e.g. "add", "edit").
    fn label(&self) -> &str;

    /// Editor closed; the saved buffer is `text`.
    async fn commit(&mut self, text: &str) -> CommitOutcome;

    /// Intermediate save (`:w`) — only fires for detached editors. Default
    /// no-op so sessions opt in only when they want live behaviour.
    /// Optional `FollowUp` is dispatched by the App, identical to commit's
    /// follow-up path.
    async fn live_apply(&mut self, _text: &str) -> Option<FollowUp> {
        None
    }
}

/// Result of `commit`. Maps directly onto what the App's editor loop should
/// do next.
pub enum CommitOutcome {
    /// Saved successfully. Optional notification text.
    Done { message: Option<String> },

    /// User-facing problem (parse / validation / conflict). The session has
    /// produced a fresh buffer with the error already rendered in whatever
    /// syntax fits the format. App reopens the editor with this content and
    /// hands the next save back to the same session.
    Reopen { content: String },

    /// User cancelled or nothing to do. Optional notification text.
    Cancelled { message: Option<String> },

    /// Saved, but the App has more work to do (e.g. prompt for a shortcut).
    FollowUp(FollowUp),
}

/// Side-effects that only the App can perform after a session finishes.
pub enum FollowUp {
    /// An in-place edit (`edit`/notes) succeeded; patch only the edited
    /// row in the originating pane ([`crate::views::content_view::ContentView::patch_row`])
    /// instead of full-reloading — reload is reserved for external changes.
    /// The App re-fetches the node's fresh content and keeps the visible
    /// row's structural fields; it falls back to a pane reload when the row
    /// isn't visible or the fetch fails.
    PatchContentRow {
        view_index: usize,
        pane_id: PaneId,
        node_id: String,
        message: String,
    },
    /// A child-create action (`add`/`A`) succeeded; splice the new child
    /// into the originating pane *locally* — never a full reload (reload is
    /// reserved for external changes, `r`). For a tree pane the App arms the
    /// parent's expansion and re-fetches only that parent's children
    /// (`spawn_tree_expand`), so `expanded` and every sibling subtree cache
    /// stay intact (no collapse) and the cursor stays on the parent. For a
    /// flat/drill pane it re-runs the drill-down at the parent level — the
    /// historical create behaviour.
    InsertContentChild {
        view_index: usize,
        pane_id: PaneId,
        parent_node_id: String,
        child_node_type: String,
        message: String,
    },
    /// A tag create/edit committed from a content/adapter tab (`type: tag`)
    /// succeeded; reload the originating content pane so its `tag_symbols` /
    /// `tag_names` columns re-render. Kept distinct from the in-place
    /// content-row patch path so the tag flow stays self-contained.
    ReloadContentPaneForTag {
        view_index: usize,
        pane_id: PaneId,
        message: String,
    },
    /// Live-apply a YAML tracking filter without persisting.
    ApplyTrackingFilter { content: String },
    /// Live-apply a content view query without persisting.
    ApplyContentFilter {
        view_index: usize,
        content: String,
        save_name: Option<String>,
    },
    /// Final close: apply + persist tracking filter; optional shortcut prompt.
    CloseTrackingFilter {
        content: String,
        name: String,
        is_new: bool,
    },
    /// Final close: apply + optionally save content query; optional shortcut prompt.
    CloseContentQuery {
        view_index: usize,
        content: String,
        save_name: Option<String>,
        is_new: bool,
    },
    /// Set the inline query-error overlay (e.g. tree-edit parse/apply error).
    SetQueryError(String),
    /// Replace the items shown in a specific content pane with the
    /// result of a custom adapter query (e.g. raw SQL via the Postgres
    /// query editor). `status` is an optional bar message — `Some` for
    /// non-resultset statements (`"5 row(s) affected"`), `None` when
    /// the rows themselves are the answer. Errors take a different
    /// path (`SetQueryError`).
    ///
    /// `page` carries adapter-reported pagination info (the half-open
    /// window the result represents, `has_next`/`has_prev` flags) so
    /// the pane's next/prev-page keys can re-execute the query.
    /// `custom_query` is the source the pane re-runs on a page flip;
    /// `None` for non-paginable results (DDL/DML, multi-statement).
    ReplaceContentItems {
        view_index: usize,
        pane_id: PaneId,
        items: Vec<NodeSummary>,
        status: Option<String>,
        page: Option<PageInfo>,
        custom_query: Option<CustomQueryRunState>,
    },
    /// A YAML config file under `~/.config/not_yet_done/` was saved.
    /// The App attempts a granular reload (single view rebuilt) or a
    /// full reload (tui.yaml / adapter config). Reload failures reopen
    /// the editor with an error banner via
    /// [`crate::edit_session::FileEditSession::with_error`] — the old
    /// in-memory config keeps running until the user fixes the file.
    ReloadConfig { path: std::path::PathBuf },
    /// A saved-query body was edited (`:query edit/new`); the content
    /// view at `view_index` should refresh its saved-query list so the
    /// new body is picked up on next apply. `message` is surfaced via
    /// the notification bar.
    ReloadContentSavedQueries {
        view_index: usize,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal echo session: template fixed, commit returns whatever it gets.
    struct EchoSession {
        template: String,
        last_commit: Option<String>,
    }

    #[async_trait]
    impl EditSession for EchoSession {
        fn template(&self) -> &str { &self.template }
        fn suffix(&self) -> &str { ".md" }
        fn scope(&self) -> SessionScope { SessionScope::Tasks }
        fn label(&self) -> &str { "echo" }
        async fn commit(&mut self, text: &str) -> CommitOutcome {
            self.last_commit = Some(text.to_string());
            CommitOutcome::Done { message: Some(format!("got {} bytes", text.len())) }
        }
    }

    #[tokio::test]
    async fn trait_is_object_safe_and_callable() {
        let mut session: Box<dyn EditSession> = Box::new(EchoSession {
            template: "hello".into(),
            last_commit: None,
        });
        assert_eq!(session.template(), "hello");
        assert_eq!(session.suffix(), ".md");

        let outcome = session.commit("world").await;
        match outcome {
            CommitOutcome::Done { message } => {
                assert_eq!(message.as_deref(), Some("got 5 bytes"));
            }
            _ => panic!("expected Done"),
        }
    }

    #[tokio::test]
    async fn live_apply_default_is_noop() {
        let mut session = EchoSession { template: String::new(), last_commit: None };
        let follow_up = session.live_apply("anything").await;
        assert!(follow_up.is_none());
        assert!(session.last_commit.is_none());
    }
}
