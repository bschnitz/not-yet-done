//! The session actor: owns one sidecar process and serialises all access to
//! it. A [`SessionHandle`] is a cheap, clonable reference to the actor; when
//! the last handle drops, the actor task is aborted and its sidecar (and hence
//! its browser) is killed.
//!
//! Access is serialised on purpose: a single browser session can't service
//! concurrent operations safely, so the actor processes one command at a time.
//! That also keeps the sidecar protocol trivial — every request is immediately
//! followed by exactly its response on the next stdout line.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::dto::{MsCalEvent, MsTimeRange};
use crate::error::MsOfficeError;
use crate::sidecar::{Sidecar, SidecarEvent};

/// How the sidecar process is launched.
#[derive(Clone, Debug)]
pub struct SidecarConfig {
    /// Node binary to run (default `node`, resolved via `PATH`).
    pub node_bin: PathBuf,
    /// Path to the sidecar entry script (`sidecar/index.js`). Defaults to the
    /// `NYD_OFFICE365_SIDECAR` env var, else a relative fallback that the
    /// consumer is expected to override.
    pub script: PathBuf,
    /// Per-request timeout for a sidecar round-trip. Generous by default: a
    /// single op drives a real browser (navigation, redirects, capturing the
    /// data-plane request, then replaying it), which is far slower than a REST
    /// call.
    pub request_timeout: Duration,
    /// How many times the sidecar re-drives an interactive sign-in when the
    /// user is too slow and the MFA challenge lapses before they answer. `0`
    /// means try once with no retry. Passed to the sidecar as an env var; the
    /// retry loop lives there (only it can observe the lapse and restart).
    pub mfa_max_retries: u32,
}

impl Default for SidecarConfig {
    fn default() -> Self {
        Self {
            node_bin: PathBuf::from("node"),
            script: std::env::var_os("NYD_OFFICE365_SIDECAR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("office365-web-sidecar/index.js")),
            request_timeout: Duration::from_secs(120),
            mfa_max_retries: 2,
        }
    }
}

/// Resolved credentials used to drive the interactive sign-in unattended.
///
/// These are *already-resolved* secret values (from `pass`, a keyring, etc.),
/// not a provider spec — the wrapper stays app-agnostic and never learns where
/// they came from. They are handed to the sidecar over the JSON protocol, never
/// via the environment (the browser child inherits the env, which would leak the
/// password into `/proc/<pid>/environ`).
#[derive(Clone, Default)]
pub struct LoginCredentials {
    /// UPN to type into the email field or match against an account tile.
    pub username: Option<String>,
    /// Password to type into the password field. Never logged or Debug-printed.
    pub password: Option<String>,
}

impl LoginCredentials {
    /// Whether there is anything worth sending to the sidecar.
    pub fn is_empty(&self) -> bool {
        self.username.is_none() && self.password.is_none()
    }
}

impl std::fmt::Debug for LoginCredentials {
    /// Redacts the password so it never reaches a log or panic message.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginCredentials")
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Everything needed to open (or look up) a session.
///
/// `account_key` is the registry key: two configs sharing a key share one
/// browser session. `profile_dir` is the persistent browser profile so a login
/// (and its SSO cookies) survives restarts.
#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub account_key: String,
    pub login_hint: Option<String>,
    pub profile_dir: PathBuf,
    /// Resting display mode: `true` keeps the browser invisible for silent
    /// polls, `false` always shows the window.
    pub headless: bool,
    /// When resting headless, briefly go headed for an interactive sign-in step
    /// (MFA) then drop back to headless. Ignored when `headless` is `false`
    /// (already visible). Defaults to `true` at the config layer.
    pub auto_headed: bool,
    pub start_url: Option<String>,
    /// Resolved credentials for unattended sign-in. `None` (or empty) leaves the
    /// login fully manual — the sidecar opens a headed window as before.
    pub credentials: Option<LoginCredentials>,
    pub sidecar: SidecarConfig,
}

/// Progress of the browser session's out-of-band load, pushed as the sidecar
/// signs in and pages the calendar surface in.
///
/// `fraction` is a best-effort completion estimate in `[0, 1]`, or `None` while
/// the load is running but not yet quantifiable (the interactive sign-in, before
/// month paging starts). `done` marks the terminal push — all data is in. This
/// mirrors the sidecar's `calendarProgress`/`calendarLoaded` events without the
/// wrapper depending on the calendar crate's own progress type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoadStatus {
    pub fraction: Option<f32>,
    pub done: bool,
}

