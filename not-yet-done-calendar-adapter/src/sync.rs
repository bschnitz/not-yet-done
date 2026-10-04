//! The root's `sync` action: fetch the chosen connections now — on-demand ones
//! included —, land every result in the store, and report per connection.
//!
//! It is the entry point for anything that keeps the store fresh from outside
//! the TUI (a scheduler running `nyd adapter calendar root sync`), so a
//! connection that did not sync makes the whole action fail: the caller's
//! exit code has to say so.
//!
//! One kind of failure is told apart: a fetch that failed after its sign-in
//! asked a person for something ([`SyncOutcome::Unanswered`]). Nothing is
//! broken there — nobody approved the sign-in in time — so when that is the
//! only kind in a report, the action defers instead of failing.

use std::time::Duration;

use chrono::Utc;
use futures::future::join_all;

use not_yet_done_content::ActionArgs;

use crate::fetch::{Fetcher, Staleness};
use crate::persist::{self, Ingest};

/// The argument naming the connections to sync (ids, comma-separated or as a
/// list). Absent means every connection.
pub(crate) const CONNECTIONS_ARG: &str = "connections";

/// The argument bounding each connection's fetch, in seconds.
pub(crate) const TIMEOUT_ARG: &str = "timeout";

/// How long a connection may take when `timeout` is not given — long enough
/// for an interactive sign-in, short enough that a sign-in nobody sees (a
/// hidden browser waiting for a password) cannot hang the caller for good.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// How one connection's sync ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SyncOutcome {
    /// The fetch landed; the connection now holds `events` events.
    Synced { events: usize },
    /// The fetch landed in memory, but the store refused it.
    Unpersisted(String),
    /// The fetch came back empty over known events, which were kept.
    EmptyKept,
    /// The fetch itself failed.
    Failed(String),
    /// The fetch failed while its sign-in was waiting on a person — a number
    /// to approve, a code to type — who did not answer. Whatever the fetch
    /// said about it is kept. A sign-in that breaks *after* asking lands here
    /// too; only a later sync can tell the two apart.
    Unanswered(String),
}

impl SyncOutcome {
    fn is_failure(&self) -> bool {
        !matches!(self, SyncOutcome::Synced { .. })
    }

    fn describe(&self) -> String {
        match self {
            SyncOutcome::Synced { events: 1 } => "1 event".to_string(),
            SyncOutcome::Synced { events } => format!("{events} events"),
            SyncOutcome::Unpersisted(why) => format!("fetched, but not stored: {why}"),
            SyncOutcome::EmptyKept => {
                "came back empty, kept the known events (sign-in problem?)".to_string()
            }
            SyncOutcome::Failed(why) => format!("failed: {why}"),
            SyncOutcome::Unanswered(why) => {
                format!("waited for a sign-in nobody answered ({why})")
            }
        }
    }
}

/// One line per synced connection, in configuration order.
#[derive(Debug)]
pub(crate) struct SyncReport {
    pub(crate) lines: Vec<(String, SyncOutcome)>,
}

impl SyncReport {
    /// Whether any connection did not sync.
    pub(crate) fn failed(&self) -> bool {
        self.lines.iter().any(|(_, outcome)| outcome.is_failure())
    }

    /// Whether the sync should be tried again later rather than reported as
    /// broken: something did not sync, and every connection that did not
    /// was waiting on a person who did not answer.
    pub(crate) fn deferred(&self) -> bool {
        self.failed()
            && self.lines.iter().all(|(_, outcome)| {
                !outcome.is_failure() || matches!(outcome, SyncOutcome::Unanswered(_))
            })
    }

