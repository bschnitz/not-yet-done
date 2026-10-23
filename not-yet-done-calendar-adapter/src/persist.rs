//! Keeps the adapter's in-memory cache a mirror of the persistent
//! [`EventStore`].
//!
//! Every fetch result goes through [`ingest`]: it is written to the store
//! first and the connection's slice of the memory cache is then reloaded from
//! there, so memory never holds something the store does not. [`catch_up`]
//! covers the other direction — another process (a scheduled
//! `nyd adapter calendar root do sync`) stored newer events — by comparing each
//! connection's store revision with the one the cache was last loaded at.
//!
//! Without a store (unit tests) both degrade to the plain memory cache.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};

use not_yet_done_calendar_core::TimeRange;

use crate::adapter::{Cache, MergedEvent, replace_connection, store_connection};
use crate::store::EventStore;

/// The store plus the revision each connection's cached slice was loaded at.
pub(crate) struct Persisted {
    store: EventStore,
    loaded: Mutex<HashMap<String, i64>>,
}

impl Persisted {
    pub(crate) fn new(store: EventStore) -> Self {
        Self {
            store,
            loaded: Mutex::new(HashMap::new()),
        }
    }

    #[cfg(test)]
    pub(crate) fn store(&self) -> &EventStore {
        &self.store
    }

    /// Forget which revisions the cache holds — after the memory cache was
    /// cleared, the next [`catch_up`] reloads every connection.
    pub(crate) fn forget_loaded(&self) {
        self.loaded.lock().unwrap().clear();
    }

    fn mark_loaded(&self, connection: &str, revision: i64) {
        self.loaded
            .lock()
            .unwrap()
            .insert(connection.to_string(), revision);
    }

    fn loaded_at(&self, connection: &str) -> Option<i64> {
        self.loaded.lock().unwrap().get(connection).copied()
    }
}

/// How a fetch result landed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Ingest {
    /// The events replaced the connection's slice (store and memory).
    Stored,
    /// The memory cache holds the events, but writing the store failed.
    Unpersisted(String),
    /// The fetch came back empty while events are known for the connection —
    /// almost always a sign-in glitch (a login page parsed as "no events"), so
    /// the known events were kept.
    EmptyKept,
    /// The fetch came back empty and nothing was known before: recorded as a
    /// successful sync of an empty calendar.
    EmptyAccepted,
}

impl Ingest {
    /// Whether the visible event set may have changed (callers invalidate).
    pub(crate) fn changed(&self) -> bool {
        matches!(self, Ingest::Stored | Ingest::Unpersisted(_))
    }
}

/// Land one connection's successful fetch of `range`. `keep_from` is the
/// oldest instant worth keeping (the configured window's start).
///
/// `trust_empty` says the backend's empty answer is a fact rather than a
/// symptom (see `CalendarBackend::empty_is_authoritative`): the empty-clobber
/// guard is then skipped, and an empty fetch clears the connection.
pub(crate) async fn ingest(
    cache: &Cache,
    persisted: Option<&Persisted>,
    connection: &str,
    range: &TimeRange,
    keep_from: DateTime<Utc>,
    events: &[MergedEvent],
    trust_empty: bool,
) -> Ingest {
    let Some(p) = persisted else {
        return ingest_memory_only(cache, connection, events, trust_empty).await;
    };
    let now = Utc::now();
    if events.is_empty() && !trust_empty {
        let known = match p.store.has_events(connection).await {
            Ok(known) => known,
            Err(_) => cache_has_connection(cache, connection).await,
        };
        if known {
            let _ = p
                .store
                .record_failure(
                    connection,
                    "the fetch returned no events; kept the previous ones",
                    now,
                )
                .await;
            return Ingest::EmptyKept;
        }
    }
    match p
        .store
        .save(connection, range, keep_from, events, now)
        .await
    {
        Ok(revision) => match p.store.load(connection).await {
            Ok(stored) => {
                replace_connection(cache, connection, &stored).await;
                p.mark_loaded(connection, revision);
                if events.is_empty() {
                    Ingest::EmptyAccepted
                } else {
                    Ingest::Stored
                }
            }
            Err(e) => {
                store_connection(cache, connection, events).await;
                Ingest::Unpersisted(e.to_string())
            }
        },
        Err(e) => {
            if events.is_empty() {
                return Ingest::EmptyAccepted;
            }
            store_connection(cache, connection, events).await;
            Ingest::Unpersisted(e.to_string())
        }
    }
}

