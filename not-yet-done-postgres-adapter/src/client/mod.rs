//! Thin wrapper around `tokio_postgres` for the queries this adapter
//! needs. One `PostgresClient` per `PostgresAdapter`; it lazily opens
//! the transport tunnel (kept alive for the adapter's lifetime once
//! built) and one tokio-postgres session **per database**, since
//! `dbname` is fixed at connect time. Sessions are cached in a
//! `HashMap<dbname, Client>`.

pub mod cursor;
pub mod sql_shape;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{watch, Mutex};
use tokio_postgres::{Client, Config, NoTls, SimpleQueryMessage};

use not_yet_done_content::AdapterStatus;
use not_yet_done_transport::{Connection as TransportConnection, TransportConfig};

use crate::config::{PostgresAuth, SslMode};

/// Snapshot of everything `child_process_env` needs to assemble libpq
/// environment variables. Populated at the end of the first successful
/// [`PostgresClient::connect_session`] and cleared on [`tear_down`].
/// Held behind a `std::sync::Mutex` so adapter trait methods, which are
/// sync, can read it without blocking on the tokio runtime.
#[derive(Clone)]
struct ChildEnvBase {
    /// Whatever `TransportConnection::host` reports — `127.0.0.1` for
    /// SSH-tunneled mode, the real target host for `direct` mode.
    host: String,
    /// Local port the live transport listens on.
    port: u16,
    /// Resolved DB password (e.g. from `pass`-based provider). Kept in
    /// RAM only — never persisted.
    password: String,
    /// Matches whatever `auth.sslmode` is currently configured. Mirrors
    /// the value the live tokio_postgres client uses, so the LSP /
    /// scripts negotiate the same transport.
    sslmode: SslMode,
}

/// One row of `SELECT datname, pg_get_userbyid(datdba), pg_encoding_to_char(encoding) FROM pg_database`.
#[derive(Debug, Clone)]
pub struct DatabaseEntry {
    pub name: String,
    pub owner: String,
    pub encoding: String,
}

/// Per-schema metadata used as a child of a database.
#[derive(Debug, Clone)]
pub struct SchemaEntry {
    pub name: String,
    pub owner: String,
}

/// One row of `pg_class`-derived table metadata.
#[derive(Debug, Clone)]
pub struct TableEntry {
    pub database: String,
    pub schema: String,
    pub name: String,
    pub owner: String,
    /// `pg_class.reltuples` — *estimated* row count. Cheap (no scan)
    /// but stale until the table is analyzed.
    pub estimated_rows: i64,
}

/// One paginated `SELECT * FROM schema.table` result.
///
/// Each cell is text-cast in SQL so the adapter does not have to know
/// every postgres type. NULLs come back as `None`.
#[derive(Debug, Clone)]
pub struct RowsPage {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    /// True when the database returned a full page — used as a cheap
    /// "probably more rows" hint so we can skip a `COUNT(*)`.
    pub has_more: bool,
}

/// Output of [`PostgresClient::execute_raw_sql`].
///
/// Multi-statement queries are allowed (`simple_query` runs them in
/// one round-trip). Only the **last** statement's output ends up here:
/// either rows + columns (for SELECT-style statements) or a status
/// string like `"5 row(s) affected"` (for UPDATE/DELETE/INSERT without
/// RETURNING, or for DDL).
#[derive(Debug, Clone)]
pub struct RawSqlOutcome {
    /// Column names of the last result set, in order. Empty when the
    /// last statement had no result set.
    pub columns: Vec<String>,
    /// Rows of the last result set. Cells are text (NULL → `None`).
    pub rows: Vec<Vec<Option<String>>>,
    /// Status text for non-resultset statements; `None` when the last
    /// statement returned a result set.
    pub status: Option<String>,
}

