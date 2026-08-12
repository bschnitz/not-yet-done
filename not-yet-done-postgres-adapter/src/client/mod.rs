//! Thin wrapper around `tokio_postgres` for the queries this adapter
//! needs. One `PostgresClient` per `PostgresAdapter`; it lazily opens
//! the transport tunnel (kept alive for the adapter's lifetime once
//! built) and one tokio-postgres session **per database**, since
//! `dbname` is fixed at connect time. Sessions are cached in a
//! `HashMap<dbname, Client>`.

pub mod cursor;

pub(crate) use not_yet_done_sql_core::quote_ident;
/// Pure-text SQL sniffers, shared with every other SQL adapter.
pub use not_yet_done_sql_core::sql_shape;
/// How a row is addressed and what one read of it looks like — the same
/// vocabulary every SQL adapter's row editor uses, see
/// [`not_yet_done_sql_core::row_edit`].
pub use not_yet_done_sql_core::{RowCell, RowKeySource, RowKeySpec, RowRead};

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{Mutex, watch};
use tokio_postgres::{Client, Config, NoTls, SimpleQueryMessage};

use not_yet_done_content::{AdapterStatus, CredentialProvider};
use not_yet_done_transport::{Connection as TransportConnection, SshAuth, TransportConfig};

use crate::adapter::PostgresCredentials;
use crate::adapter::auth::{FIELD_PASSWORD, FIELD_SSH_KEY_PASSPHRASE, FIELD_SSH_PASSWORD};
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

/// Which kind of relation a [`TableEntry`] describes.
///
/// Both come out of the same `pg_class` scan and differ in one column,
/// `relkind` — which is also the only reason the catalogue queries below
/// are parameterized instead of duplicated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationKind {
    Table,
    View,
}

impl RelationKind {
    /// The `pg_class.relkind` character to filter on.
    ///
    /// Materialized views (`m`) are deliberately not a variant: their
    /// definition is editable text like a view's, but replacing it means
    /// dropping and re-populating the stored result, which is a data
    /// operation and belongs in a DB script rather than behind a save key.
    fn relkind(self) -> &'static str {
        match self {
            RelationKind::Table => "r",
            RelationKind::View => "v",
        }
    }
}

/// One row of `pg_class`-derived relation metadata — a base table or a
/// view, told apart by [`TableEntry::kind`].
#[derive(Debug, Clone)]
pub struct TableEntry {
    pub database: String,
    pub schema: String,
    pub name: String,
    pub kind: RelationKind,
    pub owner: String,
    /// `pg_class.reltuples` — *estimated* row count. Cheap (no scan)
    /// but stale until the table is analyzed. Meaningless for a view
    /// (postgres reports `-1`), so callers skip it for those.
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
    /// Resolved DB password, cached across connects.
    ///
    /// Two reasons this is cached rather than resolved per connect:
    ///
    /// 1. **The resolver can block on the user.** A `command` provider
    ///    running `pass` triggers a GPG pinentry prompt, and the person
    ///    at the keyboard takes however long they take. That wait must
    ///    not be charged to `query_timeout` (see
    ///    [`PostgresClient::run_with_timeout`], which pre-resolves
    ///    *before* arming the clock), and it must not be paid again on
    ///    every reconnect or every dedicated cursor session.
    /// 2. It survives [`PostgresClient::tear_down`] deliberately: a
    ///    dropped tunnel says nothing about the password's validity, so
    ///    re-prompting there would be pure noise. Only an actual
    ///    authentication rejection clears it, via
    ///    [`PostgresClient::invalidate_password`].
    ///
    /// RAM only, never persisted — same lifetime guarantee as
    /// [`ChildEnvBase::password`].
    password_cache: Mutex<Option<String>>,
    /// The `auth:` block, when the config has one. Present exactly when
    /// some provider slot delegates to it (`{type: script-result}` /
    /// `{type: prompt}`); the config validation keeps the two in step.
    credentials: Option<Arc<PostgresCredentials>>,
}

