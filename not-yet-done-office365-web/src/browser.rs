//! The browser a session drives, and the socket it is driven over.
//!
//! A session starts one `drunken-browser` per account — hidden while the session rests
//! headless, its profile in the session's `profile_dir` — and talks to its control socket:
//! one JSON object per line, both ways. A line this side sends is an *ask* of the window
//! (open this flow, run it, answer what it asks, show yourself), and every ask is answered
//! under its id. What the window says on its own while a run is going arrives with no id at
//! all: `news`. The wire is drunken-browser's documented socket (`docs/reference/window-asks.md`
//! over there); nothing here is private to it.
//!
//! ```text
//! → {"id":1,"host":{"ask":"line","line":"test-open nyd-calendar"}}
//! ← {"message":"answered","id":1,"answer":{"answer":"said","said":"opened nyd-calendar"}}
//! → {"id":3,"host":{"ask":"flow_run","what":"whole","data":{"from":"…","until":"…"}}}
//! ← {"message":"news","news":{"news":"started","flow":"nyd-calendar"}}
//! ← {"message":"news","news":{"news":"told","said":"Tap 42 on your phone to sign in"}}
//! ← {"message":"news","news":{"news":"over","tally":{"ran":3,"failed":0,"broke":0},
//!                              "stopped":false,"yielded":{"calendar":{"events":[…]}}}}
//! ```
//!
//! One background **reader task** owns the socket's read half and demultiplexes it: an
//! answer goes to whichever ask registered its id, a piece of news goes onto a broadcast
//! stream that any number of listeners watch. The reader runs for the browser's lifetime, so
//! news is heard whether or not an ask is in flight — a `told` lands while `run` is parked on
//! the same run, and the session's forwarder still relays it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;

use crate::error::MsOfficeError;
use crate::session::SessionConfig;

/// How long the window gets to answer one ask. The window answers on its own turn and gives
/// up on an ask after five seconds itself, so anything past that is a socket that has gone
/// quiet, not a window that is busy.
const ASK_TIMEOUT: Duration = Duration::from_secs(15);

/// How long a freshly started browser gets to open its socket.
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// How long, after `flow_run` was answered, the run has to actually start. A run that the
/// pre-flight check refused — a value nobody can produce — answers with the refusal and
/// starts nothing, and this is how that is told apart from a slow one.
const START_OF_RUN: Duration = Duration::from_secs(10);

/// One thing the window said on its own, typed as far as the session cares.
///
/// The `did` announcements (one per line of the flow) are not carried: a session wants to
/// know how far a run is by steps, what it says, what it asks, and how it ended.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Happening {
    /// A run of the named flow began.
    Started { flow: String },
    /// Step `step` of the run began.
    Began { step: usize },
    /// Step `step` of the run is over.
    Ended { step: usize },
    /// The run is waiting on a person for this value, as the hole is written —
    /// `{{secret.otp}}`. Answered with `flow_answer` (see [`Link::answer`]).
    Asking { hole: String },
    /// The run said this to whoever attends it — a flow's `tell:` line, rendered. Nothing
    /// to answer.
    Told { said: String },
    /// A line of the run failed or broke, and this is the sentence it ended on: the action
    /// as the flow reads it, and why. Kept for the error a failed run turns into.
    Trouble { said: String },
    /// The run is over. `yielded` is what the flow's `yields:` named, each under its name.
    Over {
        tally: Tally,
        stopped: bool,
        yielded: BTreeMap<String, Value>,
    },
}

/// The count a run ends on: steps that ran, and of those, how many failed a claim or broke.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct Tally {
    #[serde(default)]
    pub ran: usize,
    #[serde(default)]
    pub failed: usize,
    #[serde(default)]
    pub broke: usize,
}

impl Tally {
    /// Whether every step that ran got where it was going.
    pub fn clean(&self) -> bool {
        self.failed == 0 && self.broke == 0
    }
}

/// What a run handed back, once it is over.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Run {
    pub tally: Tally,
    pub stopped: bool,
    pub yielded: BTreeMap<String, Value>,
    /// The last sentence the run said about a line that failed or broke, for the error a
    /// failed run turns into. `None` on a clean run.
    pub trouble: Option<String>,
}

/// The answer to one ask, routed to it by the reader.
enum Outcome {
    /// The window answered; this is the `answer` (or a command's `reply`).
    Answered(Value),
    /// The window said why it could not.
    Failed(String),
    /// The socket closed before an answer came.
    Closed,
}

/// What the reader task and the asks share: the inboxes of the asks in flight, and the
/// news stream.
struct Shared {
    pending: Mutex<HashMap<u64, oneshot::Sender<Outcome>>>,
    news_tx: broadcast::Sender<Happening>,
}

/// A connection to the browser's control socket: asks go in, answers and news come out.
///
/// Cheap to share (`Arc`), because two parties write to it at once: the session actor parked
/// on a run, and whoever answers a prompt that run raised. The write half is behind an async
/// mutex held only for the length of one line, never across an answer.
pub(crate) struct Link {
    writer: tokio::sync::Mutex<OwnedWriteHalf>,
    next_id: AtomicU64,
    shared: Arc<Shared>,
    _reader: AbortOnDrop,
}

