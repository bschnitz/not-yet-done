//! Postgres server-side cursor sessions.
//!
//! Pattern (DBeaver-style):
//!
//! ```text
//! BEGIN;
//! DECLARE _nyd_cur_<id> NO SCROLL CURSOR FOR <user-sql>;
//! FETCH FORWARD <page_size + 1> FROM _nyd_cur_<id>;   -- repeat per page
//! …
//! ROLLBACK;                                            -- on close
//! ```
//!
//! Each session owns a **dedicated** `tokio_postgres::Client` that is
//! kept _outside_ of `PostgresClient.sessions` — the cursor needs its
//! own transaction state, and we do not want one user's pinned cursor
//! to block another database operation reusing the cached session.
//!
//! Multi-statement scripts (e.g. `SET search_path = foo; SELECT …`)
//! are supported: the prelude runs inside the same transaction before
//! the cursor declaration. When the trailing statement is _not_ a
//! SELECT/WITH the cursor path bails out and the caller is expected to
//! fall back to `execute_raw_sql`.

use std::sync::Arc;

use tokio_postgres::{Client, SimpleQueryMessage};

use crate::client::sql_shape::{
    has_multiple_statements, looks_like_select_or_with, split_trailing_statement,
    strip_leading_sql_noise,
};
use crate::client::{PostgresClient, RawSqlOutcome, RowsPage, quote_ident};

/// Live cursor handle. The struct is opaque to the TUI — only the
/// adapter-level registry (Phase 3) ever touches its fields.
pub struct CursorSession {
    /// Dedicated session driving the open transaction. Dropping the
    /// session aborts the transaction, which is the recovery path if
    /// the caller forgets to call `close_cursor`.
    pub(crate) client: Arc<Client>,
    pub(crate) cursor_name: String,
    pub(crate) database: String,
    pub(crate) columns: Vec<String>,
    /// Logical row index of the next row the cursor will fetch.
    /// Maintained client-side because `FETCH` does not return its own
    /// offset; we just sum the page sizes we've already consumed.
    pub(crate) next_offset: u32,
}

impl CursorSession {
    pub fn database(&self) -> &str {
        &self.database
    }

    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    pub fn next_offset(&self) -> u32 {
        self.next_offset
    }

    pub fn cursor_name(&self) -> &str {
        &self.cursor_name
    }
}

/// Result of [`PostgresClient::open_cursor`].
///
/// `Cursor` paths return both the live session and the rows fetched on
/// the very first page (avoids a round-trip — the only reason we
/// declared the cursor was to read rows out of it).
///
/// `NonCursor` is the fallback for scripts whose trailing statement is
/// not a SELECT/WITH: we executed the whole script directly and got
/// back a one-shot `RawSqlOutcome` (DDL/DML status, possibly an empty
/// rowset).
pub enum OpenCursorOutcome {
    Cursor {
        session: CursorSession,
        first_page: RowsPage,
    },
    NonCursor(RawSqlOutcome),
}

impl PostgresClient {
    /// Open a NO-SCROLL cursor for the trailing `SELECT/WITH` of the
    /// given script. Prelude statements (everything before the last
    /// `;` separator) execute inside the same transaction.
    ///
    /// `page_size` is the user-visible page size; we actually
    /// `FETCH FORWARD <page_size + 1>` so the caller can decide
    /// "has_more" without a follow-up query.
    pub async fn open_cursor(
        &self,
        dbname: &str,
        sql: &str,
        page_size: u32,
    ) -> Result<OpenCursorOutcome, String> {
        self.run_with_timeout(&format!("open cursor on {dbname}"), async {
            // Inspect the trailing statement before doing any network
            // work. The fallback path (non-SELECT) shares the regular
            // `execute_raw_sql` codepath, so we don't even need a
            // dedicated session for it.
            let stripped = strip_leading_sql_noise(sql);
            let body_view = stripped.trim_end().trim_end_matches(';').trim();
            let (prelude, last) = match split_trailing_statement(stripped) {
                Some((p, l)) => (Some(p), l),
                None => (None, body_view),
            };
            if !looks_like_select_or_with(last) {
                let outcome = self.execute_raw_sql_owned(dbname, sql).await?;
                return Ok(OpenCursorOutcome::NonCursor(outcome));
            }
            // The last-statement keyword check rules out a SELECT with
            // an embedded `;` inside a string literal: `has_multiple`
            // is true on the whole body when the split picked it up
            // wrongly. In that case we treat the script as a single
            // statement (no prelude).
            let (prelude, last) = if prelude.is_some() && !has_multiple_statements(body_view) {
                (None, body_view)
            } else {
                (prelude, last)
            };

            let session = self.connect_dedicated_session(dbname).await?;
            let cursor_name = mint_cursor_name();

            // Build one `simple_query` payload so the BEGIN, prelude,
            // and DECLARE arrive in a single round-trip. Cursors are
            // tied to the transaction in which they were declared, so
            // we must NOT split this across calls.
            let prelude_sql = prelude.map(|p| format!("{p} ")).unwrap_or_default();
            let declare = format!(
                "BEGIN; {prelude_sql}DECLARE {} NO SCROLL CURSOR FOR {};",
                quote_ident(&cursor_name),
                last,
            );
            session
                .simple_query(&declare)
                .await
                .map_err(|e| format!("declare cursor on {dbname}: {e}"))?;

            // First page. `+1` lets the caller derive `has_more` from
            // page length, matching the LIMIT/OFFSET path.
            let fetch_sql = format!(
                "FETCH FORWARD {} FROM {}",
                page_size.saturating_add(1),
                quote_ident(&cursor_name),
            );
            let (columns, rows) = collect_fetch(&session, &fetch_sql).await?;
            let has_more = rows.len() as u32 > page_size;
            let rows = if has_more {
                rows.into_iter().take(page_size as usize).collect()
            } else {
                rows
            };

            let next_offset = if has_more {
                page_size
            } else {
                rows.len() as u32
            };
            let first_page = RowsPage {
                columns: columns.clone(),
                rows,
                has_more,
            };
            let session = CursorSession {
                client: session,
                cursor_name,
                database: dbname.to_string(),
                columns,
                next_offset,
            };
            Ok(OpenCursorOutcome::Cursor {
                session,
                first_page,
            })
        })
        .await
    }