/// What a [`SessionPrompt`] asks of the user. App-agnostic on purpose — the
/// wrapper never learns how a frontend renders it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    /// A bare acknowledgement is enough (number-match MFA: the user approves on
    /// their phone and the browser advances on its own).
    Acknowledge,
    /// A typed value is expected (an OTC code). `secret` masks the input.
    Text { secret: bool },
}

/// A mid-operation request the browser session raises when it needs the user to
/// provide — or acknowledge — something before it can continue (chiefly MFA
/// during an interactive sign-in). Delivered on the stream from
/// [`SessionHandle::take_prompts`]; the consumer collects input and calls one
/// of the answer methods, which writes the reply back to the sidecar.
///
/// App-agnostic: it reports the sidecar's *facts* (what kind of input, any
/// display detail such as the number to match) and owns the reply plumbing,
/// but carries no human prose — phrasing the prompt is the consumer's job.
pub struct SessionPrompt {
    kind: PromptKind,
    detail: Option<String>,
    req_id: String,
    responder: crate::sidecar::PromptResponder,
}

impl SessionPrompt {
    pub(crate) fn new(
        req_id: String,
        kind: PromptKind,
        detail: Option<String>,
        responder: crate::sidecar::PromptResponder,
    ) -> Self {
        Self {
            kind,
            detail,
            req_id,
            responder,
        }
    }

    /// What input is expected.
    pub fn kind(&self) -> PromptKind {
        self.kind
    }

    /// Read-only detail to show the user (e.g. the MFA number), if any.
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// Answer a [`PromptKind::Text`] prompt with the typed value.
    pub async fn answer_text(self, value: String) -> Result<(), MsOfficeError> {
        self.responder
            .respond(&self.req_id, Some(value), false)
            .await
    }

    /// Acknowledge a [`PromptKind::Acknowledge`] prompt (no value).
    pub async fn acknowledge(self) -> Result<(), MsOfficeError> {
        self.responder.respond(&self.req_id, None, false).await
    }

    /// Signal that the user dismissed the prompt without answering.
    pub async fn cancel(self) -> Result<(), MsOfficeError> {
        self.responder.respond(&self.req_id, None, true).await
    }
}

/// Result of a login check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginState {
    /// The session is authenticated and ready.
    LoggedIn,
    /// The session was not authenticated; the sidecar opened a visible window
    /// for the user to complete MFA. Retry the operation after they finish.
    InteractiveLoginOpened,
}

/// A cheap, clonable reference to a live session actor.
#[derive(Clone)]
pub struct SessionHandle {
    pub(crate) inner: Arc<SessionInner>,
}

impl SessionHandle {
    /// Calendar operations on this session.
    pub fn calendar(&self) -> crate::calendar::CalendarApi {
        crate::calendar::CalendarApi::new(self.inner.clone())
    }

    /// Ensure the session is authenticated, opening an interactive login if
    /// not. Idempotent and cheap once logged in.
    pub async fn ensure_logged_in(&self) -> Result<LoginState, MsOfficeError> {
        self.inner.ensure_login().await
    }