/// Wraps a Postgres connection pool keyed by database name.
/// Reconnects lazily on first error — a long-lived TCP/SSH session
/// can drop without the application noticing until the next query.
pub struct PostgresClient {
    transport_cfg: TransportConfig,
    auth: PostgresAuth,
    /// Lazy SSH tunnel (or direct passthrough). Built on first
    /// `client()` call so the TUI startup path doesn't pay the SSH
    /// handshake. Once open it stays alive for the adapter's lifetime
    /// — postgres-session reconnects reuse the same tunnel.
    transport: Mutex<Option<TransportConnection>>,
    /// One session per database name. Each entry is independently
    /// reconnected when its `is_closed()` flips.
    sessions: Mutex<HashMap<String, Arc<Client>>>,
    /// Hard deadline for individual postgres operations. When the
    /// deadline fires the cached session **and** the transport are
    /// torn down — see [`PostgresClient::tear_down`]. `None` means
    /// "wait forever", matching libpq's default behaviour.
    query_timeout: Option<Duration>,
    /// Live status broadcast. Exposed via `subscribe_status()` on the
    /// adapter so the TUI can render a countdown while a query is in
    /// flight. The Postgres adapter never enters `Connecting`/`NeedsCreds`
    /// (auth is synchronous-in-yaml and connect failures surface as
    /// regular errors), so in practice this only toggles between
    /// `Ready` and `Busy`.
    status_tx: watch::Sender<AdapterStatus>,
    /// Counter bumped on every `tear_down`. External resources whose
    /// lifetime is tied to the underlying transport — most notably
    /// cursor sessions held outside the `sessions` cache — sample this
    /// at handoff time and compare on use to decide "this resource is
    /// still alive". Shared via [`PostgresClient::teardown_generation`].
    teardown_generation: Arc<AtomicU64>,
    /// Snapshot of live endpoint + resolved password for sync reads
    /// from `ContentAdapter::child_process_env` (which can't await).
    /// `None` until the first successful connect; cleared in
    /// [`PostgresClient::tear_down`] so a stale tunnel never leaks into
    /// a child process.
    env_cache: Arc<StdMutex<Option<ChildEnvBase>>>,
}

impl PostgresClient {
    pub fn new(
        transport_cfg: TransportConfig,
        auth: PostgresAuth,
        query_timeout: Option<Duration>,
    ) -> Self {
        let (status_tx, _) = watch::channel(AdapterStatus::Ready);
        Self {
            transport_cfg,
            auth,
            transport: Mutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            query_timeout,
            status_tx,
            teardown_generation: Arc::new(AtomicU64::new(0)),
            env_cache: Arc::new(StdMutex::new(None)),
        }
    }

    pub fn subscribe_status(&self) -> watch::Receiver<AdapterStatus> {
        self.status_tx.subscribe()
    }