impl PostgresClient {
    pub fn new(
        transport_cfg: TransportConfig,
        auth: PostgresAuth,
        query_timeout: Option<Duration>,
        credentials: Option<Arc<PostgresCredentials>>,
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
            password_cache: Mutex::new(None),
            credentials,
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
    ///
    /// **Credentials are resolved before the clock is armed.** `fut`
    /// typically reaches `connect_session`, which needs the DB password;
    /// with a `command` provider that means running `pass`, which means a
    /// GPG pinentry dialog and an unbounded wait on a human. Charging
    /// that wait to `query_timeout` made a 30s deadline fire while the
    /// user was still typing their passphrase — the timeout is meant to
    /// bound *the server's* slowness, not the operator's. So we pull the
    /// password up front (cached, so only the first call ever waits) and
    /// start counting at the first byte of actual connection work.
    ///
    /// The transport handshake stays *inside* the deadline on purpose:
    /// opening the SSH tunnel is network work, and a stalled tunnel is
    /// exactly the failure mode `query_timeout` exists to catch.
    async fn run_with_timeout<F, T>(&self, label: &str, fut: F) -> Result<T, String>
    where
        F: std::future::Future<Output = Result<T, String>>,
    {
        // Un-timed, but still visible: announce Busy with `timeout_secs:
        // 0` (the "no deadline" encoding the status bar already handles)
        // so a pinentry wait doesn't look like a frozen TUI.
        if !self.password_is_cached().await {
            let _ = self.status_tx.send(AdapterStatus::Busy {
                label: format!("{label} (resolving credentials)"),
                started_at_unix_ms: now_unix_ms(),
                timeout_secs: 0,
                progress: None,
            });
        }
        if let Err(e) = self.prewarm_credentials().await {
            let _ = self.status_tx.send(AdapterStatus::Ready);
            return Err(e);
        }

        let started_at_unix_ms = now_unix_ms();
        let timeout_secs = self.query_timeout.map(|d| d.as_secs()).unwrap_or(0);
        let _ = self.status_tx.send(AdapterStatus::Busy {
            label: label.to_string(),
            started_at_unix_ms,
            timeout_secs,
            progress: None,
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
        // `password_cache` is intentionally NOT cleared: a dead tunnel
        // tells us nothing about whether the password is still valid, and
        // dropping it here would make every timeout re-open a pinentry
        // dialog. Only `invalidate_password` (on an actual auth
        // rejection) discards it.
        self.teardown_generation.fetch_add(1, Ordering::Release);
    }

    /// Whether a resolved password is already in hand. Used only to
    /// decide whether to announce a "resolving credentials" status —
    /// racing with a concurrent resolve is harmless (worst case the
    /// status line is skipped or shown redundantly).
    async fn password_is_cached(&self) -> bool {
        self.password_cache.lock().await.is_some()
    }

    /// Everything an upcoming connect may have to ask a human for,
    /// pulled *before* the deadline is armed.
    ///
    /// One fetch of the auth block covers every delegated slot at once —
    /// the tunnel's secret included, which is why this exists next to
    /// [`Self::resolved_password`]: a connection whose *tunnel* is
    /// interactive but whose database password is not would otherwise pay
    /// the dialog wait inside `query_timeout`.
    async fn prewarm_credentials(&self) -> Result<(), String> {
        if self.credentials.is_some() {
            self.credential_fields().await?;
        }
        self.resolved_password().await?;
        Ok(())
    }

    /// The resolved `auth:` block, mirroring its interactive status into
    /// our own channel while the round runs.
    ///
    /// The mirror is what makes the credential dialog visible at all: the
    /// TUI watches the *adapter's* status, and the orchestrator publishes
    /// `NeedsCreds` on its own. Only the interactive states travel —
    /// forwarding `Ready` would stomp on the `Busy` this client sets
    /// around the call.
    async fn credential_fields(&self) -> Result<HashMap<String, String>, String> {
        let bridge = self
            .credentials
            .as_ref()
            .ok_or("internal: a slot delegates to `auth:` but no auth block was built")?;
        let mut rx = bridge.subscribe_status();
        let mut mirroring = true;
        let fut = bridge.fields();
        tokio::pin!(fut);
        loop {
            tokio::select! {
                resolved = &mut fut => return resolved,
                changed = rx.changed(), if mirroring => match changed {
                    Ok(()) => {
                        let status = rx.borrow_and_update().clone();
                        if matches!(
                            status,
                            AdapterStatus::NeedsCreds { .. }
                                | AdapterStatus::Connecting { .. }
                                | AdapterStatus::Failed { .. }
                        ) {
                            let _ = self.status_tx.send(status);
                        }
                    }
                    Err(_) => mirroring = false,
                },
            }
        }
    }

    /// One field of the auth block, by mechanism field name.
    async fn credential_field(&self, field: &str) -> Result<String, String> {
        self.credential_fields()
            .await?
            .remove(field)
            .ok_or_else(|| {
                format!("auth: no value for `{field}` — the credential source did not supply it")
            })
    }

    /// The transport config with every delegated secret filled in.
    ///
    /// The tunnel resolves providers itself, but it has no way to reach
    /// the auth block — so a delegating hop gets its value substituted as
    /// a literal here, right before the handshake. Borrowed (no clone)
    /// whenever nothing delegates, which is every config that predates
    /// the auth block.
    async fn effective_transport_cfg(&self) -> Result<Cow<'_, TransportConfig>, String> {
        let delegates = self.transport_cfg.ssh.iter().any(|hop| match &hop.auth {
            SshAuth::Password { password } => password.needs_frontend(),
            SshAuth::PublicKey {
                passphrase: Some(p),
                ..
            } => p.needs_frontend(),
            _ => false,
        });
        if !delegates {
            return Ok(Cow::Borrowed(&self.transport_cfg));
        }

        let mut cfg = self.transport_cfg.clone();
        for hop in cfg.ssh.iter_mut() {
            match &mut hop.auth {
                SshAuth::Password { password } if password.needs_frontend() => {
                    *password = CredentialProvider::Literal {
                        value: self.credential_field(FIELD_SSH_PASSWORD).await?,
                    };
                }
                SshAuth::PublicKey {
                    passphrase: Some(p),
                    ..
                } if p.needs_frontend() => {
                    *p = CredentialProvider::Literal {
                        value: self.credential_field(FIELD_SSH_KEY_PASSPHRASE).await?,
                    };
                }
                _ => {}
            }
        }
        Ok(Cow::Owned(cfg))
    }

    /// The DB password, resolving it through the configured provider on
    /// first use and caching it afterwards (see [`Self::password_cache`]
    /// for why). The lock is held across the resolve so two concurrent
    /// connects can't both trigger a pinentry prompt.
    async fn resolved_password(&self) -> Result<String, String> {
        let mut guard = self.password_cache.lock().await;
        if let Some(pw) = guard.as_ref() {
            return Ok(pw.clone());
        }
        let pw = if self.auth.password.needs_frontend() {
            self.credential_field(FIELD_PASSWORD).await?
        } else {
            self.auth
                .password
                .build_resolver()
                .map_err(|e| format!("password provider: {e}"))?
                .resolve()
                .await
                .map_err(|e| format!("resolve password: {e}"))?
        };
        *guard = Some(pw.clone());
        Ok(pw)
    }

    /// Discard the cached password so the next connect re-runs the
    /// provider. Called only when the server actually rejected our
    /// credentials — a rotated secret is the one case where re-prompting
    /// is the right answer rather than an annoyance.
    async fn invalidate_password(&self) {
        *self.password_cache.lock().await = None;
        // A rejected password is the one case where re-running the
        // credential script is the point, not an annoyance.
        if let Some(bridge) = &self.credentials {
            bridge.invalidate().await;
        }
    }

    /// Snapshot of the child-process env source values (host, port,
    /// password, sslmode) if the transport is live, else `None`.
    /// Sync — see [`ChildEnvBase`] for why. Intended for use by
    /// [`crate::adapter::PostgresAdapter::child_process_env`].
    pub fn child_env_base(&self) -> Option<HashMap<String, String>> {
        let snap = match self.env_cache.lock().expect("env_cache poisoned").clone() {
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
                let cfg = self.effective_transport_cfg().await?;
                let conn = not_yet_done_transport::connect(&cfg)
                    .await
                    .map_err(|e| format!("transport: {e}"))?;
                *t = Some(conn);
            }
            let conn = t.as_ref().expect("just set");
            (conn.host.clone(), conn.port)
        };

        // Cached after the first resolve — and pre-warmed outside the
        // deadline by `run_with_timeout`, so a `pass` pinentry wait never
        // eats into `query_timeout`.
        let pw = self.resolved_password().await?;

        let mut cfg = Config::new();
        cfg.host(&host)
            .port(port)
            .user(&self.auth.user)
            .password(&pw)
            .dbname(dbname)
            .ssl_mode(map_sslmode(self.auth.sslmode));

        let (client, connection) = match cfg.connect(NoTls).await {
            Ok(pair) => pair,
            Err(e) => {
                // Only an authentication rejection means the cached
                // password is wrong; every other connect failure (host
                // unreachable, dead tunnel, missing database) leaves it
                // valid and must not trigger a fresh pinentry prompt.
                if is_auth_failure(&e) {
                    self.invalidate_password().await;
                }
                return Err(format!("postgres connect ({dbname}): {e}"));
            }
        };
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
    pub async fn list_tables(&self, dbname: &str, schema: &str) -> Result<Vec<TableEntry>, String> {
        self.list_relations(dbname, schema, RelationKind::Table)
            .await
    }

    /// Views in `dbname.schema`. Same shape as [`Self::list_tables`] —
    /// the tree renders both branches the same way, and a view is
    /// queryable exactly like a table.
    pub async fn list_views(&self, dbname: &str, schema: &str) -> Result<Vec<TableEntry>, String> {
        self.list_relations(dbname, schema, RelationKind::View)
            .await
    }

    /// One `pg_class` scan for whichever [`RelationKind`] the caller
    /// wants. `relkind` is bound as a parameter rather than spliced, so
    /// tables and views cannot drift apart in what they select.
    async fn list_relations(
        &self,
        dbname: &str,
        schema: &str,
        kind: RelationKind,
    ) -> Result<Vec<TableEntry>, String> {
        self.run_with_timeout(&format!("list relations of {dbname}.{schema}"), async {
            let client = self.client(dbname).await?;
            let rows = client
                .query(
                    "SELECT c.relname, \
                            pg_get_userbyid(c.relowner) AS owner, \
                            c.reltuples::bigint AS estimated_rows \
                     FROM pg_class c \
                     JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE n.nspname = $1 AND c.relkind::text = $2 \
                     ORDER BY c.relname",
                    &[&schema, &kind.relkind()],
                )
                .await
                .map_err(|e| format!("query pg_class: {e}"))?;

            Ok(rows
                .into_iter()
                .map(|r| TableEntry {
                    database: dbname.to_string(),
                    schema: schema.to_string(),
                    name: r.get::<_, String>(0),
                    kind,
                    owner: r.try_get::<_, String>(1).unwrap_or_default(),
                    estimated_rows: r.try_get::<_, i64>(2).unwrap_or(0),
                })
                .collect())
        })
        .await
    }

    /// Every base table the connection can see, across all non-system
    /// databases and non-system schemas.
    pub async fn list_all_tables(&self) -> Result<Vec<TableEntry>, String> {
        self.list_all_relations(RelationKind::Table).await
    }

    /// Every view the connection can see, across all non-system
    /// databases and non-system schemas.
    pub async fn list_all_views(&self) -> Result<Vec<TableEntry>, String> {
        self.list_all_relations(RelationKind::View).await
    }

    /// Iterates `list_databases` and runs one cross-schema `pg_class`
    /// scan per database. A database that fails to query (auth/connection)
    /// is skipped silently — the other databases still appear, which
    /// matches DBeaver's behavior.
    async fn list_all_relations(&self, kind: RelationKind) -> Result<Vec<TableEntry>, String> {
        // `list_databases` is itself timeout-wrapped; we additionally
        // wrap the per-database `pg_class` scan so a single hung db
        // doesn't block the whole walk past the deadline.
        let dbs = self.list_databases().await?;
        let mut out = Vec::new();
        for db in dbs {
            let label = format!("list all relations in {}", db.name);
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
                             WHERE c.relkind::text = $1 \
                               AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
                               AND n.nspname NOT LIKE 'pg\\_%' \
                             ORDER BY n.nspname, c.relname",
                            &[&kind.relkind()],
                        )
                        .await
                        .map_err(|e| format!("query pg_class on {}: {e}", db.name))?;
                    Ok(rows
                        .into_iter()
                        .map(|r| TableEntry {
                            database: db.name.clone(),
                            schema: r.get::<_, String>(0),
                            name: r.get::<_, String>(1),
                            kind,
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

    /// Everything queryable in a single database across all non-system
    /// schemas — base tables **and** views — returned as
    /// `(schema, relation)` pairs sorted by `(schema, relation)`. Used by
    /// the script-editor's name completion, where the caller only wants
    /// names, not the full [`TableEntry`] (owner + estimated row count).
    ///
    /// Views belong in this list because a script can select from one just
    /// as it can from a table; leaving them out only means the completion
    /// silently knows less than the database does. One round trip per call
    /// — the result is small enough that caching it would cost more in
    /// invalidation logic than re-querying.
    pub async fn list_tables_in_database(
        &self,
        dbname: &str,
    ) -> Result<Vec<(String, String)>, String> {
        self.run_with_timeout(&format!("list relations in {dbname}"), async {
            let client = self.client(dbname).await?;
            let rows = client
                .query(
                    "SELECT n.nspname, c.relname \
                     FROM pg_class c \
                     JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE c.relkind IN ('r', 'v') \
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

    /// Paginated `SELECT * FROM schema.relation LIMIT … OFFSET …`.
    ///
    /// Two-step: pull column names from `information_schema.columns`,
    /// then issue a dynamically-built SELECT that text-casts every
    /// column (`col::text`). That sidesteps `tokio_postgres::FromSql`
    /// dispatch on unknown types — the adapter only ever sees strings
    /// (or `None` for NULL).
    ///
    /// A base table is ordered by `ctid`, which keeps page boundaries
    /// stable across `>`/`<` navigation as long as the table isn't being
    /// mutated. **A view has no `ctid`** — it is not a physical relation,
    /// so that clause would fail outright. Views are therefore read
    /// unordered and keep whatever order their own definition produces:
    /// a view with an `ORDER BY` in its body paginates stably, one
    /// without it only as stably as postgres' plan happens to be. That is
    /// the honest trade — the alternative, ordering by an arbitrary
    /// column, would be neither stable nor cheap.
    pub async fn query_rows(
        &self,
        dbname: &str,
        schema: &str,
        table: &str,
        kind: RelationKind,
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
                return Err(format!("{schema}.{table} has no columns or does not exist"));
            }

            let select_list = columns
                .iter()
                .map(|c| format!("{}::text", quote_ident(c)))
                .collect::<Vec<_>>()
                .join(", ");
            let qualified = format!("{}.{}", quote_ident(schema), quote_ident(table));
            let order_by = match kind {
                RelationKind::Table => "ORDER BY ctid ",
                RelationKind::View => "",
            };
            let sql = format!(
                "SELECT {select_list} FROM {qualified} {order_by}\
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

            Ok(RowsPage {
                columns,
                rows,
                has_more,
            })
        })
        .await
    }

    /// Which columns address one row of `schema.table` for the row editor.
    ///
    /// A primary key first; failing that the narrowest unique index over
    /// `NOT NULL` columns, named in the buffer header so the user can see
    /// which one was picked. `ctid` is deliberately *not* a fallback: it
    /// moves on every `UPDATE` and on `VACUUM FULL`, so a statement built
    /// from the `ctid` a page happened to show could hit a different row
    /// than the one on screen.
    ///
    /// A view is refused with the reason. Postgres can update through a
    /// simple view, but it has no key of its own to address a single row
    /// by, and guessing one from the body is exactly the kind of guess a
    /// save key must not make.
    pub async fn row_key_spec(
        &self,
        dbname: &str,
        schema: &str,
        table: &str,
    ) -> Result<RowKeySpec, String> {
        self.run_with_timeout(&format!("key of {schema}.{table}"), async {
            let client = self.client(dbname).await?;
            // `relkind` is Postgres' internal `"char"` type, which
            // `tokio_postgres` has no mapping for — read it as text.
            let relkind: Option<String> = client
                .query_opt(
                    "SELECT c.relkind::text \
                     FROM pg_class c \
                     JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE n.nspname = $1 AND c.relname = $2",
                    &[&schema, &table],
                )
                .await
                .map_err(|e| format!("looking {schema}.{table} up failed: {e}"))?
                .map(|row| row.get::<_, String>(0));
            match relkind.as_deref() {
                Some("v") | Some("m") => {
                    return Err(format!(
                        "{schema}.{table} is a view, which has no key to address a single row \
                         by — edit the row in the underlying table instead"
                    ));
                }
                None => {
                    return Err(format!("no relation named {schema}.{table} in {dbname}"));
                }
                Some(_) => {}
            }

            // Every usable unique index, best candidate first: the primary
            // key, then the narrowest. Partial and expression indexes are
            // excluded — neither addresses an arbitrary row.
            let rows = client
                .query(
                    "SELECT ci.relname, \
                            i.indisprimary, \
                            (SELECT array_agg(a.attname ORDER BY k.ord) \
                               FROM unnest(i.indkey::int2[]) WITH ORDINALITY AS k(attnum, ord) \
                               JOIN pg_attribute a ON a.attrelid = i.indrelid \
                                AND a.attnum = k.attnum AND NOT a.attisdropped), \
                            (SELECT bool_and(a.attnotnull) \
                               FROM unnest(i.indkey::int2[]) AS k(attnum) \
                               JOIN pg_attribute a ON a.attrelid = i.indrelid \
                                AND a.attnum = k.attnum) \
                       FROM pg_index i \
                       JOIN pg_class ci ON ci.oid = i.indexrelid \
                       JOIN pg_class c ON c.oid = i.indrelid \
                       JOIN pg_namespace n ON n.oid = c.relnamespace \
                      WHERE n.nspname = $1 AND c.relname = $2 \
                        AND i.indisunique AND i.indisvalid \
                        AND i.indpred IS NULL AND i.indexprs IS NULL \
                      ORDER BY i.indisprimary DESC, \
                               cardinality(i.indkey::int2[]) ASC, ci.relname",
                    &[&schema, &table],
                )
                .await
                .map_err(|e| format!("reading the keys of {schema}.{table} failed: {e}"))?;

            for row in &rows {
                let name: String = row.get(0);
                let is_primary: bool = row.get(1);
                let Some(columns) = row.try_get::<_, Option<Vec<String>>>(2).unwrap_or(None) else {
                    continue;
                };
                if columns.is_empty() {
                    continue;
                }
                if is_primary {
                    return Ok(RowKeySpec {
                        columns,
                        source: RowKeySource::PrimaryKey,
                    });
                }
                // A NULL in a unique index column does not conflict with
                // another NULL, so such an index can match several rows —
                // it is no key at all for this purpose.
                if row.try_get::<_, Option<bool>>(3).unwrap_or(None) == Some(true) {
                    return Ok(RowKeySpec {
                        columns,
                        source: RowKeySource::UniqueIndex(name),
                    });
                }
            }
            Err(format!(
                "{schema}.{table} has neither a primary key nor a unique index over NOT NULL \
                 columns, so a single row of it cannot be addressed — use a DB script with a \
                 WHERE of your own"
            ))
        })
        .await
    }

    /// The row at `offset` of the same listing the tree shows, read for
    /// editing. `None` when the relation has fewer rows than that.
    ///
    /// The ordering has to match [`PostgresClient::query_rows`] exactly, or
    /// the offset in a row's node id would address a different row here
    /// than the one that was rendered.
    pub async fn read_row_at(
        &self,
        dbname: &str,
        schema: &str,
        table: &str,
        keys: &RowKeySpec,
        offset: u32,
    ) -> Result<Option<RowRead>, String> {
        let rows = self
            .read_rows(
                dbname,
                schema,
                table,
                keys,
                &format!("ORDER BY ctid LIMIT 1 OFFSET {offset}"),
            )
            .await?;
        Ok(rows.into_iter().next())
    }

    /// Every row matching `where_sql` (built by
    /// [`not_yet_done_sql_core::row_edit::render_where`]), capped at two:
    /// callers only need to know whether the key addresses one row or
    /// several.
    pub async fn read_rows_where(
        &self,
        dbname: &str,
        schema: &str,
        table: &str,
        keys: &RowKeySpec,
        where_sql: &str,
    ) -> Result<Vec<RowRead>, String> {
        self.read_rows(
            dbname,
            schema,
            table,
            keys,
            &format!("WHERE {where_sql} LIMIT 2"),
        )
        .await
    }

    /// Shared read path: the key columns first, then the row itself, so a
    /// caller can split them apart positionally without worrying about a
    /// key column also appearing among the data columns.
    ///
    /// Every value is text-cast, as everywhere else in this client. No cell
    /// is marked read-only: Postgres' text output is also valid text input
    /// for the same type (`bytea` prints as `\x…` and reads back as those
    /// bytes), so what the buffer shows is what a literal writes back.
    async fn read_rows(
        &self,
        dbname: &str,
        schema: &str,
        table: &str,
        keys: &RowKeySpec,
        suffix: &str,
    ) -> Result<Vec<RowRead>, String> {
        self.run_with_timeout(&format!("row of {schema}.{table}"), async {
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
                return Err(format!("{schema}.{table} has no columns or does not exist"));
            }

            let text_list = |names: &[String]| {
                names
                    .iter()
                    .map(|c| format!("{}::text", quote_ident(c)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let sql = format!(
                "SELECT {}, {} FROM {}.{} {suffix}",
                text_list(&keys.columns),
                text_list(&columns),
                quote_ident(schema),
                quote_ident(table),
            );
            let data_rows = client
                .query(&sql, &[])
                .await
                .map_err(|e| format!("reading a row of {schema}.{table}: {e}"))?;

            let key_count = keys.columns.len();
            Ok(data_rows
                .iter()
                .map(|row| {
                    let cell = |i: usize| row.try_get::<_, Option<String>>(i).unwrap_or(None);
                    RowRead {
                        key_values: keys
                            .columns
                            .iter()
                            .enumerate()
                            .map(|(i, column)| (column.clone(), cell(i)))
                            .collect(),
                        cells: columns
                            .iter()
                            .enumerate()
                            .map(|(i, column)| RowCell::editable(column, cell(key_count + i)))
                            .collect(),
                    }
                })
                .collect())
        })
        .await
    }

    /// Run one write statement and report how many rows it changed.
    ///
    /// `execute` rather than `simple_query`: it refuses a multi-statement
    /// string, which is the guard that keeps a generated `UPDATE` a single
    /// statement even if a value somehow carried a semicolon.
    pub async fn execute_write(&self, dbname: &str, sql: &str) -> Result<u64, String> {
        self.run_with_timeout(&format!("write on {dbname}"), async {
            let client = self.client(dbname).await?;
            client.execute(sql, &[]).await.map_err(|e| format!("{e}"))
        })
        .await
    }

    /// The complete `CREATE OR REPLACE VIEW … AS …` statement for one
    /// view, or `None` when `schema.name` is not a view in `dbname`.
    ///
    /// `pg_get_viewdef` returns only the `SELECT` body — postgres does not
    /// store the statement the user originally typed, it re-prints the
    /// parsed definition. The `CREATE OR REPLACE VIEW` header is assembled
    /// here so callers see (and save) one whole statement, which is also
    /// what makes the saved text and the stored text directly comparable.
    ///
    /// The re-printed body is why an edit looks reformatted on the next
    /// open: postgres normalizes whitespace, casing and qualification. It
    /// is the same definition, just in the server's own spelling.
    pub async fn view_definition(
        &self,
        dbname: &str,
        schema: &str,
        name: &str,
    ) -> Result<Option<String>, String> {
        self.run_with_timeout(&format!("definition of view {schema}.{name}"), async {
            let client = self.client(dbname).await?;
            let row = client
                .query_opt(
                    "SELECT pg_get_viewdef(c.oid, true) \
                     FROM pg_class c \
                     JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE n.nspname = $1 AND c.relname = $2 AND c.relkind = 'v'",
                    &[&schema, &name],
                )
                .await
                .map_err(|e| format!("reading the definition of {schema}.{name} failed: {e}"))?;
            let Some(row) = row else {
                return Ok(None);
            };
            let body: String = row
                .try_get::<_, Option<String>>(0)
                .unwrap_or(None)
                .unwrap_or_default();
            let body = body.trim().trim_end_matches(';');
            Ok(Some(format!(
                "CREATE OR REPLACE VIEW {}.{} AS\n{body}",
                quote_ident(schema),
                quote_ident(name)
            )))
        })
        .await
    }

    /// Replace one view's definition with `create_sql`, run verbatim.
    ///
    /// Unlike SQLite there is nothing to drop: `CREATE OR REPLACE VIEW` is
    /// a single atomic statement, and postgres resolves the body eagerly,
    /// so a definition over a misspelled table is rejected here rather
    /// than on the first read. Dependent views and grants survive.
    ///
    /// The one thing it cannot do is change the result columns — postgres
    /// refuses to rename, retype or drop an existing view column (only
    /// appending is allowed). That error is passed through to the caller
    /// unchanged: `DROP VIEW … CASCADE` would take dependent objects and
    /// grants with it, which is not something a save key should do
    /// silently. A restructure belongs in a DB script.
    ///
    /// `create_sql` must have been checked to be a single `CREATE VIEW`
    /// naming *this* schema and view — see
    /// [`not_yet_done_sql_core::view_ddl::parse_create_view`]. That check
    /// includes the schema qualifier on purpose: an unqualified name would
    /// resolve against the session's `search_path` and could land the
    /// definition in a different schema than the one being edited. Fixing
    /// that here with `SET LOCAL search_path` would need an explicit
    /// transaction block, and a failed statement inside one leaves this
    /// cached session aborted for every later query — so the qualifier is
    /// required instead of worked around.
    pub async fn replace_view(
        &self,
        dbname: &str,
        schema: &str,
        name: &str,
        create_sql: &str,
    ) -> Result<(), String> {
        self.run_with_timeout(&format!("replace view {schema}.{name}"), async {
            let client = self.client(dbname).await?;
            client
                .batch_execute(create_sql)
                .await
                .map_err(|e| format!("replacing {schema}.{name} failed: {e}"))?;
            Ok(())
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
    pub async fn execute_raw_sql(&self, dbname: &str, sql: &str) -> Result<RawSqlOutcome, String> {
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
                            cur_columns =
                                row.columns().iter().map(|c| c.name().to_string()).collect();
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
                        last = Some(RawSqlOutcome {
                            columns,
                            rows,
                            status,
                        });
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

/// Whether a failed connect was the server rejecting our credentials, as
/// opposed to any of the many ways a connect can fail without saying
/// anything about the password (unreachable host, dead tunnel, unknown
/// database, TLS refusal).
///
/// Distinguishing the two is what lets us cache the resolved password
/// aggressively — see [`PostgresClient::password_cache`]. A connection
/// error carrying no `SqlState` never reached the auth exchange at all,
/// so it counts as "not an auth problem".
fn is_auth_failure(e: &tokio_postgres::Error) -> bool {
    use tokio_postgres::error::SqlState;
    matches!(
        e.code(),
        Some(&SqlState::INVALID_PASSWORD) | Some(&SqlState::INVALID_AUTHORIZATION_SPECIFICATION)
    )
}

fn map_sslmode(m: crate::config::SslMode) -> tokio_postgres::config::SslMode {
    use tokio_postgres::config::SslMode as Tps;
    match m {
        crate::config::SslMode::Disable => Tps::Disable,
        crate::config::SslMode::Prefer => Tps::Prefer,
        crate::config::SslMode::Require => Tps::Require,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique scratch path under `$TMPDIR`, removed by the caller. The
    /// crate has no `tempfile` dev-dep (see `query.rs` for the same
    /// pattern).
    fn scratch_path(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::AtomicU64;
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("nyd-pgclient-{tag}-{}-{n}", std::process::id()))
    }

    /// A client whose password provider is a shell script. No transport
    /// is ever opened — every test here exercises only the credential
    /// cache, which sits in front of any network work.
    fn client_with_password_script(script: &str) -> PostgresClient {
        let transport: TransportConfig =
            serde_yaml::from_str("target:\n  host: db.internal.invalid\n  port: 5432\n")
                .expect("transport yaml parses");
        let auth: PostgresAuth = serde_yaml::from_str(&format!(
            "user: tester\npassword:\n  type: command\n  script: '{script}'\n"
        ))
        .expect("auth yaml parses");
        PostgresClient::new(transport, auth, Some(Duration::from_secs(1)), None)
    }

    /// How many times the probe script has run so far.
    fn invocations(counter: &std::path::Path) -> usize {
        std::fs::read_to_string(counter)
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }

    /// The whole point of caching the password: a `pass`-backed provider
    /// pops a GPG pinentry dialog, so it must be asked exactly once —
    /// not once per connect, and not once per cursor session.
    #[tokio::test]
    async fn password_resolver_runs_once_and_is_cached() {
        let counter = scratch_path("pw-calls");
        let client = client_with_password_script(&format!(
            "echo x >> \"{}\"; echo hunter2",
            counter.display()
        ));

        assert!(!client.password_is_cached().await);
        assert_eq!(client.resolved_password().await.unwrap(), "hunter2");
        assert!(client.password_is_cached().await);
        assert_eq!(invocations(&counter), 1);

        // Second (and third) ask must not spawn the script again.
        assert_eq!(client.resolved_password().await.unwrap(), "hunter2");
        assert_eq!(client.resolved_password().await.unwrap(), "hunter2");
        assert_eq!(
            invocations(&counter),
            1,
            "cached password must not re-run the provider"
        );

        let _ = std::fs::remove_file(&counter);
    }

    /// A timeout tears down the tunnel, but the password stays valid —
    /// re-prompting there would mean a pinentry dialog on every stalled
    /// query.
    #[tokio::test]
    async fn tear_down_keeps_the_cached_password() {
        let counter = scratch_path("pw-teardown");
        let client = client_with_password_script(&format!(
            "echo x >> \"{}\"; echo hunter2",
            counter.display()
        ));

        client.resolved_password().await.unwrap();
        client.tear_down().await;

        assert!(client.password_is_cached().await);
        client.resolved_password().await.unwrap();
        assert_eq!(invocations(&counter), 1);

        let _ = std::fs::remove_file(&counter);
    }

    /// The one case where re-asking is correct: the server rejected the
    /// credentials, so the secret has presumably rotated.
    #[tokio::test]
    async fn invalidate_password_forces_a_fresh_resolve() {
        let counter = scratch_path("pw-invalidate");
        let client = client_with_password_script(&format!(
            "echo x >> \"{}\"; echo hunter2",
            counter.display()
        ));

        client.resolved_password().await.unwrap();
        client.invalidate_password().await;
        assert!(!client.password_is_cached().await);

        client.resolved_password().await.unwrap();
        assert_eq!(invocations(&counter), 2);

        let _ = std::fs::remove_file(&counter);
    }

    /// A failing provider must surface as an error and leave the cache
    /// empty, so the next attempt retries instead of serving a stale or
    /// empty secret.
    #[tokio::test]
    async fn failed_resolve_does_not_poison_the_cache() {
        let client = client_with_password_script("exit 7");
        assert!(client.resolved_password().await.is_err());
        assert!(!client.password_is_cached().await);
    }
}
