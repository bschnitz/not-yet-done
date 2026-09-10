//! The plugin against a browser that is not one.
//!
//! A fake on the other end of the socket is what makes this testable at
//! all: a real drunken-browser wants CEF, a display and a login somewhere
//! to perform, and none of that is what this crate does. What it does is
//! turn one vocabulary into the other, and the way to check that is to
//! speak the browser's half and read nyd's.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixListener;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// Long enough for a loaded machine, short enough that a plugin which is
/// waiting for something that will never come fails the test rather than
/// the suite's own deadline.
const PATIENCE: Duration = Duration::from_secs(10);

/// A plugin under test, with both of its conversations in reach.
struct Fixture {
    child: Child,
    to_plugin: ChildStdin,
    from_plugin: Lines<BufReader<ChildStdout>>,
    browser: Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    to_nyd_socket: tokio::net::unix::OwnedWriteHalf,
    _dir: tempfile::TempDir,
}

impl Fixture {
    /// Start the plugin against a socket this test answers on, and hand it
    /// the opening line.
    async fn start(request: &[&str], extra: &[&str]) -> Self {
        let dir = tempfile::tempdir().expect("a directory");
        let socket = dir.path().join("browser.sock");
        // Bound before the plugin runs: its first act is to connect, and a
        // listener that is not there yet is a race, not a test.
        let listener = UnixListener::bind(&socket).expect("bind");

        let mut child = Command::new(env!("CARGO_BIN_EXE_nyd-auth-drunken"))
            .arg("--socket")
            .arg(&socket)
            .arg("--flow")
            .arg("login")
            .arg("--show")
            .arg("never")
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn the plugin");

        let mut to_plugin = child.stdin.take().expect("stdin");
        let from_plugin = BufReader::new(child.stdout.take().expect("stdout")).lines();

        let start = json!({ "start": { "protocol": 1, "request": request, "session": null } });
        write(&mut to_plugin, &start).await;

        let (stream, _) = tokio::time::timeout(PATIENCE, listener.accept())
            .await
            .expect("the plugin connected")
            .expect("accept");
        let (read, to_nyd_socket) = stream.into_split();

        Self {
            child,
            to_plugin,
            from_plugin,
            browser: BufReader::new(read).lines(),
            to_nyd_socket,
            _dir: dir,
        }
    }

    /// The next line the plugin said to nyd.
    async fn said(&mut self) -> Value {
        let line = tokio::time::timeout(PATIENCE, self.from_plugin.next_line())
            .await
            .expect("the plugin said something")
            .expect("read")
            .expect("the plugin closed stdout early");
        serde_json::from_str(&line).expect("a protocol line")
    }

    /// The next line the plugin sent the browser.
    async fn asked(&mut self) -> Value {
        let line = tokio::time::timeout(PATIENCE, self.browser.next_line())
            .await
            .expect("the plugin asked something")
            .expect("read")
            .expect("the plugin closed the socket early");
        serde_json::from_str(&line).expect("a wire line")
    }

    /// Answer whatever the plugin last asked the window, as the window.
    async fn answer(&mut self, ask: &str) -> Value {
        let line = self.asked().await;
        let host = line.get("host").cloned().unwrap_or(Value::Null);
        assert_eq!(
            host.get("ask").and_then(Value::as_str),
            Some(ask),
            "expected the plugin to ask `{ask}`, got {line}"
        );
        let id = line.get("id").cloned().unwrap();
        let answer = json!({ "message": "answered", "id": id, "answer": { "answer": "said", "said": null } });
        write_socket(&mut self.to_nyd_socket, &answer).await;
        host
    }

    /// Say something to the plugin as the window does, unasked.
    async fn news(&mut self, news: Value) {
        write_socket(
            &mut self.to_nyd_socket,
            &json!({ "message": "news", "news": news }),
        )
        .await;
    }

    /// Answer the form the plugin asked nyd for.
    async fn input(&mut self, answers: &[(&str, &str)]) {
        let answers: BTreeMap<&str, &str> = answers.iter().copied().collect();
        write(&mut self.to_plugin, &json!({ "input": answers })).await;
    }

    async fn finished(mut self) -> std::process::ExitStatus {
        tokio::time::timeout(PATIENCE, self.child.wait())
            .await
            .expect("the plugin exited")
            .expect("wait")
    }