    /// Subscribe to the browser session's load-progress stream.
    ///
    /// Fires as the session loads a freshly-authenticated calendar surface: a
    /// fraction-less nudge the instant an interactive sign-in completes (the app
    /// issued its first data-plane fetch), then a [`LoadStatus`] per captured
    /// month as `getCalendarView` pages the window in, then a terminal
    /// `done` push once all data is in. A consumer that showed empty data during
    /// login can listen here and re-fetch the instant the calendar is available,
    /// and surface a progress banner while it pages — instead of waiting for a
    /// periodic poll.
    pub fn subscribe_loaded(&self) -> broadcast::Receiver<LoadStatus> {
        self.inner.loaded_tx.subscribe()
    }

    /// Take the session's mid-operation prompt stream (see [`SessionPrompt`]).
    /// **Single-consumer**: each prompt carries a one-shot reply path, so there
    /// is exactly one receiver. Callable once — later calls (and a session that
    /// raises no prompts) return `None`.
    pub fn take_prompts(&self) -> Option<mpsc::Receiver<SessionPrompt>> {
        self.inner.prompt_rx.lock().unwrap().take()
    }
}

/// The shared, ref-counted session state behind every [`SessionHandle`].
pub(crate) struct SessionInner {
    tx: mpsc::Sender<Command>,
    /// Broadcasts the sidecar's load-progress pushes ([`LoadStatus`]). Held here
    /// (not just inside the actor) so a [`SessionHandle`] can subscribe before
    /// the sidecar — hence the first push — even exists.
    pub(crate) loaded_tx: broadcast::Sender<LoadStatus>,
    /// The single receiver for [`SessionPrompt`]s, parked here until a consumer
    /// takes it via [`SessionHandle::take_prompts`]. `mpsc` (not broadcast)
    /// because each prompt owns a one-shot reply path; `Mutex<Option<…>>` gives
    /// the take-once semantics. The matching sender lives in the actor's event
    /// forwarder.
    prompt_rx: std::sync::Mutex<Option<mpsc::Receiver<SessionPrompt>>>,
    // Aborts the actor task (and thus kills the sidecar) when the last handle
    // to this session is dropped.
    _actor: AbortOnDrop,
}

impl SessionInner {
    /// Start a new session actor + sidecar for `config`.
    pub(crate) fn spawn(config: SessionConfig) -> Arc<Self> {
        let (tx, rx) = mpsc::channel(32);
        let (loaded_tx, _) = broadcast::channel::<LoadStatus>(16);
        let (prompt_tx, prompt_rx) = mpsc::channel::<SessionPrompt>(8);
        let actor = tokio::spawn(actor_loop(config, rx, loaded_tx.clone(), prompt_tx));
        Arc::new(Self {
            tx,
            loaded_tx,
            prompt_rx: std::sync::Mutex::new(Some(prompt_rx)),
            _actor: AbortOnDrop(actor),
        })
    }

    pub(crate) async fn ensure_login(&self) -> Result<LoginState, MsOfficeError> {
        let (resp, rx) = oneshot::channel();
        self.send(Command::EnsureLogin { resp }).await?;
        rx.await
            .map_err(|_| MsOfficeError::Other("session actor dropped the response".into()))?
    }

    pub(crate) async fn get_calendar_view(
        &self,
        range: MsTimeRange,
    ) -> Result<Vec<MsCalEvent>, MsOfficeError> {
        let (resp, rx) = oneshot::channel();
        self.send(Command::GetCalendarView { range, resp }).await?;
        rx.await
            .map_err(|_| MsOfficeError::Other("session actor dropped the response".into()))?
    }

    async fn send(&self, cmd: Command) -> Result<(), MsOfficeError> {
        self.tx
            .send(cmd)
            .await
            .map_err(|_| MsOfficeError::Other("session actor is gone".into()))
    }
}

/// Commands the actor processes, one at a time.
enum Command {
    EnsureLogin {
        resp: oneshot::Sender<Result<LoginState, MsOfficeError>>,
    },
    GetCalendarView {
        range: MsTimeRange,
        resp: oneshot::Sender<Result<Vec<MsCalEvent>, MsOfficeError>>,
    },
}

