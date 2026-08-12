//! The `calendar` ContentAdapter.
//!
//! Aggregates events from any number of configured connections into one flat,
//! time-sorted, groupable list — the calendar counterpart of the trackings
//! view. Each connection is a [`CalendarBackend`]; the adapter fans
//! [`CalendarBackend::list_events`] out across all of them, namespaces every
//! event id with its connection id, and merges the results.
//!
//! Grouping (by day/week/account/…) is engine-side via the view's `group_by`;
//! the adapter only emits the fields to group and sort on. Live update is
//! decoupled: a background task re-fetches on an interval and, when the set of
//! events actually changed, pushes [`Invalidation::All`] over the content
//! layer's broadcast channel — the frontend's generic watcher reloads, with no
//! calendar-specific code on that side.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Local, Utc};
use tokio::sync::{RwLock, broadcast, mpsc, watch};
use tokio::task::AbortHandle;

use natural_date::resolve_datetime;
use not_yet_done_calendar_core::{
    CalEvent, CalendarBackend, EventDraft, LoadProgress, ShowAs, TimeRange,
};
use not_yet_done_content::*;

use crate::config::{
    CalendarConfig, DEFAULT_POLL_INTERVAL_SECS, DEFAULT_WINDOW_FUTURE_DAYS,
    DEFAULT_WINDOW_PAST_DAYS, Window, default_reminder_leads,
};
use crate::query::CalendarQuery;
use crate::registry::backend_factories;

fn other_err(msg: impl Into<String>) -> ContentError {
    ContentError::Other(msg.into().into())
}

/// Wall-clock milliseconds since the Unix epoch, for [`AdapterStatus::Busy`]'s
/// `started_at_unix_ms` (the frontend computes elapsed against it each tick).
fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Opt-in reminder-scheduler tracing (`NYD_DEBUG_REMINDER=1`). Appends one line
/// to `$TMPDIR/nyd-reminder-debug.log` so a single live TUI run pins the stage
/// at which reminders stall: arming, per-tick cache/due counts, and each fire.
/// No-op unless the env var is set (mirrors the `NYD_DEBUG_TREEFIND` pattern).
fn rem_trace(args: std::fmt::Arguments<'_>) {
    if std::env::var_os("NYD_DEBUG_REMINDER").is_none() {
        return;
    }
    let path = std::env::temp_dir().join("nyd-reminder-debug.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let ts = Local::now().to_rfc3339();
        let _ = writeln!(f, "[reminder {ts}] {args}");
    }
}

fn event_node_type() -> NodeType {
    NodeType {
        type_id: "calendar:event".into(),
        mime_type: "text/markdown".into(),
        syntax: Some("markdown".into()),
        file_extension: "md".into(),
        display_name: "Event".into(),
    }
}

fn event_columns() -> Vec<ColumnSchema> {
    [
        ("start", "Start", "datetime"),
        ("end", "End", "datetime"),
        ("account", "Account", "text"),
        ("show_as", "Show as", "text"),
        ("title", "Title", "text"),
    ]
    .into_iter()
    .map(|(key, label, value_type)| ColumnSchema::new(key, label).typed(value_type))
    .collect()
}

// ---------------------------------------------------------------------------
// `New event` action (prototype)
// ---------------------------------------------------------------------------
//
// A single `InputSpec::Form` action on the calendar root that exercises every
// form-field kind the new spec-driven driver renders — Text, DateTime (natural
// language), Toggle, and Select — so the form UX can be refined end-to-end
// against a real, non-trivial form before the write path exists.
//
// It is deliberately a **dry run**: `CalendarBackend` is read-only today, so
// `execute` validates the whole form (including resolving the natural-language
// start/end via `natural-date`) and reports the event it *would* create,
// without touching any backend. When a backend write path (CalDAV `PUT`,
// Graph `POST`) lands, only `execute_create` changes — the form contract and
// the frontend are already in place.

/// The "Show as" choices offered by the form, paired with the [`ShowAs`] token
/// they map to. The label is what the user picks; the enum is what a future
/// write path would send.
const SHOW_AS_CHOICES: &[(&str, ShowAs)] = &[
    ("Busy", ShowAs::Busy),
    ("Free", ShowAs::Free),
    ("Tentative", ShowAs::Tentative),
    ("Out of office", ShowAs::OutOfOffice),
    ("Working elsewhere", ShowAs::WorkingElsewhere),
];

/// One write-target calendar, flattened out of a connection's
/// [`CalendarBackend::list_calendars`] — the unit the "New event" form's
/// "Calendar" picker offers. All connections' calendars are pooled into one
/// list (see [`CalendarAdapter::ensure_calendars`]), so the picker spans every
/// account at once.
#[derive(Clone, Debug)]
struct GlobalCalendar {
    /// Connection that owns this calendar ([`CalendarBackend::connection_id`]) —
    /// how `execute_create` finds the backend to write through.
    conn_id: String,
    /// Backend-local calendar id, passed verbatim to
    /// [`CalendarBackend::create_event`].
    cal_id: String,
    /// Picker label: the calendar name, prefixed with the connection label when
    /// more than one connection is configured (and suffixed on a collision) so
    /// the flattened list stays unambiguous.
    label: String,
}

/// Build the `New event` Form spec. The `calendar` picker is only added when
/// there is a genuine choice (more than one writable calendar) — with a single
/// target it is unambiguous, so the field would be noise.
fn create_event_spec(calendars: &[GlobalCalendar]) -> InputSpec {
    let show_as: Vec<String> = SHOW_AS_CHOICES.iter().map(|(l, _)| l.to_string()).collect();
    let mut fields = vec![
        FormFieldSpec::text("title", "Title"),
        FormFieldSpec::datetime("start", "Start", true).with_default("today 9:00"),
        FormFieldSpec::datetime("end", "End", true).with_default("today 10:00"),
        FormFieldSpec::toggle("all_day", "All day"),
        FormFieldSpec::select("show_as", "Show as", show_as).with_default("Busy"),
        FormFieldSpec::text("location", "Location").optional(),
        FormFieldSpec::text("body", "Notes").optional(),
    ];
    if calendars.len() > 1 {
        // Right after the title, before the times: pick the calendar first.
        let labels: Vec<String> = calendars.iter().map(|c| c.label.clone()).collect();
        fields.insert(
            1,
            FormFieldSpec::select("calendar", "Calendar", labels.clone())
                .with_default(labels[0].clone()),
        );
    }
    InputSpec::Form { fields }
}

/// The root's `New event` action (default key `a`, action-bar placement).
fn create_event_action(calendars: &[GlobalCalendar]) -> NodeAction {
    NodeAction::new("create", "New event", create_event_spec(calendars))
}

// -- Form-value helpers (the calendar counterpart of local-adapter's form.rs) --

/// Read a Form field, treating absent or whitespace-only values as `None`.
fn form_opt(values: &HashMap<String, String>, key: &str) -> Option<String> {
    values
        .get(key)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// Read a required Form field; error if absent or empty.
fn form_required<'a>(values: &'a HashMap<String, String>, key: &str) -> Result<&'a str> {
    match values.get(key).map(|v| v.trim()) {
        Some(v) if !v.is_empty() => Ok(v),
        _ => Err(other_err(format!("field '{key}' is required"))),
    }
}

/// A Toggle delivers `"true"`/`"false"`; anything but `"true"` (incl. absent)
/// is off.
fn form_flag(values: &HashMap<String, String>, key: &str) -> bool {
    values.get(key).map(|v| v == "true").unwrap_or(false)
}

/// `execute("create")` — validate the form, then write the event through the
/// backend that owns the chosen calendar and refresh that connection's cache
/// slice so the new event shows immediately.
async fn execute_create(
    backends: &[Box<dyn CalendarBackend>],
    calendars: &[GlobalCalendar],
    cache: &Cache,
    window: Window,
    inv_tx: &broadcast::Sender<Invalidation>,
    values: &HashMap<String, String>,
) -> Result<ActionOutcome> {
    let title = form_required(values, "title")?.to_string();
    let all_day = form_flag(values, "all_day");
    let now = Local::now();

    // Natural-language start/end → concrete UTC instants (the same resolver the
    // query/filter layer uses), so "tomorrow 14:00" or "next mon 9am" just work.
    let start_raw = form_required(values, "start")?;
    let start = resolve_datetime(start_raw, now)
        .ok_or_else(|| other_err(format!("could not understand start time: '{start_raw}'")))?;
    let end_raw = form_required(values, "end")?;
    let end = resolve_datetime(end_raw, now)
        .ok_or_else(|| other_err(format!("could not understand end time: '{end_raw}'")))?;
    if !all_day && end <= start {
        return Err(other_err("end must be after start"));
    }

    // Map the "Show as" label the user picked back to its `ShowAs` token.
    let show_as_label = form_opt(values, "show_as").unwrap_or_else(|| "Busy".to_string());
    let show_as = SHOW_AS_CHOICES
        .iter()
        .find(|(l, _)| *l == show_as_label)
        .map(|(_, s)| *s)
        .unwrap_or(ShowAs::Busy);
    let location = form_opt(values, "location");
    let body = form_opt(values, "body");

    // Resolve the target calendar. With a picker present the form carries the
    // chosen label; without one (a single writable calendar) fall back to it.
    // No writable calendar at all → nothing to create against.
    let target = match form_opt(values, "calendar") {
        Some(label) => calendars
            .iter()
            .find(|c| c.label == label)
            .ok_or_else(|| other_err(format!("unknown calendar: '{label}'")))?,
        None => calendars
            .first()
            .ok_or_else(|| other_err("no writable calendar available to create the event in"))?,
    };

    // Find the backend that owns the target calendar.
    let backend = backends
        .iter()
        .find(|b| b.connection_id() == target.conn_id)
        .ok_or_else(|| other_err(format!("no connection for calendar '{}'", target.label)))?;

    let draft = EventDraft {
        title: title.clone(),
        start,
        end,
        all_day,
        location: location.clone(),
        body,
        show_as,
    };

    backend
        .create_event(Some(&target.cal_id), &draft)
        .await
        .map_err(|e| other_err(format!("could not create event: {e}")))?;

    // Re-fetch just this connection's slice so the new event lands in the cache
    // and the frontend reloads to show it — the same per-connection path the
    // ready-watch uses, so siblings' cached events are untouched.
    let range = window.range(Utc::now());
    if let Ok(events) = fetch_one(backend.as_ref(), &range).await {
        if store_connection(cache, &target.conn_id, &events).await {
            let _ = inv_tx.send(Invalidation::All);
        }
    }

    let when = if all_day {
        start
            .with_timezone(&Local)
            .format("%Y-%m-%d (all day)")
            .to_string()
    } else {
        format!(
            "{} – {}",
            start.with_timezone(&Local).format("%Y-%m-%d %H:%M"),
            end.with_timezone(&Local).format("%H:%M"),
        )
    };
    let mut msg = format!(
        "Created “{title}” in {}: {when} [{show_as_label}]",
        target.label
    );
    if let Some(loc) = location {
        msg.push_str(&format!(" @ {loc}"));
    }
    Ok(ActionOutcome::Done { message: Some(msg) })
}

/// A [`CalEvent`] after the adapter has stamped it with a globally-unique id
/// and its origin account label — the unit rows and detail nodes are built
/// from.
#[derive(Clone, Debug)]
struct MergedEvent {
    /// `calendar:<connection_id>:<uid>` — unique across all connections.
    global_id: String,
    /// Connection label surfaced as the "Account" column.
    account: String,
    event: CalEvent,
}