async fn ingest_memory_only(
    cache: &Cache,
    connection: &str,
    events: &[MergedEvent],
    trust_empty: bool,
) -> Ingest {
    if events.is_empty() && trust_empty {
        replace_connection(cache, connection, events).await;
        Ingest::EmptyAccepted
    } else if store_connection(cache, connection, events).await {
        Ingest::Stored
    } else if cache_has_connection(cache, connection).await {
        Ingest::EmptyKept
    } else {
        Ingest::EmptyAccepted
    }
}

async fn cache_has_connection(cache: &Cache, connection: &str) -> bool {
    let prefix = format!("calendar:{connection}:");
    cache.read().await.keys().any(|k| k.starts_with(&prefix))
}

/// Drop one event deleted at the source from the cache and the store. A
/// re-fetch cannot do this: when the deleted event was the connection's last
/// one, the fetch comes back empty and [`ingest`] keeps the known events.
pub(crate) async fn forget(
    cache: &Cache,
    persisted: Option<&Persisted>,
    connection: &str,
    global_id: &str,
) {
    cache.write().await.remove(global_id);
    if let Some(p) = persisted {
        if let Ok(revision) = p.store.remove(connection, global_id, Utc::now()).await {
            p.mark_loaded(connection, revision);
        }
    }
}

/// Record a failed fetch of `connection` in the store (no-op without one).
pub(crate) async fn note_failure(persisted: Option<&Persisted>, connection: &str, error: &str) {
    if let Some(p) = persisted {
        let _ = p.store.record_failure(connection, error, Utc::now()).await;
    }
}

