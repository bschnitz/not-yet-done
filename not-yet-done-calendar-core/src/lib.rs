//! Calendar backend contract — the decoupling seam of the calendar adapter.
//!
//! The `calendar` content adapter aggregates events from any number of
//! configured connections, where each connection speaks some calendar
//! protocol. This crate defines the seam between the aggregating adapter and
//! those protocol implementations:
//!
//! - [`CalEvent`] — the protocol-neutral event DTO the adapter turns into rows.
//! - [`CalendarBackend`] — a single connection to one calendar source; the
//!   adapter holds one boxed backend per configured connection and fans out
//!   [`CalendarBackend::list_events`] across all of them.
//! - [`CalendarBackendFactory`] — constructs a backend from a connection's
//!   opaque config block (a YAML string, mirroring how the host builds an
//!   adapter from `adapter.config`). The adapter never parses backend config.
//!
//! A backend implementation (e.g. Microsoft Graph) lives in its own crate that
//! depends **only** on this one — never on the adapter crate and never on the
//! TUI — so backends are independent, feature-gated units. Write support is
//! intentionally absent for now: a future `CalendarWriteBackend` extension
//! trait can add it without touching the read seam or any existing backend.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// Half-open time window `[start, end)` an event listing is scoped to. The
/// adapter derives it from its configured look-behind / look-ahead window and
/// passes the same range to every backend so their results line up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl TimeRange {
    pub fn new(start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        Self { start, end }
    }
}

/// Progress of an asynchronous, out-of-band load — the payload of
/// [`CalendarBackend::subscribe_ready`].
///
/// A browser-driven backend loads its data incrementally (an interactive
/// sign-in, then paging month by month); each step it can estimate makes the
/// aggregating adapter (a) re-fetch so freshly-arrived rows appear, and (b)
/// surface a "still loading" banner with a percentage.
///
/// `fraction` is a best-effort estimate in `[0, 1]`, or `None` while the load
/// is underway but no fraction can yet be quantified — chiefly during the
/// interactive sign-in, before month paging starts. A frontend renders `Some`
/// as a percentage and `None` as an indeterminate "still loading" cue (no
/// number). `done` marks the terminal fire (the load settled — clear the
/// banner), which a backend sets even when it never produced a clean `1.0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoadProgress {
    /// Best-effort completion estimate clamped to `[0, 1]`, or `None` when the
    /// load is running but not yet quantifiable (an interactive sign-in).
    pub fraction: Option<f32>,
    /// True on the final fire, once no more data is coming for this load.
    pub done: bool,
}

impl LoadProgress {
    /// A load that has fully settled (`fraction = Some(1.0)`, `done = true`).
    pub fn complete() -> Self {
        Self {
            fraction: Some(1.0),
            done: true,
        }
    }

    /// An in-progress fire at a known `fraction` (clamped to `[0, 1]`).
    pub fn at(fraction: f32) -> Self {
        Self {
            fraction: Some(fraction.clamp(0.0, 1.0)),
            done: false,
        }
    }

    /// An in-progress fire with no fraction yet — the load is running but not
    /// quantifiable (interactive sign-in). The frontend shows the banner
    /// without a percentage until a later `at` supplies one.
    pub fn indeterminate() -> Self {
        Self {
            fraction: None,
            done: false,
        }
    }
}

/// How the organiser marks the time an event occupies (Microsoft Graph
/// `showAs`, iCalendar `TRANSP`/`STATUS`). Surfaced as a sortable/groupable
/// column. Anything a backend can't map lands in [`ShowAs::Unknown`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShowAs {
    Free,
    Tentative,
    Busy,
    OutOfOffice,
    WorkingElsewhere,
    Unknown,
}

impl ShowAs {
    /// Stable lower-case token used as the metadata field value (so sorting
    /// and grouping are deterministic across backends).
    pub fn as_str(&self) -> &'static str {
        match self {
            ShowAs::Free => "free",
            ShowAs::Tentative => "tentative",
            ShowAs::Busy => "busy",
            ShowAs::OutOfOffice => "oof",
            ShowAs::WorkingElsewhere => "elsewhere",
            ShowAs::Unknown => "unknown",
        }
    }
}