/// Owns the sidecar and answers commands sequentially. The sidecar is launched
/// lazily on the first command so merely holding a handle costs no browser.
async fn actor_loop(
    config: SessionConfig,
    mut rx: mpsc::Receiver<Command>,
    loaded_tx: broadcast::Sender<LoadStatus>,
    prompt_tx: mpsc::Sender<SessionPrompt>,
) {
    let mut sidecar: Option<Sidecar> = None;
    // Kept alive for the actor's lifetime: forwards the sidecar's unsolicited
    // events onto the session-level `loaded_tx` / `prompt_tx`. Spawned the first
    // time the sidecar is created (see `ensure_sidecar`).
    let mut _fwd: Option<AbortOnDrop> = None;

    while let Some(cmd) = rx.recv().await {
        match cmd {
            Command::EnsureLogin { resp } => {
                let result =
                    match ensure_sidecar(&mut sidecar, &mut _fwd, &config, &loaded_tx, &prompt_tx)
                        .await
                    {
                        Ok(sc) => sc.ensure_login(config.login_hint.as_deref()).await,
                        Err(e) => Err(e),
                    };
                let _ = resp.send(result);
            }
            Command::GetCalendarView { range, resp } => {
                let result =
                    match ensure_sidecar(&mut sidecar, &mut _fwd, &config, &loaded_tx, &prompt_tx)
                        .await
                    {
                        Ok(sc) => sc.get_calendar_view(&range).await,
                        Err(e) => Err(e),
                    };
                let _ = resp.send(result);
            }
        }
    }
}

/// Lazily launch the sidecar, reusing it across commands. On first launch, also
/// start the forwarder that turns the sidecar's [`SidecarEvent`] pushes into
/// session-level `loaded_tx` fires.
async fn ensure_sidecar<'a>(
    slot: &'a mut Option<Sidecar>,
    fwd: &mut Option<AbortOnDrop>,
    config: &SessionConfig,
    loaded_tx: &broadcast::Sender<LoadStatus>,
    prompt_tx: &mpsc::Sender<SessionPrompt>,
) -> Result<&'a mut Sidecar, MsOfficeError> {
    if slot.is_none() {
        let sidecar = Sidecar::launch(config).await?;
        let mut events = sidecar.subscribe_events();
        // A responder cloned up front so the forwarder can hand each prompt its
        // reply path without touching the (actor-borrowed) `Sidecar`.
        let responder = sidecar.prompt_responder();
        let loaded_tx = loaded_tx.clone();
        let prompt_tx = prompt_tx.clone();
        *fwd = Some(AbortOnDrop(tokio::spawn(async move {
            while let Ok(event) = events.recv().await {
                // Translate each sidecar push. A send with no consumer is a
                // harmless no-op (load) / dropped prompt (no interactive
                // frontend) — the raising op then sees a closed reply path.
                match event {
                    SidecarEvent::Progress(fraction) => {
                        let _ = loaded_tx.send(LoadStatus {
                            fraction,
                            done: false,
                        });
                    }
                    SidecarEvent::Loaded => {
                        let _ = loaded_tx.send(LoadStatus {
                            fraction: Some(1.0),
                            done: true,
                        });
                    }
                    SidecarEvent::Prompt(spec) => {
                        let kind = if spec.expects_text {
                            PromptKind::Text {
                                secret: spec.secret,
                            }
                        } else {
                            PromptKind::Acknowledge
                        };
                        let prompt =
                            SessionPrompt::new(spec.req_id, kind, spec.detail, responder.clone());
                        let _ = prompt_tx.send(prompt).await;
                    }
                }
            }
        })));
        *slot = Some(sidecar);
    }
    Ok(slot.as_mut().expect("just populated"))
}

/// Aborts a spawned task on drop.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
