//! `office365-web` calendar backend.
//!
//! For tenants where Microsoft Graph is blocked (device-compliance Conditional
//! Access) and the calendar lives behind Outlook on the web, this backend
//! reads events by driving an authenticated browser session — provided by the
//! app-agnostic [`not_yet_done_office365_web`] wrapper — instead of a REST API.
//!
//! It is a thin adapter over that wrapper: resolve the (shared, by account key)
//! session, ask its [`CalendarApi`] for the range, and map each
//! [`MsCalEvent`] onto calendar-core's neutral [`CalEvent`]. All browser and
//! protocol concerns stay in the wrapper crate.

mod config;

use async_trait::async_trait;

use not_yet_done_calendar_core::{
    CalEvent, CalendarBackend, CalendarBackendFactory, CalendarError, LoadProgress, ShowAs,
    TimeRange,
};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

use not_yet_done_content::auth::CredentialResolver;
use not_yet_done_content::{BusEvent, HostEventBus, publish_event, subscribe_events};
use not_yet_done_office365_web::{
    LoginCredentials, MsCalEvent, MsOfficeError, MsOfficeWeb, MsShowAs, MsTimeRange, PromptKind,
    SessionConfig, SessionHandle,
};

use config::Office365WebConfig;

/// Payload key carrying the typed one-time code across the bus. A `form`
/// action's `emit` returns the typed value under this key; [`wait_for_otc`]
/// reads it back.
const MFA_CODE_FIELD: &str = "code";

/// Payload key carrying the display-only number-match digits. A configured
/// `notify` action reads it as `{number}`.
const NUMBER_FIELD: &str = "number";

/// Bus topic: the login reached a **number-match** challenge. Payload carries
/// `{ "number": "<digits>" }` for a `notify` action to display; the browser
/// advances on its own once the user approves in their authenticator app, so
/// this is fire-and-forget (no reply expected).
const TOPIC_NUMBER_MATCH: &str = "office365-web:mfa:number-match";

/// Bus topic: the login needs a **one-time code** typed in. A request (carries
/// a `correlation_id`); the bound action collects the code and emits a reply
/// with the same `correlation_id` carrying `{ "code": "…" }`.
const TOPIC_OTC_REQUIRED: &str = "office365-web:mfa:otc-required";

/// Bus topic: the interactive sign-in for this connection **completed** — the
/// calendar became readable. Lets a `notify` opened for a number-match close
/// itself via `on_event: { "office365-web:mfa:resolved": close }`.
const TOPIC_MFA_RESOLVED: &str = "office365-web:mfa:resolved";

/// One `office365-web` connection.
///
/// It resolves its (possibly shared) session on first use and then **retains
/// the handle** for its own lifetime. That matters: the session registry only
/// holds sessions weakly, so if nobody kept a handle the browser would be torn
/// down the moment a fetch returned — killing an in-progress interactive login
/// and reopening a fresh window on every poll. Backends are long-lived (one per
/// connection, for the adapter's lifetime), so holding the handle here keeps
/// exactly one browser alive per `account_key` across polls and logins.
pub struct Office365WebBackend {
    connection_id: String,
    label: String,
    session_config: SessionConfig,
    /// Resolvers for the sign-in credentials, resolved lazily on first use and
    /// injected into the session config. Kept off `SessionConfig` so the wrapper
    /// crate never depends on the credential system.
    username_resolver: Option<Box<dyn CredentialResolver>>,
    password_resolver: Option<Box<dyn CredentialResolver>>,
    /// Retained session handle, populated lazily on the first `list_events`.
    session: Mutex<Option<SessionHandle>>,
    /// Re-broadcasts the session's load activity as the backend's
    /// [`CalendarBackend::subscribe_ready`] progress stream. Created up front
    /// (before any session) so the adapter can subscribe at startup; a forwarder
    /// task is wired to the session the first time one is created.
    ///
    /// A [`LoadProgress::indeterminate`] fires when a session is first created
    /// (the interactive sign-in begins, no fraction yet), then one
    /// [`LoadProgress::at`] per captured month as `getCalendarView` pages the
    /// window in, then a terminal [`LoadProgress::complete`] once all data is in.
    ready_tx: broadcast::Sender<LoadProgress>,
    /// Host cross-adapter event bus (from [`HostContext`]). The login flow
    /// publishes MFA [`BusEvent`](not_yet_done_content::BusEvent)s here (e.g.
    /// `office365-web:mfa:number-match`) so a configured `event_actions`
    /// binding can drive the UI, and subscribes for the answer — without this
    /// backend depending on the TUI.
    event_bus: Arc<dyn HostEventBus>,
}

