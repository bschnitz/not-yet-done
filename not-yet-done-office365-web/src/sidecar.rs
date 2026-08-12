//! The out-of-process Playwright sidecar and the newline-delimited JSON
//! protocol used to talk to it.
//!
//! Wire format: one JSON object per line, both ways.
//!
//! ```text
//! → {"id":1,"op":"ensureLogin","params":{"loginHint":"…"}}
//! ← {"id":1,"ok":true,"result":{"state":"loggedIn"}}
//! → {"id":2,"op":"getCalendarView","params":{"start":"…","end":"…"}}
//! ← {"id":2,"pending":true}          // heartbeat — still working, not the answer
//! ← {"id":2,"ok":true,"result":{"events":[ … ]}}
//! ← {"id":1,"ok":false,"error":{"kind":"loginRequired","message":"…"}}
//! ```
//!
//! A stdout line is one of three things, demultiplexed by a single background
//! **reader task** that owns the pipe:
//!
//! - a *terminal* response `{"id":N,"ok":…}` — routed to whichever in-flight
//!   request registered id `N`;
//! - a `{"id":N,"pending":true}` heartbeat — the sidecar is still working on an
//!   operation with no natural time bound (an interactive sign-in); routed to
//!   request `N` to reset its idle timeout;
//! - an unsolicited event `{"event":"…"}` with no id — a push the sidecar makes
//!   on its own (e.g. `calendarLoaded` the instant the app's authenticated
//!   calendar fetch is captured). Because the reader runs continuously, such a
//!   push is delivered even when no request is in flight — that is the whole
//!   point of the reader: the old inline "read the next line inside `request`"
//!   scheme could only ever see a line while a request was pending.
//!
//! The sidecar MUST keep stdout pure JSONL and send all logging to stderr.

use std::collections::HashMap;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::dto::{MsCalEvent, MsTimeRange};
use crate::error::MsOfficeError;
use crate::session::{LoginCredentials, LoginState, SessionConfig};

/// An unsolicited push from the sidecar, not tied to any request. The wrapper
/// surfaces these so a consumer can react to browser-side state changes that
/// happen out of band (chiefly: the calendar loading and becoming ready, or a
/// mid-operation request for user input).
///
/// (No `Copy`/`Eq`: a fraction is an `f32` — only `PartialEq` — and a prompt
/// carries owned `String`s.)
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SidecarEvent {
    /// Intermediate progress of a genuine (re)load. `Some(f)` is a best-effort
    /// completion fraction in `[0, 1]` (one per captured month as the window
    /// pages in); `None` is a fraction-less nudge that the authenticated surface
    /// is loading (a sign-in just completed). Not terminal.
    Progress(Option<f32>),
    /// The app's authenticated calendar load settled — all data is in and a
    /// fetch would now succeed. Terminal: emitted once per load cycle, at the
    /// end of the paging pass.
    Loaded,
    /// The sidecar reached a point where it needs the user to provide — or
    /// merely acknowledge — something before it can continue (chiefly an MFA
    /// step during an interactive sign-in). The consumer collects the input and
    /// replies with [`PromptResponder::respond`], correlated by
    /// [`PromptSpec::req_id`].
    Prompt(PromptSpec),
}

/// The payload of a `promptRequest` push. The sidecar reports the *facts* it
/// knows — a correlation id, whether it wants a typed value or a bare
/// acknowledgement, and any display detail it scraped (e.g. the MFA number to
/// match). How to phrase the prompt to the user is the *consumer's* decision
/// (from config), not the sidecar's, so no human prose travels on this seam.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PromptSpec {
    /// Correlation id echoed back in the `provideInput` reply so the sidecar
    /// resolves the right pending prompt.
    pub req_id: String,
    /// `true` when the sidecar expects a typed value (an OTC code), `false`
    /// when a bare acknowledgement is enough (number-match: the user approves
    /// on their phone; the browser advances on its own).
    pub expects_text: bool,
    /// Hint that a typed value is a secret (mask it). Meaningless when
    /// `expects_text` is false.
    pub secret: bool,
    /// Read-only detail the sidecar scraped for display — e.g. the MFA number
    /// to match. `None` when there is nothing to show beyond the prompt.
    pub detail: Option<String>,
}

