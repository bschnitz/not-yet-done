//! One running backend program: started once, talked to until it exits.
//!
//! Calls are matched to replies by id, so several may be out at once; a
//! reader task owns stdout and the child, dispatches each line, and puts a
//! form in front of the person without holding up the replies behind it.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use child_log::ChildLog;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use not_yet_done_calendar_core::{CalendarError, LoadProgress};
use not_yet_done_content::auth::ScriptFormField;
use not_yet_done_content::{
    ActionInput, FormFieldKind, FormFieldSpec, InputSpec, PromptAnswer, PromptRequest,
};

use crate::protocol::{ErrorKind, FromBackend, PROTOCOL, ReplyError, ToBackend};

/// How long a freshly started backend gets to say `hello`.
const HELLO_PATIENCE: Duration = Duration::from_secs(30);

/// How long a backend gets to exit on its own after `cancel`, before it is
/// killed. Long enough to close a browser.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, ReplyError>>>>>;
type Stdin = Arc<tokio::sync::Mutex<ChildStdin>>;

/// What starts a backend: who it is, how to run it, and its `start` line.
pub(crate) struct StartSpec {
    /// Names the log file and the backend in messages.
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) connection: String,
    pub(crate) config: serde_json::Value,
    pub(crate) secrets: BTreeMap<String, String>,
}

/// Where a backend's forms go: the frontend's prompt stream, and the label it
/// shows as their source.
#[derive(Clone)]
pub(crate) struct PromptSink {
    pub(crate) source: String,
    pub(crate) tx: mpsc::Sender<PromptRequest>,
}

pub(crate) struct Process {
    who: String,
    log_path: PathBuf,
    /// The only strong handle: dropping the process closes stdin, which is
    /// the backend's cue to exit. Form answers hold a [`Weak`].
    stdin: Stdin,
    pending: Pending,
    next_id: AtomicU64,
    alive: Arc<AtomicBool>,
    empty_is_authoritative: bool,
    progress: broadcast::Sender<LoadProgress>,
    attention: watch::Sender<Option<String>>,
    /// Fired on drop, so a backend that ignores `cancel` is still killed.
    shutdown: Option<oneshot::Sender<()>>,
}

impl Process {
    /// Start the backend under `sh -c`, hand it `start` and wait for `hello`.
    ///
    /// `asked` counts every time the backend asks the person something —
    /// an attention or a form.
    pub(crate) async fn start(
        spec: StartSpec,
        prompts: Option<PromptSink>,
        asked: Arc<AtomicU64>,
    ) -> Result<Arc<Process>, CalendarError> {
        let who = format!("calendar backend `{}`", spec.name);
        // As for an auth plugin: the log is not the protocol, and the stderr
        // this process inherited may belong to a TUI holding the screen.
        let log = ChildLog::named(
            not_yet_done_content::http_log::log_directory(),
            &format!("calendar-{}", spec.name),
        );
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&spec.command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(log.stdio())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| CalendarError::Config(format!("{who}: spawn `{}`: {e}", spec.command)))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| CalendarError::Other(format!("{who}: stdin pipe missing")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CalendarError::Other(format!("{who}: stdout pipe missing")))?;
        let mut lines = BufReader::new(stdout).lines();
        let log_path = log.path().to_path_buf();

        write_line(
            &who,
            &mut stdin,
            &ToBackend::Start {
                protocol: PROTOCOL,
                connection: spec.connection,
                config: spec.config,
                secrets: spec.secrets,
            },
        )
        .await?;
        let empty_is_authoritative = match next_said(&who, &mut lines, HELLO_PATIENCE).await {
            Ok(Some(FromBackend::Hello {
                protocol: PROTOCOL,
                empty_is_authoritative,
            })) => empty_is_authoritative,
            Ok(Some(FromBackend::Hello { protocol, .. })) => {
                return Err(CalendarError::Config(format!(
                    "{who} speaks protocol {protocol}, nyd speaks {PROTOCOL}"
                )));
            }
            Ok(Some(FromBackend::Fatal(why))) => {
                return Err(CalendarError::Other(format!("{who}: {why}")));
            }
            Ok(Some(other)) => {
                return Err(CalendarError::Other(format!(
                    "{who} answered `start` with {other:?} instead of `hello`"
                )));
            }
            Ok(None) => {
                return Err(CalendarError::Other(format!(
                    "{who} exited before saying `hello`; its own account of it is in {}",
                    log_path.display()
                )));
            }
            Err(e) => return Err(e),
        };