/// Shared cache of the last successful merge, keyed by global id. Kept warm by
/// both `list()` and the poll loop so `get_by_id` (detail navigation) resolves
/// without another network round-trip.
type Cache = Arc<RwLock<HashMap<String, MergedEvent>>>;

/// Aggregated loading state across all connections, shared by the per-backend
/// ready-watch tasks (`spawn_ready_watch`). The banner is Busy while ANY
/// connection is still loading and Ready only when the last one settles — so a
/// fast connection finishing can't drop the banner while a slower sibling is
/// still mid-login (the single latest-wins `status_tx` would otherwise let the
/// last writer win regardless of the others).
#[derive(Default)]
struct LoadState {
    /// connection id → its latest reported fraction, present only while that
    /// connection is loading (removed on its terminal `done`).
    loading: HashMap<String, Option<f32>>,
    /// Unix ms when the current busy episode began (set on the empty→non-empty
    /// transition), so the banner's elapsed time counts from the first
    /// connection that started rather than resetting on every tick.
    started_at: u64,
}

impl LoadState {
    /// Fold one backend's ready fire into the state and return the banner to
    /// publish. `done` retires the connection; any other fire (re)marks it
    /// loading with its latest fraction. Busy carries the least-advanced
    /// still-loading connection's fraction (indeterminate if any connection
    /// hasn't reported one yet), Ready once none remain.
    fn update(&mut self, conn_id: &str, progress: &LoadProgress, now_ms: u64) -> AdapterStatus {
        let was_empty = self.loading.is_empty();
        if progress.done {
            self.loading.remove(conn_id);
        } else {
            self.loading.insert(conn_id.to_string(), progress.fraction);
        }
        if self.loading.is_empty() {
            return AdapterStatus::Ready;
        }
        if was_empty {
            self.started_at = now_ms;
        }
        // Represent the cohort by its least-advanced member: indeterminate if
        // any loading connection hasn't reported a fraction, else the minimum.
        let fraction = if self.loading.values().any(Option::is_none) {
            None
        } else {
            self.loading
                .values()
                .filter_map(|f| *f)
                .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        };
        AdapterStatus::Busy {
            label: "Loading calendar".to_string(),
            started_at_unix_ms: self.started_at,
            // Open-ended (interactive login) — no countdown; the percentage
            // (when known) is the cue.
            timeout_secs: 0,
            progress: fraction,
        }
    }
}

pub struct CalendarAdapter {
    instance_id: String,
    backends: Arc<Vec<Box<dyn CalendarBackend>>>,
    window: Window,
    poll_interval: Duration,
    cache: Cache,
    inv_tx: broadcast::Sender<Invalidation>,
    /// Fires a [`Reminder`] ahead of each timed event's start. The frontend
    /// decides whether to act on it; the adapter only schedules the fire.
    rem_tx: broadcast::Sender<Reminder>,
    /// Lead times (minutes) at which a reminder fires ahead of an event's
    /// start — one fire per entry, so `[15, 5]` reminds twice. Sorted
    /// descending, deduped, non-negative.
    reminder_leads: Vec<i64>,
    /// Latches `true` the first time the adapter is asked for data (a `list()`
    /// or detail resolve). The poll loop stays dormant until then — so with the
    /// view's `manual_connect: true` nothing (not even the browser) starts
    /// before the user's first reload; in the default eager mode the initial
    /// view load arms it right away.
    armed_tx: watch::Sender<bool>,
    /// Named saved queries, one `.yaml` file per name under the instance's
    /// data dir — the same story as the trackings tab, so a query the user
    /// saves survives a restart and shows up in the query menu.
    saved_queries: FsQueryStore,
    /// The most recent query `list()` was called with — the single source of
    /// truth for the load window. `list()`, the background poll, and the
    /// post-login ready-watch all derive their [`TimeRange`] from it via
    /// [`effective_range`], so all three fetch exactly the span the query needs
    /// (down to nothing beyond it, up to as far ahead as it reaches). `None`
    /// until the first `list()` → the configured window is the universe.
    current_query: Arc<RwLock<Option<String>>>,
    /// Publishes the adapter's live loading state to the frontend
    /// ([`ContentAdapter::subscribe_status`]). The ready-watch drives it to
    /// [`AdapterStatus::Busy`] (carrying the backend's progress fraction) while
    /// an out-of-band load runs, back to [`AdapterStatus::Ready`] when it
    /// settles. The periodic poll never touches it, so the "still loading"
    /// banner is exclusive to genuine (initial / post-login) loads, not the
    /// 60-second refresh.
    status_tx: watch::Sender<AdapterStatus>,
    /// Bumped by `list()` whenever the query text changes. A background fetch
    /// (poll or ready-watch) snapshots it before fetching and, if it moved by
    /// the time the fetch returns, drops the result without storing or
    /// invalidating — so a query change aborts the value of any in-flight
    /// request: stale rows for the old window never overwrite the new one.
    query_gen: Arc<AtomicU64>,
    /// Abort handles for the per-connection background fetches kicked by
    /// [`spawn_fetch`] (the list-triggered active load). A hard
    /// [`refresh`](ContentAdapter::refresh) (the `r` key) aborts every one so an
    /// in-flight fetch — including a backend still blocked on an interactive
    /// sign-in — is cancelled immediately, not merely superseded. Replaced
    /// wholesale on each `spawn_fetch`, so it never grows past the connection
    /// count.
    fetch_tasks: Arc<Mutex<Vec<AbortHandle>>>,
    /// Latches `true` the first time [`take_prompt_requests`] merges the
    /// backends' prompt streams, so a second call yields `None` rather than a
    /// dead channel with no forwarders (the receivers were already moved out of
    /// the backends). Mirrors the take-once contract of the content trait.
    prompts_taken: Arc<AtomicBool>,
    /// The pooled write-target calendars across every connection, discovered
    /// lazily by [`ensure_calendars`](CalendarAdapter::ensure_calendars) the
    /// first time the root is addressed for a form (`get_by_id("root")`). A
    /// **sync** lock so the sync `actions_for_type` / `CalendarRoot::actions`
    /// can read a snapshot without awaiting; enumeration itself runs on the
    /// async form-open path.
    calendars: Arc<std::sync::RwLock<Vec<GlobalCalendar>>>,
    /// Latches `true` once `ensure_calendars` has run, so the (possibly empty)
    /// discovered list is cached rather than re-enumerated on every form open.
    calendars_loaded: Arc<AtomicBool>,
}

impl CalendarAdapter {
    /// Build one backend per configured connection through the compile-time
    /// registry, then assemble the adapter and start its poll loop.
    pub(crate) fn from_config(
        instance_id: String,
        cfg: CalendarConfig,
        ctx: &not_yet_done_content::HostContext,
    ) -> std::result::Result<Self, String> {
        let factories = backend_factories();
        let by_type: HashMap<&str, &Box<dyn not_yet_done_calendar_core::CalendarBackendFactory>> =
            factories.iter().map(|f| (f.backend_type(), f)).collect();

        let mut backends: Vec<Box<dyn CalendarBackend>> = Vec::new();
        for conn in cfg.connections {
            let factory = by_type.get(conn.backend.as_str()).ok_or_else(|| {
                format!(
                    "connection '{}': unknown backend '{}' (not compiled in — enable its cargo feature)",
                    conn.id, conn.backend
                )
            })?;
            // Re-serialise the opaque sub-tree to YAML text; the backend parses
            // its own shape. The adapter never interprets it.
            let backend_config = serde_yaml::to_string(&conn.config).map_err(|e| {
                format!(
                    "connection '{}': cannot serialise backend config: {e}",
                    conn.id
                )
            })?;
            let backend = factory
                .create(&conn.id, &backend_config, ctx)
                .map_err(|e| format!("connection '{}': {e}", conn.id))?;
            backends.push(backend);
        }

        if backends.is_empty() {
            return Err("calendar adapter needs at least one connection".to_string());
        }

        let window = Window {
            past_days: cfg.window_past_days.unwrap_or(DEFAULT_WINDOW_PAST_DAYS),
            future_days: cfg.window_future_days.unwrap_or(DEFAULT_WINDOW_FUTURE_DAYS),
        };
        let poll_interval = Duration::from_secs(
            cfg.poll_interval_secs
                .unwrap_or(DEFAULT_POLL_INTERVAL_SECS)
                .max(1),
        );
        let (inv_tx, _) = broadcast::channel(16);
        let (rem_tx, _) = broadcast::channel(64);
        let (armed_tx, _) = watch::channel(false);
        let (status_tx, _) = watch::channel(AdapterStatus::Ready);
        // Normalise the configured leads: drop negatives, dedup, sort
        // descending so the scheduler checks the earliest heads-up first.
        let mut reminder_leads: Vec<i64> = cfg
            .reminder_lead_minutes
            .unwrap_or_else(default_reminder_leads)
            .into_iter()
            .filter(|&m| m >= 0)
            .collect();
        reminder_leads.sort_unstable_by(|a, b| b.cmp(a));
        reminder_leads.dedup();

        let queries_root = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("not_yet_done")
            .join("calendar")
            .join(&instance_id)
            .join("queries");

        let adapter = Self {
            instance_id,
            backends: Arc::new(backends),
            window,
            poll_interval,
            cache: Arc::new(RwLock::new(HashMap::new())),
            inv_tx,
            rem_tx,
            reminder_leads,
            armed_tx,
            saved_queries: FsQueryStore::new(queries_root, ".yaml"),
            current_query: Arc::new(RwLock::new(None)),
            status_tx,
            query_gen: Arc::new(AtomicU64::new(0)),
            fetch_tasks: Arc::new(Mutex::new(Vec::new())),
            prompts_taken: Arc::new(AtomicBool::new(false)),
            calendars: Arc::new(std::sync::RwLock::new(Vec::new())),
            calendars_loaded: Arc::new(AtomicBool::new(false)),
        };
        adapter.spawn_poll();
        adapter.spawn_reminders();
        adapter.spawn_ready_watch();
        Ok(adapter)
    }

    /// The pooled write-target calendars across every connection, discovering
    /// them once and caching the result. Each backend's
    /// [`CalendarBackend::list_calendars`] is queried (a backend that errors or
    /// can't enumerate is simply skipped — it contributes no targets); only
    /// `writable` calendars are offered. Labels are the calendar name, prefixed
    /// with the connection label when more than one connection is configured,
    /// and suffixed `(2)`, `(3)`, … on a collision so the flattened picker stays
    /// unambiguous.
    ///
    /// Runs on the async form-open path (`get_by_id("root")`), never on the fast
    /// initial listing, so a discovery round-trip never delays the first paint.
    async fn ensure_calendars(&self) -> Vec<GlobalCalendar> {
        if self.calendars_loaded.load(Ordering::SeqCst) {
            return self.calendars.read().unwrap().clone();
        }
        let multi = self.backends.len() > 1;
        let mut out: Vec<GlobalCalendar> = Vec::new();
        for backend in self.backends.iter() {
            let Ok(cals) = backend.list_calendars().await else {
                continue; // a backend that can't enumerate offers no targets
            };
            for cal in cals {
                if !cal.writable {
                    continue;
                }
                let base = if multi {
                    format!("{} — {}", backend.connection_label(), cal.name)
                } else {
                    cal.name.clone()
                };
                let mut label = base.clone();
                let mut n = 2;
                while out.iter().any(|g| g.label == label) {
                    label = format!("{base} ({n})");
                    n += 1;
                }
                out.push(GlobalCalendar {
                    conn_id: backend.connection_id().to_string(),
                    cal_id: cal.id,
                    label,
                });
            }
        }
        *self.calendars.write().unwrap() = out.clone();
        self.calendars_loaded.store(true, Ordering::SeqCst);
        out
    }

