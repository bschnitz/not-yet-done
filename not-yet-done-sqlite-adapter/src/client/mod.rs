//! Connection handling and catalogue reads.
//!
//! One pool per database file, opened lazily and cached — a SQLite handle
//! is cheap, but not so cheap that we want a fresh one per keystroke.
//!
//! The catalogue queries are deliberately conservative: `sqlite_master`
//! rather than the newer `sqlite_schema` alias or the `pragma_table_list`
//! table-valued function, so the adapter also works against an
//! unbundled system libsqlite3 that predates them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::TryStreamExt;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::{Column, Executor, Row, TypeInfo, ValueRef};
use tokio::sync::{Mutex, watch};

use not_yet_done_content::AdapterStatus;
use not_yet_done_sql_core::{RowCell, quote_ident};

use crate::sources::{SourceEntry, resolve_sources};

/// One database file, as the tree renders it.
#[derive(Clone, Debug)]
pub struct DatabaseEntry {
    pub key: String,
    pub label: String,
    pub path: PathBuf,
    /// File size in bytes, `None` when it could not be read.
    pub size_bytes: Option<u64>,
}

/// One table or view inside a database file.
#[derive(Clone, Debug)]
pub struct TableEntry {
    /// The owning database's [`SourceEntry::key`].
    pub database: String,
    pub name: String,
    /// `table` or `view`.
    pub kind: String,
    /// Row estimate from `sqlite_stat1`, i.e. from the last `ANALYZE`.
    /// SQLite keeps no live statistics, so this is `None` unless somebody
    /// has analyzed the database — that's honest, and much cheaper than
    /// a `COUNT(*)` per table on every listing.
    pub estimated_rows: Option<i64>,
}

/// One page of rows, rendered as text.
#[derive(Clone, Debug)]
pub struct RowsPage {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub has_more: bool,
}

// How a row is addressed and what one read of it looks like is the same
// vocabulary in every SQL dialect, so it lives in sql-core beside the
// buffer protocol. Re-exported here because this client's signatures are
// where callers meet it.
pub use not_yet_done_sql_core::{RowKeySource, RowKeySpec, RowRead};

/// Result of running a free-form script through [`SqliteClient::execute_raw_sql`].
#[derive(Clone, Debug, Default)]
pub struct RawSqlOutcome {
    /// Column names of the last result set, in order. Empty when the last
    /// statement produced no rows.
    pub columns: Vec<String>,
    /// Rows of the last result set. Cells are text (NULL → `None`).
    pub rows: Vec<Vec<Option<String>>>,
    /// Status text when the last statement produced no rows; `None` when
    /// it did.
    pub status: Option<String>,
}

pub struct SqliteClient {
    patterns: Vec<String>,
    read_only: bool,
    busy_timeout: Duration,
    query_timeout: Option<Duration>,
    /// Resolved `sources:` expansion. Cached because every listing needs
    /// it; dropped by [`SqliteClient::invalidate`] so the `r` reload
    /// picks up files that appeared since.
    sources: Mutex<Option<Arc<Vec<SourceEntry>>>>,
    /// One pool per source key.
    pools: Mutex<HashMap<String, SqlitePool>>,
    status_tx: watch::Sender<AdapterStatus>,
}

impl SqliteClient {
    pub fn new(
        patterns: Vec<String>,
        read_only: bool,
        busy_timeout: Duration,
        query_timeout: Option<Duration>,
    ) -> Self {
        let (status_tx, _) = watch::channel(AdapterStatus::Ready);
        Self {
            patterns,
            read_only,
            busy_timeout,
            query_timeout,
            sources: Mutex::new(None),
            pools: Mutex::new(HashMap::new()),
            status_tx,
        }
    }

    pub fn subscribe_status(&self) -> watch::Receiver<AdapterStatus> {
        self.status_tx.subscribe()
    }

    /// Drop the resolved source list and close every pool. The next call
    /// re-globs the filesystem and reopens what it finds — which is what
    /// makes a reload notice a new, moved or deleted database file.
    pub async fn invalidate(&self) {
        *self.sources.lock().await = None;
        let pools: Vec<SqlitePool> = self.pools.lock().await.drain().map(|(_, p)| p).collect();
        for pool in pools {
            pool.close().await;
        }
    }

    /// The resolved source list, expanding the patterns on first use.
    pub async fn sources(&self) -> Arc<Vec<SourceEntry>> {
        let mut guard = self.sources.lock().await;
        if let Some(cached) = guard.as_ref() {
            return Arc::clone(cached);
        }
        let resolved = Arc::new(resolve_sources(&self.patterns).await);
        *guard = Some(Arc::clone(&resolved));
        resolved
    }