impl Office365WebBackend {
    /// Return the retained session, creating (and storing) it on first use.
    ///
    /// The lock is never held across the `await`: we snapshot the current
    /// handle, and if absent create one and store a clone. A concurrent racer
    /// that also creates one is harmless — the registry dedupes by
    /// `account_key`, so both end up referring to the same browser session.
    async fn session(&self) -> Result<SessionHandle, CalendarError> {
        if let Some(handle) = self.session.lock().unwrap().clone() {
            return Ok(handle);
        }
        let mut config = self.session_config.clone();
        config.credentials = Some(self.resolve_credentials().await?);
        // A session is about to be built — the interactive sign-in / first load
        // begins now. Announce loading with no fraction yet (a percentage is
        // meaningless during a user-paced sign-in) so the adapter raises an
        // indeterminate "still loading" banner; the forwarder below supplies
        // per-month fractions once paging starts and completes it on `loaded`.
        let _ = self.ready_tx.send(LoadProgress::indeterminate());
        let handle = MsOfficeWeb::session(config).await.map_err(map_err)?;

        // Store, but only wire the forwarder if we actually won the store race:
        // a concurrent racer creates the *same* underlying session (the registry
        // dedupes by account key), so wiring once is correct and avoids
        // duplicate ready fires.
        let mut guard = self.session.lock().unwrap();
        if let Some(existing) = guard.clone() {
            return Ok(existing);
        }
        *guard = Some(handle.clone());
        drop(guard);

        // Forward the session's load-progress pushes onto the backend's ready
        // stream for the adapter, translating the wrapper's LoadStatus into
        // calendar-core's LoadProgress: a terminal `done` → complete(); a numeric
        // fraction → at(f); a fraction-less nudge → indeterminate(). Detached:
        // lives with the (long-lived) backend.
        //
        // A terminal `done` is also the moment the interactive sign-in truly
        // finished (the calendar became readable), so we publish
        // `office365-web:mfa:resolved` here — that is what lets a `notify`
        // opened for a number-match challenge close itself. Idempotent: a later
        // `done` (e.g. a poll reload) with no popup open is a no-op.
        let mut loaded = handle.subscribe_loaded();
        let ready_tx = self.ready_tx.clone();
        let resolved_bus = Arc::clone(&self.event_bus);
        let resolved_source = self.connection_id.clone();
        tokio::spawn(async move {
            while let Ok(status) = loaded.recv().await {
                let progress = if status.done {
                    publish_event(
                        resolved_bus.as_ref(),
                        BusEvent::new(
                            TOPIC_MFA_RESOLVED,
                            resolved_source.clone(),
                            serde_json::Value::Null,
                        ),
                    );
                    LoadProgress::complete()
                } else {
                    match status.fraction {
                        Some(f) => LoadProgress::at(f),
                        None => LoadProgress::indeterminate(),
                    }
                };
                let _ = ready_tx.send(progress);
            }
        });

        // Wire the prompt translator: turn the session's neutral
        // `SessionPrompt`s into MFA `BusEvent`s and route any answer back to the
        // sidecar. A number-match challenge is fire-and-forget (publish the
        // number, acknowledge immediately — the browser advances once the user
        // approves on their phone); a one-time code is a request/response — mint
        // a `correlation_id`, publish `otc-required`, and wait on the bus for a
        // reply carrying that id. If nobody is listening on the bus, cancel the
        // sign-in cleanly rather than block forever. Detached; ends when the
        // session's prompt stream closes (session torn down).
        if let Some(mut prompts) = handle.take_prompts() {
            let bus = Arc::clone(&self.event_bus);
            let source = self.connection_id.clone();
            tokio::spawn(async move {
                let mut seq: u64 = 0;
                while let Some(sp) = prompts.recv().await {
                    match sp.kind() {
                        PromptKind::Acknowledge => {
                            // Display-only number match: publish the number and
                            // acknowledge (the sidecar treats the reply as a mere
                            // overlay dismissal; the browser advances on its own).
                            let number = sp.detail().unwrap_or_default().to_string();
                            publish_event(
                                bus.as_ref(),
                                BusEvent::new(
                                    TOPIC_NUMBER_MATCH,
                                    source.clone(),
                                    serde_json::json!({ NUMBER_FIELD: number }),
                                ),
                            );
                            let _ = sp.acknowledge().await;
                        }
                        PromptKind::Text { .. } => {
                            // One-time code: request/response over the bus.
                            seq += 1;
                            let correlation_id = format!("{source}:otc:{seq}");
                            // Subscribe *before* publishing so a fast reply is
                            // never missed. If no consumer is armed, abort the
                            // sign-in cleanly instead of hanging.
                            let mut rx = subscribe_events(bus.as_ref());
                            if bus.receiver_count(not_yet_done_content::EVENT_CHANNEL) == 0 {
                                let _ = sp.cancel().await;
                                continue;
                            }
                            publish_event(
                                bus.as_ref(),
                                BusEvent::new(
                                    TOPIC_OTC_REQUIRED,
                                    source.clone(),
                                    serde_json::Value::Null,
                                )
                                .with_correlation(correlation_id.clone()),
                            );
                            let code = wait_for_otc(&mut rx, &correlation_id).await;
                            match code {
                                Some(code) if !code.is_empty() => {
                                    let _ = sp.answer_text(code).await;
                                }
                                _ => {
                                    let _ = sp.cancel().await;
                                }
                            }
                        }
                    }
                }
            });
        }
        Ok(handle)
    }