/// One calendar event, normalised across protocols. `start`/`end` are always
/// UTC instants — the adapter renders them in local time and groups on them.
/// `uid` is the backend-local stable identifier; the adapter namespaces it
/// with the connection id so ids from different connections never collide.
#[derive(Clone, Debug)]
pub struct CalEvent {
    /// Backend-local stable id (Graph event id, iCal UID, …). Unique within
    /// one connection; the adapter prefixes the connection id for global
    /// uniqueness.
    pub uid: String,
    /// Display name of the calendar/mailbox this event belongs to.
    pub calendar: String,
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub all_day: bool,
    pub location: Option<String>,
    pub organizer: Option<String>,
    pub show_as: ShowAs,
    /// Plain-text body for the preview pane, if the backend fetched one.
    pub body: Option<String>,
    /// Web link to open the event in the source app, if any.
    pub url: Option<String>,
}

/// A calendar collection a connection exposes as a possible **write target**.
///
/// The adapter enumerates these across every connection
/// ([`CalendarBackend::list_calendars`]) and offers them as one flat picker in
/// the "New event" form, so the user chooses *which* calendar — of any
/// connection — a new event lands in. `id` is the backend-local handle the same
/// backend later resolves in [`CalendarBackend::create_event`] (a CalDAV
/// collection URL, a Graph calendar id); the adapter treats it as opaque.
#[derive(Clone, Debug)]
pub struct CalendarRef {
    /// Backend-local id of the collection — passed back verbatim as
    /// `create_event`'s `calendar_id`. Opaque to the adapter.
    pub id: String,
    /// Human-readable calendar name (e.g. "Personal", "Team"). The adapter
    /// prefixes it with the connection label when several connections are
    /// configured, so the flattened picker stays unambiguous.
    pub name: String,
    /// Whether new events may be created here. A backend that can't tell
    /// reports `true`; the create attempt then surfaces any server refusal.
    pub writable: bool,
}

/// The event a "New event" form describes, before any backend has written it —
/// the write counterpart of the read-only [`CalEvent`] (no `uid`/`url`/
/// `organizer`/`calendar`: those are assigned by the server, and the target
/// calendar is passed separately to [`CalendarBackend::create_event`]).
#[derive(Clone, Debug)]
pub struct EventDraft {
    pub title: String,
    /// Start instant (UTC). For an all-day event only the date part is used.
    pub start: DateTime<Utc>,
    /// End instant (UTC), exclusive. For an all-day event only the date part is
    /// used (an event on a single day ends the following date).
    pub end: DateTime<Utc>,
    pub all_day: bool,
    pub location: Option<String>,
    pub body: Option<String>,
    pub show_as: ShowAs,
}

/// Failure of a backend operation. The adapter maps these onto the content
/// layer's error type; keeping a dedicated enum here spares backends a
/// dependency on the content crate's error type.
#[derive(Debug, thiserror::Error)]
pub enum CalendarError {
    #[error("auth error: {0}")]
    Auth(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("config error: {0}")]
    Config(String),
    #[error("{0}")]
    Other(String),
}

/// A live connection to one calendar source. Reading is mandatory;
/// [`list_calendars`](CalendarBackend::list_calendars) /
/// [`create_event`](CalendarBackend::create_event) default to "read-only" so a
/// backend opts into writing only when it implements them.
#[async_trait]
pub trait CalendarBackend: Send + Sync {
    /// Stable id of the connection this backend serves — the `id` from the
    /// connection's config entry. The adapter uses it to namespace event ids.
    fn connection_id(&self) -> &str;

    /// Human-readable label for the connection (e.g. the mailbox / account
    /// name). Surfaced as the "Account" column so aggregated rows show their
    /// origin. Defaults to the connection id.
    fn connection_label(&self) -> &str {
        self.connection_id()
    }

    /// All events overlapping `range`, across every calendar this connection
    /// exposes (aggregation across a connection's own calendars is the
    /// backend's concern; aggregation across connections is the adapter's).
    async fn list_events(&self, range: &TimeRange) -> Result<Vec<CalEvent>, CalendarError>;

    /// The calendars of this connection that can be offered as write targets
    /// for a new event, in the connection's own order.
    ///
    /// The adapter flattens these across every connection into the single
    /// "Calendar" picker on the "New event" form. A read-only backend (or one
    /// that can't enumerate its calendars) keeps the default empty list, so it
    /// simply contributes no targets — no event can be created against it.
    async fn list_calendars(&self) -> Result<Vec<CalendarRef>, CalendarError> {
        Ok(Vec::new())
    }

    /// Create `draft` in this connection's calendar identified by `calendar_id`
    /// (an `id` from [`list_calendars`](CalendarBackend::list_calendars)), or
    /// the connection's default calendar when `None`. Returns the created event
    /// as the backend now sees it (server-assigned `uid`, canonical times).
    ///
    /// The default is a hard read-only error, so a backend gains write support
    /// only by overriding this — existing read-only backends (and the
    /// browser-driven office365-web wrapper) inherit the refusal unchanged.
    async fn create_event(
        &self,
        calendar_id: Option<&str>,
        draft: &EventDraft,
    ) -> Result<CalEvent, CalendarError> {
        let _ = (calendar_id, draft);
        Err(CalendarError::Other(
            "this calendar connection is read-only".into(),
        ))
    }