    /// `label: outcome` per connection, joined for a status line.
    pub(crate) fn message(&self) -> String {
        self.lines
            .iter()
            .map(|(label, outcome)| format!("{label}: {}", outcome.describe()))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// The backend indices `args` asks for, in configuration order. An unknown id
/// is an error naming the known ones — a typo must not pass as a sync.
pub(crate) fn select(fetcher: &Fetcher, args: &ActionArgs) -> Result<Vec<usize>, String> {
    let Some(requested) = args.list(CONNECTIONS_ARG) else {
        return Ok(fetcher.fetchable(true));
    };
    let ids: Vec<String> = requested
        .iter()
        .flat_map(|item| item.split(','))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();
    if ids.is_empty() {
        return Ok(fetcher.fetchable(true));
    }
    let unknown: Vec<&str> = ids
        .iter()
        .filter(|id| fetcher.index_of(id).is_none())
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        let known: Vec<&str> = fetcher.backends.iter().map(|b| b.connection_id()).collect();
        return Err(format!(
            "unknown connection {} (known: {})",
            unknown.join(", "),
            known.join(", ")
        ));
    }
    Ok(fetcher
        .fetchable(true)
        .into_iter()
        .filter(|&idx| ids.iter().any(|id| id == fetcher.backends[idx].connection_id()))
        .collect())
}

/// The per-connection time limit `args` asks for (whole seconds, > 0).
pub(crate) fn timeout(args: &ActionArgs) -> Result<Duration, String> {
    let Some(raw) = args.get(TIMEOUT_ARG) else {
        return Ok(DEFAULT_TIMEOUT);
    };
    raw.as_int()
        .or_else(|| raw.as_text().and_then(|t| t.trim().parse().ok()))
        .filter(|&secs| secs > 0)
        .map(|secs| Duration::from_secs(secs as u64))
        .ok_or_else(|| format!("{TIMEOUT_ARG} expects a positive number of seconds"))
}

/// Sync `indices` over the configured window, all at once — a connection
/// waiting for an interactive sign-in must not hold up the others. A
/// connection that takes longer than `limit` counts as failed (and is noted
/// so in the store); one whose sign-in asked a person for something on the
/// way counts as [unanswered](SyncOutcome::Unanswered) instead.
pub(crate) async fn run(fetcher: &Fetcher, indices: &[usize], limit: Duration) -> SyncReport {
    let range = fetcher.window.range(Utc::now());
    let results = join_all(indices.iter().map(|&idx| {
        let range = &range;
        async move {
            let backend = &fetcher.backends[idx];
            let asked_before = backend.sign_in_prompts();
            let result = match tokio::time::timeout(
                limit,
                fetcher.sync_one(idx, range, Staleness::Keep),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => {
                    let why = format!(
                        "{}: no answer within {}s",
                        backend.connection_label(),
                        limit.as_secs()
                    );
                    persist::note_failure(
                        fetcher.persisted.as_deref(),
                        backend.connection_id(),
                        &why,
                    )
                    .await;
                    Err(why)
                }
            };
            (result, backend.sign_in_prompts() > asked_before)
        }
    }))
    .await;
    let mut lines = Vec::with_capacity(indices.len());
    for (&idx, (result, asked)) in indices.iter().zip(results) {
        let backend = &fetcher.backends[idx];
        let label = backend.connection_label();
        let outcome = match result {
            Ok(Some(Ingest::Stored | Ingest::EmptyAccepted)) | Ok(None) => SyncOutcome::Synced {
                events: fetcher.cached_count(backend.connection_id()).await,
            },
            Ok(Some(Ingest::Unpersisted(why))) => SyncOutcome::Unpersisted(why),
            Ok(Some(Ingest::EmptyKept)) => SyncOutcome::EmptyKept,
            // Fetch errors name their connection for the store and the log;
            // the report already leads with it.
            Err(why) => {
                let why = why
                    .strip_prefix(&format!("{label}: "))
                    .map(str::to_string)
                    .unwrap_or(why);
                if asked {
                    SyncOutcome::Unanswered(why)
                } else {
                    SyncOutcome::Failed(why)
                }
            }
        };
        lines.push((label.to_string(), outcome));
    }
    SyncReport { lines }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::AtomicU64;
    use std::sync::Arc;

    use async_trait::async_trait;
    use chrono::Duration;
    use not_yet_done_calendar_core::{CalEvent, CalendarBackend, CalendarError, ShowAs, TimeRange};
    use tokio::sync::{broadcast, RwLock};

    use crate::config::Window;
    use crate::persist::Persisted;
    use crate::store::{connect, EventStore};

    /// A backend that answers every fetch with the same scripted result.
    struct Scripted {
        id: String,
        answer: Result<usize, String>,
    }

    #[async_trait]
    impl CalendarBackend for Scripted {
        fn connection_id(&self) -> &str {
            &self.id
        }
        async fn list_events(&self, range: &TimeRange) -> Result<Vec<CalEvent>, CalendarError> {
            let count = self.answer.clone().map_err(CalendarError::Network)?;
            Ok((0..count)
                .map(|i| CalEvent {
                    uid: format!("e{i}"),
                    calendar: "Work".to_string(),
                    title: format!("Meeting {i}"),
                    start: range.start + Duration::hours(i as i64 + 1),
                    end: range.start + Duration::hours(i as i64 + 2),
                    all_day: false,
                    location: None,
                    organizer: None,
                    show_as: ShowAs::Busy,
                    body: None,
                    url: None,
                    online_meeting: None,
                    attendees: Vec::new(),
                    my_response: None,
                    cancelled: false,
                })
                .collect())
        }
    }

    /// A backend whose fetch never answers — a sign-in nobody completes.
    struct Silent;

    #[async_trait]
    impl CalendarBackend for Silent {
        fn connection_id(&self) -> &str {
            "silent"
        }
        async fn list_events(&self, _range: &TimeRange) -> Result<Vec<CalEvent>, CalendarError> {
            std::future::pending().await
        }
    }

    /// A backend whose sign-in asks a person to approve it and then waits
    /// for good — the approval nobody gave.
    struct Asking {
        asked: AtomicU64,
    }

    #[async_trait]
    impl CalendarBackend for Asking {
        fn connection_id(&self) -> &str {
            "asking"
        }
        async fn list_events(&self, _range: &TimeRange) -> Result<Vec<CalEvent>, CalendarError> {
            self.asked.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            std::future::pending().await
        }
        fn sign_in_prompts(&self) -> u64 {
            self.asked.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    fn asking() -> Box<dyn CalendarBackend> {
        Box::new(Asking {
            asked: AtomicU64::new(0),
        })
    }

    fn scripted(id: &str, answer: Result<usize, String>) -> Box<dyn CalendarBackend> {
        Box::new(Scripted {
            id: id.to_string(),
            answer,
        })
    }

    async fn fetcher(
        name: &str,
        backends: Vec<Box<dyn CalendarBackend>>,
        on_demand: &[&str],
    ) -> Fetcher {
        let url = format!("sqlite:file:{name}?mode=memory&cache=shared");
        let db = Arc::new(connect(&url).await.expect("store opens"));
        Fetcher {
            backends: Arc::new(backends),
            on_demand: Arc::new(on_demand.iter().map(|s| s.to_string()).collect::<HashSet<_>>()),
            cache: Arc::new(RwLock::new(HashMap::new())),
            persisted: Some(Arc::new(Persisted::new(EventStore::new(db, "cal")))),
            window: Window {
                past_days: 1,
                future_days: 1,
            },
            inv_tx: broadcast::channel(16).0,
            query_gen: Arc::new(AtomicU64::new(0)),
        }
    }

    fn args(connections: &str) -> ActionArgs {
        ActionArgs::new().with(CONNECTIONS_ARG, connections)
    }

    #[tokio::test]
    async fn no_argument_selects_every_connection_including_on_demand() {
        let f = fetcher(
            "sync_select_all",
            vec![scripted("a", Ok(1)), scripted("b", Ok(1))],
            &["b"],
        )
        .await;
        assert_eq!(select(&f, &ActionArgs::new()).unwrap(), vec![0, 1]);
    }

    #[tokio::test]
    async fn a_comma_list_selects_those_in_configuration_order() {
        let f = fetcher(
            "sync_select_some",
            vec![scripted("a", Ok(1)), scripted("b", Ok(1)), scripted("c", Ok(1))],
            &[],
        )
        .await;
        assert_eq!(select(&f, &args("c, a")).unwrap(), vec![0, 2]);
    }

    #[tokio::test]
    async fn an_unknown_connection_is_refused() {
        let f = fetcher("sync_select_unknown", vec![scripted("a", Ok(1))], &[]).await;
        let err = select(&f, &args("a,nope")).unwrap_err();
        assert!(err.contains("nope") && err.contains("known: a"), "{err}");
    }

    #[tokio::test]
    async fn a_sync_stores_every_connection_and_counts_its_events() {
        let f = fetcher(
            "sync_run_ok",
            vec![scripted("a", Ok(2)), scripted("b", Ok(0))],
            &["b"],
        )
        .await;
        let report = run(&f, &[0, 1], DEFAULT_TIMEOUT).await;
        assert!(!report.failed());
        assert_eq!(report.message(), "a: 2 events; b: 0 events");
        let store = f.persisted.as_ref().unwrap().store();
        let states = store.sync_states().await.unwrap();
        assert!(states.iter().all(|s| s.synced_at.is_some() && s.last_error.is_none()));
        assert_eq!(states.len(), 2);
    }

    #[tokio::test]
    async fn one_failing_connection_fails_the_sync_but_not_the_others() {
        let f = fetcher(
            "sync_run_partial",
            vec![scripted("a", Err("down".into())), scripted("b", Ok(1))],
            &[],
        )
        .await;
        let report = run(&f, &[0, 1], DEFAULT_TIMEOUT).await;
        assert!(report.failed());
        assert_eq!(report.message(), "a: failed: network error: down; b: 1 event");
    }

    #[tokio::test]
    async fn an_empty_answer_over_known_events_fails_the_sync() {
        let f = fetcher("sync_run_empty_kept", vec![scripted("a", Ok(1))], &[]).await;
        assert!(!run(&f, &[0], DEFAULT_TIMEOUT).await.failed());
        let emptied = Fetcher {
            backends: Arc::new(vec![scripted("a", Ok(0))]),
            ..f
        };
        let report = run(&emptied, &[0], DEFAULT_TIMEOUT).await;
        assert!(report.failed());
        assert_eq!(report.lines[0].1, SyncOutcome::EmptyKept);
    }

    #[tokio::test]
    async fn a_connection_that_never_answers_times_out_and_is_noted() {
        let f = fetcher(
            "sync_run_timeout",
            vec![Box::new(Silent), scripted("b", Ok(1))],
            &[],
        )
        .await;
        let report = run(&f, &[0, 1], std::time::Duration::from_millis(50)).await;
        assert!(report.failed());
        assert_eq!(
            report.message(),
            "silent: failed: no answer within 0s; b: 1 event"
        );
        let states = f.persisted.as_ref().unwrap().store().sync_states().await.unwrap();
        let silent = states.iter().find(|s| s.connection == "silent").unwrap();
        assert!(silent.last_error.as_deref().unwrap().contains("no answer"));
    }

    #[tokio::test]
    async fn a_sign_in_nobody_answered_defers_rather_than_fails() {
        let f = fetcher("sync_run_unanswered", vec![asking(), scripted("b", Ok(1))], &[]).await;
        let report = run(&f, &[0, 1], std::time::Duration::from_millis(50)).await;
        assert!(report.failed());
        assert!(report.deferred());
        assert_eq!(
            report.message(),
            "asking: waited for a sign-in nobody answered (no answer within 0s); b: 1 event"
        );
    }

    #[tokio::test]
    async fn a_real_failure_beside_an_unanswered_sign_in_is_not_deferred() {
        let f = fetcher(
            "sync_run_unanswered_and_broken",
            vec![asking(), scripted("b", Err("down".into()))],
            &[],
        )
        .await;
        let report = run(&f, &[0, 1], std::time::Duration::from_millis(50)).await;
        assert!(report.failed());
        assert!(!report.deferred());
    }

    #[tokio::test]
    async fn a_silent_timeout_without_a_question_is_a_failure() {
        let f = fetcher("sync_run_silent_not_deferred", vec![Box::new(Silent)], &[]).await;
        let report = run(&f, &[0], std::time::Duration::from_millis(50)).await;
        assert!(!report.deferred());
        assert!(matches!(report.lines[0].1, SyncOutcome::Failed(_)));
    }

    #[test]
    fn the_timeout_argument_takes_positive_seconds() {
        assert_eq!(timeout(&ActionArgs::new()).unwrap(), DEFAULT_TIMEOUT);
        assert_eq!(
            timeout(&ActionArgs::new().with(TIMEOUT_ARG, "90")).unwrap(),
            std::time::Duration::from_secs(90)
        );
        assert!(timeout(&ActionArgs::new().with(TIMEOUT_ARG, "0")).is_err());
        assert!(timeout(&ActionArgs::new().with(TIMEOUT_ARG, "soon")).is_err());
    }

    #[test]
    fn the_report_reads_per_connection() {
        let report = SyncReport {
            lines: vec![
                ("Work".to_string(), SyncOutcome::Synced { events: 1 }),
                ("Home".to_string(), SyncOutcome::Unpersisted("disk full".into())),
            ],
        };
        assert_eq!(
            report.message(),
            "Work: 1 event; Home: fetched, but not stored: disk full"
        );
    }
}