/// Reload every connection whose stored revision moved past the one the cache
/// holds. Returns whether the cache changed. A store that cannot be read
/// leaves the cache as it is.
pub(crate) async fn catch_up(cache: &Cache, persisted: Option<&Persisted>) -> bool {
    let Some(p) = persisted else {
        return false;
    };
    let Ok(states) = p.store.sync_states().await else {
        return false;
    };
    let mut changed = false;
    for state in states {
        if p.loaded_at(&state.connection) == Some(state.revision) {
            continue;
        }
        let Ok(stored) = p.store.load(&state.connection).await else {
            continue;
        };
        replace_connection(cache, &state.connection, &stored).await;
        p.mark_loaded(&state.connection, state.revision);
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use chrono::TimeZone;
    use not_yet_done_calendar_core::{Attendee, CalEvent, Response, ShowAs};
    use tokio::sync::RwLock;

    use crate::store::connect;

    async fn persisted(name: &str, scope: &str) -> (Arc<sea_orm::DatabaseConnection>, Persisted) {
        let url = format!("sqlite:file:{name}?mode=memory&cache=shared");
        let db = Arc::new(connect(&url).await.expect("store opens"));
        let p = Persisted::new(EventStore::new(Arc::clone(&db), scope));
        (db, p)
    }

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, day, hour, 0, 0).unwrap()
    }

    fn event(conn: &str, uid: &str, day: u32, hour: u32, title: &str) -> MergedEvent {
        MergedEvent {
            global_id: format!("calendar:{conn}:{uid}"),
            account: "Work".to_string(),
            event: CalEvent {
                uid: uid.to_string(),
                calendar: "Work".to_string(),
                title: title.to_string(),
                start: at(day, hour),
                end: at(day, hour + 1),
                all_day: false,
                location: Some("Room 1".to_string()),
                organizer: None,
                show_as: ShowAs::Busy,
                body: None,
                url: None,
                online_meeting: None,
                attendees: Vec::new(),
                my_response: None,
                cancelled: false,
            },
        }
    }

    fn empty_cache() -> Cache {
        Arc::new(RwLock::new(HashMap::new()))
    }

    fn range(from_day: u32, to_day: u32) -> TimeRange {
        TimeRange::new(at(from_day, 0), at(to_day, 0))
    }

    async fn titles(cache: &Cache) -> Vec<String> {
        let mut t: Vec<String> = cache
            .read()
            .await
            .values()
            .map(|m| m.event.title.clone())
            .collect();
        t.sort();
        t
    }

    #[tokio::test]
    async fn a_fetch_lands_in_the_store_and_survives_a_fresh_cache() {
        let (db, p) = persisted("cal_store_survives", "calendar").await;
        let cache = empty_cache();
        let events = [
            event("a", "1", 10, 9, "Standup"),
            event("a", "2", 11, 9, "Review"),
        ];
        let outcome = ingest(
            &cache,
            Some(&p),
            "a",
            &range(9, 20),
            at(1, 0),
            &events,
            false,
        )
        .await;
        assert_eq!(outcome, Ingest::Stored);

        // A second process: new cache, same database.
        let other = Persisted::new(EventStore::new(db, "calendar"));
        let fresh = empty_cache();
        assert!(
            catch_up(&fresh, Some(&other)).await,
            "the stored slice loads"
        );
        assert_eq!(titles(&fresh).await, ["Review", "Standup"]);
        let stored = fresh
            .read()
            .await
            .get("calendar:a:1")
            .cloned()
            .expect("by id");
        assert_eq!(stored.event.location.as_deref(), Some("Room 1"));
        assert_eq!(stored.event.show_as, ShowAs::Busy);

        assert!(
            !catch_up(&fresh, Some(&other)).await,
            "nothing new the second time"
        );
    }

    #[tokio::test]
    async fn attendees_the_own_answer_and_a_cancellation_survive_the_store() {
        let (db, p) = persisted("cal_store_attendees", "calendar").await;
        let mut called_off = event("a", "1", 10, 9, "Standup");
        called_off.event.attendees = vec![
            Attendee {
                name: "Doe, Jane".to_string(),
                response: Response::Accepted,
            },
            Attendee {
                name: "Bob Example".to_string(),
                response: Response::Pending,
            },
        ];
        called_off.event.my_response = Some(Response::Tentative);
        called_off.event.cancelled = true;
        let events = [called_off.clone(), event("a", "2", 11, 9, "Review")];
        ingest(
            &empty_cache(),
            Some(&p),
            "a",
            &range(9, 20),
            at(1, 0),
            &events,
            false,
        )
        .await;

        let fresh = empty_cache();
        catch_up(
            &fresh,
            Some(&Persisted::new(EventStore::new(db, "calendar"))),
        )
        .await;
        let stored = fresh
            .read()
            .await
            .get("calendar:a:1")
            .cloned()
            .expect("by id");
        assert_eq!(stored.event.attendees, called_off.event.attendees);
        assert_eq!(stored.event.my_response, Some(Response::Tentative));
        assert!(stored.event.cancelled);
        let plain = fresh
            .read()
            .await
            .get("calendar:a:2")
            .cloned()
            .expect("by id");
        assert!(plain.event.attendees.is_empty());
        assert_eq!(plain.event.my_response, None);
        assert!(!plain.event.cancelled);
    }

    #[tokio::test]
    async fn catch_up_sees_a_newer_sync_from_another_process() {
        let (db, reader) = persisted("cal_store_newer", "calendar").await;
        let cache = empty_cache();
        let writer = Persisted::new(EventStore::new(db, "calendar"));
        let writer_cache = empty_cache();

        ingest(
            &writer_cache,
            Some(&writer),
            "a",
            &range(9, 20),
            at(1, 0),
            &[event("a", "1", 10, 9, "Old")],
            false,
        )
        .await;
        assert!(catch_up(&cache, Some(&reader)).await);
        ingest(
            &writer_cache,
            Some(&writer),
            "a",
            &range(9, 20),
            at(1, 0),
            &[event("a", "1", 10, 9, "New")],
            false,
        )
        .await;
        assert!(catch_up(&cache, Some(&reader)).await);
        assert_eq!(titles(&cache).await, ["New"]);
    }

    #[tokio::test]
    async fn a_fetch_replaces_only_its_range_and_its_connection() {
        let (_db, p) = persisted("cal_store_range", "calendar").await;
        let cache = empty_cache();
        ingest(
            &cache,
            Some(&p),
            "a",
            &range(1, 28),
            at(1, 0),
            &[
                event("a", "early", 3, 9, "Early"),
                event("a", "gone", 10, 9, "Gone"),
            ],
            false,
        )
        .await;
        ingest(
            &cache,
            Some(&p),
            "b",
            &range(1, 28),
            at(1, 0),
            &[event("b", "x", 10, 9, "Other")],
            false,
        )
        .await;

        // A narrower fetch of `a` that no longer has `gone`.
        ingest(
            &cache,
            Some(&p),
            "a",
            &range(9, 12),
            at(1, 0),
            &[event("a", "new", 11, 9, "New")],
            false,
        )
        .await;
        assert_eq!(titles(&cache).await, ["Early", "New", "Other"]);
    }

    #[tokio::test]
    async fn events_before_the_window_are_pruned() {
        let (_db, p) = persisted("cal_store_prune", "calendar").await;
        let cache = empty_cache();
        ingest(
            &cache,
            Some(&p),
            "a",
            &range(1, 28),
            at(1, 0),
            &[event("a", "old", 2, 9, "Old")],
            false,
        )
        .await;
        ingest(
            &cache,
            Some(&p),
            "a",
            &range(10, 28),
            at(10, 0),
            &[event("a", "n", 11, 9, "Now")],
            false,
        )
        .await;
        assert_eq!(titles(&cache).await, ["Now"]);
    }

    #[tokio::test]
    async fn an_empty_fetch_over_known_events_keeps_them_and_records_why() {
        let (_db, p) = persisted("cal_store_empty_kept", "calendar").await;
        let cache = empty_cache();
        ingest(
            &cache,
            Some(&p),
            "a",
            &range(1, 28),
            at(1, 0),
            &[event("a", "1", 10, 9, "Keep")],
            false,
        )
        .await;
        let outcome = ingest(&cache, Some(&p), "a", &range(1, 28), at(1, 0), &[], false).await;
        assert_eq!(outcome, Ingest::EmptyKept);
        assert_eq!(titles(&cache).await, ["Keep"]);
        let state = p.store().sync_states().await.unwrap().remove(0);
        assert!(state.synced_at.is_some(), "the last success stays recorded");
        assert!(
            state.last_error.is_some(),
            "the attempt is recorded as failed"
        );
    }

    #[tokio::test]
    async fn a_trusted_empty_fetch_clears_the_known_events() {
        let (_db, p) = persisted("cal_store_empty_trusted", "calendar").await;
        let cache = empty_cache();
        ingest(
            &cache,
            Some(&p),
            "a",
            &range(1, 28),
            at(1, 0),
            &[event("a", "1", 10, 9, "Gone")],
            true,
        )
        .await;
        let outcome = ingest(&cache, Some(&p), "a", &range(1, 28), at(1, 0), &[], true).await;
        assert_eq!(outcome, Ingest::EmptyAccepted);
        assert!(
            titles(&cache).await.is_empty(),
            "an empty calendar is empty"
        );
        let state = p.store().sync_states().await.unwrap().remove(0);
        assert!(
            state.last_error.is_none(),
            "and that is a success, not a failure"
        );
    }

    #[tokio::test]
    async fn an_empty_fetch_of_an_empty_calendar_is_a_sync() {
        let (_db, p) = persisted("cal_store_empty_ok", "calendar").await;
        let cache = empty_cache();
        let outcome = ingest(&cache, Some(&p), "a", &range(1, 28), at(1, 0), &[], false).await;
        assert_eq!(outcome, Ingest::EmptyAccepted);
        assert!(!outcome.changed());
        let state = p.store().sync_states().await.unwrap().remove(0);
        assert_eq!(state.revision, 1);
        assert!(state.last_error.is_none());
    }

    #[tokio::test]
    async fn a_failure_keeps_the_last_success() {
        let (_db, p) = persisted("cal_store_failure", "calendar").await;
        let cache = empty_cache();
        ingest(
            &cache,
            Some(&p),
            "a",
            &range(1, 28),
            at(1, 0),
            &[event("a", "1", 10, 9, "Keep")],
            false,
        )
        .await;
        note_failure(Some(&p), "a", "session expired").await;
        let state = p.store().sync_states().await.unwrap().remove(0);
        assert_eq!(state.revision, 1, "a failure is no new data");
        assert!(state.synced_at.is_some());
        assert_eq!(state.last_error.as_deref(), Some("session expired"));
    }

    #[tokio::test]
    async fn scopes_do_not_see_each_other() {
        let (db, p) = persisted("cal_store_scopes", "one").await;
        ingest(
            &empty_cache(),
            Some(&p),
            "a",
            &range(1, 28),
            at(1, 0),
            &[event("a", "1", 10, 9, "Mine")],
            false,
        )
        .await;
        let other = Persisted::new(EventStore::new(db, "two"));
        let cache = empty_cache();
        assert!(!catch_up(&cache, Some(&other)).await);
        assert!(cache.read().await.is_empty());
    }

    #[tokio::test]
    async fn a_forgotten_event_is_gone_for_every_process_even_the_last_one() {
        let (db, p) = persisted("cal_store_forget", "calendar").await;
        let cache = empty_cache();
        let events = [
            event("a", "1", 10, 9, "Standup"),
            event("a", "2", 11, 9, "Review"),
        ];
        ingest(
            &cache,
            Some(&p),
            "a",
            &range(9, 20),
            at(1, 0),
            &events,
            false,
        )
        .await;
        let other = Persisted::new(EventStore::new(db, "calendar"));
        let other_cache = empty_cache();
        catch_up(&other_cache, Some(&other)).await;

        forget(&cache, Some(&p), "a", "calendar:a:1").await;
        assert_eq!(titles(&cache).await, ["Review"]);
        assert!(
            !catch_up(&cache, Some(&p)).await,
            "its own cache is current"
        );
        assert!(catch_up(&other_cache, Some(&other)).await);
        assert_eq!(titles(&other_cache).await, ["Review"]);

        // The last event: a re-fetch would come back empty and keep it.
        forget(&cache, Some(&p), "a", "calendar:a:2").await;
        assert!(catch_up(&other_cache, Some(&other)).await);
        assert!(other_cache.read().await.is_empty());
    }
}