impl Link {
    /// Connect to the socket at `path`.
    pub(crate) async fn connect(path: &Path) -> std::io::Result<Arc<Self>> {
        let stream = UnixStream::connect(path).await?;
        let (read, write) = stream.into_split();
        // Room for a whole run's worth of news between two turns of a slow listener; a
        // listener that falls further behind than this hears `Lagged` and says so.
        let (news_tx, _) = broadcast::channel(256);
        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
            news_tx,
        });
        let reader = tokio::spawn(reader_loop(BufReader::new(read), Arc::clone(&shared)));
        Ok(Arc::new(Self {
            writer: tokio::sync::Mutex::new(write),
            next_id: AtomicU64::new(0),
            shared,
            _reader: AbortOnDrop(reader),
        }))
    }

    /// Everything the window says on its own, from now on.
    pub(crate) fn news(&self) -> broadcast::Receiver<Happening> {
        self.shared.news_tx.subscribe()
    }

    /// Ask the window something and wait for its answer.
    pub(crate) async fn ask(&self, host: Value) -> Result<Value, MsOfficeError> {
        self.send(json!({ "host": host })).await
    }

    /// Answer what the run is asking for — or refuse it with `None`.
    pub(crate) async fn answer(&self, value: Option<String>) -> Result<(), MsOfficeError> {
        self.ask(json!({ "ask": "flow_answer", "value": value }))
            .await
            .map(|_| ())
    }

    /// Put the window up (`true`) or take it down (`false`).
    pub(crate) async fn show(&self, on: bool) -> Result<(), MsOfficeError> {
        self.ask(json!({ "ask": "show", "on": on }))
            .await
            .map(|_| ())
    }

    /// Whether the window is on screen. Asked of the window rather than remembered here:
    /// the window is the one that knows, and a browser this side adopted was put up or
    /// taken down by somebody else.
    pub(crate) async fn shown(&self) -> Result<bool, MsOfficeError> {
        let answer = self.ask(json!({ "ask": "show" })).await?;
        answer["shown"].as_bool().ok_or_else(|| {
            MsOfficeError::Browser(format!(
                "the window did not say whether it is shown: {answer}"
            ))
        })
    }

    /// Take the window down if it is up, put it up if it is down. Returns what it is now.
    pub(crate) async fn toggle(&self) -> Result<bool, MsOfficeError> {
        let on = !self.shown().await?;
        self.show(on).await?;
        Ok(on)
    }

    async fn send(&self, mut line: Value) -> Result<Value, MsOfficeError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        line["id"] = json!(id);
        // Register the inbox before writing, so a fast answer cannot arrive before the
        // reader knows where it goes.
        let (tx, rx) = oneshot::channel();
        self.shared.pending.lock().unwrap().insert(id, tx);

        let mut text = line.to_string();
        text.push('\n');
        let written = async {
            let mut w = self.writer.lock().await;
            w.write_all(text.as_bytes()).await?;
            w.flush().await
        }
        .await;
        if let Err(e) = written {
            self.shared.pending.lock().unwrap().remove(&id);
            return Err(MsOfficeError::Browser(format!("write to the browser: {e}")));
        }

        let outcome = match tokio::time::timeout(ASK_TIMEOUT, rx).await {
            Err(_) => {
                self.shared.pending.lock().unwrap().remove(&id);
                return Err(MsOfficeError::Timeout);
            }
            Ok(Err(_)) => Outcome::Closed,
            Ok(Ok(outcome)) => outcome,
        };
        match outcome {
            Outcome::Answered(value) => Ok(value),
            Outcome::Failed(why) => Err(MsOfficeError::Browser(why)),
            Outcome::Closed => Err(MsOfficeError::Browser(
                "the browser closed its socket".into(),
            )),
        }
    }
}

/// Owns the read half and routes every line: an answer to the ask that carries its id,
/// news onto the broadcast. Ends when the socket closes, failing every ask still waiting.
async fn reader_loop(mut read: BufReader<OwnedReadHalf>, shared: Arc<Shared>) {
    let mut buf = String::new();
    loop {
        buf.clear();
        match read.read_line(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = buf.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match classify(&message) {
            Parsed::Answer(id, outcome) => {
                if let Some(tx) = shared.pending.lock().unwrap().remove(&id) {
                    let _ = tx.send(outcome);
                }
            }
            Parsed::News(happening) => {
                let _ = shared.news_tx.send(happening);
            }
            Parsed::Ignore => {}
        }
    }
    let mut map = shared.pending.lock().unwrap();
    for (_, tx) in map.drain() {
        let _ = tx.send(Outcome::Closed);
    }
}

/// One line from the socket, sorted.
#[derive(Debug)]
enum Parsed {
    Answer(u64, Outcome),
    News(Happening),
    Ignore,
}

impl std::fmt::Debug for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Outcome::Answered(v) => write!(f, "Answered({v})"),
            Outcome::Failed(why) => write!(f, "Failed({why:?})"),
            Outcome::Closed => write!(f, "Closed"),
        }
    }
}