    /// Assemble the root node from the adapter's shared state plus the given
    /// pooled calendars — the write-target list the root's `create` form and
    /// `execute` need. `root()` passes the cached snapshot (no discovery);
    /// `get_by_id("root")` passes the freshly-ensured list.
    fn build_root(&self, calendars: Vec<GlobalCalendar>) -> CalendarRoot {
        CalendarRoot {
            backends: Arc::clone(&self.backends),
            window: self.window,
            cache: Arc::clone(&self.cache),
            calendars,
            inv_tx: self.inv_tx.clone(),
        }
    }

    /// Kick off the background poll loop (no-op outside a Tokio runtime, e.g.
    /// in a sync unit test). Detached: it lives for the adapter's lifetime.
    fn spawn_poll(&self) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let backends = Arc::clone(&self.backends);
        let cache = Arc::clone(&self.cache);
        let window = self.window;
        let current_query = Arc::clone(&self.current_query);
        let interval = self.poll_interval;
        let inv_tx = self.inv_tx.clone();
        let query_gen = Arc::clone(&self.query_gen);
        let mut armed_rx = self.armed_tx.subscribe();

        handle.spawn(async move {
            // Stay dormant until the adapter is first asked for data (see
            // `armed_tx`): until then no fetch — and thus no browser — starts.
            while !*armed_rx.borrow_and_update() {
                if armed_rx.changed().await.is_err() {
                    return; // adapter dropped
                }
            }

            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // `interval()` fires immediately; the arming `list()` just fetched,
            // so consume that first tick and let polling land one interval out
            // rather than duplicating the load. The first real tick sets the
            // baseline signature (no invalidation); later changes emit `All`.
            ticker.tick().await;
            let mut last_sig: Option<u64> = None;

            loop {
                ticker.tick().await;
                let gen_start = query_gen.load(Ordering::SeqCst);
                let range = effective_range(&*current_query.read().await, &window, Utc::now());
                // Bound the poll fetch at half the poll window: a fetch that
                // can't answer within that budget is abandoned and retried next
                // tick, so a wedged backend never stalls the loop for a whole
                // interval. The initial interactive login is *not* subject to
                // this — it runs on `list()`/the ready-watch, not the poll.
                let merged =
                    match tokio::time::timeout(interval / 2, fetch_all(&backends, &range)).await {
                        Ok(Ok(merged)) => merged,
                        // Transient failure or timeout: keep the last good
                        // signature/cache so a blip doesn't spuriously look like
                        // "everything changed".
                        Ok(Err(_)) | Err(_) => continue,
                    };
                // Query changed while this fetch was in flight → its window is
                // stale. Drop it; the next tick already reads the new query.
                if query_gen.load(Ordering::SeqCst) != gen_start {
                    continue;
                }
                // Was the cache empty *before* this store? Used below to detect
                // "data first arrived" (see the emit condition).
                let was_empty = cache.read().await.is_empty();
                if !store_cache(&cache, &merged).await {
                    // Empty merge over a non-empty cache → transient glitch, kept
                    // last-good. Don't touch the signature or notify anyone.
                    continue;
                }
                let sig = signature(&merged);

                // Emit an invalidation when the set the user sees actually
                // changed — OR when data first arrives over a previously empty
                // cache. The latter auto-refreshes the view after an initial
                // load that came back empty because the browser backend was
                // still signing in (so the user need not press `r` again once
                // the background poll finally gets the events).
                let changed = last_sig.map(|prev| prev != sig).unwrap_or(false);
                if changed || (was_empty && !merged.is_empty()) {
                    let _ = inv_tx.send(Invalidation::All);
                }
                last_sig = Some(sig);
            }
        });
    }

    /// Watch every backend's event-driven "data became ready" signal and
    /// refresh the view the instant it fires — the push counterpart to the
    /// periodic poll.
    ///
    /// This is what makes freshness after an asynchronous browser login correct:
    /// the office365-web backend signals [`LoadProgress`] as its interactive
    /// sign-in runs and the calendar surface pages in (see
    /// [`CalendarBackend::subscribe_ready`]). Every fire drives the frontend
    /// banner: [`AdapterStatus::Busy`] carrying the fraction while `!done`
    /// (`None` → indeterminate cue, `Some` → percentage), back to
    /// [`AdapterStatus::Ready`] on the terminal fire.
    ///
    /// Only *load-boundary* fires trigger a fetch: the terminal `done`, and a
    /// fraction-less `None` nudge (login just completed / surface loading). On
    /// those we fetch once and, if the merge is good (non-empty over a previously
    /// empty cache, or otherwise storable), push [`Invalidation::All`] so the
    /// generic frontend watcher reloads — no second `r`, no waiting for the poll.
    /// Intermediate `Some(fraction)` ticks move the banner only and never
    /// re-fetch: the fetch they'd trigger *is* the paging load emitting them, so
    /// re-fetching would feed back on itself. The poll's job narrows back to
    /// catching *external* changes on its interval — and it never touches the
    /// status, so the banner is exclusive to genuine loads.
    ///
    /// One detached task per backend that offers a signal; backends that resolve
    /// synchronously (`subscribe_ready` → `None`) contribute none. No-op outside
    /// a Tokio runtime (e.g. a sync unit test).
    fn spawn_ready_watch(&self) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        // Shared across every backend's watcher so the banner reflects *all*
        // connections at once: Busy while ANY connection is still loading, Ready
        // only when the last one settles. A single latest-wins status channel
        // written by independent per-backend tasks would otherwise let a fast
        // connection's terminal `done`→`Ready` clobber a slower sibling's `Busy`,
        // flipping the banner to "ready" (and, over a still-empty cache, to the
        // "press r to reload" placeholder) mid-login.
        let load_state = Arc::new(std::sync::Mutex::new(LoadState::default()));
        for (idx, backend) in self.backends.iter().enumerate() {
            let Some(mut ready_rx) = backend.subscribe_ready() else {
                continue;
            };
            let conn_id = backend.connection_id().to_string();
            let backends = Arc::clone(&self.backends);
            let cache = Arc::clone(&self.cache);
            let window = self.window;
            let current_query = Arc::clone(&self.current_query);
            let inv_tx = self.inv_tx.clone();
            let status_tx = self.status_tx.clone();
            let query_gen = Arc::clone(&self.query_gen);
            let load_state = Arc::clone(&load_state);
            handle.spawn(async move {
                loop {
                    match ready_rx.recv().await {
                        Ok(progress) => {
                            // Fold this backend's fire into the shared load state
                            // and publish the AGGREGATE banner: Busy carrying the
                            // least-advanced still-loading connection's fraction
                            // while any connection loads, Ready only once every
                            // connection has settled. This is what keeps the banner
                            // up while a slow sibling is mid-login even after a fast
                            // one has finished.
                            let status = {
                                let mut st = load_state.lock().unwrap();
                                st.update(&conn_id, &progress, now_unix_ms())
                            };
                            let _ = status_tx.send(status);

                            // Decide whether to re-fetch. A *fraction-less* fire
                            // (`done`, or a `None` "surface loading / login done"
                            // nudge) is a load boundary: drive one fetch so
                            // freshly-available data lands. A fire carrying a
                            // fraction is a pure progress tick from the in-flight
                            // paging load — move the banner only, and DON'T
                            // re-fetch: that fetch would drive the very load that
                            // emits the ticks, feeding back on itself.
                            if progress.fraction.is_some() && !progress.done {
                                continue;
                            }

                            // Boundary fetch, SPAWNED — never awaited inline. The
                            // refetch queues behind the still-running paging op on
                            // the (serialised) backend, so awaiting it here would
                            // block this loop for the whole load. The interim
                            // `Some(fraction)` progress ticks would then pile up in
                            // the broadcast buffer and — because `status_tx` is a
                            // watch channel (latest-wins) — be coalesced away before
                            // the frontend ever rendered a percentage. Spawning lets
                            // the loop keep draining ticks and updating the banner
                            // live; the fetch reconciles the cache on its own.
                            //
                            // Crucially the boundary fetch hits only THIS backend
                            // (`fetch_one`), not the whole fleet: fanning out over
                            // all connections would park this fetch behind a slower
                            // sibling whose `list_events` is still queued behind its
                            // own interactive login — so a finished connection's
                            // events would wait for the slowest one. Fetching just
                            // the connection that became ready lands its events the
                            // instant it's done, and storing only its slice
                            // (`store_connection`) leaves the siblings' cached
                            // events untouched.
                            let backends = Arc::clone(&backends);
                            let cache = Arc::clone(&cache);
                            let current_query = Arc::clone(&current_query);
                            let inv_tx = inv_tx.clone();
                            let query_gen = Arc::clone(&query_gen);
                            let conn_id = conn_id.clone();
                            tokio::spawn(async move {
                                let gen_start = query_gen.load(Ordering::SeqCst);
                                let range = effective_range(
                                    &*current_query.read().await,
                                    &window,
                                    Utc::now(),
                                );
                                let Ok(events) = fetch_one(backends[idx].as_ref(), &range).await
                                else {
                                    return;
                                };
                                // Query changed mid-fetch → the window this result
                                // covers is stale; drop it (same rule as the poll).
                                if query_gen.load(Ordering::SeqCst) != gen_start {
                                    return;
                                }
                                // Per-connection store with the same empty-clobber
                                // guard: a login blip that yields an empty fetch for
                                // this connection must not wipe its good cached
                                // slice — nor any sibling's.
                                if store_connection(&cache, &conn_id, &events).await {
                                    let _ = inv_tx.send(Invalidation::All);
                                }
                            });
                        }
                        // Missed a fire (slow consumer): the next fetch still
                        // reconciles, so just carry on.
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        // Backend gone → its sender dropped → stop this watcher.
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }
    }

    /// Kick off the reminder scheduler (no-op outside a Tokio runtime). Shares
    /// the poll loop's arming latch and its warm cache. Instead of polling on a
    /// fixed cadence, it computes the **next** moment any timed event enters a
    /// lead window (`start - lead`), sleeps precisely until then, and fires one
    /// [`Reminder`] per event/lead as it comes due — deduping so each fires once.
    ///
    /// The event set is not static: a poll can add or delete events and a create
    /// action inserts one, each of which may change what the next reminder is.
    /// So the schedule is recomputed from scratch on every cache change: the
    /// scheduler `select!`s its sleep against the adapter's invalidation stream
    /// (`inv_tx`, the same signal that drives the frontend reload), and any
    /// change wakes it to re-derive the next deadline against the fresh set.
    ///
    /// A [`REMINDER_MAX_SLEEP`] cap bounds each individual sleep. A precise
    /// `sleep` runs on the *monotonic* clock, which does not advance across a
    /// laptop suspend and is not corrected for wall-clock jumps — and neither of
    /// those emits an invalidation. The cap turns the long idle wait into short
    /// hops, so after the machine wakes (or the clock steps) the next tick
    /// re-reads the wall clock and catches any now-due reminder within the cap,
    /// at the cost of a cheap in-memory rescan. Firing is cheap when nobody
    /// subscribes (the broadcast send is a no-op with no receivers), so the
    /// frontend's opt-in gate need not reach back into the adapter.
    fn spawn_reminders(&self) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            rem_trace(format_args!(
                "spawn_reminders: NO tokio runtime → scheduler NOT started"
            ));
            return;
        };
        let cache = Arc::clone(&self.cache);
        let rem_tx = self.rem_tx.clone();
        let leads = self.reminder_leads.clone();
        let mut armed_rx = self.armed_tx.subscribe();
        // Wake the scheduler whenever the cached event set changes, so the next
        // deadline always reflects freshly added / removed events.
        let mut inv_rx = self.inv_tx.subscribe();

        if leads.is_empty() {
            rem_trace(format_args!(
                "spawn_reminders: leads empty → scheduler NOT started"
            ));
            return; // no lead times configured → the scheduler has nothing to do
        }
        rem_trace(format_args!(
            "spawn_reminders: started, leads={leads:?}, waiting to be armed"
        ));

        handle.spawn(async move {
            // Same dormancy contract as the poll loop: nothing until first use.
            while !*armed_rx.borrow_and_update() {
                if armed_rx.changed().await.is_err() {
                    rem_trace(format_args!("scheduler: adapter dropped before arming"));
                    return; // adapter dropped
                }
            }
            rem_trace(format_args!(
                "scheduler: ARMED, entering deadline loop (max sleep {}s)",
                REMINDER_MAX_SLEEP.as_secs()
            ));

            // Dedup key is (event id, lead minutes): each event reminds once
            // per configured lead, so `[15, 5]` fires two distinct reminders.
            let mut fired: HashSet<(String, i64)> = HashSet::new();
            loop {
                let now = Utc::now();

                // One scan of the warm cache: everything already due to fire now,
                // plus the earliest still-pending fire moment to sleep until next.
                let (due, next_fire) = {
                    let guard = cache.read().await;
                    scan_reminders(&guard, &leads, now, &mut fired)
                };

                for r in due {
                    rem_trace(format_args!(
                        "FIRE: title={:?} when={} lead={}min",
                        r.title, r.when, r.lead_minutes
                    ));
                    let _ = rem_tx.send(r);
                }

                // Sleep until the next deadline (capped), recomputing the delay
                // against a fresh `now` so the scan's own duration is absorbed.
                let sleep_for = match next_fire {
                    Some(t) => (t - Utc::now())
                        .to_std()
                        .unwrap_or(Duration::ZERO)
                        .min(REMINDER_MAX_SLEEP),
                    None => REMINDER_MAX_SLEEP,
                };
                rem_trace(format_args!(
                    "scan: next_fire={:?}, sleeping {}s",
                    next_fire.map(|t| t.with_timezone(&Local).to_rfc3339()),
                    sleep_for.as_secs()
                ));

                // Wake on whichever comes first: the deadline, or a cache change.
                tokio::select! {
                    _ = tokio::time::sleep(sleep_for) => {}
                    r = inv_rx.recv() => match r {
                        // Changed (or we lagged behind changes): loop and recompute
                        // the schedule against the fresh event set.
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                        // Sender gone → the adapter dropped → stop the scheduler.
                        Err(broadcast::error::RecvError::Closed) => {
                            rem_trace(format_args!("scheduler: invalidation channel closed → stopping"));
                            break;
                        }
                    },
                }
            }
        });
    }
}

