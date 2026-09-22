//! Running a form script: one process per round, the payload in a file, the
//! answer in `$NYD_OUTPUT_FILE`.
//!
//! The transport is the one view scripts already use — the request is written
//! to a temp file whose path is the script's `argv[1]`, and the script writes
//! its answer to the path in `$NYD_OUTPUT_FILE`. stdout stays free on purpose:
//! it is where a `print()`, a library warning or a stray `set -x` lands, and
//! under an interactive mode it belongs to the user's terminal anyway.
//!
//! [`FormSession`] owns the loop but does not drive it. `next` runs exactly
//! one round and comes back; who waits for the user in between — a popup in
//! the TUI, `fieldsmith` prompts in the CLI — is the frontend's business.
//! Anything else would mean a library that blocks an event loop.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use not_yet_done_content::script_form::{
    advance, build_request, parse_round, FormState, ScriptFormError, ScriptRound,
    CHATTY_ROUND_COUNT,
};
use serde_json::Value;
use thiserror::Error;

/// Why a round could not be run or believed.
#[derive(Debug, Error)]
pub enum FormRunError {
    /// The request could not be handed over, or the answer not picked up.
    #[error("{0}")]
    Io(String),
    /// The process itself failed.
    #[error("{0}")]
    Crashed(String),
    /// The script ran, but what it said cannot be used.
    #[error(transparent)]
    Protocol(#[from] ScriptFormError),
}

/// What the frontend does next.
#[derive(Debug, Clone, PartialEq)]
pub enum FormStep {
    /// Show this form and come back with the values.
    Ask(FormState),
    /// The conversation is over; this is the ordinary script answer, to be
    /// handed to the usual script path (`commands` and friends).
    Done(Value),
    /// The script gave up. The message is for the user.
    Failed(String),
}

/// One round's outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct Round {
    pub step: FormStep,
    /// Worth showing the user or writing to the log: whatever the script put
    /// on stderr, plus anything about the answer that was odd without being
    /// fatal (an error naming a field that does not exist).
    pub notices: Vec<String>,
}

/// `ETXTBSY`, the kernel's "someone still has this file open for writing".
/// Spelled out because `io::ErrorKind` has no stable name for it yet; the
/// number is the same on Linux and the BSDs.
const ETXTBSY: i32 = 26;

/// How often a spawn is worth retrying while the file is still busy. Five
/// tries spread over ~30 ms: long enough for a sibling process to reach its
/// own `exec`, short enough that a genuinely locked file still fails fast.
const TEXT_FILE_BUSY_RETRIES: u64 = 5;

/// Keeps two sessions in one process from writing over each other's files —
/// a hook opening a form while a form is on screen is unusual but not
/// forbidden.
static NEXT_SESSION: AtomicUsize = AtomicUsize::new(0);

/// One conversation with one form script.
///
/// Built once, then asked for round after round. The form as it stands is
/// [`FormSession::state`]; the request for the next round is assembled from
/// the scope payload, the values just submitted and the script's own opaque
/// state.
pub struct FormSession {
    script: PathBuf,
    /// How the script is named in messages — its file name, which is what
    /// the user picked in the menu.
    label: String,
    payload: Value,
    env: HashMap<String, String>,
    request_path: PathBuf,
    answer_path: PathBuf,
    state: Option<FormState>,
    rounds: usize,
}