    /// `FETCH FORWARD <page_size + 1>` from an existing cursor.
    ///
    /// The session is updated in place so its `next_offset` reflects
    /// rows already consumed.
    pub async fn fetch_cursor_page(
        &self,
        session: &mut CursorSession,
        page_size: u32,
    ) -> Result<RowsPage, String> {
        self.run_with_timeout(
            &format!("fetch from cursor on {}", session.database),
            async {
                let fetch_sql = format!(
                    "FETCH FORWARD {} FROM {}",
                    page_size.saturating_add(1),
                    quote_ident(&session.cursor_name),
                );
                let (columns, rows) = collect_fetch(&session.client, &fetch_sql).await?;
                let has_more = rows.len() as u32 > page_size;
                let rows: Vec<Vec<Option<String>>> = if has_more {
                    rows.into_iter().take(page_size as usize).collect()
                } else {
                    rows
                };
                session.next_offset = session.next_offset.saturating_add(rows.len() as u32);
                // Column list might have changed only if the cursor
                // was re-declared, which we don't support; mirror the
                // session columns for stability.
                let columns = if columns.is_empty() {
                    session.columns.clone()
                } else {
                    columns
                };
                Ok(RowsPage {
                    columns,
                    rows,
                    has_more,
                })
            },
        )
        .await
    }

    /// `ROLLBACK` the transaction holding the cursor, then drop the
    /// dedicated session. After this call the [`CursorSession`] is
    /// consumed and any attempt to reuse its handles would be a
    /// compile error.
    pub async fn close_cursor(&self, session: CursorSession) -> Result<(), String> {
        let label = format!("close cursor on {}", session.database);
        self.run_with_timeout(&label, async move {
            // Use simple_query so the response is just consumed; we
            // don't care about its content.
            session
                .client
                .simple_query("ROLLBACK")
                .await
                .map_err(|e| format!("rollback cursor: {e}"))?;
            Ok(())
        })
        .await
    }

    /// Variant of [`PostgresClient::execute_raw_sql`] that owns its
    /// own retry wrapper. Used by `open_cursor`'s fallback path so we
    /// don't double-wrap `run_with_timeout`.
    async fn execute_raw_sql_owned(
        &self,
        dbname: &str,
        sql: &str,
    ) -> Result<RawSqlOutcome, String> {
        // We're already inside `run_with_timeout` — call the inner
        // logic directly via the public surface to avoid duplicating
        // the simple_query scraping. We _do_ duplicate the run_with_
        // wrapper one level by calling `execute_raw_sql`, but at
        // worst that's two nested watch::send transitions and a
        // single network round-trip; the alternative is exporting a
        // raw helper.
        self.execute_raw_sql(dbname, sql).await
    }
}

async fn collect_fetch(
    client: &Client,
    sql: &str,
) -> Result<(Vec<String>, Vec<Vec<Option<String>>>), String> {
    let messages = client
        .simple_query(sql)
        .await
        .map_err(|e| format!("fetch: {e}"))?;

    let mut columns: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    for msg in messages {
        match msg {
            SimpleQueryMessage::RowDescription(cols) => {
                columns = cols.iter().map(|c| c.name().to_string()).collect();
            }
            SimpleQueryMessage::Row(row) => {
                if columns.is_empty() {
                    columns = row.columns().iter().map(|c| c.name().to_string()).collect();
                }
                let cells = (0..row.len())
                    .map(|i| row.get(i).map(|s| s.to_string()))
                    .collect();
                rows.push(cells);
            }
            _ => {}
        }
    }
    Ok((columns, rows))
}

fn mint_cursor_name() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    // 14 hex chars is plenty for a per-process unique cursor name and
    // keeps the SQL short for debugging.
    format!("_nyd_cur_{:08x}{:06x}", ts & 0xFFFF_FFFF, n & 0xFFFFFF)
}