        let (progress, _) = broadcast::channel(16);
        let (attention, _) = watch::channel(None);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let process = Arc::new(Process {
            who: who.clone(),
            log_path: log_path.clone(),
            stdin: Arc::new(tokio::sync::Mutex::new(stdin)),
            pending: Arc::default(),
            next_id: AtomicU64::new(1),
            alive: Arc::new(AtomicBool::new(true)),
            empty_is_authoritative,
            progress,
            attention,
            shutdown: Some(shutdown_tx),
        });
        let reader = Reader {
            who,
            log_path,
            stdin: Arc::downgrade(&process.stdin),
            pending: Arc::clone(&process.pending),
            alive: Arc::clone(&process.alive),
            progress: process.progress.clone(),
            attention: process.attention.clone(),
            prompts,
            asked,
        };
        tokio::spawn(reader.run(child, lines, shutdown_rx));
        Ok(process)
    }

    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub(crate) fn empty_is_authoritative(&self) -> bool {
        self.empty_is_authoritative
    }

    pub(crate) fn subscribe_progress(&self) -> broadcast::Receiver<LoadProgress> {
        self.progress.subscribe()
    }

    pub(crate) fn subscribe_attention(&self) -> watch::Receiver<Option<String>> {
        self.attention.subscribe()
    }

    /// Send the call `make` builds for a fresh id, and wait for its reply.
    ///
    /// The error is the protocol's own, so the caller can still tell a call
    /// the backend does not offer (`unsupported`) from one that failed.
    pub(crate) async fn call(
        &self,
        what: &str,
        make: impl FnOnce(u64) -> ToBackend,
        patience: Duration,
    ) -> Result<serde_json::Value, ReplyError> {
        if !self.is_alive() {
            return Err(self.gone());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let written = {
            let mut stdin = self.stdin.lock().await;
            write_line(&self.who, &mut stdin, &make(id)).await
        };
        if let Err(e) = written {
            self.pending.lock().unwrap().remove(&id);
            return Err(ReplyError::from(&e));
        }
        match tokio::time::timeout(patience, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err(self.gone()),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(network(format!(
                    "{} did not answer `{what}` within {}s; what it was doing is in {}",
                    self.who,
                    patience.as_secs(),
                    self.log_path.display()
                )))
            }
        }
    }

    fn gone(&self) -> ReplyError {
        network(format!(
            "{} is no longer running; its own account of it is in {}",
            self.who,
            self.log_path.display()
        ))
    }
}

impl Drop for Process {
    /// Tell the backend to clean up. Closing stdin (the last strong handle
    /// goes with the task below) says it a second time; the shutdown signal
    /// lets the reader kill a backend that heard neither.
    fn drop(&mut self) {
        let stdin = Arc::clone(&self.stdin);
        let who = self.who.clone();
        let shutdown = self.shutdown.take();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let mut stdin = stdin.lock().await;
                let _ = write_line(&who, &mut stdin, &ToBackend::Cancel {}).await;
                drop(stdin);
                if let Some(shutdown) = shutdown {
                    let _ = shutdown.send(());
                }
            });
        }
    }
}

/// The reader task's share of a [`Process`].
struct Reader {
    who: String,
    log_path: PathBuf,
    stdin: Weak<tokio::sync::Mutex<ChildStdin>>,
    pending: Pending,
    alive: Arc<AtomicBool>,
    progress: broadcast::Sender<LoadProgress>,
    attention: watch::Sender<Option<String>>,
    prompts: Option<PromptSink>,
    asked: Arc<AtomicU64>,
}

impl Reader {
    async fn run(
        self,
        mut child: Child,
        mut lines: Lines<BufReader<ChildStdout>>,
        mut shutdown: oneshot::Receiver<()>,
    ) {
        let mut loading = false;
        let ended = loop {
            let line = tokio::select! {
                line = lines.next_line() => line,
                _ = &mut shutdown => break None,
            };
            let line = match line {
                Ok(Some(line)) if line.trim().is_empty() => continue,
                Ok(Some(line)) => line,
                Ok(None) => break None,
                Err(e) => break Some(format!("{}: reading its output: {e}", self.who)),
            };
            let said = match serde_json::from_str::<FromBackend>(&line) {
                Ok(said) => said,
                Err(e) => {
                    break Some(format!("{}: this is not a protocol line ({e})", self.who));
                }
            };
            match said {
                FromBackend::Reply { id, ok, error } => {
                    let reply = match error {
                        Some(e) => Err(e),
                        None => Ok(ok),
                    };
                    if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
                        let _ = tx.send(reply);
                    }
                }
                FromBackend::Progress(p) => {
                    loading = !p.done;
                    let _ = self.progress.send(p);
                }
                FromBackend::Attention(said) => {
                    if said.is_some() {
                        self.asked.fetch_add(1, Ordering::Relaxed);
                    }
                    let _ = self.attention.send(said);
                }
                FromBackend::Form {
                    id,
                    header,
                    error,
                    fields,
                } => {
                    self.asked.fetch_add(1, Ordering::Relaxed);
                    tokio::spawn(answer_form(
                        self.who.clone(),
                        self.prompts.clone(),
                        self.stdin.clone(),
                        id,
                        header,
                        error,
                        fields,
                    ));
                }
                FromBackend::Fatal(why) => break Some(format!("{}: {why}", self.who)),
                // A second hello says nothing new.
                FromBackend::Hello { .. } => {}
            }
        };