impl FormSession {
    /// Start a conversation. `payload` is the ordinary `scope:` payload —
    /// `{"node": …}`, `{"rows": …}` and so on; the protocol adds `input` and
    /// `state` to it.
    pub fn new(script: impl Into<PathBuf>, payload: Value) -> Self {
        let script = script.into();
        let label = script
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| script.to_string_lossy().into_owned());
        let id = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!("nyd-form-{}-{id}", std::process::id()));
        Self {
            script,
            label,
            payload,
            env: HashMap::new(),
            request_path: base.with_extension("request.json"),
            answer_path: base.with_extension("answer.json"),
            state: None,
            rounds: 0,
        }
    }

    /// Environment for the child — the adapter's variables. `NYD_OUTPUT_FILE`
    /// is set afterwards, so the `NYD_*` namespace stays ours whatever an
    /// adapter exposes.
    pub fn with_env(mut self, env: HashMap<String, String>) -> Self {
        self.env = env;
        self
    }

    /// The form as it stands, or `None` before the first round.
    pub fn state(&self) -> Option<&FormState> {
        self.state.as_ref()
    }

    /// How many rounds have been run.
    pub fn rounds(&self) -> usize {
        self.rounds
    }

    /// Run one round.
    ///
    /// `submitted` is what the frontend collected from the form on screen —
    /// empty for the opening round. It is passed to the script as `input`
    /// verbatim: a field hidden by its `visible_when` is absent from it, and
    /// that absence is the point, not an omission to repair.
    pub fn next(&mut self, submitted: &BTreeMap<String, String>) -> Result<Round, FormRunError> {
        let mut notices = Vec::new();
        self.rounds += 1;
        if self.rounds == CHATTY_ROUND_COUNT {
            // No limit — every round needs a submit and `Esc` is always
            // there. But a script bouncing between two forms looks exactly
            // like a slow one until something says so.
            notices.push(format!(
                "form script `{}` is on round {}",
                self.label, self.rounds
            ));
        }

        let script_state = self
            .state
            .as_ref()
            .map(|s| s.script_state.clone())
            .unwrap_or(Value::Null);
        let request = build_request(&self.label, self.payload.clone(), submitted, &script_state)?;
        let output = self.run_once(&request.to_string(), &mut notices)?;

        match parse_round(&self.label, &output)? {
            ScriptRound::Failed(message) => Ok(Round {
                step: FormStep::Failed(message),
                notices,
            }),
            ScriptRound::Result(value) => Ok(Round {
                step: FormStep::Done(value),
                notices,
            }),
            ScriptRound::Form(form) => {
                let (state, warnings) = advance(&self.label, self.state.as_ref(), submitted, form)?;
                notices.extend(warnings);
                self.state = Some(state.clone());
                Ok(Round {
                    step: FormStep::Ask(state),
                    notices,
                })
            }
        }
    }

    /// One process: request in a file, answer in `$NYD_OUTPUT_FILE`.
    fn run_once(&self, request: &str, notices: &mut Vec<String>) -> Result<String, FormRunError> {
        use std::process::{Command, Stdio};

        std::fs::write(&self.request_path, request).map_err(|e| {
            FormRunError::Io(format!("could not hand `{}` its request ({e})", self.label))
        })?;
        let _ = std::fs::remove_file(&self.answer_path);

        let spawn = || {
            Command::new(&self.script)
                .arg(&self.request_path)
                .current_dir(self.script.parent().unwrap_or(Path::new(".")))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .envs(&self.env)
                .env("NYD_OUTPUT_FILE", &self.answer_path)
                .spawn()
        };
        let mut child = spawn();
        // A script saved moments ago can still be open for writing in a
        // process we forked in between — the child inherited the descriptor
        // and the kernel refuses to exec the file until that child execs in
        // turn. It passes in microseconds, and the alternative is telling the
        // user that the script they just wrote cannot be run.
        for attempt in 1..=TEXT_FILE_BUSY_RETRIES {
            match &child {
                Err(e) if e.raw_os_error() == Some(ETXTBSY) => {
                    std::thread::sleep(std::time::Duration::from_millis(2 * attempt));
                    child = spawn();
                }
                _ => break,
            }
        }
        let output = child
            .and_then(|child| child.wait_with_output())
            .map_err(|e| FormRunError::Io(format!("could not run `{}` ({e})", self.label)))?;

        // stderr is the script's channel to the user, stdout its scratch pad
        // — the same split every other script mode makes.
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if !stderr.is_empty() {
            notices.push(stderr.clone());
        }
        if !output.status.success() {
            return Err(FormRunError::Crashed(format!(
                "form script `{}` exited with {}{}",
                self.label,
                output.status,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            )));
        }

        if !self.answer_path.exists() {
            return Err(
                ScriptFormError::new(&self.label, "wrote nothing to $NYD_OUTPUT_FILE").into(),
            );
        }
        let answer = std::fs::read_to_string(&self.answer_path).map_err(|e| {
            FormRunError::Io(format!(
                "could not read the answer of `{}` ({e})",
                self.label
            ))
        })?;
        let _ = std::fs::remove_file(&self.answer_path);
        Ok(answer)
    }
}

impl Drop for FormSession {
    /// The request file carries the selected row, so it does not get to
    /// outlive the conversation it was written for.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.request_path);
        let _ = std::fs::remove_file(&self.answer_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    /// An executable script in a directory that lives as long as the test.
    fn script(dir: &tempfile::TempDir, body: &str) -> PathBuf {
        let path = dir.path().join("f.sh");
        let mut file = std::fs::File::create(&path).expect("create");
        write!(file, "#!/bin/sh\n# mode: form\n{body}").expect("write");
        drop(file);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn the_opening_round_yields_the_declared_form() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = script(
            &dir,
            r#"printf '%s' '{"form":{"header":"New item","fields":[{"name":"title"}]}}' > "$NYD_OUTPUT_FILE"
"#,
        );
        let mut session = FormSession::new(path, json!({}));
        let round = session.next(&values(&[])).expect("one round");
        match round.step {
            FormStep::Ask(state) => {
                assert_eq!(state.header.as_deref(), Some("New item"));
                assert_eq!(state.fields.len(), 1);
                assert_eq!(state.fields[0].key, "title");
            }
            other => panic!("expected a form, got {other:?}"),
        }
        assert_eq!(session.rounds(), 1);
    }

    #[test]
    fn the_second_round_sees_what_the_user_typed_and_can_end_the_conversation() {
        let dir = tempfile::tempdir().expect("tempdir");
        // The script has no memory, so it decides by what it is handed.
        let path = script(
            &dir,
            r#"if grep -q '"title"' "$1"; then
  printf '%s' '{"result":{"commands":[":echo done"]}}' > "$NYD_OUTPUT_FILE"
else
  printf '%s' '{"form":{"fields":[{"name":"title"}]}}' > "$NYD_OUTPUT_FILE"
fi
"#,
        );
        let mut session = FormSession::new(path, json!({}));
        assert!(matches!(
            session.next(&values(&[])).expect("round 1").step,
            FormStep::Ask(_)
        ));
        let round = session
            .next(&values(&[("title", "Rework")]))
            .expect("round 2");
        assert_eq!(
            round.step,
            FormStep::Done(json!({"commands": [":echo done"]}))
        );
        assert_eq!(session.rounds(), 2);
    }

    #[test]
    fn the_scripts_own_state_comes_back_to_it_next_round() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = script(
            &dir,
            r#"if grep -q '"seen"' "$1"; then
  printf '%s' '{"result":{"remembered":true}}' > "$NYD_OUTPUT_FILE"
else
  printf '%s' '{"form":{"fields":[{"name":"a"}]},"state":{"seen":1}}' > "$NYD_OUTPUT_FILE"
fi
"#,
        );
        let mut session = FormSession::new(path, json!({}));
        session.next(&values(&[])).expect("round 1");
        let round = session.next(&values(&[("a", "x")])).expect("round 2");
        assert_eq!(round.step, FormStep::Done(json!({"remembered": true})));
    }

