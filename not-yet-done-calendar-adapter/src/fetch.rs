//! Fetching connections and landing the results — the one path every fetch
//! (list-triggered, background poll, post-login ready-watch, after a create,
//! explicit sync) takes into the store and the cache.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use tokio::sync::broadcast;
use tokio::task::AbortHandle;

use not_yet_done_calendar_core::{CalendarBackend, TimeRange};
use not_yet_done_content::Invalidation;

use crate::adapter::{Cache, MergedEvent};
use crate::config::Window;
use crate::persist::{self, Ingest, Persisted};

/// Everything a fetch needs to land its result. Cheap to clone (all shared
/// handles), so each spawned task carries its own.
#[derive(Clone)]
pub(crate) struct Fetcher {
    pub(crate) backends: Arc<Vec<Box<dyn CalendarBackend>>>,
    /// Ids of the `fetch: on_demand` connections.
    pub(crate) on_demand: Arc<HashSet<String>>,
    pub(crate) cache: Cache,
    pub(crate) persisted: Option<Arc<Persisted>>,
    pub(crate) window: Window,
    pub(crate) inv_tx: broadcast::Sender<Invalidation>,
    /// See `CalendarAdapter::query_gen`.
    pub(crate) query_gen: Arc<AtomicU64>,
}

/// Whether a fetch may discard its result when the query moved meanwhile.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Staleness {
    /// Drop the result if the query generation changed during the fetch — for
    /// background fetches whose window derives from the current query.
    DropIfQueryMoved,
    /// Always land it — for an explicit sync.
    Keep,
}

impl Fetcher {
    /// Indices of the backends a fetch may touch: the live ones, plus the
    /// on-demand ones when `include_on_demand`.
    pub(crate) fn fetchable(&self, include_on_demand: bool) -> Vec<usize> {
        (0..self.backends.len())
            .filter(|&i| {
                include_on_demand || !self.on_demand.contains(self.backends[i].connection_id())
            })
            .collect()
    }

    /// Index of the backend serving `connection`.
    pub(crate) fn index_of(&self, connection: &str) -> Option<usize> {
        self.backends
            .iter()
            .position(|b| b.connection_id() == connection)
    }

    /// Fetch one connection for `range` and land the result: stored and
    /// mirrored on success (invalidating the view when the set changed), noted
    /// in the store on failure. `Ok(None)` means the result was dropped as
    /// stale.
    pub(crate) async fn sync_one(
        &self,
        idx: usize,
        range: &TimeRange,
        staleness: Staleness,
    ) -> Result<Option<Ingest>, String> {
        let backend = self.backends[idx].as_ref();
        let conn = backend.connection_id().to_string();
        let gen_start = self.query_gen.load(Ordering::SeqCst);
        let fetched = fetch_one(backend, range).await;
        if matches!(staleness, Staleness::DropIfQueryMoved)
            && self.query_gen.load(Ordering::SeqCst) != gen_start
        {
            return Ok(None);
        }
        match fetched {
            Ok(events) => {
                let outcome = self.land(&conn, range, &events).await;
                if outcome.changed() {
                    let _ = self.inv_tx.send(Invalidation::All);
                }
                Ok(Some(outcome))
            }
            Err(e) => {
                persist::note_failure(self.persisted.as_deref(), &conn, &e).await;
                Err(e)
            }
        }
    }

    /// Land an already-fetched result of `connection` (no invalidation).
    pub(crate) async fn land(
        &self,
        connection: &str,
        range: &TimeRange,
        events: &[MergedEvent],
    ) -> Ingest {
        let keep_from = self.window.range(Utc::now()).start;
        persist::ingest(
            &self.cache,
            self.persisted.as_deref(),
            connection,
            range,
            keep_from,
            events,
        )
        .await
    }

    /// Load whatever another process stored since the cache was last filled.
    pub(crate) async fn catch_up(&self) -> bool {
        persist::catch_up(&self.cache, self.persisted.as_deref()).await
    }

    /// Kick one detached fetch per fetchable connection for `range`, each
    /// landing its slice the moment it arrives — so a slow backend's
    /// interactive login can't hold up the others, nor block the caller.
    /// The abort handles replace `tasks`' previous round, so a hard refresh
    /// can cancel them. No-op outside a Tokio runtime.
    pub(crate) fn spawn_round(
        &self,
        include_on_demand: bool,
        range: TimeRange,
        tasks: &Mutex<Vec<AbortHandle>>,
    ) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let handles = self
            .fetchable(include_on_demand)
            .into_iter()
            .map(|idx| {
                let fetcher = self.clone();
                handle
                    .spawn(async move {
                        let _ = fetcher
                            .sync_one(idx, &range, Staleness::DropIfQueryMoved)
                            .await;
                    })
                    .abort_handle()
            })
            .collect();
        *tasks.lock().unwrap() = handles;
    }
}

/// Fetch a single backend and namespace its events, sorted by start instant
/// (then global id) for a stable order. An error carries the connection label.
pub(crate) async fn fetch_one(
    backend: &dyn CalendarBackend,
    range: &TimeRange,
) -> Result<Vec<MergedEvent>, String> {
    let conn = backend.connection_id();
    let events = backend
        .list_events(range)
        .await
        .map_err(|e| format!("{}: {e}", backend.connection_label()))?;
    let mut merged: Vec<MergedEvent> = events
        .into_iter()
        .map(|event| MergedEvent {
            global_id: format!("calendar:{conn}:{}", event.uid),
            account: event.calendar.clone(),
            event,
        })
        .collect();
    merged.sort_by(|a, b| {
        a.event
            .start
            .cmp(&b.event.start)
            .then_with(|| a.global_id.cmp(&b.global_id))
    });
    Ok(merged)
}
