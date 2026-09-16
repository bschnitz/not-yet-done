//! The other side of the boundary: drunken-browser's control socket.
//!
//! Every shape in here is written by hand out of [`serde_json`] rather than
//! borrowed from drunken-browser's crates, and that is the point — see the
//! module doc in `main.rs`. What is duplicated is the handful of field names
//! below and nothing else, which is the price of a coupling that a `cargo
//! update` cannot break.
//!
//! # Whose browser it is
//!
//! By default this plugin starts one and quits it again, on a socket of its
//! own in the runtime directory. A login that runs unattended cannot borrow
//! the browser a person happens to be reading the news in: the flow would
//! open tabs in their session, and quitting afterwards would close it.
//!
//! What survives between logins is the *profile* on disk, which is where a
//! browser keeps a session anyway.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use crate::options::Options;

/// How long a browser we started gets to quit before it is killed.
const QUIT_GRACE: Duration = Duration::from_secs(5);

/// How often the socket of a browser that is starting is tried.
const CONNECT_EVERY: Duration = Duration::from_millis(100);

pub struct Browser {
    write: tokio::net::unix::OwnedWriteHalf,
    lines: mpsc::Receiver<Value>,
    /// News that arrived while an answer was being waited for. Kept rather
    /// than dropped: a run that finished between two asks would otherwise
    /// take its `over` with it.
    news: VecDeque<Value>,
    next_id: u64,
    /// The browser this plugin started, and is therefore responsible for.
    /// `None` when it attached to one somebody else runs.
    child: Option<Child>,
    ours: Option<PathBuf>,
}

impl Browser {
    /// Connect to the browser named on the command line, starting one if
    /// none was.
    pub async fn open(options: &Options) -> Result<Self, String> {
        match &options.socket {
            Some(path) => {
                let stream = UnixStream::connect(path)
                    .await
                    .map_err(|e| format!("no browser on {}: {e}", path.display()))?;
                Ok(Self::over(stream, None, None))
            }
            None => Self::start(options).await,
        }
    }