    /// Resolve the configured credentials. The username falls back to
    /// `login_hint` when no `username:` provider is set, so a config that only
    /// sets `login_hint` + `password` still fills the account picker/email.
    async fn resolve_credentials(&self) -> Result<LoginCredentials, CalendarError> {
        let username = match &self.username_resolver {
            Some(r) => Some(resolve(r.as_ref(), "username").await?),
            None => self.session_config.login_hint.clone(),
        };
        let password = match &self.password_resolver {
            Some(r) => Some(resolve(r.as_ref(), "password").await?),
            None => None,
        };
        Ok(LoginCredentials { username, password })
    }
}

/// Resolve one credential, mapping any failure to a calendar auth error.
async fn resolve(resolver: &dyn CredentialResolver, field: &str) -> Result<String, CalendarError> {
    resolver
        .resolve()
        .await
        .map_err(|e| CalendarError::Auth(format!("resolve {field}: {e}")))
}

/// Wait for the bus reply to an `otc-required` request, matched by
/// `correlation_id` (topic-agnostic — any event carrying the id counts). The
/// reply's payload carries the typed code under [`MFA_CODE_FIELD`]; a reply
/// without it (a NACK / cancel from the rule engine) yields `None`. `None` also
/// on a closed channel. A `Lagged` receiver is resynced by simply continuing —
/// the reply, if it was among the dropped events, is retried by the sign-in
/// (a fresh OTC field re-prompts).
async fn wait_for_otc(
    rx: &mut broadcast::Receiver<not_yet_done_content::HostEvent>,
    correlation_id: &str,
) -> Option<String> {
    use broadcast::error::RecvError;
    loop {
        match rx.recv().await {
            Ok(payload) => {
                let Some(ev) = BusEvent::from_host_event(&payload) else {
                    continue;
                };
                if ev.correlation_id.as_deref() != Some(correlation_id) {
                    continue;
                }
                return ev
                    .payload
                    .get(MFA_CODE_FIELD)
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => return None,
        }
    }
}

#[async_trait]
impl CalendarBackend for Office365WebBackend {
    fn connection_id(&self) -> &str {
        &self.connection_id
    }

    fn connection_label(&self) -> &str {
        &self.label
    }

    async fn list_events(&self, range: &TimeRange) -> Result<Vec<CalEvent>, CalendarError> {
        let session = self.session().await?;
        let events = session
            .calendar()
            .get_view(MsTimeRange::new(range.start, range.end))
            .await
            .map_err(map_err)?;
        Ok(events
            .into_iter()
            .map(|e| map_event(e, &self.label))
            .collect())
    }

    fn subscribe_ready(&self) -> Option<broadcast::Receiver<LoadProgress>> {
        Some(self.ready_tx.subscribe())
    }
}

/// Registers as backend type `office365-web`.
#[derive(Default)]
pub struct Office365WebBackendFactory;

impl Office365WebBackendFactory {
    pub fn new() -> Self {
        Self
    }
}

impl CalendarBackendFactory for Office365WebBackendFactory {
    fn backend_type(&self) -> &str {
        "office365-web"
    }

    fn create(
        &self,
        connection_id: &str,
        config: &str,
        ctx: &not_yet_done_content::HostContext,
    ) -> Result<Box<dyn CalendarBackend>, CalendarError> {
        let cfg: Office365WebConfig = serde_yaml::from_str(config).map_err(|e| {
            CalendarError::Config(format!("invalid office365-web backend config: {e}"))
        })?;
        let label = cfg
            .name
            .clone()
            .unwrap_or_else(|| connection_id.to_string());
        // Build the credential resolvers while the providers are still on `cfg`
        // (the resolvers themselves hold no secret until first resolved).
        let resolvers = cfg
            .build_credential_resolvers()
            .map_err(|e| CalendarError::Config(format!("office365-web credentials: {e}")))?;
        let (ready_tx, _) = broadcast::channel(16);
        Ok(Box::new(Office365WebBackend {
            connection_id: connection_id.to_string(),
            label,
            session_config: cfg.into_session_config(),
            username_resolver: resolvers.username,
            password_resolver: resolvers.password,
            session: Mutex::new(None),
            ready_tx,
            event_bus: ctx.event_bus.clone(),
        }))
    }
}

fn map_event(e: MsCalEvent, label: &str) -> CalEvent {
    CalEvent {
        uid: e.id,
        calendar: label.to_string(),
        title: e.subject.unwrap_or_default(),
        start: e.start,
        end: e.end,
        all_day: e.is_all_day,
        location: e.location.filter(|s| !s.trim().is_empty()),
        organizer: e.organizer.filter(|s| !s.trim().is_empty()),
        show_as: map_show_as(e.show_as),
        body: e.body_preview.filter(|s| !s.trim().is_empty()),
        url: e.web_link.filter(|s| !s.trim().is_empty()),
    }
}

fn map_show_as(s: MsShowAs) -> ShowAs {
    match s {
        MsShowAs::Free => ShowAs::Free,
        MsShowAs::Tentative => ShowAs::Tentative,
        MsShowAs::Busy => ShowAs::Busy,
        MsShowAs::Oof => ShowAs::OutOfOffice,
        MsShowAs::WorkingElsewhere => ShowAs::WorkingElsewhere,
        MsShowAs::Unknown => ShowAs::Unknown,
    }
}

fn map_err(e: MsOfficeError) -> CalendarError {
    match e {
        MsOfficeError::LoginRequired => {
            CalendarError::Auth("interactive login required for office365-web session".into())
        }
        MsOfficeError::Timeout => CalendarError::Network("office365-web sidecar timed out".into()),
        MsOfficeError::Sidecar(m) => CalendarError::Network(format!("office365-web sidecar: {m}")),
        MsOfficeError::Protocol(m) => CalendarError::Other(format!("office365-web protocol: {m}")),
        MsOfficeError::Other(m) => CalendarError::Other(m),
    }
}

#[cfg(test)]
mod otc_wait_tests {
    use super::*;
    use not_yet_done_content::InMemoryHostBus;

    #[tokio::test]
    async fn returns_code_from_matching_correlation_reply() {
        let bus = InMemoryHostBus::default();
        let mut rx = subscribe_events(&bus);
        // A reply on a different topic but the right correlation id still counts.
        publish_event(
            &bus,
            BusEvent::new(
                "anything",
                "tui",
                serde_json::json!({ MFA_CODE_FIELD: "123456" }),
            )
            .with_correlation("conn:otc:1"),
        );
        assert_eq!(
            wait_for_otc(&mut rx, "conn:otc:1").await.as_deref(),
            Some("123456")
        );
    }

    #[tokio::test]
    async fn ignores_replies_for_other_correlation_ids() {
        let bus = InMemoryHostBus::default();
        let mut rx = subscribe_events(&bus);
        publish_event(
            &bus,
            BusEvent::new("t", "tui", serde_json::json!({ MFA_CODE_FIELD: "999" }))
                .with_correlation("someone-else"),
        );
        publish_event(
            &bus,
            BusEvent::new("t", "tui", serde_json::json!({ MFA_CODE_FIELD: "42" }))
                .with_correlation("mine"),
        );
        assert_eq!(wait_for_otc(&mut rx, "mine").await.as_deref(), Some("42"));
    }

    #[tokio::test]
    async fn nack_reply_without_code_yields_none() {
        let bus = InMemoryHostBus::default();
        let mut rx = subscribe_events(&bus);
        // A cancel/NACK from the rule engine: right correlation, no code field.
        publish_event(
            &bus,
            BusEvent::new("t", "tui", serde_json::json!({ "nack": true })).with_correlation("mine"),
        );
        assert_eq!(wait_for_otc(&mut rx, "mine").await, None);
    }
}
