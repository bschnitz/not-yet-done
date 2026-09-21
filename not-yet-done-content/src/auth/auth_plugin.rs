//! The line protocol behind
//! [`CredentialProvider::Plugin`](super::CredentialProvider::Plugin) — see
//! ADR 0010.
//!
//! A credential script (see [`credential_script`](super::credential_script))
//! is a fresh process per round and remembers nothing, which is a virtue for
//! a password store and fatal for a login only a browser can perform: the
//! half-finished session *is* the process's state, so round two would open a
//! second browser onto a login the first one had already half-passed.
//!
//! An auth plugin is therefore started **once** and answered on its own
//! stdio. It may say five things, in any order and any number of times until
//! one of the last two ends the conversation:
//!
//! - `step` — what it is doing now. Becomes the login's reported step.
//! - `attention` — a step that will not move until a person acts, and
//!   `null` to take it back. There is nothing to submit: the user taps a
//!   phone, and all the login can do is say so and keep waiting.
//! - `form` — ask the user this, then hand me the answers. The same
//!   [`ScriptForm`] a credential script asks with.
//! - `result` — here are the values, we are done.
//! - `error` — give up, here is why.
//!
//! **stdout is the protocol, stderr is the log.** A plugin's diagnostics go
//! to stderr, and from there into a file of its own in nyd's log directory
//! (`<log dir>/plugin-<name>.log`) — not, unlike a credential script's, into
//! a pipe quoted back on failure: a long-lived process writing into a pipe
//! nobody drains until it exits would eventually block on a full one, and
//! the terminal nyd inherited may be a TUI's alternate screen, where a
//! child's line lands across the drawing and stays there. The file is named
//! in the error when a plugin fails, and can be tailed while one runs. A
//! plugin that wants something shown to the user says `error`.
//!
//! Secrets travel in `result` and nowhere else: `step`, `attention` and
//! `form` reach the status channel, the log and possibly a desktop
//! notification. Nothing here can check that, which makes it part of the
//! plugin's contract.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;

use child_log::ChildLog;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

use super::CredentialError;
use super::credential_script::{ScriptForm, check_form};

/// The protocol version nyd announces in `start`. A plugin that cannot speak
/// it answers `error` — which is why the version travels in the opening line
/// rather than in a handshake round-trip of its own.
pub const PROTOCOL: u32 = 1;

/// How long a plugin gets to exit on its own after `cancel` or after its
/// stdin closes, before it is killed. Long enough to close a browser, short
/// enough not to hold up a login that already has its answer.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// What nyd writes to the plugin, one JSON object per line.
///
/// Readable as well as writable, and owned rather than borrowed, so that a
/// plugin written in Rust reads the line nyd wrote instead of transcribing
/// its shape into a second set of types — the drift between two spellings of
/// one protocol being the thing this module exists to prevent. The few
/// allocations that costs happen once per login.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ToPlugin {
    /// Once, first.
    Start {
        protocol: u32,
        /// The field names bound to this plugin, so one generic plugin can
        /// serve several adapters without guessing.
        request: Vec<String>,
        /// The session blob that just expired, so a plugin may refresh
        /// instead of logging in again. `null` when there is nothing to
        /// refresh: a first login, or one the server rejected.
        session: Option<String>,
    },
    /// Answers to the form last asked — every answer collected so far, as
    /// [`ScriptRequest`](super::credential_script::ScriptRequest) carries
    /// them. A plugin does remember its own state, but a re-asked form's
    /// earlier answers are the runtime's to keep, not the plugin's.
    Input(BTreeMap<String, String>),
    /// Give up and shut down.
    Cancel {},
}

/// One line from the plugin, after validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginSaid {
    /// What the plugin is doing now.
    Step(String),
    /// The login is waiting on a person doing something elsewhere.
    Attention(String),
    /// That wait is over.
    AttentionOver,
    /// Ask the user this, then send the answers back.
    Form(ScriptForm),
    /// The requested values. Ends the conversation.
    Values {
        values: BTreeMap<String, String>,
        /// When the values stop being usable, if the plugin knows — an
        /// upper bound on the configured session policy, never an
        /// extension of it.
        expires_at_unix_ms: Option<u64>,
    },
    /// The plugin gave up; the message is shown to the user.
    Failed(String),
}