    async fn start(options: &Options) -> Result<Self, String> {
        let path = private_socket();
        // Left over from a plugin that was killed rather than allowed to
        // shut down: a stale socket file is not a browser, and binding is
        // the browser's job.
        let _ = std::fs::remove_file(&path);

        let line = format!("{} --hidden --ipc={}", options.browser, path.display());
        let child = Command::new("sh")
            .arg("-c")
            .arg(&line)
            .stdin(Stdio::null())
            // Its log is our log is nyd's log, all on the stderr this
            // process inherited. A pipe would have to be drained by
            // somebody, and nobody here is free to.
            //
            // Not stdout, though, whatever the browser puts there: this
            // process's stdout IS the line protocol nyd reads, and a child
            // that inherits it can end a login with a sentence meant for a
            // terminal. Chromium writes exactly one such sentence -- "opening
            // in an existing browser session", when a second browser meets a
            // profile that is already in use -- and nyd answered it with
            // "this is not a protocol line". Dropped rather than piped,
            // because a pipe nobody drains is the reason the other two
            // streams are inherited in the first place.
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            // The backstop for every path that cannot wait: a plugin that
            // goes away must not leave a browser behind it.
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("starting `{line}`: {e}"))?;

        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(options.start_timeout_secs);
        let mut child = child;
        loop {
            if let Ok(stream) = UnixStream::connect(&path).await {
                return Ok(Self::over(stream, Some(child), Some(path)));
            }
            // Asked before the clock, because a browser that has already
            // exited will never open the socket and its status is the
            // useful half of the message.
            if let Ok(Some(status)) = child.try_wait() {
                // The one cause worth naming: Chromium allows a single
                // browser per profile directory, and every login here shares
                // one profile on purpose -- so a second login that starts
                // while the first is still up dies right here. Its own
                // reason is on the stderr above; this is the line nyd shows.
                return Err(format!(
                    "`{line}` exited ({status}) without opening a socket \
                     -- another browser may already be holding the profile \
                     (a login of its own still running?); its reason is on \
                     nyd's stderr"
                ));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "`{line}` did not open {} within {}s",
                    path.display(),
                    options.start_timeout_secs
                ));
            }
            tokio::time::sleep(CONNECT_EVERY).await;
        }
    }

    fn over(stream: UnixStream, child: Option<Child>, ours: Option<PathBuf>) -> Self {
        let (read, write) = stream.into_split();
        // A task rather than a future the main loop awaits, so that reading
        // the socket is never the branch a `select!` cancels half a line in.
        let (tx, lines) = mpsc::channel(64);
        tokio::spawn(async move {
            let mut reader = BufReader::new(read).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                match serde_json::from_str::<Value>(&line) {
                    Ok(value) => {
                        if tx.send(value).await.is_err() {
                            return;
                        }
                    }
                    // The socket is a contract; a line that is not JSON is
                    // the other end's bug, and dropping it beats stopping.
                    Err(e) => eprintln!("nyd-auth-drunken: unreadable line from the browser: {e}"),
                }
            }
        });
        Self {
            write,
            lines,
            news: VecDeque::new(),
            next_id: 1,
            child,
            ours,
        }
    }

    /// Ask the window something — `drunken-host`'s vocabulary.
    pub async fn ask(&mut self, what: Value) -> Result<Value, String> {
        let id = self.send("host", what).await?;
        self.answer_to(id).await
    }

    /// Type a line into the window's command line, as a person would.
    pub async fn line(&mut self, line: impl Into<String>) -> Result<Value, String> {
        self.ask(json!({ "ask": "line", "line": line.into() }))
            .await
    }

    /// Tell the browser something — `drunken_protocol`'s vocabulary.
    pub async fn command(&mut self, command: Value) -> Result<Value, String> {
        let id = self.send("command", command).await?;
        self.answer_to(id).await
    }

    /// The next thing the window says on its own, waited for as long as it
    /// takes. The deadline against a stuck browser is nyd's, one process
    /// out: it is the one that knows how long a step may take.
    pub async fn next_news(&mut self) -> Result<Value, String> {
        if let Some(news) = self.news.pop_front() {
            return Ok(news);
        }
        loop {
            let message = self.next_message().await?;
            if let Some(news) = as_news(&message) {
                return Ok(news);
            }
        }
    }

    async fn send(&mut self, kind: &str, payload: Value) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        let mut line = json!({ "id": id, kind: payload }).to_string();
        line.push('\n');
        self.write
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("writing to the browser: {e}"))?;
        self.write
            .flush()
            .await
            .map_err(|e| format!("writing to the browser: {e}"))?;
        Ok(id)
    }

    async fn answer_to(&mut self, id: u64) -> Result<Value, String> {
        loop {
            let message = self.next_message().await?;
            let mine = message.get("id").and_then(Value::as_u64) == Some(id);
            match message.get("message").and_then(Value::as_str) {
                Some("answered") if mine => {
                    return Ok(message.get("answer").cloned().unwrap_or(Value::Null));
                }
                Some("reply") if mine => {
                    return Ok(message.get("reply").cloned().unwrap_or(Value::Null));
                }
                Some("refused") if mine => {
                    return Err(format!(
                        "the browser refused it: {}",
                        show(message.get("error"))
                    ));
                }
                // A failure with no id is the one message that cannot be
                // attributed, and is therefore everybody's.
                Some("failed") if mine || message.get("id").is_none_or(Value::is_null) => {
                    return Err(show(message.get("why")));
                }
                _ => {
                    if let Some(news) = as_news(&message) {
                        self.news.push_back(news);
                    }
                }
            }
        }
    }

    async fn next_message(&mut self) -> Result<Value, String> {
        self.lines
            .recv()
            .await
            .ok_or_else(|| "the browser closed its socket".to_string())
    }

    /// Quit the browser this plugin started, and take its socket with it.
    /// A browser somebody else runs is left exactly as it was found.
    pub async fn close(mut self) {
        if self.child.is_none() {
            return;
        }
        let _ = self.command(json!({ "command": "quit" })).await;
        if let Some(mut child) = self.child.take()
            && tokio::time::timeout(QUIT_GRACE, child.wait())
                .await
                .is_err()
        {
            let _ = child.start_kill();
        }
        if let Some(path) = &self.ours {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A socket nobody else will pick: this process, in the directory the
/// system clears for us.
fn private_socket() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join(format!("nyd-auth-drunken-{}.sock", std::process::id()))
}

fn as_news(message: &Value) -> Option<Value> {
    match message.get("message").and_then(Value::as_str) {
        Some("news") => message.get("news").cloned(),
        _ => None,
    }
}

/// A reason out of a message, in words, whatever shape it arrived in.
fn show(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "no reason given".to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}