/// A frame the reader routes to a single in-flight request by id.
enum Frame {
    /// A `pending:true` heartbeat — keep waiting, reset the idle timeout.
    Heartbeat,
    /// The terminal answer for this request.
    Terminal(TerminalResp),
    /// The sidecar's stdout closed (EOF/error) before this request finished.
    Closed,
}

/// The payload of a terminal response line, already demuxed to its request.
struct TerminalResp {
    ok: bool,
    result: Option<Value>,
    error: Option<SidecarError>,
}

/// Per-request inbox: the reader pushes [`Frame`]s here keyed by request id.
type Pending = Arc<Mutex<HashMap<u64, mpsc::UnboundedSender<Frame>>>>;

/// A running sidecar process and its piped stdio.
///
/// Its lifecycle is engineered so the browser can **never outlive this
/// process**: the sidecar runs in its own process group, [`Drop`] signals the
/// whole group (node *and* its browser grandchildren), and a parent-death pipe
/// gives node an EOF even on a hard `SIGKILL`/crash of the app. See [`Drop`].
pub(crate) struct Sidecar {
    // Kept so the child is killed (and reaped) on drop (via `kill_on_drop`).
    _child: Child,
    /// Behind an async mutex + `Arc` so the in-flight-request path (which holds
    /// `&mut Sidecar`) is **not** the only writer: a [`PromptResponder`] clones
    /// this handle and writes a `provideInput` reply concurrently, while the
    /// actor is still parked awaiting the response to the very request that
    /// raised the prompt (an interactive sign-in). `request` only holds the
    /// lock for the brief write, never across its response await, so the two
    /// writers never deadlock.
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
    next_id: u64,
    request_timeout: std::time::Duration,
    /// In-flight requests, keyed by id; the reader routes frames here.
    pending: Pending,
    /// Unsolicited sidecar pushes (see [`SidecarEvent`]). A `broadcast` so any
    /// number of consumers can watch, and so a send with no listener is a
    /// cheap no-op.
    events_tx: broadcast::Sender<SidecarEvent>,
    /// Aborts the stdout reader task when the sidecar is dropped. (Dropping the
    /// child closes stdout and the reader would end on its own EOF anyway; this
    /// is the belt to that suspenders.)
    _reader: AbortOnDrop,
    /// The sidecar's process-group id (== the node pid, since we launch it as
    /// its own group leader). Signalling `-pgid` reaches node and every browser
    /// process it spawned in one shot, so none is orphaned on teardown.
    pgid: Option<i32>,
    /// Write end of the parent-death pipe. This process holds the only copy
    /// (it's close-on-exec, so the browser never inherits it). Kept alive for
    /// the sidecar's lifetime; when it drops — or the app dies for any reason —
    /// the OS closes it and node observes EOF and shuts the browser down.
    _death_pipe_write: OwnedFd,
}