/// Sort one line by its `message`. Pure, so it is tested on its own.
fn classify(message: &Value) -> Parsed {
    let id = message.get("id").and_then(Value::as_u64);
    match message.get("message").and_then(Value::as_str) {
        Some("answered") => match id {
            Some(id) => Parsed::Answer(
                id,
                Outcome::Answered(message.get("answer").cloned().unwrap_or(Value::Null)),
            ),
            None => Parsed::Ignore,
        },
        Some("reply") => match id {
            Some(id) => Parsed::Answer(
                id,
                Outcome::Answered(message.get("reply").cloned().unwrap_or(Value::Null)),
            ),
            None => Parsed::Ignore,
        },
        Some("failed") => match id {
            Some(id) => {
                let why = message
                    .get("why")
                    .and_then(Value::as_str)
                    .unwrap_or("the window did not say why")
                    .to_string();
                Parsed::Answer(id, Outcome::Failed(why))
            }
            // A failure with no id is the socket saying a line was not JSON — ours never
            // are, and there is no ask to fail.
            None => Parsed::Ignore,
        },
        Some("refused") => match id {
            Some(id) => Parsed::Answer(
                id,
                Outcome::Failed(format!(
                    "refused: {}",
                    message.get("error").cloned().unwrap_or(Value::Null)
                )),
            ),
            None => Parsed::Ignore,
        },
        Some("news") => match message.get("news").and_then(happening_of) {
            Some(h) => Parsed::News(h),
            None => Parsed::Ignore,
        },
        // `event` is the browser's own (a tab opened, a page loaded); not the session's
        // business.
        _ => Parsed::Ignore,
    }
}

/// The `news` object, typed. `None` for a kind the session does not carry.
fn happening_of(news: &Value) -> Option<Happening> {
    let text = |key: &str| news.get(key).and_then(Value::as_str).map(str::to_string);
    match news.get("news").and_then(Value::as_str)? {
        "started" => Some(Happening::Started {
            flow: text("flow")?,
        }),
        "run" => {
            let happened = news.get("happened")?;
            let step = happened.get("step").and_then(Value::as_u64)? as usize;
            match happened.get("happened").and_then(Value::as_str)? {
                "began" => Some(Happening::Began { step }),
                "ended" => Some(Happening::Ended { step }),
                "did" => trouble_in(happened).map(|said| Happening::Trouble { said }),
                _ => None,
            }
        }
        "asking" => Some(Happening::Asking {
            hole: text("hole")?,
        }),
        "told" => Some(Happening::Told {
            said: text("said")?,
        }),
        "over" => {
            let tally = news
                .get("tally")
                .cloned()
                .and_then(|t| serde_json::from_value(t).ok())
                .unwrap_or_default();
            let stopped = news
                .get("stopped")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let yielded = news
                .get("yielded")
                .cloned()
                .and_then(|y| serde_json::from_value(y).ok())
                .unwrap_or_default();
            Some(Happening::Over {
                tally,
                stopped,
                yielded,
            })
        }
        _ => None,
    }
}

/// The sentence a `did` line ended on when it went wrong, if this is such a line.
fn trouble_in(happened: &Value) -> Option<String> {
    let line = happened.get("line")?;
    let outcome = line.get("outcome")?;
    match outcome.get("outcome").and_then(Value::as_str)? {
        "failed" | "broke" => {
            let said = outcome.get("said").and_then(Value::as_str);
            let action = line.get("said").and_then(Value::as_str).unwrap_or("a line");
            Some(match said {
                Some(why) => format!("{action}: {why}"),
                None => action.to_string(),
            })
        }
        _ => None,
    }
}

/// A `drunken-browser` process the session started, and the link to it.
///
/// Its socket lives in the runtime directory under the account key, so a browser that
/// outlived an earlier process of ours (the app was killed hard, the browser was not) is
/// found on the next start and adopted rather than doubled — one browser per profile is
/// also what the profile's lock allows.
pub(crate) struct Browser {
    link: Arc<Link>,
    /// The process, where this side started it. `None` for an adopted one.
    child: Option<Child>,
    socket: PathBuf,
}