        self.alive.store(false, Ordering::SeqCst);
        let why = ended.unwrap_or_else(|| {
            format!(
                "{} exited; its own account of it is in {}",
                self.who,
                self.log_path.display()
            )
        });
        for (_, tx) in self.pending.lock().unwrap().drain() {
            let _ = tx.send(Err(network(why.clone())));
        }
        let _ = self.attention.send(None);
        // A load that was under way will not finish; settle the banner.
        if loading {
            let _ = self.progress.send(LoadProgress::complete());
        }
        if tokio::time::timeout(SHUTDOWN_GRACE, child.wait()).await.is_err() {
            let _ = child.start_kill();
        }
    }
}

fn network(message: String) -> ReplyError {
    ReplyError {
        kind: ErrorKind::Network,
        message,
    }
}

/// Put the backend's form in front of the person and send back what they
/// said — or `dismissed` when nobody is there to ask or nobody answered.
async fn answer_form(
    who: String,
    prompts: Option<PromptSink>,
    stdin: Weak<tokio::sync::Mutex<ChildStdin>>,
    id: u64,
    header: Option<String>,
    error: Option<String>,
    fields: Vec<ScriptFormField>,
) {
    let values = match prompts {
        Some(sink) => ask(sink, header, error, &fields).await,
        None => None,
    };
    let line = match values {
        Some(values) => ToBackend::Input { form: id, values },
        None => ToBackend::Dismissed { form: id },
    };
    if let Some(stdin) = stdin.upgrade() {
        let mut stdin = stdin.lock().await;
        let _ = write_line(&who, &mut stdin, &line).await;
    }
}

async fn ask(
    sink: PromptSink,
    header: Option<String>,
    error: Option<String>,
    fields: &[ScriptFormField],
) -> Option<BTreeMap<String, String>> {
    let fields: Vec<FormFieldSpec> = fields
        .iter()
        .map(|f| FormFieldSpec {
            key: f.name.clone(),
            label: f.effective_label(),
            kind: FormFieldKind::Text,
            required: !f.optional,
            default: f.prefill.clone(),
            masked: f.masked,
            visible_when: None,
        })
        .collect();
    let (tx, rx) = oneshot::channel();
    let request = PromptRequest {
        source: sink.source,
        prompt: header.unwrap_or_else(|| "Input required".to_string()),
        detail: error,
        input: InputSpec::Form { fields },
        respond: tx,
    };
    sink.tx.send(request).await.ok()?;
    match rx.await {
        Ok(PromptAnswer::Provided(ActionInput::Form(values))) => Some(values.into_iter().collect()),
        _ => None,
    }
}

/// The backend's next line, parsed; `None` once its stdout closed.
async fn next_said(
    who: &str,
    lines: &mut Lines<BufReader<ChildStdout>>,
    patience: Duration,
) -> Result<Option<FromBackend>, CalendarError> {
    let deadline = tokio::time::Instant::now() + patience;
    loop {
        let line = match tokio::time::timeout_at(deadline, lines.next_line()).await {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => return Ok(None),
            Ok(Err(e)) => {
                return Err(CalendarError::Other(format!("{who}: reading its output: {e}")));
            }
            Err(_) => {
                return Err(CalendarError::Network(format!(
                    "{who} said nothing for {}s",
                    patience.as_secs()
                )));
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        return serde_json::from_str(&line)
            .map(Some)
            .map_err(|e| CalendarError::Other(format!("{who}: this is not a protocol line ({e})")));
    }
}

async fn write_line(
    who: &str,
    stdin: &mut ChildStdin,
    line: &ToBackend,
) -> Result<(), CalendarError> {
    let mut payload = serde_json::to_string(line)
        .map_err(|e| CalendarError::Other(format!("{who}: encoding a line: {e}")))?;
    payload.push('\n');
    stdin
        .write_all(payload.as_bytes())
        .await
        .map_err(|e| CalendarError::Network(format!("{who}: writing to it: {e}")))?;
    stdin
        .flush()
        .await
        .map_err(|e| CalendarError::Network(format!("{who}: writing to it: {e}")))
}