    /// Look one source up by key.
    pub async fn source(&self, key: &str) -> Option<SourceEntry> {
        self.sources().await.iter().find(|s| s.key == key).cloned()
    }

    /// Every configured database file, with its on-disk size.
    pub async fn list_databases(&self) -> Result<Vec<DatabaseEntry>, String> {
        let sources = self.sources().await;
        let mut entries = Vec::with_capacity(sources.len());
        for source in sources.iter() {
            let size_bytes = tokio::fs::metadata(&source.path)
                .await
                .ok()
                .map(|m| m.len());
            entries.push(DatabaseEntry {
                key: source.key.clone(),
                label: source.label.clone(),
                path: source.path.clone(),
                size_bytes,
            });
        }
        Ok(entries)
    }

    /// Tables and views inside one database file.
    pub async fn list_tables(&self, key: &str) -> Result<Vec<TableEntry>, String> {
        let pool = self.pool(key).await?;
        let stats = self.table_row_estimates(&pool).await;
        let rows = self
            .run(
                sqlx::query(
                    "SELECT name, type FROM sqlite_master \
                 WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' \
                 ORDER BY type DESC, name",
                )
                .fetch_all(&pool),
            )
            .await
            .map_err(|e| format!("listing tables failed: {e}"))?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let name: String = row.get("name");
                TableEntry {
                    database: key.to_string(),
                    kind: row.get("type"),
                    estimated_rows: stats.get(&name).copied(),
                    name,
                }
            })
            .collect())
    }

    /// Tables and views across every configured database file. A file
    /// that cannot be opened is skipped rather than failing the whole
    /// listing — one corrupt or vanished database should not hide the
    /// rest.
    pub async fn list_all_tables(&self) -> Result<Vec<TableEntry>, String> {
        let sources = self.sources().await;
        let mut all = Vec::new();
        for source in sources.iter() {
            if let Ok(tables) = self.list_tables(&source.key).await {
                all.extend(tables);
            }
        }
        Ok(all)
    }

    /// One page of `SELECT *` from a table or view.
    ///
    /// `limit + 1` rows are requested so `has_more` is known without a
    /// second round trip or a `COUNT(*)` over the whole table.
    pub async fn query_rows(
        &self,
        key: &str,
        table: &str,
        offset: u32,
        limit: u32,
    ) -> Result<RowsPage, String> {
        let pool = self.pool(key).await?;
        let sql = format!(
            "SELECT * FROM {} LIMIT {} OFFSET {}",
            quote_ident(table),
            u64::from(limit) + 1,
            offset
        );
        // The only interpolated user input is the table name, and it goes
        // through `quote_ident` (which doubles any `"`), so it cannot
        // escape its identifier. A table name can't be a bind parameter,
        // so there is no non-dynamic way to write this.
        let rows = self
            .run(sqlx::query(sqlx::AssertSqlSafe(sql)).fetch_all(&pool))
            .await
            .map_err(|e| format!("reading rows of '{table}' failed: {e}"))?;

        let has_more = rows.len() > limit as usize;
        let columns = rows
            .first()
            .map(|row| {
                row.columns()
                    .iter()
                    .map(|c| c.name().to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let rows = rows
            .iter()
            .take(limit as usize)
            .map(|row| (0..columns.len()).map(|i| cell_to_string(row, i)).collect())
            .collect();
        Ok(RowsPage {
            columns,
            rows,
            has_more,
        })
    }

    /// Run a free-form script against one database file and return the
    /// last statement's result set.
    ///
    /// Statements run in order on one connection, so a prelude of
    /// `PRAGMA`/`CREATE TEMP …` lines followed by the `SELECT` the user
    /// wants to look at behaves the way the editor suggests it should.
    /// "Last statement" is taken literally, as in the Postgres adapter: a
    /// script ending in a statement without rows reports a status instead
    /// of the previous statement's rows.
    ///
    /// Unlike [`Self::query_rows`] nothing here is quoted or escaped — the
    /// whole point is to run what the user wrote. The pool is read-only
    /// unless the adapter config says otherwise, which is what keeps a
    /// stray `DELETE` in a scratch buffer an error rather than damage.
    pub async fn execute_raw_sql(&self, key: &str, sql: &str) -> Result<RawSqlOutcome, String> {
        let pool = self.pool(key).await?;
        let sql = sql.to_string();
        self.run(async move {
            // `raw_sql` (rather than `query`) so a multi-statement script
            // goes through unprepared, which is the only way SQLite will
            // accept more than one statement in a single call.
            let mut stream = pool.fetch_many(sqlx::raw_sql(sqlx::AssertSqlSafe(sql)));
            // Rows of the statement currently streaming, and of the last
            // one that finished — see the "last statement" note above.
            let mut current: Option<(Vec<String>, Vec<Vec<Option<String>>>)> = None;
            let mut last: Option<(Vec<String>, Vec<Vec<Option<String>>>)> = None;
            let mut affected = 0u64;
            while let Some(step) = stream.try_next().await? {
                match step {
                    // End of one statement.
                    sqlx::Either::Left(done) => {
                        last = current.take();
                        affected = done.rows_affected();
                    }
                    sqlx::Either::Right(row) => {
                        let (columns, rows) = current.get_or_insert_with(|| {
                            let columns = row
                                .columns()
                                .iter()
                                .map(|c| c.name().to_string())
                                .collect::<Vec<_>>();
                            (columns, Vec::new())
                        });
                        rows.push(
                            (0..columns.len())
                                .map(|i| cell_to_string(&row, i))
                                .collect(),
                        );
                    }
                }
            }
            // `current` is normally drained by the trailing statement's
            // result; falling back to it keeps an unterminated statement
            // from losing its rows.
            let (columns, rows) = last.or(current).unwrap_or_default();
            let status = if rows.is_empty() {
                Some(match affected {
                    0 => "ok, no rows".to_string(),
                    1 => "1 row affected".to_string(),
                    n => format!("{n} rows affected"),
                })
            } else {
                None
            };
            Ok(RawSqlOutcome {
                columns,
                rows,
                status,
            })
        })
        .await
        .map_err(|e| self.explain_readonly(e))
    }

    /// Whether this client's pools are opened read-only, i.e. whether a
    /// write can succeed at all. Callers use it to refuse a write *with an
    /// explanation* instead of letting SQLite answer with a bare "attempt
    /// to write a readonly database".
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// The `CREATE VIEW …` text of one view, verbatim as SQLite stores it
    /// in `sqlite_master`.
    ///
    /// `None` when the database holds no *view* of that name — a table of
    /// the same name included, because a table's definition is not
    /// something this can hand back for editing.
    pub async fn view_definition(&self, key: &str, name: &str) -> Result<Option<String>, String> {
        let pool = self.pool(key).await?;
        let row = self
            .run(
                sqlx::query("SELECT sql FROM sqlite_master WHERE type = 'view' AND name = ?1")
                    .bind(name.to_string())
                    .fetch_optional(&pool),
            )
            .await
            .map_err(|e| format!("reading the definition of '{name}' failed: {e}"))?;
        Ok(row.and_then(|row| row.try_get::<Option<String>, _>("sql").ok().flatten()))
    }

    /// Replace one view's definition: drop the old view and create the new
    /// one in a single transaction, then read from it once to prove the
    /// definition works.
    ///
    /// The drop is unavoidable — SQLite has neither `ALTER VIEW` nor
    /// `CREATE OR REPLACE VIEW`. It is safe anyway because DDL is
    /// transactional here: if the `CREATE` fails, the rollback brings the
    /// old view back, so a rejected edit cannot leave the database with
    /// one view fewer than it started with.
    ///
    /// The trailing `SELECT` is not a formality. SQLite resolves a view's
    /// body lazily: a view over a misspelled table is created without
    /// complaint and only fails when somebody reads it — which would be
    /// long after the editor closed. Reading it here, inside the
    /// transaction, turns that into an error the user still has the buffer
    /// for.
    ///
    /// `create_sql` is the user's own text and is run verbatim. It must
    /// have been checked to be a single `CREATE VIEW` for `name` first —
    /// see [`not_yet_done_sql_core::view_ddl::parse_create_view`].
    pub async fn replace_view(
        &self,
        key: &str,
        name: &str,
        create_sql: &str,
    ) -> Result<(), String> {
        let pool = self.pool(key).await?;
        // Only the view name is interpolated here, and it goes through
        // `quote_ident`; an identifier cannot be a bind parameter.
        let drop_sql = format!("DROP VIEW IF EXISTS {}", quote_ident(name));
        let smoke_sql = format!("SELECT * FROM {} LIMIT 0", quote_ident(name));
        let create_sql = create_sql.to_string();
        self.run(async move {
            let mut tx = pool.begin().await?;
            sqlx::query(sqlx::AssertSqlSafe(drop_sql))
                .execute(&mut *tx)
                .await?;
            sqlx::query(sqlx::AssertSqlSafe(create_sql))
                .execute(&mut *tx)
                .await?;
            sqlx::query(sqlx::AssertSqlSafe(smoke_sql))
                .fetch_optional(&mut *tx)
                .await?;
            tx.commit().await?;
            Ok::<(), sqlx::Error>(())
        })
        .await
        .map_err(|e| self.explain_readonly(e))
    }

    /// Which columns identify one row of `table`, for the row editor's
    /// `WHERE` clause.
    ///
    /// A declared primary key is preferred; without one the implicit
    /// `rowid` serves, which every ordinary SQLite table has. The `Err`
    /// cases are the ones where a single row cannot be addressed at all,
    /// and each explains itself — the message reaches the user in the
    /// editor's banner.
    pub async fn row_key_spec(&self, key: &str, table: &str) -> Result<RowKeySpec, String> {
        let pool = self.pool(key).await?;
        let kind: Option<String> = self
            .run(
                sqlx::query_scalar("SELECT type FROM sqlite_master WHERE name = ?1")
                    .bind(table)
                    .fetch_optional(&pool),
            )
            .await
            .map_err(|e| format!("looking '{table}' up failed: {e}"))?;
        match kind.as_deref() {
            Some("view") => {
                return Err(format!(
                    "{table} is a view, and SQLite cannot write through one — edit the row \
                     in the underlying table instead"
                ));
            }
            None => return Err(format!("no table named {table} in this database")),
            _ => {}
        }

        // `pragma_table_info` would be shorter, but the table-valued
        // pragma functions need a newer libsqlite3 than this adapter
        // assumes elsewhere; the plain PRAGMA works everywhere.
        let sql = format!("PRAGMA table_info({})", quote_ident(table));
        let rows = self
            .run(sqlx::query(sqlx::AssertSqlSafe(sql)).fetch_all(&pool))
            .await
            .map_err(|e| format!("reading the columns of '{table}' failed: {e}"))?;
        let mut pk: Vec<(i64, String)> = rows
            .iter()
            .filter_map(|row| {
                let position: i64 = row.try_get("pk").ok()?;
                if position <= 0 {
                    return None;
                }
                Some((position, row.try_get::<String, _>("name").ok()?))
            })
            .collect();
        if !pk.is_empty() {
            // `pk` is the 1-based position within the key, so a composite
            // key comes back in its declared order.
            pk.sort_by_key(|(position, _)| *position);
            return Ok(RowKeySpec {
                columns: pk.into_iter().map(|(_, name)| name).collect(),
                source: RowKeySource::PrimaryKey,
            });
        }

        // No declared key: fall back to the implicit one. A WITHOUT ROWID
        // table always has a primary key, so reaching this point without a
        // rowid means the table is one of the rare shapes that genuinely
        // cannot address a single row.
        let probe = format!("SELECT rowid FROM {} LIMIT 0", quote_ident(table));
        self.run(sqlx::query(sqlx::AssertSqlSafe(probe)).fetch_optional(&pool))
            .await
            .map_err(|_| {
                format!(
                    "{table} has neither a primary key nor a rowid, so a single row cannot be \
                 addressed — add a primary key, or change the row from a DB script"
                )
            })?;
        Ok(RowKeySpec {
            columns: vec!["rowid".into()],
            source: RowKeySource::RowId,
        })
    }

    /// The row at `offset` of the same unordered `SELECT *` the tree pages
    /// through, together with the values of its key columns.
    ///
    /// Offsets are how the tree addresses rows, and a `SELECT` without
    /// `ORDER BY` may reorder between two reads — which is why the key
    /// values read here are what every later statement uses, and why the
    /// buffer shows the values: a row that moved is visible as the wrong
    /// content, not written to silently.
    pub async fn read_row_at(
        &self,
        key: &str,
        table: &str,
        keys: &RowKeySpec,
        offset: u32,
    ) -> Result<Option<RowRead>, String> {
        let rows = self
            .read_rows(key, table, keys, &format!("LIMIT 1 OFFSET {offset}"))
            .await?;
        Ok(rows.into_iter().next())
    }

    /// Every row matching `where_sql` (built by
    /// [`not_yet_done_sql_core::row_edit::render_where`]), capped at two:
    /// callers only need to know whether the key addresses one row or
    /// several.
    pub async fn read_rows_where(
        &self,
        key: &str,
        table: &str,
        keys: &RowKeySpec,
        where_sql: &str,
    ) -> Result<Vec<RowRead>, String> {
        self.read_rows(key, table, keys, &format!("WHERE {where_sql} LIMIT 2"))
            .await
    }

    /// Shared read path: the key columns first, then the row itself, so a
    /// caller can split them apart positionally without worrying about a
    /// key column also appearing among the data columns.
    async fn read_rows(
        &self,
        key: &str,
        table: &str,
        keys: &RowKeySpec,
        suffix: &str,
    ) -> Result<Vec<RowRead>, String> {
        let pool = self.pool(key).await?;
        let key_list = keys
            .columns
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("SELECT {key_list}, * FROM {} {suffix}", quote_ident(table));
        let rows = self
            .run(sqlx::query(sqlx::AssertSqlSafe(sql)).fetch_all(&pool))
            .await
            .map_err(|e| format!("reading a row of '{table}' failed: {e}"))?;

        let key_count = keys.columns.len();
        Ok(rows
            .iter()
            .map(|row| {
                let key_values = keys
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(i, column)| (column.clone(), cell_to_string(row, i)))
                    .collect();
                let cells = row
                    .columns()
                    .iter()
                    .enumerate()
                    .skip(key_count)
                    .map(|(i, column)| {
                        let (value, faithful) = cell_to_text(row, i);
                        if faithful {
                            RowCell::editable(column.name(), value)
                        } else {
                            RowCell::read_only(column.name(), value)
                        }
                    })
                    .collect();
                RowRead { cells, key_values }
            })
            .collect())
    }

    /// Run one statement that writes, and report how many rows it changed.
    ///
    /// `sql` is built by [`not_yet_done_sql_core::row_edit::build_update`]
    /// and is a single statement, so it goes through the prepared path —
    /// unlike [`Self::execute_raw_sql`], which exists to run whatever the
    /// user typed.
    pub async fn execute_write(&self, key: &str, sql: &str) -> Result<u64, String> {
        let pool = self.pool(key).await?;
        let sql = sql.to_string();
        self.run(async move {
            let done = sqlx::query(sqlx::AssertSqlSafe(sql)).execute(&pool).await?;
            Ok::<u64, sqlx::Error>(done.rows_affected())
        })
        .await
        .map_err(|e| self.explain_readonly(e))
    }

    /// SQLite's own wording ("attempt to write a readonly database") says
    /// nothing about *why* the database is read-only, which is a
    /// configuration choice the user can change — so say it.
    fn explain_readonly(&self, error: String) -> String {
        if self.read_only && error.to_lowercase().contains("readonly") {
            format!(
                "{error} (this adapter runs read_only; set read_only: false in its config to allow writes)"
            )
        } else {
            error
        }
    }

    /// Row estimates keyed by table name, from `sqlite_stat1`'s first
    /// `stat` token. The table only exists after an `ANALYZE`, so a
    /// failure here means "no statistics", not "broken database".
    async fn table_row_estimates(&self, pool: &SqlitePool) -> HashMap<String, i64> {
        let Ok(rows) = self
            .run(sqlx::query("SELECT tbl, stat FROM sqlite_stat1").fetch_all(pool))
            .await
        else {
            return HashMap::new();
        };
        rows.into_iter()
            .filter_map(|row| {
                let table: String = row.try_get("tbl").ok()?;
                let stat: String = row.try_get("stat").ok()?;
                let count = stat.split_whitespace().next()?.parse().ok()?;
                Some((table, count))
            })
            .collect()
    }

    /// The pool for one source, opened on first use.
    async fn pool(&self, key: &str) -> Result<SqlitePool, String> {
        if let Some(pool) = self.pools.lock().await.get(key) {
            return Ok(pool.clone());
        }
        let source = self
            .source(key)
            .await
            .ok_or_else(|| format!("no configured database with key '{key}'"))?;
        let options = SqliteConnectOptions::new()
            .filename(&source.path)
            .read_only(self.read_only)
            // Never conjure a database: an empty file appearing because
            // of a typo in `sources:` would be worse than an error.
            .create_if_missing(false)
            .busy_timeout(self.busy_timeout);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .map_err(|e| format!("opening '{}' failed: {e}", source.path.display()))?;
        // Another task may have won the race while we were connecting;
        // keep whichever pool landed in the map first so callers can't
        // end up with two live pools for one file.
        let mut pools = self.pools.lock().await;
        Ok(pools.entry(source.key).or_insert(pool).clone())
    }

    /// Run one database operation under the configured timeout, flipping
    /// the adapter status to `Busy` for its duration so the TUI can show
    /// a countdown.
    async fn run<T, F>(&self, fut: F) -> Result<T, String>
    where
        F: std::future::Future<Output = Result<T, sqlx::Error>>,
    {
        let Some(timeout) = self.query_timeout else {
            return fut.await.map_err(|e| e.to_string());
        };
        let _ = self.status_tx.send(AdapterStatus::Busy {
            label: "sqlite query".to_string(),
            started_at_unix_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            timeout_secs: timeout.as_secs(),
            progress: None,
        });
        let result = match tokio::time::timeout(timeout, fut).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err(format!("timed out after {}s", timeout.as_secs())),
        };
        let _ = self.status_tx.send(AdapterStatus::Ready);
        result
    }
}