impl Browser {
    /// Start the browser for `config` — or adopt the one already answering on its socket.
    pub(crate) async fn start(config: &SessionConfig) -> Result<Self, MsOfficeError> {
        let socket = socket_path(&config.account_key);
        if let Ok(link) = Link::connect(&socket).await {
            return Ok(Self {
                link,
                child: None,
                socket,
            });
        }
        // Nobody answers: a stale socket file from a browser that is gone would make the
        // new one fail to bind.
        let _ = std::fs::remove_file(&socket);
        std::fs::create_dir_all(&config.profile_dir).ok();

        // The browser logs to stderr. Into a file in the profile directory — inheriting it
        // would corrupt a TUI's alternate screen, and a file can be tailed.
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(config.profile_dir.join("browser.log"))
            .map(std::process::Stdio::from)
            .unwrap_or_else(|_| std::process::Stdio::null());

        let mut cmd = Command::new(&config.browser.bin);
        if config.headless {
            cmd.arg("--hidden");
        }
        cmd.arg(format!("--ipc={}", socket.display()))
            .arg(format!("--app-id={}", app_id(&config.account_key)))
            .env("DRUNKEN_BROWSER_DATA", &config.profile_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(log)
            // Its own process group, so teardown can reach the browser and every process
            // it spawned in one signal.
            .process_group(0);
        let mut child = cmd.spawn().map_err(|e| {
            MsOfficeError::Browser(format!("start {}: {e}", config.browser.bin.display()))
        })?;

        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        let link = loop {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(MsOfficeError::Browser(format!(
                    "the browser exited before opening its socket ({status}); see {}",
                    config.profile_dir.join("browser.log").display()
                )));
            }
            if let Ok(link) = Link::connect(&socket).await {
                break link;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(MsOfficeError::Browser(format!(
                    "the browser did not open {} within {}s",
                    socket.display(),
                    START_TIMEOUT.as_secs()
                )));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        Ok(Self {
            link,
            child: Some(child),
            socket,
        })
    }

    /// The link, for whoever needs to ask the window something out of band (answer a
    /// prompt, show the window).
    pub(crate) fn link(&self) -> Arc<Link> {
        Arc::clone(&self.link)
    }

    /// Open `flow` in the window and run the whole of it, with `data` as what the run is
    /// about. `progress` is told `(steps done, steps in the flow)` as the run moves.
    /// Returns once the run is over; a run that has not ended by `timeout` is an error.
    pub(crate) async fn run(
        &self,
        flow: &str,
        data: &BTreeMap<String, String>,
        timeout: Duration,
        mut progress: impl FnMut(usize, usize),
    ) -> Result<Run, MsOfficeError> {
        // Subscribe first: news from the run must not slip past between the ask and the
        // first `recv`.
        let mut news = self.link.news();
        let opened = self
            .link
            .ask(json!({ "ask": "line", "line": format!("test-open {flow}") }))
            .await?;
        let state = self.link.ask(json!({ "ask": "flow_state" })).await?;
        let open = state.get("flow").filter(|f| !f.is_null()).ok_or_else(|| {
            MsOfficeError::Run(format!(
                "the window did not open the flow `{flow}`: {}",
                said(&opened)
            ))
        })?;
        let its_name = open
            .get("path")
            .and_then(Value::as_str)
            .and_then(|p| Path::new(p).file_name())
            .map(|n| n.to_string_lossy().into_owned());
        let wanted = Path::new(flow)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        if its_name.is_some() && its_name != wanted {
            return Err(MsOfficeError::Run(format!(
                "the window has `{}` open, not `{flow}`: {}",
                its_name.unwrap_or_default(),
                said(&opened)
            )));
        }
        let steps = open
            .get("steps")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);

        let started = self
            .link
            .ask(json!({ "ask": "flow_run", "what": "whole", "data": data }))
            .await?;
        progress(0, steps);

        let deadline = tokio::time::Instant::now() + timeout;
        let first = tokio::time::Instant::now() + START_OF_RUN;
        let mut heard = false;
        let mut trouble = None;
        loop {
            // Until the first word from the run, the shorter of the two deadlines: a run
            // that has not said anything by then never started.
            let until = if heard { deadline } else { first.min(deadline) };
            let next = match tokio::time::timeout_at(until, news.recv()).await {
                Ok(Ok(h)) => h,
                Ok(Err(broadcast::error::RecvError::Closed)) => {
                    return Err(MsOfficeError::Browser(
                        "the browser closed its socket mid-run".into(),
                    ));
                }
                Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                    return Err(MsOfficeError::Protocol(format!(
                        "fell {n} lines behind the run's news"
                    )));
                }
                Err(_) if !heard => {
                    // The run never started. A run the pre-flight check refused — a value
                    // nobody can produce — answers with the refusal and starts nothing.
                    return Err(MsOfficeError::Run(format!(
                        "the run did not start: {}",
                        said(&started)
                    )));
                }
                Err(_) => return Err(MsOfficeError::Timeout),
            };
            heard = true;
            match next {
                Happening::Started { .. } => {}
                Happening::Began { step } => progress(step, steps),
                Happening::Ended { step } => progress(step + 1, steps),
                Happening::Asking { .. } | Happening::Told { .. } => {}
                Happening::Trouble { said } => trouble = Some(said),
                Happening::Over {
                    tally,
                    stopped,
                    yielded,
                } => {
                    return Ok(Run {
                        tally,
                        stopped,
                        yielded,
                        trouble,
                    });
                }
            }
        }
    }
}

/// The `said` of a `said` answer, or the whole answer where it is something else.
fn said(answer: &Value) -> String {
    match answer.get("said") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// Where the browser for `account_key` listens: the runtime directory, so the path is short
/// (a socket path has 108 bytes) and gone with the login session.
fn socket_path(account_key: &str) -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join(format!("nyd-office365-{}.sock", slug(account_key)))
}

/// What the window manager knows the browser as.
fn app_id(account_key: &str) -> String {
    format!("nyd-office365-{}", slug(account_key))
}

/// `account_key`, reduced to what a file name and an app id are happy with.
fn slug(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Aborts a spawned task on drop.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Drop for Browser {
    /// The browser never outlives the session. It is asked to quit the way a person would
    /// (`quit` on its socket, so the profile is closed cleanly), and a detached thread
    /// kills the whole process group shortly after in case it did not — off the async
    /// runtime, so this `Drop` never blocks it.
    fn drop(&mut self) {
        let socket = self.socket.clone();
        let pgid = self
            .child
            .as_ref()
            .and_then(Child::id)
            .map(|pid| pid as i32)
            .filter(|pid| *pid > 1);
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            if let Ok(mut s) = std::os::unix::net::UnixStream::connect(&socket) {
                let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
                let _ = s.write_all(b"{\"id\":1,\"command\":{\"command\":\"quit\"}}\n");
                let _ = s.read_to_end(&mut Vec::new());
            }
            if let Some(pgid) = pgid {
                std::thread::sleep(Duration::from_secs(1));
                unsafe { libc::kill(-pgid, libc::SIGKILL) };
            }
        });
    }
}

/// A window that is not there: a socket in a temporary directory, answered by a script.
///
/// The script sees every line the client sends (and `Null` once, when the client connects)
/// and says what goes back — nothing (`None`) to have the ask answered politely, or the
/// lines to write, where a `Null` line hangs up on the client.
#[cfg(test)]
pub(crate) mod fake {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    use super::AbortOnDrop;

    pub(crate) type Script = dyn Fn(&Value) -> Option<Vec<Value>> + Send + Sync;