impl Sidecar {
    /// Spawn the Node sidecar for `config`. Session-scoped configuration is
    /// passed via env so the protocol stays about operations, not setup.
    pub(crate) async fn launch(config: &SessionConfig) -> Result<Self, MsOfficeError> {
        std::fs::create_dir_all(&config.profile_dir).ok();

        // A browser killed hard (SIGKILL / crash) leaves a dangling profile
        // lock that blocks the next `launchPersistentContext`. If it belongs to
        // a dead pid, it's stale — clear it so we can relaunch. See the reaper.
        reap_stale_singleton_lock(&config.profile_dir);

        // The sidecar logs to stderr. Route it to a file inside the profile
        // dir — inheriting it would corrupt a TUI's alternate screen, and a
        // file lets the user `tail -f` it. Best-effort: fall back to a null
        // sink if the log can't be opened.
        let log_path = config.profile_dir.join("sidecar.log");
        let stderr = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map(std::process::Stdio::from)
            .unwrap_or_else(|_| std::process::Stdio::null());

        // Parent-death pipe: node inherits the read end and watches it for EOF;
        // we keep the (close-on-exec) write end so the browser never inherits
        // it. When this process dies for ANY reason the OS closes the write end
        // and node tears the browser down — our hardest lifetime guarantee.
        let (read_fd, death_pipe_write) = make_death_pipe()?;

        let mut cmd = Command::new(&config.sidecar.node_bin);
        cmd.arg(&config.sidecar.script)
            .env("NYD_O365_PROFILE_DIR", &config.profile_dir)
            .env("NYD_O365_HEADLESS", if config.headless { "1" } else { "0" })
            .env(
                "NYD_O365_AUTO_HEADED",
                if config.auto_headed { "1" } else { "0" },
            )
            .env("NYD_O365_PARENT_PIPE_FD", read_fd.to_string())
            .env(
                "NYD_O365_MFA_MAX_RETRIES",
                config.sidecar.mfa_max_retries.to_string(),
            )
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(stderr)
            // Own process group so teardown can signal node and every browser
            // process it spawned together, leaving nothing orphaned.
            .process_group(0)
            .kill_on_drop(true);
        if let Some(hint) = &config.login_hint {
            cmd.env("NYD_O365_LOGIN_HINT", hint);
        }
        if let Some(url) = &config.start_url {
            cmd.env("NYD_O365_START_URL", url);
        }

        let spawn_result = cmd.spawn();
        // The read end now belongs to the child; drop our copy either way so we
        // don't leak it (and so node holds the only read end → EOF works).
        unsafe { libc::close(read_fd) };
        let mut child =
            spawn_result.map_err(|e| MsOfficeError::Sidecar(format!("spawn sidecar: {e}")))?;

        // pid == pgid because the child is its own group leader.
        let pgid = child.id().map(|id| id as i32);
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| MsOfficeError::Sidecar("sidecar has no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| MsOfficeError::Sidecar("sidecar has no stdout".into()))?;

        // Demux stdout in a dedicated task: it owns the pipe and routes every
        // line to the right place (a waiting request, or the event stream). A
        // small broadcast buffer is plenty — events are rare and coalesced.
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (events_tx, _) = broadcast::channel(16);
        let reader = tokio::spawn(reader_loop(
            BufReader::new(stdout),
            Arc::clone(&pending),
            events_tx.clone(),
        ));

        let mut sidecar = Self {
            _child: child,
            stdin: Arc::new(tokio::sync::Mutex::new(stdin)),
            next_id: 0,
            request_timeout: config.sidecar.request_timeout,
            pending,
            events_tx,
            _reader: AbortOnDrop(reader),
            pgid,
            _death_pipe_write: death_pipe_write,
        };

        // Hand the resolved credentials to the sidecar over the protocol (never
        // via env — the browser child inherits env and would leak the password).
        // Sent once, up front, so both `ensureLogin` and the login triggered
        // lazily by `getCalendarView` can drive the sign-in unattended.
        if let Some(creds) = &config.credentials {
            if !creds.is_empty() {
                sidecar.configure(creds).await?;
            }
        }

        Ok(sidecar)
    }

    /// Push the resolved credentials to the sidecar. Idempotent on the sidecar
    /// side; only the fields present are set.
    async fn configure(&mut self, creds: &LoginCredentials) -> Result<(), MsOfficeError> {
        self.request(
            "configure",
            json!({
                "username": creds.username,
                "password": creds.password,
            }),
        )
        .await
        .map(|_| ())
    }

    pub(crate) async fn ensure_login(
        &mut self,
        login_hint: Option<&str>,
    ) -> Result<LoginState, MsOfficeError> {
        let result = self
            .request("ensureLogin", json!({ "loginHint": login_hint }))
            .await?;
        match result.get("state").and_then(Value::as_str) {
            Some("loggedIn") => Ok(LoginState::LoggedIn),
            Some("interactiveLoginOpened") => Ok(LoginState::InteractiveLoginOpened),
            other => Err(MsOfficeError::Protocol(format!(
                "unexpected login state {other:?}"
            ))),
        }
    }

    pub(crate) async fn get_calendar_view(
        &mut self,
        range: &MsTimeRange,
    ) -> Result<Vec<MsCalEvent>, MsOfficeError> {
        let result = self
            .request(
                "getCalendarView",
                json!({
                    "start": range.start.to_rfc3339(),
                    "end": range.end.to_rfc3339(),
                }),
            )
            .await?;
        let events = result
            .get("events")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        serde_json::from_value(events)
            .map_err(|e| MsOfficeError::Protocol(format!("bad calendar events: {e}")))
    }

    /// Subscribe to the sidecar's unsolicited event stream.
    pub(crate) fn subscribe_events(&self) -> broadcast::Receiver<SidecarEvent> {
        self.events_tx.subscribe()
    }

    /// A cheap, clonable handle that can write a `provideInput` reply to a
    /// [`SidecarEvent::Prompt`] **without** the `&mut Sidecar` the actor is
    /// holding while parked on the request that raised the prompt. It shares
    /// the same `stdin` mutex, so its write is serialised against ordinary
    /// requests but does not depend on the actor's borrow.
    pub(crate) fn prompt_responder(&self) -> PromptResponder {
        PromptResponder {
            stdin: Arc::clone(&self.stdin),
        }
    }

    /// Send one request and await its terminal response.
    ///
    /// The frames for this request arrive on a private inbox filled by the
    /// reader task (routed by id), so a concurrent event push on the same stdout
    /// never derails us. The timeout guards each frame wait, so it only fires
    /// when the sidecar goes *silent* — not merely slow. An operation with no
    /// natural time bound (an interactive sign-in the user drives at their own
    /// pace, MFA included) keeps the request alive with periodic
    /// `{"id":N,"pending":true}` heartbeats; each resets the idle window. There
    /// is thus no wall-clock limit on a sign-in, while a dead sidecar still
    /// times out.
    async fn request(&mut self, op: &str, params: Value) -> Result<Value, MsOfficeError> {
        self.next_id += 1;
        let id = self.next_id;

        // Register the inbox *before* writing, so a fast response can't arrive
        // before the reader knows where to route it.
        let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();
        self.pending.lock().unwrap().insert(id, tx);

        let write = async {
            let mut line = serde_json::to_string(&json!({ "id": id, "op": op, "params": params }))
                .map_err(|e| MsOfficeError::Protocol(e.to_string()))?;
            line.push('\n');
            // Hold the stdin lock only for the write itself — never across the
            // response await below — so a concurrent `provideInput` reply can
            // interleave (see `PromptResponder`).
            let mut w = self.stdin.lock().await;
            w.write_all(line.as_bytes())
                .await
                .map_err(|e| MsOfficeError::Sidecar(format!("write: {e}")))?;
            w.flush()
                .await
                .map_err(|e| MsOfficeError::Sidecar(format!("flush: {e}")))
        };
        if let Err(e) = write.await {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }

        let outcome = loop {
            match tokio::time::timeout(self.request_timeout, rx.recv()).await {
                Err(_) => break Err(MsOfficeError::Timeout),
                Ok(None) => break Err(MsOfficeError::Sidecar("reader task gone".into())),
                Ok(Some(Frame::Heartbeat)) => continue,
                Ok(Some(Frame::Closed)) => {
                    break Err(MsOfficeError::Sidecar("sidecar closed its output".into()));
                }
                Ok(Some(Frame::Terminal(resp))) => break Ok(resp),
            }
        };
        self.pending.lock().unwrap().remove(&id);

        let resp = outcome?;
        if resp.ok {
            Ok(resp.result.unwrap_or(Value::Null))
        } else {
            let err = resp.error.unwrap_or(SidecarError {
                kind: "unknown".into(),
                message: String::new(),
            });
            if err.kind == "loginRequired" {
                Err(MsOfficeError::LoginRequired)
            } else {
                Err(MsOfficeError::Sidecar(format!(
                    "{}: {}",
                    err.kind, err.message
                )))
            }
        }
    }
}

/// Writes `provideInput` replies to the sidecar out of band — the answer path
/// for a [`SidecarEvent::Prompt`]. Cloned from a [`Sidecar`] (shares its stdin
/// mutex) and handed to the consumer alongside the prompt, so the reply can be
/// sent from the task that collected the user's input, independently of the
/// actor still awaiting the prompting request.
#[derive(Clone)]
pub(crate) struct PromptResponder {
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
}

impl PromptResponder {
    /// Reply to the prompt correlated by `req_id`. `value` carries a typed
    /// answer (for a text prompt); `cancelled` marks that the user dismissed
    /// the prompt without answering. A bare acknowledgement is `value: None,
    /// cancelled: false`. Fire-and-forget on the wire (no `id`): the sidecar
    /// consumes it to resolve its pending prompt, there is no response to await.
    pub(crate) async fn respond(
        &self,
        req_id: &str,
        value: Option<String>,
        cancelled: bool,
    ) -> Result<(), MsOfficeError> {
        let mut line = serde_json::to_string(&json!({
            "op": "provideInput",
            "reqId": req_id,
            "value": value,
            "cancelled": cancelled,
        }))
        .map_err(|e| MsOfficeError::Protocol(e.to_string()))?;
        line.push('\n');
        let mut w = self.stdin.lock().await;
        w.write_all(line.as_bytes())
            .await
            .map_err(|e| MsOfficeError::Sidecar(format!("write provideInput: {e}")))?;
        w.flush()
            .await
            .map_err(|e| MsOfficeError::Sidecar(format!("flush provideInput: {e}")))?;
        Ok(())
    }
}

/// Owns the sidecar's stdout and demultiplexes every line: terminal responses
/// and heartbeats are routed to the matching in-flight request's inbox; an
/// unsolicited `{"event":…}` push goes to the broadcast event stream. Runs for
/// the sidecar's lifetime and ends when stdout closes (EOF), failing any still-
/// waiting request so it doesn't hang.
async fn reader_loop<R: AsyncBufReadExt + Unpin>(
    mut stdout: R,
    pending: Pending,
    events_tx: broadcast::Sender<SidecarEvent>,
) {
    loop {
        let mut buf = String::new();
        match stdout.read_line(&mut buf).await {
            Ok(0) | Err(_) => {
                // EOF or a read error: nothing more will come. Wake every waiter
                // with `Closed` so no request blocks forever, then stop.
                let mut map = pending.lock().unwrap();
                for (_, tx) in map.drain() {
                    let _ = tx.send(Frame::Closed);
                }
                return;
            }
            Ok(_) => {
                let line = buf.trim();
                if line.is_empty() {
                    continue;
                }
                match classify(line) {
                    ParsedLine::Event(ev) => {
                        let _ = events_tx.send(ev);
                    }
                    ParsedLine::Heartbeat(id) => {
                        route(&pending, id, Frame::Heartbeat);
                    }
                    ParsedLine::Terminal(id, resp) => {
                        route(&pending, id, Frame::Terminal(resp));
                    }
                    // A line we can't place (malformed, or an event kind we don't
                    // model). Not tied to any request, so there is nothing to fail
                    // — drop it rather than derail an unrelated request.
                    ParsedLine::Ignore => {}
                }
            }
        }
    }
}

/// Deliver a frame to request `id`'s inbox, if it is still waiting.
fn route(pending: &Pending, id: u64, frame: Frame) {
    if let Some(tx) = pending.lock().unwrap().get(&id) {
        let _ = tx.send(frame);
    }
}

/// What a single stdout line is, after parsing.
enum ParsedLine {
    Event(SidecarEvent),
    Heartbeat(u64),
    Terminal(u64, TerminalResp),
    Ignore,
}

/// Classify one JSONL line from the sidecar. Pure (no I/O) so it is unit-tested
/// directly. Precedence: an `event` line (no id) is a push; an `id` line is a
/// heartbeat when `pending` else a terminal response; anything else is ignored.
fn classify(line: &str) -> ParsedLine {
    let Ok(raw) = serde_json::from_str::<RawLine>(line) else {
        return ParsedLine::Ignore;
    };
    if let Some(event) = raw.event.as_deref() {
        return match event {
            "calendarLoaded" => ParsedLine::Event(SidecarEvent::Loaded),
            // `fraction` absent/null → a fraction-less "loading" nudge; a number
            // is a per-month completion estimate. Narrow to f32 for the seam.
            "calendarProgress" => {
                ParsedLine::Event(SidecarEvent::Progress(raw.fraction.map(|f| f as f32)))
            }
            // A mid-operation request for user input. Needs a correlation id to
            // be answerable; without one it is unroutable, so drop it.
            "promptRequest" => match raw.req_id {
                Some(req_id) => ParsedLine::Event(SidecarEvent::Prompt(PromptSpec {
                    req_id,
                    expects_text: raw.kind.as_deref() == Some("text"),
                    secret: raw.secret.unwrap_or(false),
                    detail: raw.detail,
                })),
                None => ParsedLine::Ignore,
            },
            _ => ParsedLine::Ignore,
        };
    }
    let Some(id) = raw.id else {
        return ParsedLine::Ignore;
    };
    if raw.pending == Some(true) {
        return ParsedLine::Heartbeat(id);
    }
    match raw.ok {
        Some(ok) => ParsedLine::Terminal(
            id,
            TerminalResp {
                ok,
                result: raw.result,
                error: raw.error,
            },
        ),
        None => ParsedLine::Ignore,
    }
}

/// Aborts a spawned task on drop (the stdout reader).
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Drop for Sidecar {
    /// Guarantee the browser never outlives us. We signal the whole process
    /// group: `SIGTERM` lets node close the browser cleanly (which removes the
    /// profile's `SingletonLock`), and because it targets the group it reaches
    /// the browser processes directly too — nothing is left orphaned holding
    /// the profile lock. A detached thread hard-kills the group shortly after
    /// as a fallback, so this `Drop` never blocks the async runtime. The
    /// `kill_on_drop` child and the death-pipe write end (dropped right after)
    /// are redundant nets on top of this.
    fn drop(&mut self) {
        if let Some(pgid) = self.pgid {
            if pgid > 1 {
                unsafe { libc::kill(-pgid, libc::SIGTERM) };
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    unsafe { libc::kill(-pgid, libc::SIGKILL) };
                });
            }
        }
    }
}