    #[test]
    fn an_error_answer_ends_the_conversation_with_its_message() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = script(
            &dir,
            "printf '%s' '{\"error\":\"the tracker is down\"}' > \"$NYD_OUTPUT_FILE\"\n",
        );
        let mut session = FormSession::new(path, json!({}));
        assert_eq!(
            session.next(&values(&[])).expect("one round").step,
            FormStep::Failed("the tracker is down".into())
        );
    }

    #[test]
    fn stderr_becomes_a_notice_and_stdout_is_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = script(
            &dir,
            r#"echo "debug print"
echo "heads up" >&2
printf '%s' '{"form":{"fields":[{"name":"a"}]}}' > "$NYD_OUTPUT_FILE"
"#,
        );
        let mut session = FormSession::new(path, json!({}));
        let round = session.next(&values(&[])).expect("one round");
        assert_eq!(round.notices, vec!["heads up"]);
    }

    #[test]
    fn a_script_that_exits_non_zero_names_its_complaint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = script(&dir, "echo 'no jira today' >&2\nexit 3\n");
        let mut session = FormSession::new(path, json!({}));
        let err = session.next(&values(&[])).expect_err("it crashed");
        assert!(matches!(err, FormRunError::Crashed(_)), "{err}");
        assert!(err.to_string().contains("no jira today"), "{err}");
    }

    #[test]
    fn a_script_that_writes_no_answer_is_a_protocol_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = script(&dir, "true\n");
        let mut session = FormSession::new(path, json!({}));
        let err = session.next(&values(&[])).expect_err("nothing was written");
        assert!(err.to_string().contains("NYD_OUTPUT_FILE"), "{err}");
    }

    #[test]
    fn the_scope_payload_reaches_the_script_beside_the_input() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Answer with what was asked, so the request itself is the assertion.
        let path = script(
            &dir,
            r#"printf '{"error":%s}' "$(tr -d '\n' < "$1" | sed 's/"/\\"/g;s/^/"/;s/$/"/')" > "$NYD_OUTPUT_FILE"
"#,
        );
        let mut session = FormSession::new(path, json!({"node": {"id": "n1"}}));
        let round = session.next(&values(&[("a", "b")])).expect("one round");
        match round.step {
            FormStep::Failed(echo) => {
                assert!(echo.contains(r#""node":{"id":"n1"}"#), "{echo}");
                assert!(echo.contains(r#""input":{"a":"b"}"#), "{echo}");
            }
            other => panic!("expected the echo, got {other:?}"),
        }
    }

    #[test]
    fn the_env_reaches_the_child() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = script(
            &dir,
            r#"printf '{"error":"%s"}' "$NYD_TEST_TOKEN" > "$NYD_OUTPUT_FILE"
"#,
        );
        let mut env = HashMap::new();
        env.insert("NYD_TEST_TOKEN".to_string(), "sesame".to_string());
        let mut session = FormSession::new(path, json!({})).with_env(env);
        assert_eq!(
            session.next(&values(&[])).expect("one round").step,
            FormStep::Failed("sesame".into())
        );
    }

    #[test]
    fn the_request_file_does_not_outlive_the_conversation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = script(
            &dir,
            "printf '%s' '{\"error\":\"x\"}' > \"$NYD_OUTPUT_FILE\"\n",
        );
        let mut session = FormSession::new(path, json!({"node": {"id": "n1"}}));
        session.next(&values(&[])).expect("one round");
        let request = session.request_path.clone();
        assert!(request.exists(), "the round left it behind");
        drop(session);
        assert!(!request.exists(), "and dropping the session takes it away");
    }
}