    pub(crate) struct Window {
        pub(crate) socket: PathBuf,
        /// Every line the client sent, in order.
        pub(crate) heard: Arc<Mutex<Vec<Value>>>,
        _dir: tempfile::TempDir,
        _server: AbortOnDrop,
    }

    /// Listen with `script` at the other end.
    pub(crate) fn window(
        script: impl Fn(&Value) -> Option<Vec<Value>> + Send + Sync + 'static,
    ) -> Window {
        let dir = tempfile::tempdir().expect("a temp dir");
        let socket = dir.path().join("window.sock");
        let listener = UnixListener::bind(&socket).expect("bind the fake window");
        let heard: Arc<Mutex<Vec<Value>>> = Arc::default();
        let script: Arc<Script> = Arc::new(script);
        let server = tokio::spawn({
            let heard = Arc::clone(&heard);
            async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    tokio::spawn(serve(stream, Arc::clone(&script), Arc::clone(&heard)));
                }
            }
        });
        Window {
            socket,
            heard,
            _dir: dir,
            _server: AbortOnDrop(server),
        }
    }

    async fn serve(
        stream: tokio::net::UnixStream,
        script: Arc<Script>,
        heard: Arc<Mutex<Vec<Value>>>,
    ) {
        let (read, mut write) = stream.into_split();
        let mut read = BufReader::new(read);
        let mut buf = String::new();
        let mut pending = script(&Value::Null).unwrap_or_default();
        loop {
            for line in pending.drain(..) {
                if line.is_null() {
                    return;
                }
                let mut text = line.to_string();
                text.push('\n');
                if write.write_all(text.as_bytes()).await.is_err() {
                    return;
                }
            }
            buf.clear();
            match read.read_line(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let Ok(request) = serde_json::from_str::<Value>(buf.trim()) else {
                continue;
            };
            heard.lock().unwrap().push(request.clone());
            pending = script(&request).unwrap_or_else(|| vec![politely(&request)]);
        }
    }

    /// The window's own answer to an ask it has nothing to say about.
    fn politely(request: &Value) -> Value {
        if request.get("command").is_some() {
            reply(request, json!({ "ok": true }))
        } else {
            answered(request, json!({ "answer": "said", "said": "ok" }))
        }
    }

    /// `answer`, under the id of `request`.
    pub(crate) fn answered(request: &Value, answer: Value) -> Value {
        json!({ "message": "answered", "id": request["id"], "answer": answer })
    }

    /// A command's `reply`, under the id of `request`.
    pub(crate) fn reply(request: &Value, reply: Value) -> Value {
        json!({ "message": "reply", "id": request["id"], "reply": reply })
    }

    /// The window failing `request`.
    pub(crate) fn failed(request: &Value, why: &str) -> Value {
        json!({ "message": "failed", "id": request["id"], "why": why })
    }

    /// One piece of news.
    pub(crate) fn news(news: Value) -> Value {
        json!({ "message": "news", "news": news })
    }

    /// What `request` asks for, if it is an ask.
    pub(crate) fn ask_of(request: &Value) -> Option<&str> {
        request.get("host")?.get("ask")?.as_str()
    }

    /// The `flow_state` answer for a shelf flow with `steps` steps.
    pub(crate) fn state_with(name: &str, steps: usize) -> Value {
        json!({
            "answer": "state",
            "flow": {
                "name": name,
                "path": format!("/shelf/flows/{name}"),
                "steps": vec![json!({}); steps],
                "edited": false,
            },
            "showing": false,
            "said": null,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{self, answered, ask_of, failed, news, state_with};
    use super::*;

    fn line(message: &str) -> Value {
        serde_json::from_str(message).expect("a JSON line")
    }

    // --- sorting lines ---

    #[test]
    fn an_answer_goes_to_the_ask_with_its_id() {
        match classify(&line(
            r#"{"message":"answered","id":7,"answer":{"answer":"said","said":"opened"}}"#,
        )) {
            Parsed::Answer(7, Outcome::Answered(answer)) => assert_eq!(answer["said"], "opened"),
            other => panic!("sorted as {other:?}"),
        }
        match classify(&line(r#"{"message":"reply","id":2,"reply":{"tabs":[]}}"#)) {
            Parsed::Answer(2, Outcome::Answered(reply)) => assert!(reply["tabs"].is_array()),
            other => panic!("sorted as {other:?}"),
        }
    }

    #[test]
    fn a_failure_carries_why_and_a_refusal_its_error() {
        match classify(&line(
            r#"{"message":"failed","id":3,"why":"no flow is open"}"#,
        )) {
            Parsed::Answer(3, Outcome::Failed(why)) => assert_eq!(why, "no flow is open"),
            other => panic!("sorted as {other:?}"),
        }
        match classify(&line(
            r#"{"message":"refused","id":4,"error":{"error":"unknown"}}"#,
        )) {
            Parsed::Answer(4, Outcome::Failed(why)) => assert!(why.starts_with("refused: ")),
            other => panic!("sorted as {other:?}"),
        }
        // A failure with no id has no ask to fail.
        assert!(matches!(
            classify(&line(r#"{"message":"failed","id":null,"why":"not JSON"}"#)),
            Parsed::Ignore
        ));
    }

    #[test]
    fn events_and_did_lines_that_went_well_are_nobodys_business() {
        assert!(matches!(
            classify(&line(
                r#"{"message":"event","event":{"event":"tab_opened"}}"#
            )),
            Parsed::Ignore
        ));
        let did_well = line(
            r#"{"message":"news","news":{"news":"run","happened":{"happened":"did","step":1,
                "line":{"verb":"go","said":"go to the calendar","outcome":{"outcome":"done"}}}}}"#,
        );
        assert!(matches!(classify(&did_well), Parsed::Ignore));
    }

    #[test]
    fn the_run_news_is_typed() {
        let started = line(r#"{"news":"started","flow":"nyd-calendar"}"#);
        assert_eq!(
            happening_of(&started),
            Some(Happening::Started {
                flow: "nyd-calendar".into()
            })
        );
        let began =
            line(r#"{"news":"run","happened":{"happened":"began","step":2,"name":"Fetch"}}"#);
        assert_eq!(happening_of(&began), Some(Happening::Began { step: 2 }));
        let ended = line(r#"{"news":"run","happened":{"happened":"ended","step":2}}"#);
        assert_eq!(happening_of(&ended), Some(Happening::Ended { step: 2 }));
        let asking = line(r#"{"news":"asking","hole":"{{secret.otp}}"}"#);
        assert_eq!(
            happening_of(&asking),
            Some(Happening::Asking {
                hole: "{{secret.otp}}".into()
            })
        );
        let told = line(r#"{"news":"told","said":"Tap 42 on your phone to sign in"}"#);
        assert_eq!(
            happening_of(&told),
            Some(Happening::Told {
                said: "Tap 42 on your phone to sign in".into()
            })
        );
        let halted = line(r#"{"news":"run","happened":{"happened":"halted","step":1}}"#);
        assert_eq!(happening_of(&halted), None);
    }

    #[test]
    fn over_carries_the_tally_and_what_was_yielded() {
        let over = line(
            r#"{"news":"over","tally":{"ran":3,"failed":1,"broke":0},"stopped":false,
                "yielded":{"calendar":{"events":[]}}}"#,
        );
        let Some(Happening::Over {
            tally,
            stopped,
            yielded,
        }) = happening_of(&over)
        else {
            panic!("not over");
        };
        assert_eq!(
            tally,
            Tally {
                ran: 3,
                failed: 1,
                broke: 0
            }
        );
        assert!(!tally.clean());
        assert!(!stopped);
        assert!(yielded["calendar"]["events"].is_array());

        // An `over` with nothing else is a stopped-or-empty run, not a parse failure.
        let bare = line(r#"{"news":"over"}"#);
        assert_eq!(
            happening_of(&bare),
            Some(Happening::Over {
                tally: Tally::default(),
                stopped: false,
                yielded: BTreeMap::new()
            })
        );
    }

    #[test]
    fn a_line_that_failed_is_trouble_with_its_sentence() {
        let failed = line(
            r#"{"happened":"did","step":0,"line":{"verb":"expect","said":"expect the inbox",
                "outcome":{"outcome":"failed","said":"nothing matched `.inbox`"}}}"#,
        );
        assert_eq!(
            trouble_in(&failed).as_deref(),
            Some("expect the inbox: nothing matched `.inbox`")
        );
        let broke_silently = line(
            r#"{"happened":"did","step":0,"line":{"verb":"go","said":"go to the calendar",
                "outcome":{"outcome":"broke"}}}"#,
        );
        assert_eq!(
            trouble_in(&broke_silently).as_deref(),
            Some("go to the calendar")
        );
        let noted = line(
            r#"{"happened":"did","step":0,"line":{"verb":"expect","said":"expect a banner",
                "outcome":{"outcome":"noted","said":"no banner"}}}"#,
        );
        assert_eq!(trouble_in(&noted), None);
    }

    #[test]
    fn an_account_key_becomes_a_file_name_and_an_app_id() {
        assert_eq!(slug("work"), "work");
        assert_eq!(slug("side project/2"), "side-project-2");
        assert_eq!(app_id("work"), "nyd-office365-work");
        assert!(socket_path("a b").ends_with("nyd-office365-a-b.sock"));
    }

    // --- the link, against a fake window ---

    #[tokio::test]
    async fn an_ask_is_answered_and_a_failure_is_an_error() {
        let window = fake::window(|request| match ask_of(request) {
            Some("flow_state") => Some(vec![answered(request, state_with("nyd-calendar", 3))]),
            Some("show") => Some(vec![failed(request, "no window to show")]),
            _ => None,
        });
        let link = Link::connect(&window.socket).await.expect("connect");

        let state = link
            .ask(json!({ "ask": "flow_state" }))
            .await
            .expect("answered");
        assert_eq!(state["flow"]["name"], "nyd-calendar");

        let err = link.show(true).await.expect_err("failed");
        assert!(matches!(err, MsOfficeError::Browser(why) if why == "no window to show"));
    }

    #[tokio::test]
    async fn a_toggle_asks_the_window_where_it_stands_and_flips_it() {
        // The window's own state; a toggle must not remember one of its own.
        let shown = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let window = fake::window({
            let shown = Arc::clone(&shown);
            move |request| match ask_of(request) {
                Some("show") => {
                    if let Some(on) = request["host"]["on"].as_bool() {
                        shown.store(on, Ordering::SeqCst);
                    }
                    Some(vec![answered(
                        request,
                        json!({ "answer": "window", "shown": shown.load(Ordering::SeqCst) }),
                    )])
                }
                _ => None,
            }
        });
        let link = Link::connect(&window.socket).await.expect("connect");

        assert!(link.shown().await.expect("answered"));
        assert!(!link.toggle().await.expect("toggled down"));
        assert!(!link.shown().await.expect("answered"));
        assert!(link.toggle().await.expect("toggled up"));
        assert!(link.shown().await.expect("answered"));

        // Asking is `show` without `on`; flipping is `show` with the new state.
        let asks: Vec<Option<bool>> = window
            .heard
            .lock()
            .unwrap()
            .iter()
            .filter(|r| ask_of(r) == Some("show"))
            .map(|r| r["host"]["on"].as_bool())
            .collect();
        assert_eq!(
            asks,
            vec![None, None, Some(false), None, None, Some(true), None]
        );
    }

    #[tokio::test]
    async fn a_window_that_does_not_say_whether_it_is_shown_is_a_browser_error() {
        let window = fake::window(|request| match ask_of(request) {
            Some("show") => Some(vec![answered(request, json!({ "answer": "said" }))]),
            _ => None,
        });
        let link = Link::connect(&window.socket).await.expect("connect");
        let err = link.shown().await.expect_err("no state");
        assert!(matches!(err, MsOfficeError::Browser(_)));
    }

    #[tokio::test]
    async fn an_answer_travels_as_flow_answer_and_a_refusal_as_null() {
        let window = fake::window(|_| None);
        let link = Link::connect(&window.socket).await.expect("connect");
        link.answer(Some("hunter2".into())).await.expect("answered");
        link.answer(None).await.expect("refused");

        let heard = window.heard.lock().unwrap();
        assert_eq!(
            heard[0]["host"],
            json!({ "ask": "flow_answer", "value": "hunter2" })
        );
        assert_eq!(
            heard[1]["host"],
            json!({ "ask": "flow_answer", "value": null })
        );
        // Every ask under its own id.
        assert_ne!(heard[0]["id"], heard[1]["id"]);
    }

    #[tokio::test]
    async fn news_is_heard_with_no_ask_in_flight() {
        let window = fake::window(|request| {
            request.is_null().then(|| {
                vec![news(
                    json!({ "news": "told", "said": "Tap 42 on your phone to sign in" }),
                )]
            })
        });
        let link = Link::connect(&window.socket).await.expect("connect");
        let mut news = link.news();
        let heard = tokio::time::timeout(Duration::from_secs(5), news.recv())
            .await
            .expect("in time")
            .expect("news");
        assert_eq!(
            heard,
            Happening::Told {
                said: "Tap 42 on your phone to sign in".into()
            }
        );
    }

    #[tokio::test]
    async fn a_window_that_hangs_up_fails_the_ask_in_flight() {
        let window = fake::window(|request| (!request.is_null()).then(|| vec![Value::Null]));
        let link = Link::connect(&window.socket).await.expect("connect");
        let err = link
            .ask(json!({ "ask": "flow_state" }))
            .await
            .expect_err("closed");
        assert!(matches!(err, MsOfficeError::Browser(why) if why.contains("closed")));
    }

    // --- a run, against a fake window ---

    fn a_window_that_runs(events: Value, outcome: &'static str) -> fake::Window {
        fake::window(move |request| match ask_of(request) {
            Some("line") => {
                assert_eq!(request["host"]["line"], "test-open nyd-calendar");
                Some(vec![answered(
                    request,
                    json!({ "answer": "said", "said": "opened nyd-calendar" }),
                )])
            }
            Some("flow_state") => Some(vec![answered(request, state_with("nyd-calendar", 2))]),
            Some("flow_run") => {
                assert_eq!(request["host"]["what"], "whole");
                assert_eq!(request["host"]["data"]["from"], "2026-09-01T00:00:00+02:00");
                Some(vec![
                    answered(request, json!({ "answer": "said", "said": "running" })),
                    news(json!({ "news": "started", "flow": "nyd-calendar" })),
                    news(
                        json!({ "news": "run", "happened": { "happened": "began", "step": 0, "name": "Sign on" } }),
                    ),
                    news(
                        json!({ "news": "run", "happened": { "happened": "did", "step": 0, "line": {
                        "verb": "expect", "said": "expect the calendar",
                        "outcome": { "outcome": outcome, "said": "nothing matched" } } } }),
                    ),
                    news(json!({ "news": "run", "happened": { "happened": "ended", "step": 0 } })),
                    news(
                        json!({ "news": "run", "happened": { "happened": "began", "step": 1, "name": "Fetch" } }),
                    ),
                    news(json!({ "news": "run", "happened": { "happened": "ended", "step": 1 } })),
                    news(
                        json!({ "news": "over", "tally": { "ran": 2, "failed": if outcome == "failed" { 1 } else { 0 }, "broke": 0 },
                        "stopped": false, "yielded": { "calendar": events } }),
                    ),
                ])
            }
            _ => None,
        })
    }

    fn adopted(window: &fake::Window) -> Browser {
        Browser {
            link: futures_block(Link::connect(&window.socket)).expect("connect"),
            child: None,
            socket: window.socket.clone(),
        }
    }

    /// Await inside a sync helper, on the test's runtime.
    fn futures_block<T>(fut: impl std::future::Future<Output = T>) -> T {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
    }

    fn range() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("from".to_string(), "2026-09-01T00:00:00+02:00".to_string()),
            ("until".to_string(), "2026-09-08T00:00:00+02:00".to_string()),
        ])
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_run_opens_the_flow_and_hands_back_what_it_yielded() {
        let window = a_window_that_runs(json!({ "events": [{ "id": "1" }] }), "done");
        let browser = adopted(&window);
        let mut progress = Vec::new();
        let run = browser
            .run(
                "nyd-calendar",
                &range(),
                Duration::from_secs(5),
                |done, steps| progress.push((done, steps)),
            )
            .await
            .expect("a run");
        assert_eq!(
            run.tally,
            Tally {
                ran: 2,
                failed: 0,
                broke: 0
            }
        );
        assert!(!run.stopped);
        assert_eq!(run.trouble, None);
        assert_eq!(run.yielded["calendar"]["events"][0]["id"], "1");
        assert_eq!(progress, vec![(0, 2), (0, 2), (1, 2), (1, 2), (2, 2)]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_run_remembers_the_line_that_failed() {
        let window = a_window_that_runs(json!({ "events": [] }), "failed");
        let browser = adopted(&window);
        let run = browser
            .run("nyd-calendar", &range(), Duration::from_secs(5), |_, _| {})
            .await
            .expect("a run");
        assert!(!run.tally.clean());
        assert_eq!(
            run.trouble.as_deref(),
            Some("expect the calendar: nothing matched")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_flow_the_window_did_not_open_is_an_error() {
        let window = fake::window(|request| match ask_of(request) {
            Some("line") => Some(vec![answered(
                request,
                json!({ "answer": "said", "said": "no such flow" }),
            )]),
            Some("flow_state") => Some(vec![answered(
                request,
                json!({ "answer": "state", "flow": null, "showing": false, "said": null }),
            )]),
            _ => None,
        });
        let browser = adopted(&window);
        let err = browser
            .run("nyd-calendar", &range(), Duration::from_secs(5), |_, _| {})
            .await
            .expect_err("not open");
        assert!(matches!(err, MsOfficeError::Run(why) if why.contains("no such flow")));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn another_flow_open_is_an_error() {
        let window = fake::window(|request| match ask_of(request) {
            Some("flow_state") => Some(vec![answered(request, state_with("tell-probe", 1))]),
            _ => None,
        });
        let browser = adopted(&window);
        let err = browser
            .run("nyd-calendar", &range(), Duration::from_secs(5), |_, _| {})
            .await
            .expect_err("wrong flow");
        assert!(matches!(err, MsOfficeError::Run(why) if why.contains("`tell-probe` open")));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_run_that_never_starts_is_the_refusal() {
        let window = fake::window(|request| match ask_of(request) {
            Some("flow_state") => Some(vec![answered(request, state_with("nyd-calendar", 2))]),
            Some("flow_run") => Some(vec![answered(
                request,
                json!({ "answer": "said", "said": "no command for calendar-password" }),
            )]),
            _ => None,
        });
        let browser = adopted(&window);
        let err = browser
            .run(
                "nyd-calendar",
                &range(),
                Duration::from_millis(300),
                |_, _| {},
            )
            .await
            .expect_err("never started");
        assert!(
            matches!(&err, MsOfficeError::Run(why) if why == "the run did not start: no command for calendar-password"),
            "{err:?}"
        );
    }

    // --- the real thing ---

    /// Starts the installed `drunken-browser`, hidden, on a throwaway profile, and runs the
    /// shelf's `tell-probe` flow through it — the whole way this crate takes for a
    /// calendar read, minus the sign-on. Ignored by default: it needs the browser and the
    /// flow. `cargo test -p not-yet-done-office365-web -- --ignored live`
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "starts the real drunken-browser; needs the tell-probe flow on the shelf"]
    async fn live_a_hidden_browser_runs_a_flow_and_is_gone_afterwards() {
        use crate::session::{Answers, BrowserConfig};

        let dir = tempfile::tempdir().expect("a temp dir");
        let config = SessionConfig {
            account_key: "live-probe".into(),
            profile_dir: dir.path().join("profile"),
            headless: true,
            auto_headed: false,
            facts: BTreeMap::new(),
            answers: Answers::new(),
            browser: BrowserConfig::default(),
        };
        let browser = Browser::start(&config).await.expect("the browser starts");
        assert!(
            browser.child.is_some(),
            "a browser of ours, not an adopted one"
        );
        let socket = browser.socket.clone();
        let mut news = browser.link().news();

        let data = BTreeMap::from([("greeting".to_string(), "hello".to_string())]);
        let mut progress = Vec::new();
        let run = browser
            .run(
                "tell-probe",
                &data,
                Duration::from_secs(60),
                |done, steps| progress.push((done, steps)),
            )
            .await
            .expect("the run is over");
        assert!(run.tally.clean(), "{run:?}");
        assert!(!run.stopped);
        assert_eq!(progress.last(), Some(&(1, 1)), "{progress:?}");

        let mut told = None;
        while let Ok(happening) = news.try_recv() {
            if let Happening::Told { said } = happening {
                told = Some(said);
            }
        }
        assert_eq!(told.as_deref(), Some("The run says hello"));

        drop(browser);
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(
            Link::connect(&socket).await.is_err(),
            "the browser should be gone once the session lets it go"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_run_that_does_not_end_in_time_is_a_timeout() {
        let window = fake::window(|request| match ask_of(request) {
            Some("flow_state") => Some(vec![answered(request, state_with("nyd-calendar", 2))]),
            Some("flow_run") => Some(vec![
                answered(request, json!({ "answer": "said", "said": "running" })),
                news(json!({ "news": "started", "flow": "nyd-calendar" })),
            ]),
            _ => None,
        });
        let browser = adopted(&window);
        let err = browser
            .run(
                "nyd-calendar",
                &range(),
                Duration::from_millis(300),
                |_, _| {},
            )
            .await
            .expect_err("timed out");
        assert!(matches!(err, MsOfficeError::Timeout), "{err:?}");
    }
}