    /// Subscribe to the backend's "data became ready" signal, if it has one.
    ///
    /// A backend whose data arrives *asynchronously and out of band* — e.g. the
    /// browser-driven `office365-web` backend, where an interactive sign-in may
    /// complete minutes after a `list_events` call already returned empty —
    /// returns a [`broadcast::Receiver`] that fires once the source is freshly
    /// loaded and a subsequent `list_events` would succeed. The aggregating
    /// adapter listens on it and refreshes the view immediately, so freshness
    /// after login is not left to the periodic poll (whose job is only to catch
    /// *external* changes).
    ///
    /// Each fire carries a [`LoadProgress`]: a best-effort completion estimate
    /// the adapter turns into a "still loading… N %" banner, plus a `done` flag
    /// marking the terminal fire (login finished / all pages in) so the adapter
    /// clears the banner. A backend that can only signal "ready now" fires a
    /// single [`LoadProgress::complete`].
    ///
    /// A backend that answers synchronously (a REST call resolves its own
    /// future) has nothing to signal and keeps the default `None`.
    fn subscribe_ready(&self) -> Option<tokio::sync::broadcast::Receiver<LoadProgress>> {
        None
    }

    /// Take this backend's stream of mid-operation user-input prompts, if it
    /// has one — e.g. the `office365-web` backend raising an MFA prompt during
    /// an interactive sign-in, mid-`list_events`.
    ///
    /// Reuses the content layer's [`PromptRequest`](not_yet_done_content::PromptRequest)
    /// (built on the Action `InputSpec`/`ActionInput` vocabulary) so the whole
    /// stack shares one input model rather than a parallel one — the seam is a
    /// pass-through, not a re-declaration. **Single-consumer** (`mpsc`, not
    /// broadcast): exactly one frontend services input and each request carries
    /// a non-cloneable one-shot responder. The aggregating adapter takes it
    /// once per backend and merges the streams across connections.
    ///
    /// Default `None` — a backend that authenticates non-interactively (REST,
    /// device-code) never prompts and doesn't override.
    fn take_prompt_requests(
        &self,
    ) -> Option<tokio::sync::mpsc::Receiver<not_yet_done_content::PromptRequest>> {
        None
    }
}

/// Constructs a [`CalendarBackend`] for one connection from its opaque config
/// block. `config` is the YAML text of the connection's backend-specific
/// section — the adapter re-serialises that sub-tree and hands it over without
/// interpreting it, exactly as the host passes `adapter.config` to an
/// [`AdapterFactory`](https://docs.rs). This keeps the adapter ignorant of
/// every backend's config shape.
pub trait CalendarBackendFactory: Send + Sync {
    /// The `backend:` discriminator this factory answers to (e.g. "microsoft").
    fn backend_type(&self) -> &str;

    /// Construct the backend. `ctx` carries the host capabilities the adapter
    /// received in [`AdapterFactory::create`](not_yet_done_content::AdapterFactory::create)
    /// — chiefly the cross-adapter [`HostEventBus`](not_yet_done_content::HostEventBus).
    /// Backends that coordinate with the UI over the bus (e.g. the office365-web
    /// login's MFA number-match prompt) take it here; a non-interactive backend
    /// (msgraph device-code) ignores it.
    fn create(
        &self,
        connection_id: &str,
        config: &str,
        ctx: &not_yet_done_content::HostContext,
    ) -> Result<Box<dyn CalendarBackend>, CalendarError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_progress_at_clamps_to_unit_interval() {
        assert_eq!(LoadProgress::at(-0.5).fraction, Some(0.0));
        assert_eq!(LoadProgress::at(1.5).fraction, Some(1.0));
        assert_eq!(LoadProgress::at(0.25).fraction, Some(0.25));
        assert!(
            !LoadProgress::at(0.25).done,
            "an `at` fire is never terminal"
        );
    }

    #[test]
    fn load_progress_complete_is_done_at_full() {
        let p = LoadProgress::complete();
        assert_eq!(p.fraction, Some(1.0));
        assert!(p.done);
    }

    #[test]
    fn load_progress_indeterminate_has_no_fraction() {
        let p = LoadProgress::indeterminate();
        assert_eq!(p.fraction, None, "no percentage is known yet");
        assert!(!p.done, "still loading");
    }
}