/// The five answer shapes as they travel. Kept separate from
/// [`PluginSaid`] so "exactly one of them" is checked in [`parse_line`]
/// rather than being expressible in the type the caller sees.
///
/// A plugin writes these; nyd reads them. The constructors below are the
/// writing half, so a plugin never has to know which key goes with which
/// and cannot put two of them on one line.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginLine {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// A `serde_json::Value` because an explicit `null` has to be told
    /// apart from an absent key: the one withdraws the attention, the
    /// other says nothing about it. `Option<Value>` alone cannot — serde
    /// reads a null into `None`, the same as no key at all — hence
    /// [`present`], which only runs when the key is there. Writing it back
    /// out works the same way round: `Some(Value::Null)` is the withdrawal,
    /// `None` is silence and is left off the line.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub attention: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<ScriptForm>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<BTreeMap<String, String>>,
    /// A sibling of `result`, not a shape of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl PluginLine {
    /// What the plugin is doing now.
    pub fn step(step: impl Into<String>) -> Self {
        Self {
            step: Some(step.into()),
            ..Self::default()
        }
    }

    /// The login is waiting on a person doing something elsewhere.
    pub fn attention(said: impl Into<String>) -> Self {
        Self {
            attention: Some(serde_json::Value::String(said.into())),
            ..Self::default()
        }
    }

    /// That wait is over.
    pub fn attention_over() -> Self {
        Self {
            attention: Some(serde_json::Value::Null),
            ..Self::default()
        }
    }

    /// Ask the user this.
    pub fn form(form: ScriptForm) -> Self {
        Self {
            form: Some(form),
            ..Self::default()
        }
    }

    /// The requested values, and when they stop being usable if that is
    /// known. Ends the conversation.
    pub fn result(values: BTreeMap<String, String>, expires_at_unix_ms: Option<u64>) -> Self {
        Self {
            result: Some(values),
            expires_at_unix_ms,
            ..Self::default()
        }
    }

    /// Give up, and say why in words the user will read.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            error: Some(message.into()),
            ..Self::default()
        }
    }
}

/// Deserialize a value that is present, `null` included, into `Some`.
///
/// Paired with `#[serde(default)]`, which is what supplies the `None` for
/// a key that is not there at all.
fn present<'de, D>(deserializer: D) -> Result<Option<serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    serde_json::Value::deserialize(deserializer).map(Some)
}

/// One plugin process, from `start` to its last word.
pub struct PluginSession {
    /// How the plugin is named in messages — configured by hand, so which
    /// one is misbehaving is the useful half.
    who: String,
    /// Where the plugin's stderr went, so a failure can say where to look.
    log: ChildLog,
    request: Vec<String>,
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
}

impl PluginSession {
    /// Start the plugin under `sh -c` and hand it the opening line.
    pub async fn start(
        name: &str,
        command: &str,
        request: &[&str],
        session: Option<&str>,
    ) -> Result<Self, CredentialError> {
        let who = format!("auth plugin `{name}`");
        // See the module doc: the log is not the protocol, a pipe nobody
        // drains is a pipe that eventually blocks, and the stderr this
        // process inherited may belong to a TUI holding the screen.
        let log = ChildLog::named(crate::http_log::log_directory(), &format!("plugin-{name}"));
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(log.stdio())
            // A backstop for the paths that cannot await a shutdown — a
            // dropped session must not leave a browser running.
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            CredentialError::ProviderError(format!("{who}: spawn `{command}`: {e}"))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| CredentialError::ProviderError(format!("{who}: stdin pipe missing")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CredentialError::ProviderError(format!("{who}: stdout pipe missing")))?;

        let mut me = Self {
            who,
            log,
            request: request.iter().map(|f| (*f).to_string()).collect(),
            child,
            stdin,
            lines: BufReader::new(stdout).lines(),
        };
        me.write(&ToPlugin::Start {
            protocol: PROTOCOL,
            request: me.request.clone(),
            session: session.map(str::to_string),
        })
        .await?;
        Ok(me)
    }

    /// Where the plugin's own diagnostics went — the file to point somebody
    /// at when what it said is not enough.
    pub fn log_path(&self) -> &std::path::Path {
        self.log.path()
    }