    /// Get the plugin past opening the flow and starting the run.
    async fn running(&mut self) {
        assert_eq!(
            step(&self.said().await),
            Some("opening `login`".to_string())
        );
        let opened = self.answer("line").await;
        assert_eq!(
            opened.get("line").and_then(Value::as_str),
            Some("test-open login")
        );
        let run = self.answer("flow_run").await;
        assert_eq!(run.get("what").and_then(Value::as_str), Some("whole"));
    }
}

async fn write(to: &mut ChildStdin, value: &Value) {
    to.write_all(format!("{value}\n").as_bytes())
        .await
        .expect("write");
    to.flush().await.expect("flush");
}

async fn write_socket(to: &mut tokio::net::unix::OwnedWriteHalf, value: &Value) {
    to.write_all(format!("{value}\n").as_bytes())
        .await
        .expect("write");
    to.flush().await.expect("flush");
}

fn step(said: &Value) -> Option<String> {
    said.get("step").and_then(Value::as_str).map(str::to_string)
}

/// The whole of an ordinary login: the flow's steps become nyd's, and what
/// it yields becomes the credential.
#[tokio::test]
async fn a_flow_that_passes_hands_back_what_it_yielded() {
    let mut plugin = Fixture::start(&["cookie"], &["--field", "cookie=session"]).await;
    plugin.running().await;

    plugin
        .news(json!({ "news": "started", "flow": "login" }))
        .await;
    assert_eq!(
        step(&plugin.said().await),
        Some("running `login`".to_string())
    );

    plugin
        .news(json!({
            "news": "run",
            "happened": { "happened": "began", "step": 0, "name": "sign in" }
        }))
        .await;
    assert_eq!(step(&plugin.said().await), Some("sign in".to_string()));

    plugin
        .news(json!({
            "news": "over",
            "tally": { "ran": 1, "failed": 0, "broke": 0 },
            "stopped": false,
            "yielded": { "session": "JSESSIONID=grape", "expires_at_unix_ms": 1_700_000_000_000u64 }
        }))
        .await;

    let result = plugin.said().await;
    assert_eq!(
        result
            .get("result")
            .and_then(|r| r.get("cookie"))
            .and_then(Value::as_str),
        Some("JSESSIONID=grape")
    );
    assert_eq!(
        result.get("expires_at_unix_ms").and_then(Value::as_u64),
        Some(1_700_000_000_000)
    );
    assert!(plugin.finished().await.success());
}

/// A `tell:` is the case nyd's attention flag exists for, and the next
/// thing the run does is what takes it back.
#[tokio::test]
async fn telling_somebody_something_is_an_attention_the_next_step_withdraws() {
    let mut plugin = Fixture::start(&["cookie"], &["--field", "cookie=session"]).await;
    plugin.running().await;

    plugin
        .news(json!({ "news": "told", "said": "Approve the sign-in on your phone" }))
        .await;
    assert_eq!(
        plugin.said().await.get("attention").and_then(Value::as_str),
        Some("Approve the sign-in on your phone")
    );

    plugin
        .news(json!({
            "news": "run",
            "happened": { "happened": "began", "step": 1, "name": "wait for the redirect" }
        }))
        .await;
    // The withdrawal comes first: the wait is over before the step that
    // follows it is announced.
    assert_eq!(plugin.said().await.get("attention"), Some(&Value::Null));
    assert_eq!(
        step(&plugin.said().await),
        Some("wait for the redirect".to_string())
    );
}

/// A hole the run asks for becomes a form, and the answer goes back in.
#[tokio::test]
async fn a_hole_becomes_a_form_and_its_answer_goes_to_the_run() {
    let mut plugin = Fixture::start(&["cookie"], &["--field", "cookie=session"]).await;
    plugin.running().await;

    plugin
        .news(json!({ "news": "asking", "hole": "{{secret.otp}}" }))
        .await;
    let form = plugin.said().await;
    let fields = form
        .get("form")
        .and_then(|f| f.get("fields"))
        .and_then(Value::as_array)
        .expect("a form with fields")
        .clone();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].get("name").and_then(Value::as_str), Some("otp"));
    assert_eq!(fields[0].get("masked").and_then(Value::as_bool), Some(true));

    plugin.input(&[("otp", "424242")]).await;
    let answered = plugin.answer("flow_answer").await;
    assert_eq!(
        answered.get("value").and_then(Value::as_str),
        Some("424242")
    );
}

