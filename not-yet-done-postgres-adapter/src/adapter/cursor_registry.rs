//! Lookup table of live `CursorSession`s keyed by an opaque `CursorId`
//! string. One registry per [`PostgresAdapter`] — the TUI never touches
//! a `CursorSession` directly, only string ids.
//!
//! ## Lifetime coupling
//!
//! The cursor sessions sit _outside_ of the client's `sessions` cache:
//! they own a dedicated `tokio_postgres::Client` and an open
//! transaction. When the client's `tear_down` fires (today: query
//! timeout) the shared transport is dropped, which kills every cursor
//! connection at once.
//!
//! The registry samples `client.teardown_generation()` at open time
//! and stores it on each entry. On `fetch` / `close` it re-reads the
//! atomic counter and compares: a mismatch means the transport was
//! reset and the cursor is dead. The error surfaces to the TUI as a
//! "cursor lost" message so the user can re-execute.
//!
//! Concurrent fetches on the same id are serialized by a per-entry
//! `Mutex<CursorSession>` instead of the registry-wide lock, so a
//! long-running fetch on one cursor never blocks lookups for another.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::Mutex;

use crate::client::cursor::{CursorSession, OpenCursorOutcome};
use crate::client::{PostgresClient, RowsPage};

/// Opaque handle the TUI carries between page-flips. Issued by
/// [`CursorRegistry::open`].
pub type CursorId = String;

pub struct CursorRegistry {
    client: Arc<PostgresClient>,
    /// Shared with the client; bumped on `tear_down`.
    teardown_gen: Arc<AtomicU64>,
    entries: Mutex<HashMap<CursorId, Entry>>,
}

struct Entry {
    session: Arc<Mutex<CursorSession>>,
    /// Value of `teardown_gen` at open time. A later mismatch flags
    /// this entry as invalidated.
    opened_at_gen: u64,
}

impl CursorRegistry {
    pub fn new(client: Arc<PostgresClient>) -> Self {
        let teardown_gen = client.teardown_generation();
        Self {
            client,
            teardown_gen,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Issue a fresh cursor for `sql`. Returns the id and the rows of
    /// the first page; the caller owns the id and must eventually
    /// call [`Self::close`] or the cursor stays alive until the next
    /// teardown.
    ///
    /// If the trailing statement is not a SELECT/WITH the cursor path
    /// degenerates: the registry executes the script directly and
    /// returns an `Err("not a cursor query: ...")`. Callers that want
    /// the script's `RawSqlOutcome` should bypass the registry and
    /// call `PostgresClient::execute_raw_sql` instead — the cursor
    /// path is for paginated SELECTs only.
    pub async fn open(
        &self,
        database: &str,
        sql: &str,
        page_size: u32,
    ) -> Result<(CursorId, RowsPage), String> {
        let gen_before = self.teardown_gen.load(Ordering::Acquire);
        let outcome = self.client.open_cursor(database, sql, page_size).await?;
        match outcome {
            OpenCursorOutcome::Cursor {
                session,
                first_page,
            } => {
                // If the transport was reset _during_ open the cursor
                // is already dead; surface that as a load error so the
                // TUI doesn't store a stale id.
                if self.teardown_gen.load(Ordering::Acquire) != gen_before {
                    return Err("cursor lost during open: connection reset".to_string());
                }
                let id = mint_cursor_id();
                let entry = Entry {
                    session: Arc::new(Mutex::new(session)),
                    opened_at_gen: gen_before,
                };
                self.entries.lock().await.insert(id.clone(), entry);
                Ok((id, first_page))
            }
            OpenCursorOutcome::NonCursor(_) => {
                Err("not a cursor query: trailing statement must be SELECT or WITH".to_string())
            }
        }
    }

    /// Fetch the next page from the cursor identified by `id`.
    ///
    /// Returns `Err("cursor lost: …")` if the registry has no such
    /// id, or if the underlying transport has been torn down since
    /// the cursor was opened.
    pub async fn fetch(&self, id: &str, page_size: u32) -> Result<RowsPage, String> {
        let (session, opened_at_gen) = {
            let guard = self.entries.lock().await;
            let entry = guard
                .get(id)
                .ok_or_else(|| "cursor lost: closed or unknown id".to_string())?;
            (Arc::clone(&entry.session), entry.opened_at_gen)
        };
        if self.teardown_gen.load(Ordering::Acquire) != opened_at_gen {
            // Best-effort tombstone; ignored if a concurrent close
            // already removed it.
            self.entries.lock().await.remove(id);
            return Err("cursor lost: connection reset".to_string());
        }
        let mut session_guard = session.lock().await;
        self.client
            .fetch_cursor_page(&mut session_guard, page_size)
            .await
    }

    /// `ROLLBACK` the underlying transaction and drop the session.
    /// No-ops on an unknown id so double-close from the TUI is safe.
    pub async fn close(&self, id: &str) -> Result<(), String> {
        let entry_opt = self.entries.lock().await.remove(id);
        let Some(entry) = entry_opt else {
            return Ok(());
        };
        // If another caller is mid-fetch they still hold an Arc clone;
        // dropping our reference lets their fetch finish and the
        // session aborts when the last Arc dies. We can only ROLLBACK
        // when we hold the sole reference.
        let Some(mutex) = Arc::into_inner(entry.session) else {
            return Ok(());
        };
        let session = mutex.into_inner();
        self.client.close_cursor(session).await
    }

    /// Number of live cursor entries (for tests / debug-banner).
    #[cfg(test)]
    async fn len(&self) -> usize {
        self.entries.lock().await.len()
    }
}

fn mint_cursor_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    format!("cur_{:08x}{:06x}", ts & 0xFFFF_FFFF, n & 0xFFFFFF)
}

#[cfg(test)]
mod tests {
    //! These tests cover the registry's bookkeeping: unknown-id
    //! lookups, double-close, and the teardown-generation invariant.
    //! Tests that need a live cursor session against a real Postgres
    //! server live in the integration smoke tests (`docs/smoke-tests.md`)
    //! — `tokio_postgres::Client` has no public constructor so we
    //! cannot synthesize a fake [`CursorSession`] in-process.
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::config::{PostgresAuth, SslMode};
    use not_yet_done_content::CredentialProvider;
    use not_yet_done_transport::{Endpoint, TransportConfig, TransportMode};