    /// The plugin's next word, waited for no longer than `patience`.
    ///
    /// The deadline covers the whole wait rather than restarting per line,
    /// so a plugin cannot hold a login open by printing blank lines.
    pub async fn next(&mut self, patience: Duration) -> Result<PluginSaid, CredentialError> {
        let deadline = tokio::time::Instant::now() + patience;
        loop {
            let line = match tokio::time::timeout_at(deadline, self.lines.next_line()).await {
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) => return Err(self.ended_early().await),
                Ok(Err(e)) => {
                    return Err(CredentialError::ProviderError(format!(
                        "{}: reading its output: {e}",
                        self.who
                    )));
                }
                Err(_) => {
                    return Err(CredentialError::ProviderError(format!(
                        "{} said nothing for {}s; what it was doing is in {}",
                        self.who,
                        patience.as_secs(),
                        self.log.path().display()
                    )));
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            return parse_line(&self.who, &self.request, &line);
        }
    }

    /// Hand the plugin the answers to the form it asked for.
    pub async fn answer(
        &mut self,
        input: &BTreeMap<String, String>,
    ) -> Result<(), CredentialError> {
        self.write(&ToPlugin::Input(input.clone())).await
    }

    /// The plugin has given its values: close stdin and let it go.
    pub async fn finish(self) {
        self.shutdown(false).await;
    }

    /// Tell the plugin to give up, and give it time to clean up. A plugin
    /// that owns a browser has a window to close, and killing its parent
    /// without a word is how a browser process is left behind.
    pub async fn cancel(self) {
        self.shutdown(true).await;
    }

    async fn shutdown(mut self, tell_it: bool) {
        if tell_it {
            let _ = self.write(&ToPlugin::Cancel {}).await;
        }
        // Closing stdin is the second half of the message: a plugin
        // reading to EOF learns there is nothing more to answer.
        drop(self.stdin);
        if tokio::time::timeout(SHUTDOWN_GRACE, self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.start_kill();
        }
    }

    async fn write(&mut self, line: &ToPlugin) -> Result<(), CredentialError> {
        let mut payload = serde_json::to_string(line).map_err(|e| {
            CredentialError::ProviderError(format!("{}: encoding a line: {e}", self.who))
        })?;
        payload.push('\n');
        self.stdin
            .write_all(payload.as_bytes())
            .await
            .map_err(|e| {
                CredentialError::ProviderError(format!("{}: writing to it: {e}", self.who))
            })?;
        self.stdin.flush().await.map_err(|e| {
            CredentialError::ProviderError(format!("{}: writing to it: {e}", self.who))
        })
    }

    /// Stdout closed before a `result` or an `error` arrived. The exit
    /// status is all there is to go on — the plugin's own diagnostics went
    /// to stderr, where a person can read them.
    /// Closed stdout means it is on its way out, but the exit status may
    /// not be collectable yet — so this waits for it rather than asking
    /// once and reporting a plugin that "just stopped", which is the same
    /// sentence for a crash and for a clean refusal.
    async fn ended_early(&mut self) -> CredentialError {
        let status = match tokio::time::timeout(SHUTDOWN_GRACE, self.child.wait()).await {
            Ok(Ok(status)) => match status.code() {
                Some(code) => format!(" (exited {code})"),
                None => " (killed)".to_string(),
            },
            _ => String::new(),
        };
        CredentialError::ProviderError(format!(
            "{} stopped talking before it said `result` or `error`{status}; \
             its own account of it is in {}",
            self.who,
            self.log.path().display()
        ))
    }
}

/// Read one line of the plugin's output.
///
/// `request` doubles as the completeness check, as it does for a credential
/// script: a `result` missing one of the requested names is a plugin bug,
/// and catching it here beats an adapter later reporting a login failure
/// for an absent field.
fn parse_line(who: &str, request: &[String], line: &str) -> Result<PluginSaid, CredentialError> {
    let raw: PluginLine = serde_json::from_str(line).map_err(|e| {
        CredentialError::ProviderError(format!("{who}: this is not a protocol line ({e})"))
    })?;

    let set = raw.step.is_some() as u8
        + raw.attention.is_some() as u8
        + raw.form.is_some() as u8
        + raw.result.is_some() as u8
        + raw.error.is_some() as u8;
    if set != 1 {
        return Err(CredentialError::ProviderError(format!(
            "{who}: expected exactly one of `step`, `attention`, `form`, \
             `result` or `error` on a line, got {set}"
        )));
    }
    if raw.expires_at_unix_ms.is_some() && raw.result.is_none() {
        return Err(CredentialError::ProviderError(format!(
            "{who}: `expires_at_unix_ms` belongs to a `result`"
        )));
    }

    if let Some(step) = raw.step {
        let step = step.trim().to_string();
        if step.is_empty() {
            return Err(CredentialError::ProviderError(format!(
                "{who} named a step with no words in it"
            )));
        }
        return Ok(PluginSaid::Step(step));
    }

    if let Some(attention) = raw.attention {
        return match attention {
            serde_json::Value::Null => Ok(PluginSaid::AttentionOver),
            serde_json::Value::String(said) => {
                let said = said.trim().to_string();
                if said.is_empty() {
                    // An empty string would read as "attend to nothing".
                    // Withdrawing the attention is spelled `null`.
                    return Err(CredentialError::ProviderError(format!(
                        "{who} asked for attention without saying what for"
                    )));
                }
                Ok(PluginSaid::Attention(said))
            }
            _ => Err(CredentialError::ProviderError(format!(
                "{who}: `attention` is a sentence for the user, or `null`"
            ))),
        };
    }

    if let Some(form) = raw.form {
        check_form(who, &form)?;
        return Ok(PluginSaid::Form(form));
    }

    if let Some(error) = raw.error {
        let error = error.trim().to_string();
        if error.is_empty() {
            return Err(CredentialError::ProviderError(format!(
                "{who} reported an empty error"
            )));
        }
        return Ok(PluginSaid::Failed(error));
    }

    let values = raw.result.unwrap_or_default();
    let missing: Vec<&str> = request
        .iter()
        .map(String::as_str)
        .filter(|f| !values.contains_key(*f))
        .collect();
    if !missing.is_empty() {
        return Err(CredentialError::Unavailable(format!(
            "{who} returned no value for {}",
            missing
                .iter()
                .map(|f| format!("`{f}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(PluginSaid::Values {
        values,
        expires_at_unix_ms: raw.expires_at_unix_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATIENCE: Duration = Duration::from_secs(10);

    fn request(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    /// Write an executable plugin and return the command to run it by.
    fn plugin(dir: &std::path::Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write plugin");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod plugin");
        path.display().to_string()
    }

    #[test]
    fn a_line_says_exactly_one_thing() {
        let who = "auth plugin `p`";
        let err = parse_line(who, &request(&["cookie"]), r#"{"step":"a","error":"b"}"#)
            .expect_err("two shapes on one line");
        assert!(format!("{err}").contains("exactly one of"), "{err}");

        let err = parse_line(who, &request(&["cookie"]), "{}").expect_err("no shape at all");
        assert!(format!("{err}").contains("got 0"), "{err}");
    }

    #[test]
    fn attention_is_withdrawn_by_null_and_not_by_an_empty_sentence() {
        let who = "auth plugin `p`";
        let names = request(&["cookie"]);
        assert_eq!(
            parse_line(who, &names, r#"{"attention":null}"#).expect("a withdrawal"),
            PluginSaid::AttentionOver
        );
        assert_eq!(
            parse_line(who, &names, r#"{"attention":"Tap your phone"}"#).expect("a wait"),
            PluginSaid::Attention("Tap your phone".into())
        );
        let err = parse_line(who, &names, r#"{"attention":"  "}"#).expect_err("nothing said");
        assert!(
            format!("{err}").contains("without saying what for"),
            "{err}"
        );
    }

    #[test]
    fn a_result_must_carry_every_field_that_was_asked_for() {
        let who = "auth plugin `p`";
        let names = request(&["cookie", "xsrf-token"]);
        let err =
            parse_line(who, &names, r#"{"result":{"cookie":"c"}}"#).expect_err("one field short");
        assert!(format!("{err}").contains("`xsrf-token`"), "{err}");

        let said = parse_line(
            who,
            &names,
            r#"{"result":{"cookie":"c","xsrf-token":"x"},"expires_at_unix_ms":7}"#,
        )
        .expect("both fields");
        assert_eq!(
            said,
            PluginSaid::Values {
                values: [("cookie", "c"), ("xsrf-token", "x")]
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                expires_at_unix_ms: Some(7),
            }
        );
    }

    #[test]
    fn an_expiry_without_a_result_is_a_line_that_means_nothing() {
        let err = parse_line(
            "auth plugin `p`",
            &request(&["cookie"]),
            r#"{"step":"a","expires_at_unix_ms":7}"#,
        )
        .expect_err("an expiry with no values");
        assert!(format!("{err}").contains("belongs to a `result`"), "{err}");
    }

    #[tokio::test]
    async fn a_plugin_is_started_once_and_talks_until_it_has_the_values() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Reads the opening line, reports two steps, and answers with a
        // value taken from the request it was handed — proving `start`
        // arrived and one process served the whole conversation.
        let command = plugin(
            dir.path(),
            "steps.sh",
            r#"#!/bin/sh
read -r start
echo '{"step":"opening the page"}'
echo '{"step":"signing in"}'
case "$start" in
  *'"protocol":1'*) echo '{"result":{"cookie":"asked with protocol 1"}}' ;;
  *) echo '{"error":"no protocol in the opening line"}' ;;
esac
"#,
        );
        let mut session = PluginSession::start("p", &command, &["cookie"], None)
            .await
            .expect("started");
        assert_eq!(
            session.next(PATIENCE).await.expect("a step"),
            PluginSaid::Step("opening the page".into())
        );
        assert_eq!(
            session.next(PATIENCE).await.expect("a step"),
            PluginSaid::Step("signing in".into())
        );
        let PluginSaid::Values { values, .. } = session.next(PATIENCE).await.expect("values")
        else {
            panic!("expected values");
        };
        assert_eq!(values["cookie"], "asked with protocol 1");
        session.finish().await;
    }

    #[tokio::test]
    async fn a_form_is_answered_on_the_same_process_that_asked() {
        let dir = tempfile::tempdir().expect("tempdir");
        let command = plugin(
            dir.path(),
            "asks.sh",
            r#"#!/bin/sh
read -r _start
echo '{"form":{"header":"Second factor","fields":[{"name":"otp","masked":true}]}}'
read -r input
echo '{"attention":"Approve it in your app"}'
echo '{"attention":null}'
printf '{"result":{"cookie":"%s"}}\n' "$(echo "$input" | sed 's/.*"otp":"\([^"]*\)".*/\1/')"
"#,
        );
        let mut session = PluginSession::start("p", &command, &["cookie"], None)
            .await
            .expect("started");
        let PluginSaid::Form(form) = session.next(PATIENCE).await.expect("a form") else {
            panic!("expected a form");
        };
        assert_eq!(form.header.as_deref(), Some("Second factor"));
        assert_eq!(form.fields[0].name, "otp");

        session
            .answer(
                &[("otp".to_string(), "424242".to_string())]
                    .into_iter()
                    .collect(),
            )
            .await
            .expect("answered");
        assert_eq!(
            session.next(PATIENCE).await.expect("attention"),
            PluginSaid::Attention("Approve it in your app".into())
        );
        assert_eq!(
            session.next(PATIENCE).await.expect("attention over"),
            PluginSaid::AttentionOver
        );
        let PluginSaid::Values { values, .. } = session.next(PATIENCE).await.expect("values")
        else {
            panic!("expected values");
        };
        assert_eq!(values["cookie"], "424242");
        session.finish().await;
    }

    #[tokio::test]
    async fn a_plugin_that_stops_talking_is_not_a_login_that_hangs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let command = plugin(
            dir.path(),
            "quits.sh",
            "#!/bin/sh\nread -r _start\nexit 3\n",
        );
        let mut session = PluginSession::start("p", &command, &["cookie"], None)
            .await
            .expect("started");
        let err = session.next(PATIENCE).await.expect_err("it left");
        let message = format!("{err}");
        assert!(message.contains("stopped talking"), "{message}");
        assert!(message.contains("exited 3"), "{message}");
    }

    #[tokio::test]
    async fn silence_ends_the_wait_rather_than_the_login_waiting_forever() {
        let dir = tempfile::tempdir().expect("tempdir");
        let command = plugin(dir.path(), "mute.sh", "#!/bin/sh\nsleep 30\n");
        let mut session = PluginSession::start("p", &command, &["cookie"], None)
            .await
            .expect("started");
        let err = session
            .next(Duration::from_millis(150))
            .await
            .expect_err("nothing came");
        assert!(format!("{err}").contains("said nothing"), "{err}");
        session.cancel().await;
    }
}