/// Create the parent-death pipe. Returns the raw read fd (left inheritable so
/// node keeps it across `exec`) and the owned write end (set close-on-exec so
/// only this process holds it — the browser must not, or EOF would never fire).
fn make_death_pipe() -> Result<(RawFd, OwnedFd), MsOfficeError> {
    let mut fds = [0 as libc::c_int; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(MsOfficeError::Sidecar(format!(
            "death pipe: {}",
            std::io::Error::last_os_error()
        )));
    }
    let (read_fd, write_fd) = (fds[0], fds[1]);
    unsafe { libc::fcntl(write_fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    let write_owned = unsafe { OwnedFd::from_raw_fd(write_fd) };
    Ok((read_fd, write_owned))
}

/// Clear a stale Chromium profile lock so a fresh launch isn't blocked.
///
/// Chromium writes `SingletonLock` as a symlink named `<hostname>-<pid>` inside
/// the user-data-dir and removes it on a clean shutdown. A hard kill (SIGKILL /
/// crash) leaves it dangling, and the next `launchPersistentContext` then fails
/// with "browser has been closed". If the referenced pid is no longer alive the
/// lock is stale, so we remove it (with its Cookie/Socket siblings). This
/// assumes a machine-local profile dir (our case): staleness is judged purely
/// by pid liveness, which is also how the app can tell a browser is still up.
fn reap_stale_singleton_lock(profile_dir: &Path) {
    let Ok(target) = std::fs::read_link(profile_dir.join("SingletonLock")) else {
        return; // no lock (or not a symlink) → nothing to do
    };
    let Some(pid) = target
        .to_string_lossy()
        .rsplit('-')
        .next()
        .and_then(|s| s.parse::<i32>().ok())
    else {
        return; // unrecognised format → leave it alone
    };
    if pid <= 1 {
        return;
    }
    let alive = unsafe { libc::kill(pid, 0) } == 0
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    if alive {
        return;
    }
    for name in ["SingletonLock", "SingletonCookie", "SingletonSocket"] {
        let _ = std::fs::remove_file(profile_dir.join(name));
    }
}

/// One stdout line, parsed permissively so a single `serde` pass can tell apart
/// a terminal response (`ok`), a heartbeat (`pending`), and an unsolicited push
/// (`event`, no `id`). Every field is optional; [`classify`] applies precedence.
#[derive(Deserialize)]
struct RawLine {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    pending: Option<bool>,
    /// Present on an unsolicited push line (`{"event":"calendarLoaded"}`).
    #[serde(default)]
    event: Option<String>,
    /// Completion fraction on a `{"event":"calendarProgress","fraction":…}`
    /// push; absent (or null) means "loading, no estimate yet".
    #[serde(default)]
    fraction: Option<f64>,
    /// Correlation id on a `{"event":"promptRequest","reqId":…}` push.
    #[serde(default, rename = "reqId")]
    req_id: Option<String>,
    /// `"text"` vs `"acknowledge"` on a `promptRequest` push.
    #[serde(default)]
    kind: Option<String>,
    /// Whether a typed value is secret, on a `promptRequest` push.
    #[serde(default)]
    secret: Option<bool>,
    /// Display detail (e.g. the MFA number) on a `promptRequest` push.
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    ok: Option<bool>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<SidecarError>,
}

#[derive(Deserialize)]
struct SidecarError {
    kind: String,
    #[serde(default)]
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_unsolicited_event() {
        assert!(matches!(
            classify(r#"{"event":"calendarLoaded"}"#),
            ParsedLine::Event(SidecarEvent::Loaded)
        ));
    }

    #[test]
    fn classifies_progress_event_with_and_without_fraction() {
        // A fraction present → a numeric progress tick.
        match classify(r#"{"event":"calendarProgress","fraction":0.5}"#) {
            ParsedLine::Event(SidecarEvent::Progress(Some(f))) => {
                assert!((f - 0.5).abs() < 1e-6);
            }
            _ => panic!("expected Progress(Some(0.5))"),
        }
        // No fraction (or null) → a fraction-less "loading" nudge.
        assert!(matches!(
            classify(r#"{"event":"calendarProgress"}"#),
            ParsedLine::Event(SidecarEvent::Progress(None))
        ));
        assert!(matches!(
            classify(r#"{"event":"calendarProgress","fraction":null}"#),
            ParsedLine::Event(SidecarEvent::Progress(None))
        ));
    }

    #[test]
    fn unknown_event_kind_is_ignored() {
        assert!(matches!(
            classify(r#"{"event":"somethingElse"}"#),
            ParsedLine::Ignore
        ));
    }

    #[test]
    fn classifies_heartbeat_by_id() {
        assert!(matches!(
            classify(r#"{"id":7,"pending":true}"#),
            ParsedLine::Heartbeat(7)
        ));
    }

    #[test]
    fn classifies_terminal_ok_and_error() {
        match classify(r#"{"id":3,"ok":true,"result":{"events":[]}}"#) {
            ParsedLine::Terminal(3, resp) => {
                assert!(resp.ok);
                assert!(resp.result.is_some());
            }
            _ => panic!("expected terminal"),
        }
        match classify(r#"{"id":4,"ok":false,"error":{"kind":"loginRequired","message":"x"}}"#) {
            ParsedLine::Terminal(4, resp) => {
                assert!(!resp.ok);
                assert_eq!(resp.error.unwrap().kind, "loginRequired");
            }
            _ => panic!("expected terminal"),
        }
    }

    #[test]
    fn malformed_and_idless_lines_are_ignored() {
        assert!(matches!(classify("not json at all"), ParsedLine::Ignore));
        assert!(matches!(classify(r#"{"foo":"bar"}"#), ParsedLine::Ignore));
    }

    /// End-to-end demux over an in-memory stdout: a heartbeat and a terminal for
    /// an in-flight request are routed to its inbox in order, an unsolicited
    /// event reaches the broadcast stream, and EOF fails any still-waiting
    /// request with `Closed`.
    #[tokio::test]
    async fn reader_demuxes_responses_events_and_eof() {
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (events_tx, mut events_rx) = broadcast::channel(16);

        // Two in-flight requests: id 1 will get a heartbeat + terminal; id 2
        // will get nothing and must be woken with `Closed` on EOF.
        let (tx1, mut rx1) = mpsc::unbounded_channel::<Frame>();
        let (tx2, mut rx2) = mpsc::unbounded_channel::<Frame>();
        pending.lock().unwrap().insert(1, tx1);
        pending.lock().unwrap().insert(2, tx2);

        let script = concat!(
            r#"{"event":"calendarLoaded"}"#,
            "\n",
            r#"{"id":1,"pending":true}"#,
            "\n",
            r#"{"id":1,"ok":true,"result":{"events":[]}}"#,
            "\n",
        );
        reader_loop(
            BufReader::new(script.as_bytes()),
            Arc::clone(&pending),
            events_tx,
        )
        .await;

        assert_eq!(events_rx.recv().await.unwrap(), SidecarEvent::Loaded);
        assert!(matches!(rx1.recv().await, Some(Frame::Heartbeat)));
        assert!(matches!(
            rx1.recv().await,
            Some(Frame::Terminal(TerminalResp { ok: true, .. }))
        ));
        // EOF reached (script ended): the untouched request 2 was woken Closed.
        assert!(matches!(rx2.recv().await, Some(Frame::Closed)));
        // And the reader drained the pending map on the way out.
        assert!(pending.lock().unwrap().is_empty());
    }
}