/// The longest the reminder scheduler ever sleeps in one hop. It normally sleeps
/// precisely until the next event's lead moment; this cap only bounds how long a
/// wall-clock jump or a resume-from-suspend — neither of which advances the
/// monotonic timer nor emits an invalidation — can delay reconciliation. After
/// the machine wakes, the next now-due reminder is caught within this window by a
/// cheap in-memory rescan.
const REMINDER_MAX_SLEEP: Duration = Duration::from_secs(60);

/// The pure core of one scheduler pass: given the current events, the configured
/// `leads` and the instant `now`, return the reminders due to fire **now** and
/// the earliest still-pending fire moment (the next deadline to sleep until),
/// while updating the `fired` dedup set (keyed by event id + lead). Only timed,
/// future events are considered — all-day and already-started events never fire.
/// Firing a due reminder inserts its key into `fired`; the set is then pruned of
/// keys whose event has left `events`, so a re-appearing event may remind again.
fn scan_reminders(
    events: &HashMap<String, MergedEvent>,
    leads: &[i64],
    now: DateTime<Utc>,
    fired: &mut HashSet<(String, i64)>,
) -> (Vec<Reminder>, Option<DateTime<Utc>>) {
    let mut due: Vec<Reminder> = Vec::new();
    let mut next_fire: Option<DateTime<Utc>> = None;
    for m in events.values() {
        // All-day events have no meaningful time-of-day lead; past events are done.
        if m.event.all_day || m.event.start <= now {
            continue;
        }
        for &lead in leads {
            let key = (m.global_id.clone(), lead);
            if fired.contains(&key) {
                continue;
            }
            let fire_at = m.event.start - chrono::Duration::minutes(lead);
            if fire_at <= now {
                // Inside the lead window already → fire now.
                fired.insert(key);
                due.push(to_reminder(m, lead));
            } else {
                // Still ahead → a candidate for the next deadline (keep the min).
                next_fire = Some(match next_fire {
                    Some(t) if t <= fire_at => t,
                    _ => fire_at,
                });
            }
        }
    }
    // Bound the dedup set: drop keys whose event has aged out of the window.
    fired.retain(|(id, _)| events.contains_key(id));
    (due, next_fire)
}

/// Project a cached event into a [`Reminder`] for a given lead time: the
/// event's local-time start, the configured `lead_minutes`, and a compact
/// detail line (account, and the location when the backend supplied one).
/// `lead_minutes` is the *configured* lead (so a `[15, 5]` config yields
/// reminders that say "15" and "5"), not the wall-clock remainder at fire time.
fn to_reminder(m: &MergedEvent, lead_minutes: i64) -> Reminder {
    let mut parts = vec![m.account.clone()];
    if let Some(loc) = m
        .event
        .location
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
    {
        parts.push(loc.to_string());
    }
    Reminder {
        id: m.global_id.clone(),
        title: event_label(&m.event),
        detail: Some(parts.join(" · ")),
        when: m.event.start.with_timezone(&Local).to_rfc3339(),
        until: Some(m.event.end.with_timezone(&Local).to_rfc3339()),
        lead_minutes,
    }
}

#[async_trait]
impl ContentAdapter for CalendarAdapter {
    fn adapter_type(&self) -> &str {
        "calendar"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            // `create` is a root Form action (New event). Prototype/dry-run
            // for now — see `execute_create`.
            supports_create: true,
            ..AdapterCapabilities::default()
        }
    }

    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        match node_type.type_id.as_str() {
            // A snapshot of whatever calendars have been discovered so far (the
            // hint bar only needs to know the action exists). The authoritative
            // form spec — with the picker populated — is built at open time from
            // the root that `get_by_id` returns, which has ensured the list.
            "calendar:root" => vec![create_event_action(&self.calendars.read().unwrap())],
            _ => Vec::new(),
        }
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        // Fast initial listing: no calendar discovery (that round-trip only
        // happens when the root is addressed for a form via `get_by_id`).
        let calendars = self.calendars.read().unwrap().clone();
        Ok(Box::new(self.build_root(calendars)))
    }

    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        let _ = self.armed_tx.send(true);
        // The root is addressable by its own id so root-scoped actions (the
        // `create` form) can round-trip through `get_by_id` for `form_prep`
        // and `execute`, exactly like any event row does. This is where the
        // write-target calendars are discovered (and cached), so the `create`
        // form the caller then reads carries the fully-populated picker.
        if id == CalendarRoot::ROOT_ID {
            let calendars = self.ensure_calendars().await;
            return Ok(Box::new(self.build_root(calendars)));
        }
        resolve_event(&self.backends, self.window, &self.cache, id).await
    }

    /// The single source of truth about what lives under a calendar node: the
    /// root lists `calendar:event` rows merged across every connection; an
    /// event is a leaf. The `list` callback fetches lazily through the same
    /// [`list_events_root`] the root's legacy `list` delegates to, sourcing all
    /// its state from the adapter's own fields (`root()` builds the root from
    /// exactly these), so `node` is never downcast.
    fn childs<'a>(&'a self, node: &'a dyn Node) -> Vec<Child<'a>> {
        match node.node_type().type_id.as_str() {
            "calendar:root" => vec![Child {
                node_type: event_node_type(),
                columns: event_columns(),
                list: Box::new(move |params| {
                    Box::pin(async move {
                        list_events_root(
                            &self.backends,
                            &self.window,
                            &self.cache,
                            &self.armed_tx,
                            &self.current_query,
                            &self.query_gen,
                            &self.inv_tx,
                            &self.fetch_tasks,
                            params,
                        )
                        .await
                    })
                }),
            }],
            _ => Vec::new(),
        }
    }

    fn subscribe_invalidations(&self) -> broadcast::Receiver<Invalidation> {
        self.inv_tx.subscribe()
    }

    /// Hard refresh (the `r` action): abort every in-flight per-connection fetch
    /// outright and drop the cache, so the ensuing `list()` runs cold and
    /// re-fetches every connection from scratch. Bumping `query_gen` first means
    /// any fetch task that slips past its `abort()` still discards its result
    /// instead of storing a stale slice. The poll loop and ready-watch daemons
    /// are deliberately left running — they are not one-shot loads; their own
    /// in-flight results fall away via the same `query_gen` guard. Re-arming the
    /// latch keeps the background poll alive across the cache clear.
    async fn refresh(&self) -> Result<()> {
        self.query_gen.fetch_add(1, Ordering::SeqCst);
        for handle in self.fetch_tasks.lock().unwrap().drain(..) {
            handle.abort();
        }
        self.cache.write().await.clear();
        let _ = self.armed_tx.send(true);
        Ok(())
    }

    fn subscribe_status(&self) -> watch::Receiver<AdapterStatus> {
        self.status_tx.subscribe()
    }

    fn subscribe_reminders(&self) -> broadcast::Receiver<Reminder> {
        self.rem_tx.subscribe()
    }

    /// Merge every backend's mid-operation prompt stream into one channel for
    /// the frontend. Aggregation across connections is the adapter's job (as
    /// with events and reminders); a backend that never prompts contributes
    /// nothing, and if *no* backend does, we offer no stream at all so the
    /// frontend can tell the difference. Take-once: the receivers are moved out
    /// of the backends, so a repeat call returns `None`.
    fn take_prompt_requests(&self) -> Option<mpsc::Receiver<PromptRequest>> {
        if self.prompts_taken.swap(true, Ordering::SeqCst) {
            return None;
        }
        let sources: Vec<_> = self
            .backends
            .iter()
            .filter_map(|b| b.take_prompt_requests())
            .collect();
        if sources.is_empty() {
            return None;
        }
        let (tx, rx) = mpsc::channel::<PromptRequest>(8);
        for mut backend_rx in sources {
            let tx = tx.clone();
            tokio::spawn(async move {
                while let Some(req) = backend_rx.recv().await {
                    // Frontend gone → stop forwarding this backend's prompts.
                    if tx.send(req).await.is_err() {
                        break;
                    }
                }
            });
        }
        Some(rx)
    }

    fn saved_query_store(&self) -> Option<&dyn SavedQueryStore> {
        Some(&self.saved_queries)
    }
}

/// Root node: a flat list of `calendar:event` rows merged across connections.
///
/// Listing now flows through [`ContentAdapter::childs`] (the single source of
/// truth), which sources its state from the adapter's own fields — so the node
/// itself only needs what `get_child` reads (backends, window, cache). The
/// list-only handles (`armed_tx`, `current_query`, `query_gen`, `inv_tx`) that
/// used to live here are gone with the node's former `list`.
struct CalendarRoot {
    backends: Arc<Vec<Box<dyn CalendarBackend>>>,
    window: Window,
    cache: Cache,
    /// The pooled write-target calendars (see [`GlobalCalendar`]) — the option
    /// list for the `create` form's picker and the lookup `execute` resolves
    /// the chosen target against. Empty until `get_by_id("root")` ensured them.
    calendars: Vec<GlobalCalendar>,
    /// Broadcast handle so a successful `create` can invalidate the view and
    /// surface the new event without waiting for the next poll.
    inv_tx: broadcast::Sender<Invalidation>,
}