/// A flow that fails is a login that failed, said in words rather than
/// left for nyd to guess from an exit code.
#[tokio::test]
async fn a_flow_that_fails_is_reported_as_an_error() {
    let mut plugin = Fixture::start(&["cookie"], &[]).await;
    plugin.running().await;

    plugin
        .news(json!({
            "news": "over",
            "tally": { "ran": 2, "failed": 1, "broke": 0 },
            "stopped": false,
            "yielded": {}
        }))
        .await;

    let error = plugin.said().await;
    let text = error
        .get("error")
        .and_then(Value::as_str)
        .expect("an error");
    assert!(text.contains("1 failed"), "{text}");
    assert!(!plugin.finished().await.success());
}

/// A flow that passes but hands back nothing is the other half of the same
/// promise, and the message says which yield was looked for.
#[tokio::test]
async fn a_flow_that_yields_nothing_says_what_was_missing() {
    let mut plugin = Fixture::start(&["cookie"], &["--field", "cookie=session"]).await;
    plugin.running().await;

    plugin
        .news(json!({
            "news": "over",
            "tally": { "ran": 1, "failed": 0, "broke": 0 },
            "stopped": false,
            "yielded": { "elsewhere": "x" }
        }))
        .await;

    let text = plugin
        .said()
        .await
        .get("error")
        .and_then(Value::as_str)
        .expect("an error")
        .to_string();
    assert!(text.contains("`cookie` (from `session`)"), "{text}");
    assert!(text.contains("`elsewhere`"), "{text}");
}

/// `cancel` stops the run rather than only stopping this process: a
/// browser left running a half-finished login is the thing the whole
/// long-lived-process design was for.
#[tokio::test]
async fn cancel_stops_the_run() {
    let mut plugin = Fixture::start(&["cookie"], &[]).await;
    plugin.running().await;

    write(&mut plugin.to_plugin, &json!({ "cancel": {} })).await;
    let stop = plugin.asked().await;
    assert_eq!(
        stop.get("host")
            .and_then(|h| h.get("line"))
            .and_then(Value::as_str),
        Some("test-stop")
    );
}

/// nyd's protocol version is checked at the door, because the alternative
/// is a plugin misreading a line it was never meant to be handed.
#[tokio::test]
async fn a_protocol_this_plugin_does_not_speak_is_refused() {
    let dir = tempfile::tempdir().expect("a directory");
    let socket: PathBuf = dir.path().join("browser.sock");
    let _listener = UnixListener::bind(&socket).expect("bind");

    let mut child = Command::new(env!("CARGO_BIN_EXE_nyd-auth-drunken"))
        .args([
            "--socket".as_ref(),
            socket.as_os_str(),
            "--flow".as_ref(),
            "login".as_ref(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn");

    let mut to_plugin = child.stdin.take().expect("stdin");
    let mut from_plugin = BufReader::new(child.stdout.take().expect("stdout")).lines();
    write(
        &mut to_plugin,
        &json!({ "start": { "protocol": 99, "request": ["cookie"], "session": null } }),
    )
    .await;

    let line = tokio::time::timeout(PATIENCE, from_plugin.next_line())
        .await
        .expect("it answered")
        .expect("read")
        .expect("a line");
    let said: Value = serde_json::from_str(&line).expect("a protocol line");
    let text = said.get("error").and_then(Value::as_str).expect("an error");
    assert!(text.contains("protocol 1"), "{text}");
    assert!(text.contains("99"), "{text}");
}

/// The plugin never opens a socket of its own here — but it must be able
/// to say so when the one it was pointed at is not there.
#[tokio::test]
async fn a_socket_with_no_browser_on_it_is_an_error_nyd_can_read() {
    let dir = tempfile::tempdir().expect("a directory");
    let socket = dir.path().join("nothing.sock");

    let mut child = Command::new(env!("CARGO_BIN_EXE_nyd-auth-drunken"))
        .args([
            "--socket".as_ref(),
            socket.as_os_str(),
            "--flow".as_ref(),
            "login".as_ref(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn");

    let mut to_plugin = child.stdin.take().expect("stdin");
    let mut from_plugin = BufReader::new(child.stdout.take().expect("stdout")).lines();
    write(
        &mut to_plugin,
        &json!({ "start": { "protocol": 1, "request": ["cookie"], "session": null } }),
    )
    .await;

    let line = tokio::time::timeout(PATIENCE, from_plugin.next_line())
        .await
        .expect("it answered")
        .expect("read")
        .expect("a line");
    let said: Value = serde_json::from_str(&line).expect("a protocol line");
    assert!(
        said.get("error")
            .and_then(Value::as_str)
            .is_some_and(|why| why.contains("no browser on")),
        "{said}"
    );
}