    /// Handle the caller can sample at resource-handoff time and
    /// later compare against to detect a teardown. The returned Arc
    /// points to the same atomic counter the client bumps in
    /// `tear_down`, so checks are lock-free.
    pub fn teardown_generation(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.teardown_generation)
    }

    /// Wrap a postgres operation with the configured `query_timeout`
    /// and publish a `Busy → Ready` status transition around it.
    ///
    /// On timeout: drop the cached session map and the transport so
    /// the next call lazily re-tunnels. We do **not** retry inline —
    /// a server that's already 7s slow is a server we want to stop
    /// hitting until the user re-issues the query.
    async fn run_with_timeout<F, T>(&self, label: &str, fut: F) -> Result<T, String>
    where
        F: std::future::Future<Output = Result<T, String>>,
    {
        let started_at_unix_ms = now_unix_ms();
        let timeout_secs = self.query_timeout.map(|d| d.as_secs()).unwrap_or(0);
        let _ = self.status_tx.send(AdapterStatus::Busy {
            label: label.to_string(),
            started_at_unix_ms,
            timeout_secs,
        });

        let outcome = match self.query_timeout {
            Some(d) => match tokio::time::timeout(d, fut).await {
                Ok(inner) => inner,
                Err(_) => {
                    self.tear_down().await;
                    Err(format!(
                        "{label}: timed out after {}s; connection reset",
                        d.as_secs()
                    ))
                }
            },
            None => fut.await,
        };

        let _ = self.status_tx.send(AdapterStatus::Ready);
        outcome
    }

    /// Drop the cached session map and the SSH tunnel. Used when a
    /// query timed out — the next `client()` call will rebuild both
    /// from scratch. Bumps `teardown_generation` last so any external
    /// observer that wins the race only sees the post-teardown counter
    /// after the transport is already gone.
    async fn tear_down(&self) {
        self.sessions.lock().await.clear();
        *self.transport.lock().await = None;
        // Drop the env snapshot so a subsequent reconnect with a fresh
        // local port can repopulate it. We don't clear the password
        // separately — it's part of the snapshot and gets re-resolved
        // along with everything else on the next `connect_session`.
        *self.env_cache.lock().expect("env_cache poisoned") = None;
        self.teardown_generation.fetch_add(1, Ordering::Release);
    }

    /// Snapshot of the child-process env source values (host, port,
    /// password, sslmode) if the transport is live, else `None`.
    /// Sync — see [`ChildEnvBase`] for why. Intended for use by
    /// [`crate::adapter::PostgresAdapter::child_process_env`].
    pub fn child_env_base(&self) -> Option<HashMap<String, String>> {
        let snap = match self
            .env_cache
            .lock()
            .expect("env_cache poisoned")
            .clone()
        {
            Some(s) => s,
            None => {
                not_yet_done_content::http_log::log_debug(
                    "pg.child_env_base",
                    "env_cache=None (no connect_session has populated it yet, or tear_down cleared it)",
                );
                return None;
            }
        };
        let sslmode = match snap.sslmode {
            SslMode::Disable => "disable",
            SslMode::Prefer => "prefer",
            SslMode::Require => "require",
        };
        let mut env = HashMap::with_capacity(5);
        env.insert("PGHOST".to_string(), snap.host.clone());
        env.insert("PGPORT".to_string(), snap.port.to_string());
        env.insert("PGUSER".to_string(), self.auth.user.clone());
        env.insert("PGPASSWORD".to_string(), snap.password);
        env.insert("PGSSLMODE".to_string(), sslmode.to_string());
        not_yet_done_content::http_log::log_debug(
            "pg.child_env_base",
            &format!(
                "env_cache=Some host={} port={} user={} sslmode={}",
                snap.host, snap.port, self.auth.user, sslmode
            ),
        );
        Some(env)
    }

    /// Default database used when the caller's [`NodeRef`] does not
    /// carry one (e.g. spawn on the database-list view itself).
    pub fn admin_database(&self) -> &str {
        &self.auth.admin_database
    }

    /// Test-only hook to seed the child-env snapshot without going
    /// through a real `connect_session`. Lets adapter-level tests
    /// exercise `child_process_env` without a live Postgres + tunnel.
    #[cfg(test)]
    pub(crate) fn set_env_cache_for_test(
        &self,
        host: &str,
        port: u16,
        password: &str,
        sslmode: SslMode,
    ) {
        *self.env_cache.lock().unwrap() = Some(ChildEnvBase {
            host: host.to_string(),
            port,
            password: password.to_string(),
            sslmode,
        });
    }

    /// Borrow a live `tokio_postgres::Client` bound to `dbname`,
    /// opening the SSH tunnel and the session on first use.
    ///
    /// Recovery: if the cached session is closed we drop the entire
    /// transport too — when one session dies through a network hiccup
    /// the tunnel itself is almost always also gone, and reconnecting
    /// to a stale local port-forward just throws "connection refused".
    /// If the first fresh connect still fails we tear down once more
    /// and retry, to cover the case where another session's warm-up
    /// succeeded but the tunnel has since died.
    async fn client(&self, dbname: &str) -> Result<Arc<Client>, String> {
        let mut guard = self.sessions.lock().await;

        if let Some(c) = guard.get(dbname) {
            if !c.is_closed() {
                return Ok(Arc::clone(c));
            }
            guard.clear();
            *self.transport.lock().await = None;
        }

        match self.connect_session(dbname).await {
            Ok(client) => {
                guard.insert(dbname.to_string(), Arc::clone(&client));
                Ok(client)
            }
            Err(initial) => {
                guard.clear();
                *self.transport.lock().await = None;
                let client = self.connect_session(dbname).await.map_err(|retry| {
                    format!("postgres connect ({dbname}): {retry} (initial: {initial})")
                })?;
                guard.insert(dbname.to_string(), Arc::clone(&client));
                Ok(client)
            }
        }
    }

    /// Open a fresh dedicated `tokio_postgres::Client` for `dbname`,
    /// bypassing the `sessions` cache. The cursor sessions hold their
    /// own clients so an open transaction does not pin the cached
    /// session map.
    pub(crate) async fn connect_dedicated_session(
        &self,
        dbname: &str,
    ) -> Result<Arc<Client>, String> {
        self.connect_session(dbname).await
    }

    /// Build the transport (if not yet open) and a fresh
    /// `tokio_postgres::Client` for `dbname`. No caching — callers
    /// own the retry / cache policy.
    async fn connect_session(&self, dbname: &str) -> Result<Arc<Client>, String> {
        let (host, port) = {
            let mut t = self.transport.lock().await;
            if t.is_none() {
                let conn = not_yet_done_transport::connect(&self.transport_cfg)
                    .await
                    .map_err(|e| format!("transport: {e}"))?;
                *t = Some(conn);
            }
            let conn = t.as_ref().expect("just set");
            (conn.host.clone(), conn.port)
        };

        let pw = self
            .auth
            .password
            .build_resolver()
            .map_err(|e| format!("password provider: {e}"))?
            .resolve()
            .await
            .map_err(|e| format!("resolve password: {e}"))?;

        let mut cfg = Config::new();
        cfg.host(&host)
            .port(port)
            .user(&self.auth.user)
            .password(&pw)
            .dbname(dbname)
            .ssl_mode(map_sslmode(self.auth.sslmode));

        let (client, connection) = cfg
            .connect(NoTls)
            .await
            .map_err(|e| format!("postgres connect ({dbname}): {e}"))?;
        // Drive the connection in the background; if it errors, the
        // next `client()` call will reconnect because `is_closed()`
        // flips to true.
        tokio::spawn(async move {
            let _ = connection.await;
        });

        // Refresh the child-env snapshot now that we know the transport
        // is live AND the password resolved successfully. Idempotent —
        // overwrites whatever was there (e.g. previous tunnel's port
        // before a tear-down/reconnect cycle).
        *self.env_cache.lock().expect("env_cache poisoned") = Some(ChildEnvBase {
            host,
            port,
            password: pw,
            sslmode: self.auth.sslmode,
        });

        Ok(Arc::new(client))
    }

    /// `SELECT datname, pg_get_userbyid(datdba), pg_encoding_to_char(encoding) FROM pg_database`,
    /// excluding template databases. Runs against the admin database
    /// (typically `postgres`).
    pub async fn list_databases(&self) -> Result<Vec<DatabaseEntry>, String> {
        self.run_with_timeout("list databases", async {
            let client = self.client(&self.auth.admin_database).await?;
            let rows = client
                .query(
                    "SELECT datname, \
                            pg_get_userbyid(datdba) AS owner, \
                            pg_encoding_to_char(encoding) AS encoding \
                     FROM pg_database \
                     WHERE datistemplate = false \
                     ORDER BY datname",
                    &[],
                )
                .await
                .map_err(|e| format!("query pg_database: {e}"))?;

            Ok(rows
                .into_iter()
                .map(|r| DatabaseEntry {
                    name: r.get::<_, String>(0),
                    owner: r.try_get::<_, String>(1).unwrap_or_default(),
                    encoding: r.try_get::<_, String>(2).unwrap_or_default(),
                })
                .collect())
        })
        .await
    }

    /// User-visible schemas in `dbname`. Filters out the system
    /// schemas `pg_catalog`, `information_schema`, and the
    /// implementation namespaces `pg_toast`/`pg_temp_*`.
    pub async fn list_schemas(&self, dbname: &str) -> Result<Vec<SchemaEntry>, String> {
        self.run_with_timeout(&format!("list schemas of {dbname}"), async {
            let client = self.client(dbname).await?;
            let rows = client
                .query(
                    "SELECT n.nspname, pg_get_userbyid(n.nspowner) AS owner \
                     FROM pg_namespace n \
                     WHERE n.nspname NOT IN ('pg_catalog', 'information_schema') \
                       AND n.nspname NOT LIKE 'pg\\_%' \
                     ORDER BY n.nspname",
                    &[],
                )
                .await
                .map_err(|e| format!("query pg_namespace: {e}"))?;

            Ok(rows
                .into_iter()
                .map(|r| SchemaEntry {
                    name: r.get::<_, String>(0),
                    owner: r.try_get::<_, String>(1).unwrap_or_default(),
                })
                .collect())
        })
        .await
    }

    /// Base tables in `dbname.schema` (excludes views, sequences,
    /// indexes, partitions). `estimated_rows` comes from
    /// `pg_class.reltuples` — cheap but stale until the next ANALYZE.
    pub async fn list_tables(
        &self,
        dbname: &str,
        schema: &str,
    ) -> Result<Vec<TableEntry>, String> {
        self.run_with_timeout(&format!("list tables of {dbname}.{schema}"), async {
            let client = self.client(dbname).await?;
            let rows = client
                .query(
                    "SELECT c.relname, \
                            pg_get_userbyid(c.relowner) AS owner, \
                            c.reltuples::bigint AS estimated_rows \
                     FROM pg_class c \
                     JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE n.nspname = $1 AND c.relkind = 'r' \
                     ORDER BY c.relname",
                    &[&schema],
                )
                .await
                .map_err(|e| format!("query pg_class: {e}"))?;

            Ok(rows
                .into_iter()
                .map(|r| TableEntry {
                    database: dbname.to_string(),
                    schema: schema.to_string(),
                    name: r.get::<_, String>(0),
                    owner: r.try_get::<_, String>(1).unwrap_or_default(),
                    estimated_rows: r.try_get::<_, i64>(2).unwrap_or(0),
                })
                .collect())
        })
        .await
    }

    /// Every base table the connection can see, across all non-system
    /// databases and non-system schemas. Iterates `list_databases` and
    /// runs one cross-schema `pg_class` scan per database. A database
    /// that fails to query (auth/connection) is skipped silently — the
    /// other databases still appear, which matches DBeaver's behavior.
    pub async fn list_all_tables(&self) -> Result<Vec<TableEntry>, String> {
        // `list_databases` is itself timeout-wrapped; we additionally
        // wrap the per-database `pg_class` scan so a single hung db
        // doesn't block the whole walk past the deadline.
        let dbs = self.list_databases().await?;
        let mut out = Vec::new();
        for db in dbs {
            let label = format!("list all tables in {}", db.name);
            let collected = self
                .run_with_timeout(&label, async {
                    let client = self.client(&db.name).await?;
                    let rows = client
                        .query(
                            "SELECT n.nspname, c.relname, \
                                    pg_get_userbyid(c.relowner) AS owner, \
                                    c.reltuples::bigint AS estimated_rows \
                             FROM pg_class c \
                             JOIN pg_namespace n ON n.oid = c.relnamespace \
                             WHERE c.relkind = 'r' \
                               AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
                               AND n.nspname NOT LIKE 'pg\\_%' \
                             ORDER BY n.nspname, c.relname",
                            &[],
                        )
                        .await
                        .map_err(|e| format!("query pg_class on {}: {e}", db.name))?;
                    Ok(rows
                        .into_iter()
                        .map(|r| TableEntry {
                            database: db.name.clone(),
                            schema: r.get::<_, String>(0),
                            name: r.get::<_, String>(1),
                            owner: r.try_get::<_, String>(2).unwrap_or_default(),
                            estimated_rows: r.try_get::<_, i64>(3).unwrap_or(0),
                        })
                        .collect::<Vec<_>>())
                })
                .await;
            // Match the previous behaviour: an unreachable db doesn't
            // tank the rest of the listing.
            if let Ok(entries) = collected {
                out.extend(entries);
            }
        }
        Ok(out)
    }

    /// Every base table in a single database across all non-system
    /// schemas, returned as `(schema, table)` pairs sorted by
    /// `(schema, table)`. Used by the script-editor's table-name
    /// completion feature where the caller only wants names, not the
    /// full [`TableEntry`] (owner + estimated row count). One round
    /// trip per call — the result is small enough that caching it
    /// would cost more in invalidation logic than re-querying.
    pub async fn list_tables_in_database(
        &self,
        dbname: &str,
    ) -> Result<Vec<(String, String)>, String> {
        self.run_with_timeout(&format!("list tables in {dbname}"), async {
            let client = self.client(dbname).await?;
            let rows = client
                .query(
                    "SELECT n.nspname, c.relname \
                     FROM pg_class c \
                     JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE c.relkind = 'r' \
                       AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
                       AND n.nspname NOT LIKE 'pg\\_%' \
                     ORDER BY n.nspname, c.relname",
                    &[],
                )
                .await
                .map_err(|e| format!("query pg_class on {dbname}: {e}"))?;
            Ok(rows
                .into_iter()
                .map(|r| (r.get::<_, String>(0), r.get::<_, String>(1)))
                .collect())
        })
        .await
    }

    /// Paginated `SELECT * FROM schema.table ORDER BY ctid LIMIT … OFFSET …`.
    ///
    /// Two-step: pull column names from `information_schema.columns`,
    /// then issue a dynamically-built SELECT that text-casts every
    /// column (`col::text`). That sidesteps `tokio_postgres::FromSql`
    /// dispatch on unknown types — the adapter only ever sees strings
    /// (or `None` for NULL). `ORDER BY ctid` keeps page boundaries
    /// stable across `>`/`<` navigation as long as the table isn't
    /// being mutated; ctid is fine here since we only list base tables
    /// (`relkind = 'r'`).
    pub async fn query_rows(
        &self,
        dbname: &str,
        schema: &str,
        table: &str,
        offset: u32,
        limit: u32,
    ) -> Result<RowsPage, String> {
        self.run_with_timeout(&format!("rows of {schema}.{table}"), async {
            let client = self.client(dbname).await?;

            let col_rows = client
                .query(
                    "SELECT column_name \
                     FROM information_schema.columns \
                     WHERE table_schema = $1 AND table_name = $2 \
                     ORDER BY ordinal_position",
                    &[&schema, &table],
                )
                .await
                .map_err(|e| format!("query columns of {schema}.{table}: {e}"))?;
            let columns: Vec<String> = col_rows
                .into_iter()
                .map(|r| r.get::<_, String>(0))
                .collect();
            if columns.is_empty() {
                return Err(format!(
                    "table {schema}.{table} has no columns or does not exist"
                ));
            }

            let select_list = columns
                .iter()
                .map(|c| format!("{}::text", quote_ident(c)))
                .collect::<Vec<_>>()
                .join(", ");
            let qualified = format!("{}.{}", quote_ident(schema), quote_ident(table));
            let sql = format!(
                "SELECT {select_list} FROM {qualified} ORDER BY ctid \
                 LIMIT $1::bigint OFFSET $2::bigint",
            );
            let data_rows = client
                .query(&sql, &[&(limit as i64), &(offset as i64)])
                .await
                .map_err(|e| format!("query rows of {schema}.{table}: {e}"))?;

            let has_more = data_rows.len() as u32 == limit;
            let rows = data_rows
                .into_iter()
                .map(|r| {
                    (0..columns.len())
                        .map(|i| r.try_get::<_, Option<String>>(i).unwrap_or(None))
                        .collect()
                })
                .collect();

            Ok(RowsPage { columns, rows, has_more })
        })
        .await
    }

    /// Run a free-form (multi-statement) SQL string against `dbname`
    /// using `simple_query`. Returns the **last** statement's output.
    ///
    /// `simple_query` interleaves messages per statement:
    /// `Row*, CommandComplete, Row*, CommandComplete, …`. We collect
    /// the rows for the most recent statement and finalize on each
    /// `CommandComplete`. Cells come back as text (NULL → `None`),
    /// which mirrors `query_rows`'s wire format and dodges
    /// `tokio_postgres::FromSql` dispatch on unknown types.
    pub async fn execute_raw_sql(
        &self,
        dbname: &str,
        sql: &str,
    ) -> Result<RawSqlOutcome, String> {
        self.run_with_timeout(&format!("custom SQL on {dbname}"), async {
            let client = self.client(dbname).await?;
            let messages = client
                .simple_query(sql)
                .await
                .map_err(|e| format!("execute query: {e}"))?;

            let mut cur_columns: Vec<String> = Vec::new();
            let mut cur_rows: Vec<Vec<Option<String>>> = Vec::new();
            let mut last: Option<RawSqlOutcome> = None;

            for msg in messages {
                match msg {
                    // Header for the following Row stream. Arrives even when
                    // the SELECT returns zero rows — capturing it here is the
                    // only way to distinguish "SELECT with empty result set"
                    // (columns known, no rows) from "non-resultset statement"
                    // (no columns, no rows) at CommandComplete time.
                    SimpleQueryMessage::RowDescription(cols) => {
                        cur_columns = cols.iter().map(|c| c.name().to_string()).collect();
                    }
                    SimpleQueryMessage::Row(row) => {
                        if cur_columns.is_empty() {
                            cur_columns = row
                                .columns()
                                .iter()
                                .map(|c| c.name().to_string())
                                .collect();
                        }
                        let cells = (0..row.len())
                            .map(|i| row.get(i).map(|s| s.to_string()))
                            .collect();
                        cur_rows.push(cells);
                    }
                    SimpleQueryMessage::CommandComplete(rows_affected) => {
                        let columns = std::mem::take(&mut cur_columns);
                        let rows = std::mem::take(&mut cur_rows);
                        let status = if rows.is_empty() && columns.is_empty() {
                            Some(format!("{rows_affected} row(s) affected"))
                        } else {
                            None
                        };
                        last = Some(RawSqlOutcome { columns, rows, status });
                    }
                    _ => {}
                }
            }

            Ok(last.unwrap_or(RawSqlOutcome {
                columns: vec![],
                rows: vec![],
                status: Some("0 row(s) affected".into()),
            }))
        })
        .await
    }
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Wrap a Postgres identifier in `"…"`, doubling any embedded `"`.
/// We have to splice schema/table/column names directly into the SQL
/// (`SELECT col::text` / `FROM schema.table`) because tokio_postgres
/// only parametrises *values*; this is the standard mitigation.
pub(crate) fn quote_ident(name: &str) -> String {
    let escaped = name.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

fn map_sslmode(m: crate::config::SslMode) -> tokio_postgres::config::SslMode {
    use tokio_postgres::config::SslMode as Tps;
    match m {
        crate::config::SslMode::Disable => Tps::Disable,
        crate::config::SslMode::Prefer => Tps::Prefer,
        crate::config::SslMode::Require => Tps::Require,
    }
}