impl CalendarRoot {
    /// Stable id of the calendar root, addressable via
    /// [`ContentAdapter::get_by_id`] so root-scoped form actions round-trip.
    const ROOT_ID: &'static str = "root";
}

/// Kick a background fetch of every connection for `range`, storing each
/// connection's slice as it lands and invalidating so the frontend reloads
/// and serves the now-warm cache. Fire-and-forget — the caller never awaits
/// it — so a slow backend's interactive login can't block the view: a fast
/// connection's events appear the instant its slice lands, independent of
/// its siblings.
///
/// One detached task per backend (each `list_events` may block for the whole
/// duration of that connection's sign-in, so they must not be serialised).
/// No-op outside a Tokio runtime (e.g. a sync unit test). The `query_gen`
/// guard drops a result whose window a concurrent query change has
/// superseded; `store_connection` swallows an empty fetch (login still in
/// flight) without invalidating, so this never spins into a reload storm —
/// the ready-watch delivers the events once the sign-in actually completes.
fn spawn_fetch(
    backends: &Arc<Vec<Box<dyn CalendarBackend>>>,
    cache: &Cache,
    inv_tx: &broadcast::Sender<Invalidation>,
    query_gen: &Arc<AtomicU64>,
    fetch_tasks: &Arc<Mutex<Vec<AbortHandle>>>,
    range: TimeRange,
) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    // Track this round's per-connection tasks so a hard `refresh()` can abort
    // them outright. A new fetch round supersedes the previous one's handles
    // (any still-running task from before is either already aborted by the
    // refresh that triggered us, or about to drop its result via `query_gen`),
    // so replace rather than append — the vec stays bounded by `backends.len()`.
    let mut handles = Vec::with_capacity(backends.len());
    for idx in 0..backends.len() {
        let backends = Arc::clone(backends);
        let cache = Arc::clone(cache);
        let inv_tx = inv_tx.clone();
        let query_gen = Arc::clone(query_gen);
        let task = handle.spawn(async move {
            let gen_start = query_gen.load(Ordering::SeqCst);
            let Ok(events) = fetch_one(backends[idx].as_ref(), &range).await else {
                return;
            };
            if query_gen.load(Ordering::SeqCst) != gen_start {
                return;
            }
            let conn_id = backends[idx].connection_id().to_string();
            if store_connection(&cache, &conn_id, &events).await {
                let _ = inv_tx.send(Invalidation::All);
            }
        });
        handles.push(task.abort_handle());
    }
    *fetch_tasks.lock().unwrap() = handles;
}

/// List the `calendar:event` rows under the root: the single fetch/merge/sort
/// body shared by [`ContentAdapter::childs`]'s `list` callback and the legacy
/// [`CalendarRoot::list`]. Sources every bit of state from the passed-in
/// adapter handles (the root holds copies of exactly these), so both call
/// sites behave identically. See [`CalendarRoot::list`]'s former inline
/// comments for the whys — arm the poll latch, publish the query as the load
/// window's source of truth (bumping `query_gen` on a change to abort stale
/// in-flight fetches), serve from the warm cache while kicking a background
/// per-connection fetch when cold or the window changed, then narrow the
/// snapshot to the active query and sort.
async fn list_events_root(
    backends: &Arc<Vec<Box<dyn CalendarBackend>>>,
    window: &Window,
    cache: &Cache,
    armed_tx: &watch::Sender<bool>,
    current_query: &Arc<RwLock<Option<String>>>,
    query_gen: &Arc<AtomicU64>,
    inv_tx: &broadcast::Sender<Invalidation>,
    fetch_tasks: &Arc<Mutex<Vec<AbortHandle>>>,
    params: ListParams,
) -> Result<ListResult> {
    if params.node_type.type_id != "calendar:event" {
        return Err(ContentError::NotSupported(format!(
            "Unknown node type: {}",
            params.node_type.type_id
        )));
    }

    // First real data request → arm the background poll loop.
    let _ = armed_tx.send(true);

    // Publish this query as the single source of truth for the load window,
    // so the background poll and the ready-watch fetch the same span (see
    // `current_query`). A *changed* query bumps the generation, which aborts
    // the value of any background fetch still in flight for the previous
    // window (it drops its result instead of storing).
    let query_changed = {
        let mut guard = current_query.write().await;
        let changed = *guard != params.query;
        if changed {
            query_gen.fetch_add(1, Ordering::SeqCst);
        }
        *guard = params.query.clone();
        changed
    };
    let range = effective_range(&params.query, window, Utc::now());

    // Serve from the warm cache — never block on a fetch here. A synchronous
    // `fetch_all` would re-park us behind the slowest backend's still-running
    // interactive login (`list_events` → `session()` blocks until that
    // connection is signed in), so a connection whose events are already
    // cached would appear to wait for a slow sibling: exactly the "items land
    // late, all at once" symptom. Instead, when we have no data for this
    // window yet (cold cache) or the query window changed, kick a background
    // per-connection fetch — it stores each connection's slice as it lands
    // and invalidates, so the frontend reloads and this `list()` serves the
    // freshly warmed cache. Each connection thus surfaces independently, the
    // instant it's ready.
    let cold = cache.read().await.is_empty();
    if cold || query_changed {
        spawn_fetch(backends, cache, inv_tx, query_gen, fetch_tasks, range);
    }

    // Snapshot the cache (the full fetched window) and narrow to the active
    // query. The cache holds the full window so detail navigation
    // (`get_by_id`) still resolves an event the active query hides. The same
    // FilterExpr DSL the trackings tab runs through the DB is evaluated here
    // in memory (natural-language start/end + title search — see `query`).
    let mut merged: Vec<MergedEvent> = {
        let guard = cache.read().await;
        guard.values().cloned().collect()
    };
    merged.sort_by(|a, b| {
        a.event
            .start
            .cmp(&b.event.start)
            .then_with(|| a.global_id.cmp(&b.global_id))
    });
    let filtered = filter_events(merged, &params.query)?;

    let mut items: Vec<NodeSummary> = filtered.iter().map(event_summary).collect();
    let applied_sort = apply_sort(&mut items, &params.sort, &event_columns());

    Ok(ListResult {
        items,
        applied_sort,
        page: None,
        batch_download_available: false,
        downloaded: vec![],
    })
}

#[async_trait]
impl Node for CalendarRoot {
    fn id(&self) -> &str {
        Self::ROOT_ID
    }

    fn label(&self) -> &str {
        "Calendar"
    }

    fn node_type(&self) -> &NodeType {
        static ROOT_TYPE: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(|| NodeType {
            type_id: "calendar:root".into(),
            mime_type: "".into(),
            syntax: None,
            file_extension: "".into(),
            display_name: "Calendar Root".into(),
        });
        &ROOT_TYPE
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: Metadata = Metadata { fields: vec![] };
        &EMPTY
    }
    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("create", ActionInput::Form(values)) => {
                execute_create(
                    &self.backends,
                    &self.calendars,
                    &self.cache,
                    self.window,
                    &self.inv_tx,
                    &values,
                )
                .await
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "action `{other}` not supported on the calendar root"
            ))),
        }
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        resolve_event(&self.backends, self.window, &self.cache, id).await
    }
}

/// Resolve the [`TimeRange`] to fetch for a given query. The query is the sole
/// filter: when it bounds the `start`/`end` columns, those bounds *are* the
/// window (down to nothing beyond them); each side the query leaves open falls
/// back to the configured [`Window`]. No query — or one with no date bound (a
/// title-only search) — means the full configured window (the universe). A
/// malformed query fetches the universe too, so the same parse error still
/// surfaces from `filter_events` rather than being swallowed here.
fn effective_range(query: &Option<String>, window: &Window, now: DateTime<Utc>) -> TimeRange {
    let base = window.range(now);
    let Some(raw) = query.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return base;
    };
    let Ok(compiled) = CalendarQuery::parse(raw) else {
        return base;
    };
    let (lo, hi) = compiled.date_bounds();
    TimeRange::new(lo.unwrap_or(base.start), hi.unwrap_or(base.end))
}

/// Fan out `list_events` across every backend and merge. Partial success is OK
/// — a connection that errors is skipped as long as at least one succeeds;
/// only an all-connections failure surfaces as an error. Results are sorted by
/// start instant (then id) so ordering — and the change signature — is stable.
async fn fetch_all(
    backends: &[Box<dyn CalendarBackend>],
    range: &TimeRange,
) -> std::result::Result<Vec<MergedEvent>, String> {
    // Fetch every connection concurrently — a slow backend (e.g. office365-web
    // driving a browser login) must not hold up the others. Each future is
    // independent; results are merged and sorted once all resolve.
    let per_backend = futures::future::join_all(backends.iter().map(|backend| async move {
        let conn = backend.connection_id();
        (
            conn,
            backend.connection_label(),
            backend.list_events(range).await,
        )
    }))
    .await;

    let mut merged: Vec<MergedEvent> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut any_ok = false;

    for (conn, label, result) in per_backend {
        match result {
            Ok(events) => {
                any_ok = true;
                for event in events {
                    merged.push(MergedEvent {
                        global_id: format!("calendar:{conn}:{}", event.uid),
                        account: event.calendar.clone(),
                        event,
                    });
                }
            }
            Err(e) => errors.push(format!("{label}: {e}")),
        }
    }

    if !any_ok {
        return Err(if errors.is_empty() {
            "no calendar connections configured".to_string()
        } else {
            errors.join("; ")
        });
    }

    merged.sort_by(|a, b| {
        a.event
            .start
            .cmp(&b.event.start)
            .then_with(|| a.global_id.cmp(&b.global_id))
    });
    Ok(merged)
}

