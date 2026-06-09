//! Edit session for the Postgres SQL editor (the `Q` keybind on a
//! Postgres rows view).
//!
//! Buffer layout: a free scratch area, then a marker line
//! (`-- ▼ THIS SQL WILL BE EXECUTED ON SAVE ▼`), then the SQL the
//! live-apply hook executes on every `:w`. On error a comment-banner
//! block is prepended on reopen and stripped on the next parse.
//!
//! The adapter persists the saved buffer to
//! `<instance_data_dir>/queries/<database>/<schema>/<table>/<script>.sql`
//! after each successful run so a TUI crash doesn't lose the user's
//! work.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
pub use not_yet_done_postgres_adapter::query::parse_query_area;
use not_yet_done_postgres_adapter::query::{
    default_query_file, query_file_path, DEFAULT_SCRIPT_NAME,
};

use not_yet_done_content::{ContentAdapter, CustomQueryContext, PageRequest};

use crate::views::content_view::{CustomQueryRunState, PaneId};

use super::{CommitOutcome, EditSession, FollowUp, SessionScope};

/// Default page size used when the Q-editor runs a SELECT. Matches the
/// `page_size: 100` of the YAML Rows-child so paginated custom-query
/// results render at the same grain as the regular row drill-down.
pub const DEFAULT_PAGE_SIZE: u32 = 100;

pub struct PostgresQuerySession {
    adapter: Arc<dyn ContentAdapter>,
    database: String,
    schema: String,
    table: String,
    script: String,
    view_index: usize,
    pane_id: PaneId,
    template: String,
}

impl PostgresQuerySession {
    pub fn new(
        adapter: Arc<dyn ContentAdapter>,
        database: String,
        schema: String,
        table: String,
        script: String,
        view_index: usize,
        pane_id: PaneId,
        initial_buffer: String,
    ) -> Self {
        Self {
            adapter,
            database,
            schema,
            table,
            script,
            view_index,
            pane_id,
            template: initial_buffer,
        }
    }

    /// Convenience: read the persisted query buffer from
    /// `<instance_data_dir>/queries/<db>/<schema>/<table>/<script>.sql`,
    /// or fall back to the default template. Defaults to the
    /// [`DEFAULT_SCRIPT_NAME`] script.
    pub async fn open(
        adapter: Arc<dyn ContentAdapter>,
        database: String,
        schema: String,
        table: String,
        view_index: usize,
        pane_id: PaneId,
    ) -> Self {
        Self::open_named(
            adapter,
            database,
            schema,
            table,
            DEFAULT_SCRIPT_NAME.to_string(),
            view_index,
            pane_id,
        )
        .await
    }

    /// Like [`open`] but for an explicitly named script under the same
    /// table directory. Used by the Skripte subview.
    pub async fn open_named(
        adapter: Arc<dyn ContentAdapter>,
        database: String,
        schema: String,
        table: String,
        script: String,
        view_index: usize,
        pane_id: PaneId,
    ) -> Self {
        let path = query_path(&adapter, &database, &schema, &table, &script);
        let initial = match tokio::fs::read_to_string(&path).await {
            Ok(text) => text,
            Err(_) => default_query_file(&schema, &table),
        };
        Self::new(
            adapter, database, schema, table, script, view_index, pane_id, initial,
        )
    }

    fn context(&self) -> CustomQueryContext {
        CustomQueryContext::new()
            .with("database", self.database.clone())
            .with_page(PageRequest {
                offset: 0,
                limit: DEFAULT_PAGE_SIZE,
            })
    }

    async fn persist(&self, content: &str) {
        let path = query_path(
            &self.adapter,
            &self.database,
            &self.schema,
            &self.table,
            &self.script,
        );
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
            database: self.database.clone(),
            mode: crate::config::view_config::PaginationMode::Server,
            cursor_id: None,
        }
    }
}

#[async_trait]
impl EditSession for PostgresQuerySession {
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

fn query_path(
    adapter: &Arc<dyn ContentAdapter>,
    database: &str,
    schema: &str,
    table: &str,
    script: &str,
) -> PathBuf {
    query_file_path(&adapter.instance_data_dir(), database, schema, table, script)
}

// ---------------------------------------------------------------------------
// Error banner (mirrors restructure.rs's pattern, but in SQL comments)
// ---------------------------------------------------------------------------

const ERROR_BANNER_START: &str = "-- ─── ERRORS ───";
const ERROR_BANNER_END: &str = "-- ─────────────────";

/// Strip a previously rendered error-banner block from the start of
/// `text` so reopening on a still-broken query doesn't stack banners.
pub(crate) fn strip_error_banner(text: &str) -> &str {
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

fn render_with_error(text: &str, error: &str) -> String {
    let stripped = strip_error_banner(text);
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
    use not_yet_done_postgres_adapter::query::QUERY_MARKER;

    #[test]
    fn parse_extracts_query_below_marker() {
        let text = format!(
            "-- scratch line\n\
             {QUERY_MARKER}\n\
             SELECT 1;\n"
        );
        assert_eq!(parse_query_area(&text), "SELECT 1;\n");
    }

    #[test]
    fn parse_with_no_marker_returns_full_text() {
        let text = "SELECT 1;\n";
        assert_eq!(parse_query_area(text), "SELECT 1;\n");
    }

    #[test]
    fn parse_handles_marker_with_trailing_whitespace() {
        let text = format!("scratch\n{QUERY_MARKER}   \nSELECT 1;\n");
        assert_eq!(parse_query_area(&text), "SELECT 1;\n");
    }

    #[test]
    fn parse_handles_multi_statement_query_area() {
        let text = format!(
            "{QUERY_MARKER}\n\
             UPDATE t SET x = 1;\n\
             SELECT * FROM t;\n"
        );
        assert_eq!(
            parse_query_area(&text),
            "UPDATE t SET x = 1;\nSELECT * FROM t;\n"
        );
    }

    #[test]
    fn parse_returns_empty_when_marker_is_last_line() {
        let text = format!("scratch\n{QUERY_MARKER}\n");
        assert_eq!(parse_query_area(&text), "");
    }

    #[test]
    fn strip_banner_removes_block() {
        let with_banner = format!(
            "{ERROR_BANNER_START}\n\
             -- • boom\n\
             {ERROR_BANNER_END}\n\
             rest\n"
        );
        assert_eq!(strip_error_banner(&with_banner), "rest\n");
    }

    #[test]
    fn strip_banner_idempotent_without_banner() {
        let text = "no banner here\n";
        assert_eq!(strip_error_banner(text), text);
    }

    #[test]
    fn render_with_error_does_not_stack() {
        let body = "SELECT 1;\n";
        let once = render_with_error(body, "syntax error");
        let twice = render_with_error(&once, "syntax error");
        assert_eq!(once, twice);
    }
}