/// Render one cell as display text.
///
/// SQLite types values, not columns, so the storage class has to be read
/// off the value itself — a single column can hold an integer in one row
/// and text in the next. BLOBs are summarized rather than dumped: their
/// bytes are usually not text, and a table view is no place to find out.
fn cell_to_string(row: &sqlx::sqlite::SqliteRow, index: usize) -> Option<String> {
    cell_to_text(row, index).0
}

/// Render one cell, and say whether that rendering is the value itself or
/// only a description of it.
///
/// The distinction only matters to the row editor: a `<blob, 12 bytes>`
/// summary written back would replace the bytes with that sentence, so
/// such a cell is shown read-only. Everything a storage class renders
/// losslessly (text, integers, reals, NULL) is editable — SQLite's type
/// affinity converts the text back on the way in.
fn cell_to_text(row: &sqlx::sqlite::SqliteRow, index: usize) -> (Option<String>, bool) {
    let Ok(raw) = row.try_get_raw(index) else {
        return (None, false);
    };
    if raw.is_null() {
        return (None, true);
    }
    match raw.type_info().name() {
        "TEXT" => (row.try_get::<String, _>(index).ok(), true),
        "INTEGER" => (
            row.try_get::<i64, _>(index).ok().map(|v| v.to_string()),
            true,
        ),
        "REAL" => (
            row.try_get::<f64, _>(index).ok().map(|v| v.to_string()),
            true,
        ),
        "BLOB" => (
            row.try_get::<Vec<u8>, _>(index)
                .ok()
                .map(|bytes| format!("<blob, {} bytes>", bytes.len())),
            false,
        ),
        other => (Some(format!("<{other}>")), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a throwaway database file with one table, two rows and one
    /// view, then browse it through the client exactly as the adapter
    /// does. Uses a writable client to create the fixture and a
    /// read-only one to read it back.
    async fn fixture(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("fixture.db");
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(options).await.expect("create db");
        sqlx::query(
            "CREATE TABLE widgets (id INTEGER PRIMARY KEY, name TEXT, weight REAL, blob BLOB)",
        )
        .execute(&pool)
        .await
        .expect("create table");
        sqlx::query("INSERT INTO widgets VALUES (1, 'first', 1.5, x'00ff'), (2, NULL, NULL, NULL)")
            .execute(&pool)
            .await
            .expect("insert");
        sqlx::query("CREATE VIEW light_widgets AS SELECT * FROM widgets WHERE weight < 2")
            .execute(&pool)
            .await
            .expect("create view");
        pool.close().await;
        path
    }

    fn client_for(path: &std::path::Path) -> SqliteClient {
        SqliteClient::new(
            vec![path.display().to_string()],
            true,
            Duration::from_millis(500),
            Some(Duration::from_secs(10)),
        )
    }

    #[tokio::test]
    async fn lists_the_configured_file_as_a_database() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path()).await;
        let client = client_for(&path);

        let dbs = client.list_databases().await.expect("list databases");
        assert_eq!(dbs.len(), 1);
        assert_eq!(dbs[0].label, "fixture.db");
        assert!(dbs[0].size_bytes.unwrap_or(0) > 0);
    }

    #[tokio::test]
    async fn lists_tables_and_views_but_no_internal_objects() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path()).await;
        let client = client_for(&path);
        let key = client.sources().await[0].key.clone();

        let tables = client.list_tables(&key).await.expect("list tables");
        let named: Vec<(&str, &str)> = tables
            .iter()
            .map(|t| (t.name.as_str(), t.kind.as_str()))
            .collect();
        assert_eq!(named, vec![("light_widgets", "view"), ("widgets", "table")]);
        // No ANALYZE has run, so there are no statistics to report.
        assert!(tables.iter().all(|t| t.estimated_rows.is_none()));
    }

    #[tokio::test]
    async fn reads_a_row_page_and_renders_every_storage_class() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path()).await;
        let client = client_for(&path);
        let key = client.sources().await[0].key.clone();

        let page = client
            .query_rows(&key, "widgets", 0, 10)
            .await
            .expect("query rows");
        assert_eq!(page.columns, vec!["id", "name", "weight", "blob"]);
        assert!(!page.has_more);
        assert_eq!(
            page.rows[0],
            vec![
                Some("1".to_string()),
                Some("first".to_string()),
                Some("1.5".to_string()),
                Some("<blob, 2 bytes>".to_string()),
            ]
        );
        // NULLs stay `None` so the caller decides how to show them.
        assert_eq!(page.rows[1], vec![Some("2".to_string()), None, None, None]);
    }

    #[tokio::test]
    async fn paginates_with_has_more_and_an_offset() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path()).await;
        let client = client_for(&path);
        let key = client.sources().await[0].key.clone();

        let first = client
            .query_rows(&key, "widgets", 0, 1)
            .await
            .expect("first page");
        assert_eq!(first.rows.len(), 1);
        assert!(first.has_more, "a second row exists");

        let second = client
            .query_rows(&key, "widgets", 1, 1)
            .await
            .expect("second page");
        assert_eq!(second.rows.len(), 1);
        assert!(!second.has_more);
        assert_eq!(second.rows[0][0], Some("2".to_string()));
    }

    #[tokio::test]
    async fn read_only_rejects_writes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path()).await;
        let client = client_for(&path);
        let key = client.sources().await[0].key.clone();
        let pool = client.pool(&key).await.expect("pool");

        let err = sqlx::query("INSERT INTO widgets VALUES (3, 'x', 1.0, NULL)")
            .execute(&pool)
            .await
            .expect_err("read_only must reject writes");
        assert!(err.to_string().to_lowercase().contains("readonly"), "{err}");
    }

    /// "Last statement wins" — the first `SELECT` is executed but its rows
    /// are not what the user asked to see.
    #[tokio::test]
    async fn execute_raw_sql_returns_the_last_statements_rows() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path()).await;
        let client = client_for(&path);
        let key = client.sources().await[0].key.clone();

        let outcome = client
            .execute_raw_sql(&key, "SELECT 42 AS answer; SELECT id, name FROM widgets;")
            .await
            .expect("run script");
        assert_eq!(outcome.columns, vec!["id", "name"]);
        assert_eq!(outcome.rows.len(), 2);
        assert_eq!(outcome.rows[0][1], Some("first".to_string()));
        assert!(outcome.status.is_none(), "a result set reports no status");
    }

    #[tokio::test]
    async fn execute_raw_sql_reports_a_status_when_there_are_no_rows() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path()).await;
        let client = client_for(&path);
        let key = client.sources().await[0].key.clone();

        let outcome = client
            .execute_raw_sql(&key, "SELECT * FROM widgets WHERE id < 0")
            .await
            .expect("run script");
        assert!(outcome.rows.is_empty());
        assert_eq!(outcome.status.as_deref(), Some("ok, no rows"));
    }

    /// A write against a read-only adapter has to fail — and say why it is
    /// read-only, because that part is a configuration choice.
    #[tokio::test]
    async fn execute_raw_sql_explains_a_rejected_write() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path()).await;
        let client = client_for(&path);
        let key = client.sources().await[0].key.clone();

        let err = client
            .execute_raw_sql(&key, "INSERT INTO widgets VALUES (3, 'x', 1.0, NULL)")
            .await
            .expect_err("read_only must reject writes");
        assert!(err.to_lowercase().contains("readonly"), "{err}");
        assert!(err.contains("read_only: false"), "{err}");
    }

    #[tokio::test]
    async fn execute_raw_sql_counts_affected_rows_when_writes_are_allowed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path()).await;
        let client = SqliteClient::new(
            vec![path.display().to_string()],
            false,
            Duration::from_millis(500),
            None,
        );
        let key = client.sources().await[0].key.clone();

        let outcome = client
            .execute_raw_sql(&key, "UPDATE widgets SET name = 'renamed' WHERE id = 1")
            .await
            .expect("run update");
        assert_eq!(outcome.status.as_deref(), Some("1 row affected"));
    }

    #[tokio::test]
    async fn execute_raw_sql_surfaces_a_syntax_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path()).await;
        let client = client_for(&path);
        let key = client.sources().await[0].key.clone();

        let err = client
            .execute_raw_sql(&key, "SELECT * FROM no_such_table")
            .await
            .expect_err("unknown table");
        assert!(err.contains("no_such_table"), "{err}");
    }

    #[tokio::test]
    async fn an_unknown_key_is_an_error_not_a_panic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path()).await;
        let client = client_for(&path);

        let err = client
            .list_tables("nope-00000000")
            .await
            .expect_err("error");
        assert!(err.contains("no configured database"), "{err}");
    }

    fn writable_client_for(path: &std::path::Path) -> SqliteClient {
        SqliteClient::new(
            vec![path.display().to_string()],
            false,
            Duration::from_millis(500),
            Some(Duration::from_secs(10)),
        )
    }

    #[tokio::test]
    async fn view_definition_returns_the_stored_statement() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path()).await;
        let client = client_for(&path);
        let key = client.sources().await[0].key.clone();

        let sql = client
            .view_definition(&key, "light_widgets")
            .await
            .expect("read definition")
            .expect("the view exists");
        assert!(sql.starts_with("CREATE VIEW light_widgets"), "{sql}");
        assert!(sql.contains("weight < 2"), "{sql}");
    }

    /// A table has a definition too, but not one this hands out for
    /// editing — and an unknown name is simply absent, not an error.
    #[tokio::test]
    async fn view_definition_ignores_tables_and_unknown_names() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path()).await;
        let client = client_for(&path);
        let key = client.sources().await[0].key.clone();

        assert!(
            client
                .view_definition(&key, "widgets")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            client
                .view_definition(&key, "nope")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn replace_view_swaps_the_definition() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path()).await;
        let client = writable_client_for(&path);
        let key = client.sources().await[0].key.clone();

        // The fixture's view shows the one row with weight 1.5; the
        // replacement shows none.
        assert_eq!(
            client
                .query_rows(&key, "light_widgets", 0, 10)
                .await
                .unwrap()
                .rows
                .len(),
            1
        );
        client
            .replace_view(
                &key,
                "light_widgets",
                "CREATE VIEW light_widgets AS SELECT * FROM widgets WHERE weight < 1",
            )
            .await
            .expect("replace the view");

        let sql = client
            .view_definition(&key, "light_widgets")
            .await
            .unwrap()
            .expect("still there");
        assert!(sql.contains("weight < 1"), "{sql}");
        assert!(
            client
                .query_rows(&key, "light_widgets", 0, 10)
                .await
                .unwrap()
                .rows
                .is_empty(),
            "the new definition is what the rows come from"
        );
    }

    /// The case the smoke test exists for: SQLite happily *creates* a view
    /// over a table that does not exist and only fails when it is read. The
    /// rollback has to leave the original view intact.
    #[tokio::test]
    async fn replace_view_rolls_back_a_definition_that_cannot_be_read() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path()).await;
        let client = writable_client_for(&path);
        let key = client.sources().await[0].key.clone();

        let err = client
            .replace_view(
                &key,
                "light_widgets",
                "CREATE VIEW light_widgets AS SELECT * FROM widgts",
            )
            .await
            .expect_err("a view nobody can read is not a view");
        assert!(err.contains("widgts"), "{err}");

        let sql = client
            .view_definition(&key, "light_widgets")
            .await
            .unwrap()
            .expect("the old view came back");
        assert!(sql.contains("weight < 2"), "{sql}");
    }

    #[tokio::test]
    async fn replace_view_on_a_read_only_adapter_explains_why() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = fixture(tmp.path()).await;
        let client = client_for(&path);
        let key = client.sources().await[0].key.clone();

        let err = client
            .replace_view(
                &key,
                "light_widgets",
                "CREATE VIEW light_widgets AS SELECT 1",
            )
            .await
            .expect_err("read_only must reject the write");
        assert!(err.contains("read_only: false"), "{err}");
        assert!(client.is_read_only());
    }

    #[tokio::test]
    async fn invalidate_picks_up_a_file_that_appeared_later() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let first = fixture(tmp.path()).await;
        let client = SqliteClient::new(
            vec![format!("{}/*.db", tmp.path().display())],
            true,
            Duration::from_millis(500),
            None,
        );
        assert_eq!(client.list_databases().await.expect("list").len(), 1);

        tokio::fs::copy(&first, tmp.path().join("second.db"))
            .await
            .expect("copy");
        // Still one: the resolution is cached on purpose.
        assert_eq!(client.list_databases().await.expect("list").len(), 1);

        client.invalidate().await;
        assert_eq!(client.list_databases().await.expect("list").len(), 2);
    }
}