    fn dummy_client() -> Arc<PostgresClient> {
        Arc::new(PostgresClient::new(
            TransportConfig {
                mode: TransportMode::Direct,
                ssh: vec![],
                target: Endpoint {
                    host: "127.0.0.1".to_string(),
                    port: 5432,
                },
            },
            PostgresAuth {
                user: "u".to_string(),
                password: CredentialProvider::Literal {
                    value: "p".to_string(),
                },
                admin_database: "postgres".to_string(),
                sslmode: SslMode::Disable,
            },
            None,
            None,
        ))
    }

    #[tokio::test]
    async fn fetch_on_unknown_id_returns_cursor_lost() {
        let reg = CursorRegistry::new(dummy_client());
        let err = reg.fetch("does-not-exist", 100).await.unwrap_err();
        assert!(
            err.contains("cursor lost"),
            "expected 'cursor lost', got: {err}"
        );
    }

    #[tokio::test]
    async fn close_on_unknown_id_is_noop() {
        let reg = CursorRegistry::new(dummy_client());
        reg.close("does-not-exist").await.unwrap();
    }

    #[tokio::test]
    async fn teardown_generation_bump_propagates_to_registry() {
        // Verifies the registry reads the same atomic the client
        // bumps in `tear_down`. We can't trigger a real tear_down
        // without a live transport, so we read & bump the Arc
        // directly — the bookkeeping is identical.
        let client = dummy_client();
        let reg = CursorRegistry::new(Arc::clone(&client));
        let teardown: Arc<AtomicU64> = client.teardown_generation();
        let before = teardown.load(Ordering::Acquire);
        teardown.fetch_add(1, Ordering::Release);
        assert_eq!(reg.teardown_gen.load(Ordering::Acquire), before + 1);
    }

    #[tokio::test]
    async fn len_starts_zero() {
        let reg = CursorRegistry::new(dummy_client());
        assert_eq!(reg.len().await, 0);
    }
}