/// Fetch a single backend and namespace its events, without touching the
/// others. The per-connection counterpart to [`fetch_all`], used by the
/// ready-watch so a finished connection's events land the instant it's done
/// rather than waiting on a slower sibling. An error surfaces verbatim (the
/// caller drops the result); success returns this connection's events sorted by
/// start instant (then global id) for a stable order within its slice.
async fn fetch_one(
    backend: &dyn CalendarBackend,
    range: &TimeRange,
) -> std::result::Result<Vec<MergedEvent>, String> {
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

/// Apply the active query to a merged event set. `None`/empty query passes
/// everything through; a malformed query (bad YAML, unknown column) surfaces as
/// an error so the user sees why rather than an inexplicably empty view.
fn filter_events(merged: Vec<MergedEvent>, query: &Option<String>) -> Result<Vec<MergedEvent>> {
    let raw = match query.as_deref().map(str::trim) {
        Some(q) if !q.is_empty() => q,
        _ => return Ok(merged),
    };
    let compiled = CalendarQuery::parse(raw).map_err(other_err)?;
    Ok(merged
        .into_iter()
        .filter(|m| compiled.matches(&m.event, &m.account))
        .collect())
}

/// Replace the cache contents with a fresh merge. Returns `false` (keeping the
/// last-good cache) when an *empty* merge would clobber a previously non-empty
/// cache: an empty result over a multi-day window is almost always a transient
/// backend/login glitch (e.g. the office365-web backend serving a login page as
/// an empty-but-Ok fetch), not a real "no events" state. Protecting the cache
/// keeps reminders and `get_by_id` working across such a blip; the user-facing
/// `list()` still reflects whatever it fetched. See callers for how a `false`
/// is treated as "transient, kept last-good".
async fn store_cache(cache: &Cache, merged: &[MergedEvent]) -> bool {
    let mut guard = cache.write().await;
    if merged.is_empty() && !guard.is_empty() {
        return false;
    }
    guard.clear();
    for m in merged {
        guard.insert(m.global_id.clone(), m.clone());
    }
    true
}

/// Replace just one connection's slice of the cache — every entry whose global
/// id carries the `calendar:<conn_id>:` prefix — leaving other connections'
/// events untouched. The per-connection counterpart to [`store_cache`], so a
/// finished connection's events can land without waiting on (or disturbing) its
/// siblings.
///
/// An *empty* fetch never stores and returns `false` (so the caller does not
/// invalidate). This serves two ends at once: it keeps a connection's last-good
/// slice across a transient login glitch (empty-but-Ok fetch — same philosophy
/// as [`store_cache`]), and it stops a *cold* fetch that comes back empty
/// (sign-in still in flight) from spinning into an invalidate→reload→refetch
/// storm. The events land later regardless: the ready-watch re-fetches this
/// connection once its sign-in actually completes. A non-empty fetch replaces
/// the slice wholesale and returns `true`.
async fn store_connection(cache: &Cache, conn_id: &str, events: &[MergedEvent]) -> bool {
    if events.is_empty() {
        return false;
    }
    let prefix = format!("calendar:{conn_id}:");
    let mut guard = cache.write().await;
    guard.retain(|k, _| !k.starts_with(&prefix));
    for m in events {
        guard.insert(m.global_id.clone(), m.clone());
    }
    true
}

/// Resolve one event by global id: serve from cache, or cold-fetch (which also
/// refills the cache) and retry once. Unknown id → [`ContentError::NotFound`].
async fn resolve_event(
    backends: &[Box<dyn CalendarBackend>],
    window: Window,
    cache: &Cache,
    id: &str,
) -> Result<Box<dyn Node>> {
    if let Some(m) = cache.read().await.get(id).cloned() {
        return Ok(Box::new(EventNode::new(m)));
    }

    let range = window.range(Utc::now());
    let merged = fetch_all(backends, &range).await.map_err(other_err)?;
    store_cache(cache, &merged).await;

    merged
        .into_iter()
        .find(|m| m.global_id == id)
        .map(|m| Box::new(EventNode::new(m)) as Box<dyn Node>)
        .ok_or_else(|| ContentError::NotFound(id.to_string()))
}

/// Stable hash of the merged event set — the poll loop compares consecutive
/// signatures to decide whether anything the user can see actually changed.
/// `merged` is already sorted, so the hash is order-independent of fetch order.
fn signature(merged: &[MergedEvent]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for m in merged {
        m.global_id.hash(&mut hasher);
        m.event.start.timestamp().hash(&mut hasher);
        m.event.end.timestamp().hash(&mut hasher);
        m.event.title.hash(&mut hasher);
        m.event.show_as.as_str().hash(&mut hasher);
    }
    hasher.finish()
}

fn event_label(event: &CalEvent) -> String {
    let title = event.title.trim();
    if title.is_empty() {
        "(no title)".to_string()
    } else {
        title.to_string()
    }
}

/// Row projection. `start`/`end` are RFC3339 in **local** time so the engine's
/// `group_by` buckets on the day the user actually sees, and `SortKind::DateTime`
/// still orders them correctly.
fn event_summary(m: &MergedEvent) -> NodeSummary {
    let e = &m.event;
    let field = |key: &str, value: String, display: &str| MetadataField {
        key: key.into(),
        value,
        display_label: display.into(),
        editable: false,
        allowed_values: None,
    };

    let fields = vec![
        field("start", e.start.with_timezone(&Local).to_rfc3339(), "Start"),
        field("end", e.end.with_timezone(&Local).to_rfc3339(), "End"),
        field("account", m.account.clone(), "Account"),
        field("title", event_label(e), "Title"),
        field(
            "location",
            e.location.clone().unwrap_or_default(),
            "Location",
        ),
        field(
            "organizer",
            e.organizer.clone().unwrap_or_default(),
            "Organizer",
        ),
        field("show_as", e.show_as.as_str().to_string(), "Show as"),
        field("all_day", e.all_day.to_string(), "All day"),
    ];

    NodeSummary {
        id: m.global_id.clone(),
        label: event_label(e),
        node_type: event_node_type(),
        metadata: Metadata { fields },
        has_children: Some(false),
    }
}

/// Markdown detail body for the preview pane.
fn render_body(m: &MergedEvent) -> String {
    let e = &m.event;
    let start = e.start.with_timezone(&Local);
    let end = e.end.with_timezone(&Local);
    let fmt = if e.all_day {
        "%Y-%m-%d"
    } else {
        "%Y-%m-%d %H:%M"
    };

    let mut s = format!("# {}\n\n", event_label(e));
    s.push_str(&format!(
        "- **When:** {} – {}\n",
        start.format(fmt),
        end.format(fmt)
    ));
    s.push_str(&format!("- **Account:** {}\n", m.account));
    if let Some(loc) = e.location.as_deref().filter(|l| !l.trim().is_empty()) {
        s.push_str(&format!("- **Location:** {loc}\n"));
    }
    if let Some(org) = e.organizer.as_deref().filter(|o| !o.trim().is_empty()) {
        s.push_str(&format!("- **Organizer:** {org}\n"));
    }
    s.push_str(&format!("- **Show as:** {}\n", e.show_as.as_str()));
    if let Some(url) = e.url.as_deref().filter(|u| !u.trim().is_empty()) {
        s.push_str(&format!("\n[Open in calendar]({url})\n"));
    }
    if let Some(body) = e.body.as_deref().filter(|b| !b.trim().is_empty()) {
        s.push_str(&format!("\n---\n\n{}\n", body.trim()));
    }
    s
}

/// Leaf node for one event; is its own [`Content`] (markdown body).
struct EventNode {
    id: String,
    label: String,
    node_type: NodeType,
    metadata: Metadata,
    body: String,
}

impl EventNode {
    fn new(m: MergedEvent) -> Self {
        let summary = event_summary(&m);
        let body = render_body(&m);
        Self {
            id: summary.id,
            label: summary.label,
            node_type: summary.node_type,
            metadata: summary.metadata,
            body,
        }
    }
}

#[async_trait]
impl Node for EventNode {
    fn id(&self) -> &str {
        &self.id
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn node_type(&self) -> &NodeType {
        &self.node_type
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn content(&self) -> Option<&dyn Content> {
        Some(self)
    }
}

#[async_trait]
impl Content for EventNode {
    fn node_type(&self) -> &NodeType {
        &self.node_type
    }

    fn version(&self) -> Option<&str> {
        None
    }

    async fn read(&self) -> Result<Vec<u8>> {
        Ok(self.body.clone().into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use not_yet_done_calendar_core::{CalendarRef, ShowAs};
    use not_yet_done_content::children;

    /// Assemble a `CalendarAdapter` from the individual state pieces a test
    /// controls (empty/warm cache, disarmed latch, …). `childs` — the single
    /// source of truth `children::list` drives — sources all its state from the
    /// adapter's own fields, so this is how a test exercises the list path.
    /// Fields the test does not care about get inert defaults.
    fn adapter_from_parts(
        backends: Arc<Vec<Box<dyn CalendarBackend>>>,
        window: Window,
        cache: Cache,
        armed_tx: watch::Sender<bool>,
        current_query: Arc<RwLock<Option<String>>>,
        query_gen: Arc<AtomicU64>,
        inv_tx: broadcast::Sender<Invalidation>,
    ) -> CalendarAdapter {
        let (rem_tx, _) = broadcast::channel(64);
        let (status_tx, _) = watch::channel(AdapterStatus::Ready);
        CalendarAdapter {
            instance_id: "test".to_string(),
            backends,
            window,
            poll_interval: Duration::from_secs(60),
            cache,
            inv_tx,
            rem_tx,
            reminder_leads: Vec::new(),
            armed_tx,
            saved_queries: FsQueryStore::new(
                std::env::temp_dir().join("nyd_cal_test_queries"),
                ".yaml",
            ),
            current_query,
            status_tx,
            query_gen,
            fetch_tasks: Arc::new(Mutex::new(Vec::new())),
            prompts_taken: Arc::new(AtomicBool::new(false)),
            calendars: Arc::new(std::sync::RwLock::new(Vec::new())),
            calendars_loaded: Arc::new(AtomicBool::new(false)),
        }
    }

    fn sample(uid: &str, hour: u32, title: &str) -> MergedEvent {
        MergedEvent {
            global_id: format!("calendar:work:{uid}"),
            account: "Work".to_string(),
            event: CalEvent {
                uid: uid.to_string(),
                calendar: "Work".to_string(),
                title: title.to_string(),
                start: Utc.with_ymd_and_hms(2030, 1, 15, hour, 0, 0).unwrap(),
                end: Utc.with_ymd_and_hms(2030, 1, 15, hour + 1, 0, 0).unwrap(),
                all_day: false,
                location: Some("Room 1".to_string()),
                organizer: Some("Alice".to_string()),
                show_as: ShowAs::Busy,
                body: Some("agenda".to_string()),
                url: Some("https://example.invalid/x".to_string()),
            },
        }
    }

    #[test]
    fn event_summary_has_grouping_and_sort_fields() {
        let row = event_summary(&sample("AAA", 9, "Sprint planning"));
        assert_eq!(row.id, "calendar:work:AAA");
        assert_eq!(row.label, "Sprint planning");
        assert_eq!(row.node_type.type_id, "calendar:event");
        assert_eq!(row.has_children, Some(false));

        let get = |key: &str| {
            row.metadata
                .fields
                .iter()
                .find(|f| f.key == key)
                .map(|f| f.value.clone())
        };
        assert_eq!(get("account").as_deref(), Some("Work"));
        assert_eq!(get("show_as").as_deref(), Some("busy"));
        assert_eq!(get("location").as_deref(), Some("Room 1"));
        assert_eq!(get("all_day").as_deref(), Some("false"));
        // start present and RFC3339-shaped (date + 'T').
        assert!(get("start").unwrap().starts_with("2030-01-15T"));
    }

    #[test]
    fn empty_title_falls_back() {
        let mut m = sample("BBB", 10, "   ");
        m.event.title = "   ".to_string();
        assert_eq!(event_summary(&m).label, "(no title)");
    }

    #[test]
    fn signature_changes_when_an_event_changes() {
        let a = vec![sample("AAA", 9, "Planning")];
        let b = vec![sample("AAA", 9, "Planning (moved)")];
        assert_ne!(signature(&a), signature(&b));
        // identical input → identical signature.
        assert_eq!(
            signature(&a),
            signature(&vec![sample("AAA", 9, "Planning")])
        );
    }

    #[test]
    fn body_renders_markdown_heading_and_link() {
        let body = render_body(&sample("AAA", 9, "Sprint planning"));
        assert!(body.starts_with("# Sprint planning"));
        assert!(body.contains("**Account:** Work"));
        assert!(body.contains("[Open in calendar](https://example.invalid/x)"));
        assert!(body.contains("agenda"));
    }

    #[test]
    fn to_reminder_carries_configured_lead_and_detail() {
        let m = sample("AAA", 9, "Sprint planning");
        let r = to_reminder(&m, 15);
        assert_eq!(r.id, "calendar:work:AAA");
        assert_eq!(r.title, "Sprint planning");
        // lead is the *configured* value, verbatim — not a wall-clock remainder.
        assert_eq!(r.lead_minutes, 15);
        // detail joins account and (present) location.
        assert_eq!(r.detail.as_deref(), Some("Work · Room 1"));
        // `when` is the local-time start, RFC3339-shaped.
        assert!(r.when.starts_with("2030-01-15T"));
    }

    #[test]
    fn to_reminder_detail_omits_blank_location() {
        let mut m = sample("AAA", 9, "No room");
        m.event.location = Some("   ".to_string());
        assert_eq!(to_reminder(&m, 5).detail.as_deref(), Some("Work"));
    }

    /// Build a keyed event map (as the live cache holds) from sample events.
    fn cache_of(events: Vec<MergedEvent>) -> HashMap<String, MergedEvent> {
        events
            .into_iter()
            .map(|m| (m.global_id.clone(), m))
            .collect()
    }

    /// `now` at a fixed clock offset from the sample events' 2030-01-15 day.
    fn at(hour: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2030, 1, 15, hour, min, 0).unwrap()
    }

    #[test]
    fn scan_fires_once_inside_the_lead_window() {
        // Event at 09:00, lead 15 → fire window opens at 08:45.
        let events = cache_of(vec![sample("AAA", 9, "Standup")]);
        let mut fired = HashSet::new();

        // 08:50 is inside the window: due now, nothing pending.
        let (due, next) = scan_reminders(&events, &[15], at(8, 50), &mut fired);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].lead_minutes, 15);
        assert!(next.is_none());
        assert!(fired.contains(&("calendar:work:AAA".to_string(), 15)));

        // A second scan must NOT re-fire (dedup via `fired`).
        let (due2, next2) = scan_reminders(&events, &[15], at(8, 51), &mut fired);
        assert!(due2.is_empty());
        assert!(next2.is_none());
    }

    #[test]
    fn scan_next_fire_is_the_earliest_pending_deadline() {
        // Two events (09:00, 10:00), lead 15 → deadlines 08:45 and 09:45.
        let events = cache_of(vec![sample("AAA", 9, "Early"), sample("BBB", 10, "Late")]);
        let mut fired = HashSet::new();

        // 08:00: neither is due yet; next_fire is the *minimum* deadline (08:45).
        let (due, next) = scan_reminders(&events, &[15], at(8, 0), &mut fired);
        assert!(due.is_empty());
        assert_eq!(next, Some(at(8, 45)));
        assert!(fired.is_empty());
    }

    #[test]
    fn scan_skips_all_day_and_past_events() {
        let mut all_day = sample("AAA", 9, "Holiday");
        all_day.event.all_day = true;
        let past = sample("BBB", 9, "Done"); // start 09:00, now later
        let events = cache_of(vec![all_day, past]);
        let mut fired = HashSet::new();

        let (due, next) = scan_reminders(&events, &[15], at(12, 0), &mut fired);
        assert!(
            due.is_empty(),
            "all-day and past events produce no reminders"
        );
        assert!(next.is_none());
        assert!(fired.is_empty());
    }

    #[test]
    fn scan_multiple_leads_fire_as_distinct_reminders() {
        // Event at 09:00, leads [15, 5] → windows open at 08:45 and 08:55.
        let events = cache_of(vec![sample("AAA", 9, "Standup")]);
        let mut fired = HashSet::new();

        // 08:56 is inside BOTH windows → two due, one per lead.
        let (due, next) = scan_reminders(&events, &[15, 5], at(8, 56), &mut fired);
        let mut leads: Vec<i64> = due.iter().map(|r| r.lead_minutes).collect();
        leads.sort_unstable();
        assert_eq!(leads, vec![5, 15]);
        assert!(next.is_none());
    }

    #[test]
    fn scan_earlier_lead_stays_pending_while_later_fires() {
        // Event at 09:00, leads [15, 5]. At 08:50 only the 15-lead (08:45) has
        // opened; the 5-lead (08:55) is still pending and drives next_fire.
        let events = cache_of(vec![sample("AAA", 9, "Standup")]);
        let mut fired = HashSet::new();

        let (due, next) = scan_reminders(&events, &[15, 5], at(8, 50), &mut fired);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].lead_minutes, 15);
        assert_eq!(next, Some(at(8, 55)));
    }

    #[test]
    fn scan_retain_drops_keys_for_aged_out_events() {
        // A stale fired-key for an event no longer in the cache must be pruned,
        // so a later re-appearance of that id can remind again.
        let events = cache_of(vec![sample("AAA", 9, "Standup")]);
        let mut fired = HashSet::new();
        fired.insert(("calendar:work:GONE".to_string(), 15));
        fired.insert(("calendar:work:AAA".to_string(), 15));

        let (_due, _next) = scan_reminders(&events, &[15], at(8, 50), &mut fired);
        assert!(
            !fired.contains(&("calendar:work:GONE".to_string(), 15)),
            "key for a vanished event is retained-out"
        );
        assert!(fired.contains(&("calendar:work:AAA".to_string(), 15)));
    }

    #[test]
    fn config_leads_scalar_and_seq_both_parse() {
        use crate::config::CalendarConfig;
        let scalar: CalendarConfig =
            serde_yaml::from_str("connections: []\nreminder_lead_minutes: 5\n").unwrap();
        assert_eq!(scalar.reminder_lead_minutes, Some(vec![5]));
        let seq: CalendarConfig =
            serde_yaml::from_str("connections: []\nreminder_lead_minutes: [15, 5]\n").unwrap();
        assert_eq!(seq.reminder_lead_minutes, Some(vec![15, 5]));
        let absent: CalendarConfig = serde_yaml::from_str("connections: []\n").unwrap();
        assert_eq!(absent.reminder_lead_minutes, None);
    }

    #[tokio::test]
    async fn store_cache_keeps_last_good_on_empty_merge() {
        let cache: Cache = Arc::new(RwLock::new(HashMap::new()));

        // A good merge populates the cache.
        let good = vec![sample("AAA", 9, "Standup"), sample("BBB", 10, "Review")];
        assert!(store_cache(&cache, &good).await, "non-empty merge stores");
        assert_eq!(cache.read().await.len(), 2);

        // An empty merge (transient login/backend glitch) must NOT clobber it.
        assert!(
            !store_cache(&cache, &[]).await,
            "empty merge over a non-empty cache is rejected"
        );
        assert_eq!(cache.read().await.len(), 2, "last-good cache is retained");

        // A fresh non-empty merge replaces wholesale (deletions propagate).
        let next = vec![sample("CCC", 11, "Retro")];
        assert!(store_cache(&cache, &next).await);
        let guard = cache.read().await;
        assert_eq!(guard.len(), 1);
        assert!(guard.contains_key("calendar:work:CCC"));
    }

    fn sample_for(conn: &str, uid: &str, hour: u32, title: &str) -> MergedEvent {
        let mut m = sample(uid, hour, title);
        m.global_id = format!("calendar:{conn}:{uid}");
        m.account = conn.to_string();
        m
    }

    #[tokio::test]
    async fn store_connection_replaces_only_its_own_slice() {
        let cache: Cache = Arc::new(RwLock::new(HashMap::new()));

        // Connection A lands its events.
        let a1 = vec![
            sample_for("a", "A1", 9, "A standup"),
            sample_for("a", "A2", 10, "A review"),
        ];
        assert!(store_connection(&cache, "a", &a1).await);
        // Connection B lands independently — A must be untouched.
        let b1 = vec![sample_for("b", "B1", 11, "B sync")];
        assert!(store_connection(&cache, "b", &b1).await);
        {
            let g = cache.read().await;
            assert_eq!(g.len(), 3);
            assert!(g.contains_key("calendar:a:A1"));
            assert!(g.contains_key("calendar:b:B1"));
        }

        // Re-storing A replaces ONLY A's slice (A2 gone, A3 in); B survives.
        let a2 = vec![sample_for("a", "A3", 12, "A retro")];
        assert!(store_connection(&cache, "a", &a2).await);
        let g = cache.read().await;
        assert_eq!(g.len(), 2);
        assert!(g.contains_key("calendar:a:A3"));
        assert!(!g.contains_key("calendar:a:A1"));
        assert!(!g.contains_key("calendar:a:A2"));
        assert!(
            g.contains_key("calendar:b:B1"),
            "sibling connection untouched"
        );
    }

    #[tokio::test]
    async fn store_connection_empty_keeps_last_good_slice() {
        let cache: Cache = Arc::new(RwLock::new(HashMap::new()));
        let a1 = vec![sample_for("a", "A1", 9, "A standup")];
        assert!(store_connection(&cache, "a", &a1).await);
        // An empty fetch for a connection that had events is a transient glitch:
        // rejected (no store, no invalidate), last-good slice retained.
        assert!(
            !store_connection(&cache, "a", &[]).await,
            "empty fetch over a non-empty slice is rejected"
        );
        assert_eq!(cache.read().await.len(), 1, "last-good slice retained");
        // An empty fetch for a connection that never had events also returns
        // false — nothing to show, so the caller must not invalidate (this is
        // what stops a cold, sign-in-in-flight fetch from spinning into a
        // reload storm).
        assert!(!store_connection(&cache, "b", &[]).await);
        assert_eq!(cache.read().await.len(), 1, "empty fetch stores nothing");
    }

    #[test]
    fn load_state_busy_until_last_connection_settles() {
        let mut st = LoadState::default();

        // Two connections start loading (indeterminate).
        let s = st.update("conn_a", &LoadProgress::indeterminate(), 1_000);
        assert!(matches!(s, AdapterStatus::Busy { .. }));
        let s = st.update("conn_b", &LoadProgress::indeterminate(), 1_100);
        assert!(matches!(s, AdapterStatus::Busy { .. }));

        // The fast connection finishes first — banner must STAY Busy (the
        // other connection is still loading).
        let s = st.update("conn_b", &LoadProgress::complete(), 2_000);
        assert!(
            matches!(s, AdapterStatus::Busy { .. }),
            "a fast connection finishing must not drop the banner while a sibling loads"
        );

        // The remaining connection finishes — now Ready.
        let s = st.update("conn_a", &LoadProgress::complete(), 3_000);
        assert!(matches!(s, AdapterStatus::Ready));
    }

    #[test]
    fn load_state_fraction_is_least_advanced_member() {
        let mut st = LoadState::default();
        st.update("a", &LoadProgress::at(0.8), 1_000);
        let s = st.update("b", &LoadProgress::at(0.3), 1_000);
        match s {
            AdapterStatus::Busy { progress, .. } => {
                assert_eq!(
                    progress,
                    Some(0.3),
                    "shows the slowest connection's fraction"
                );
            }
            _ => panic!("expected Busy"),
        }
        // Any indeterminate member forces the whole banner indeterminate.
        let s = st.update("c", &LoadProgress::indeterminate(), 1_000);
        match s {
            AdapterStatus::Busy { progress, .. } => assert_eq!(progress, None),
            _ => panic!("expected Busy"),
        }
    }

    #[test]
    fn load_state_start_time_stable_across_ticks() {
        let mut st = LoadState::default();
        // First connection to start pins the episode's start time.
        let s = st.update("a", &LoadProgress::indeterminate(), 500);
        let start = match s {
            AdapterStatus::Busy {
                started_at_unix_ms, ..
            } => started_at_unix_ms,
            _ => panic!("expected Busy"),
        };
        assert_eq!(start, 500);
        // A later tick (even from another connection) must NOT reset it.
        let s = st.update("b", &LoadProgress::at(0.5), 9_999);
        match s {
            AdapterStatus::Busy {
                started_at_unix_ms, ..
            } => {
                assert_eq!(
                    started_at_unix_ms, 500,
                    "elapsed counts from the first start"
                );
            }
            _ => panic!("expected Busy"),
        }
    }

    #[tokio::test]
    async fn store_cache_allows_empty_over_empty() {
        // An empty merge over an already-empty cache is fine (nothing to lose).
        let cache: Cache = Arc::new(RwLock::new(HashMap::new()));
        assert!(store_cache(&cache, &[]).await);
        assert!(cache.read().await.is_empty());
    }

    /// The poll loop only wakes once the adapter is first asked for data. This
    /// guards that `list()` arms that latch *before* it fetches — so the arm
    /// fires even here, where the empty backend set makes the fetch fail. With
    /// `manual_connect: true` this is what keeps the browser dormant until the
    /// user's first reload, and then reliably starts it on that reload.
    #[tokio::test]
    async fn list_arms_poll_latch_before_fetching() {
        let (armed_tx, armed_rx) = watch::channel(false);
        let (inv_tx, _) = broadcast::channel(16);
        let adapter = adapter_from_parts(
            Arc::new(Vec::new()),
            Window {
                past_days: 1,
                future_days: 1,
            },
            Arc::new(RwLock::new(HashMap::new())),
            armed_tx,
            Arc::new(RwLock::new(None)),
            Arc::new(AtomicU64::new(0)),
            inv_tx,
        );
        let root = adapter.root().await.expect("root");

        assert!(!*armed_rx.borrow(), "latch must start disarmed");

        let params = ListParams {
            node_type: event_node_type(),
            query: None,
            sort: Vec::new(),
            page: None,
            download: false,
            group_by: None,
        };
        // Fails (no backends), but the arm must have fired regardless.
        let _ = children::list(&adapter, root.as_ref(), params).await;

        assert!(
            *armed_rx.borrow(),
            "list() must arm the poll latch even when the fetch itself fails"
        );
    }

    /// The fix for the multi-connection "items land late, all at once" symptom:
    /// `list()` must serve from the warm cache rather than block on a fetch. Here
    /// there are NO backends (a fetch would yield nothing), yet a pre-warmed
    /// cache must still be returned — proving the read path no longer depends on
    /// (and cannot be stalled by) a slow backend's fetch.
    #[tokio::test]
    async fn list_serves_from_warm_cache_without_fetching() {
        let cache: Cache = Arc::new(RwLock::new(HashMap::new()));
        store_connection(
            &cache,
            "conn_a",
            &[sample_for("conn_a", "K1", 9, "Standup")],
        )
        .await;

        let (inv_tx, _) = broadcast::channel(16);
        let adapter = adapter_from_parts(
            Arc::new(Vec::new()),
            Window {
                past_days: 30,
                future_days: 30,
            },
            cache,
            watch::channel(false).0,
            Arc::new(RwLock::new(None)),
            Arc::new(AtomicU64::new(0)),
            inv_tx,
        );
        let root = adapter.root().await.expect("root");

        let params = ListParams {
            node_type: event_node_type(),
            query: None,
            sort: Vec::new(),
            page: None,
            download: false,
            group_by: None,
        };
        let result = children::list(&adapter, root.as_ref(), params)
            .await
            .expect("cache serve must succeed");
        assert_eq!(
            result.items.len(),
            1,
            "warm cache is served without a fetch"
        );
        assert_eq!(result.items[0].label, "Standup");
    }

    // -- `New event` prototype form ----------------------------------------

    fn form(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn create_event_spec_exposes_every_field_kind() {
        let InputSpec::Form { fields } = create_event_spec(&[]) else {
            panic!("create must be a Form");
        };
        let keys: Vec<&str> = fields.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "title", "start", "end", "all_day", "show_as", "location", "body"
            ]
        );
        // One of each kind the driver renders.
        assert!(matches!(fields[0].kind, FormFieldKind::Text));
        assert!(matches!(
            fields[1].kind,
            FormFieldKind::DateTime { with_time: true }
        ));
        assert!(matches!(fields[3].kind, FormFieldKind::Toggle));
        assert!(matches!(fields[4].kind, FormFieldKind::Select { .. }));
    }

    fn gc(conn: &str, cal: &str, label: &str) -> GlobalCalendar {
        GlobalCalendar {
            conn_id: conn.into(),
            cal_id: cal.into(),
            label: label.into(),
        }
    }

    #[test]
    fn create_event_spec_adds_calendar_picker_only_for_a_real_choice() {
        // Single calendar → no picker (unambiguous target).
        let InputSpec::Form { fields } = create_event_spec(&[gc("w", "c1", "Work")]) else {
            panic!()
        };
        assert!(!fields.iter().any(|f| f.key == "calendar"));

        // Two calendars → a picker slots in right after the title.
        let InputSpec::Form { fields } =
            create_event_spec(&[gc("w", "c1", "Work"), gc("p", "c2", "Personal")])
        else {
            panic!()
        };
        let keys: Vec<&str> = fields.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys[0], "title");
        assert_eq!(keys[1], "calendar");
    }

    /// A minimal writable backend that records the drafts it is asked to create,
    /// so the happy path (label→calendar resolution, draft mapping, the actual
    /// `create_event` call) can be exercised without a live server.
    struct StubBackend {
        conn_id: String,
        calendars: Vec<CalendarRef>,
        created: Arc<Mutex<Vec<(Option<String>, EventDraft)>>>,
    }

    #[async_trait]
    impl CalendarBackend for StubBackend {
        fn connection_id(&self) -> &str {
            &self.conn_id
        }
        async fn list_events(
            &self,
            _range: &TimeRange,
        ) -> std::result::Result<Vec<CalEvent>, not_yet_done_calendar_core::CalendarError> {
            Ok(Vec::new())
        }
        async fn list_calendars(
            &self,
        ) -> std::result::Result<Vec<CalendarRef>, not_yet_done_calendar_core::CalendarError>
        {
            Ok(self.calendars.clone())
        }
        async fn create_event(
            &self,
            calendar_id: Option<&str>,
            draft: &EventDraft,
        ) -> std::result::Result<CalEvent, not_yet_done_calendar_core::CalendarError> {
            self.created
                .lock()
                .unwrap()
                .push((calendar_id.map(str::to_string), draft.clone()));
            Ok(CalEvent {
                uid: "new-1".into(),
                calendar: draft.title.clone(),
                title: draft.title.clone(),
                start: draft.start,
                end: draft.end,
                all_day: draft.all_day,
                location: draft.location.clone(),
                organizer: None,
                show_as: draft.show_as,
                body: draft.body.clone(),
                url: None,
            })
        }
    }

    /// Run `execute_create` with no backends and no calendars — enough to
    /// exercise the pure validation guards that run before target resolution.
    async fn exec_create_bare(values: HashMap<String, String>) -> Result<ActionOutcome> {
        let cache: Cache = Arc::new(RwLock::new(HashMap::new()));
        let (inv_tx, _) = broadcast::channel(16);
        execute_create(
            &[],
            &[],
            &cache,
            Window {
                past_days: 1,
                future_days: 1,
            },
            &inv_tx,
            &values,
        )
        .await
    }

    #[tokio::test]
    async fn create_writes_through_the_backend_and_reports_success() {
        let created = Arc::new(Mutex::new(Vec::new()));
        let backends: Vec<Box<dyn CalendarBackend>> = vec![Box::new(StubBackend {
            conn_id: "work".into(),
            calendars: vec![CalendarRef {
                id: "cal-1".into(),
                name: "Team".into(),
                writable: true,
            }],
            created: Arc::clone(&created),
        })];
        let calendars = vec![gc("work", "cal-1", "Team")];
        let cache: Cache = Arc::new(RwLock::new(HashMap::new()));
        let (inv_tx, _) = broadcast::channel(16);

        let outcome = execute_create(
            &backends,
            &calendars,
            &cache,
            Window {
                past_days: 1,
                future_days: 1,
            },
            &inv_tx,
            &form(&[
                ("title", "Standup"),
                ("start", "today 9:00"),
                ("end", "today 9:30"),
                ("show_as", "Free"),
                ("location", "Room 1"),
            ]),
        )
        .await
        .expect("valid form writes");

        match outcome {
            ActionOutcome::Done { message: Some(m) } => {
                assert!(m.contains("Created"), "got: {m}");
                assert!(m.contains("Standup"), "got: {m}");
                assert!(m.contains("Team"), "got: {m}");
                assert!(m.contains("Room 1"), "got: {m}");
            }
            _ => panic!("expected ActionOutcome::Done"),
        }

        let rec = created.lock().unwrap();
        assert_eq!(rec.len(), 1, "backend.create_event was called exactly once");
        assert_eq!(
            rec[0].0.as_deref(),
            Some("cal-1"),
            "target calendar id passed through"
        );
        assert_eq!(rec[0].1.title, "Standup");
        assert_eq!(rec[0].1.location.as_deref(), Some("Room 1"));
        assert!(
            matches!(rec[0].1.show_as, ShowAs::Free),
            "Show-as label mapped to token"
        );
    }

    #[tokio::test]
    async fn create_rejects_unknown_calendar_label() {
        let created = Arc::new(Mutex::new(Vec::new()));
        let backends: Vec<Box<dyn CalendarBackend>> = vec![Box::new(StubBackend {
            conn_id: "work".into(),
            calendars: vec![],
            created: Arc::clone(&created),
        })];
        let calendars = vec![gc("work", "cal-1", "Team"), gc("work", "cal-2", "Personal")];
        let cache: Cache = Arc::new(RwLock::new(HashMap::new()));
        let (inv_tx, _) = broadcast::channel(16);
        let err = execute_create(
            &backends,
            &calendars,
            &cache,
            Window {
                past_days: 1,
                future_days: 1,
            },
            &inv_tx,
            &form(&[
                ("title", "X"),
                ("start", "today 9:00"),
                ("end", "today 10:00"),
                ("calendar", "Nonexistent"),
            ]),
        )
        .await
        .err()
        .expect("unknown calendar label must error");
        assert!(format!("{err}").contains("unknown calendar"), "got: {err}");
        assert!(
            created.lock().unwrap().is_empty(),
            "no write on a bad target"
        );
    }

    #[tokio::test]
    async fn create_without_any_writable_calendar_errors() {
        let err = exec_create_bare(form(&[
            ("title", "X"),
            ("start", "today 9:00"),
            ("end", "today 10:00"),
        ]))
        .await
        .err()
        .expect("no writable calendar must error");
        assert!(
            format!("{err}").contains("no writable calendar"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn create_rejects_end_before_start() {
        let err = exec_create_bare(form(&[
            ("title", "X"),
            ("start", "today 10:00"),
            ("end", "today 9:00"),
        ]))
        .await
        .err()
        .expect("end before start must error");
        assert!(
            format!("{err}").contains("end must be after start"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn create_requires_a_title() {
        let err = exec_create_bare(form(&[("start", "today 9:00"), ("end", "today 10:00")]))
            .await
            .err()
            .expect("missing title must error");
        assert!(format!("{err}").contains("title"), "got: {err}");
    }

    #[tokio::test]
    async fn create_rejects_unparseable_start() {
        let err = exec_create_bare(form(&[
            ("title", "X"),
            ("start", "not a date at all"),
            ("end", "today 10:00"),
        ]))
        .await
        .err()
        .expect("garbage start must error");
        assert!(format!("{err}").contains("start time"), "got: {err}");
    }
}
