//! The round protocol behind
//! [`CredentialProvider::ScriptResult`](super::CredentialProvider::ScriptResult).
//!
//! A `command` provider is a black box producing exactly one value: the
//! runtime runs it and takes whatever lands on stdout. Two shortcomings
//! follow from that, and both hurt the same everyday case — credentials
//! kept in a password store.
//!
//! - One value per invocation. Username and token out of `pass` means
//!   running `pass` twice, unlocking twice, waiting twice.
//! - No way to ask. When the store is locked, `pass` reaches for
//!   `pinentry`: a second window, outside the TUI, that knows nothing
//!   about the login it is part of.
//!
//! A credential script fixes both by being asked in *rounds*. Each round
//! is one process; the runtime writes a [`ScriptRequest`] to its stdin
//! and reads one of three answers off stdout:
//!
//! - `result` — here are the values, we are done.
//! - `form` — ask the user this first, then call me again.
//! - `error` — give up, here is why.
//!
//! The form is published through the ordinary
//! [`AdapterStatus::NeedsCreds`](crate::AdapterStatus::NeedsCreds)
//! contract, so both frontends render it with the code they already use
//! for `prompt` fields, and a run with nothing to ask on (a pipe, a cron
//! job) fails with the same explanation as any other interactive
//! credential.
//!
//! The script is a fresh process every round and can remember nothing, so
//! [`ScriptRequest::input`] carries *every* answer collected so far, not
//! just the newest one.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::CredentialError;
use super::title_case;

/// What the runtime writes to the script's stdin, once per round.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ScriptRequest<'a> {
    /// The field names the config wants from this script, so one generic
    /// script can serve several adapters without guessing.
    pub request: &'a [&'a str],
    /// Every answer the user has given so far, keyed by the `name` of the
    /// form field that asked for it. Empty on the first round.
    pub input: &'a BTreeMap<String, String>,
}

/// One round's answer, after validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptRound {
    /// The requested values. Ends the loop.
    Values(BTreeMap<String, String>),
    /// Ask the user this, then run another round with the answers.
    Form(ScriptForm),
    /// The script gave up; the message is shown to the user.
    Failed(String),
}

/// A form the script wants rendered on its behalf.
#[derive(Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct ScriptForm {
    /// Title for the dialog, e.g. "Unlock the password store".
    #[serde(default)]
    pub header: Option<String>,
    /// Why the form is being shown *again* — "that passphrase was
    /// rejected". Displayed with the form; unlike a top-level `error` it
    /// does not abort the login.
    #[serde(default)]
    pub error: Option<String>,
    pub fields: Vec<ScriptFormField>,
}

/// One input the script wants from the user. Shaped like
/// [`AuthField`](crate::AuthField) on purpose — the orchestrator passes
/// these straight through to the frontend.
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScriptFormField {
    /// Key the answer is filed under in the next round's `input`.
    /// Mandatory: it is what makes two password fields distinguishable.
    pub name: String,
    /// Display label. Defaults to a title-cased `name`.
    #[serde(default)]
    pub label: Option<String>,
    /// Whether the frontend masks the input.
    #[serde(default)]
    pub masked: bool,
    /// Whether the script can do without an answer to this one.
    #[serde(default)]
    pub optional: bool,
    /// Pre-filled value.
    #[serde(default)]
    pub prefill: Option<String>,
}

impl ScriptFormField {
    pub fn effective_label(&self) -> String {
        match &self.label {
            Some(l) => l.clone(),
            None => title_case(&self.name),
        }
    }
}

/// The three answer shapes as they arrive. Kept separate from
/// [`ScriptRound`] so "exactly one of them" is checked here rather than
/// being expressible in the type the orchestrator sees.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawRound {
    #[serde(default)]
    result: Option<BTreeMap<String, String>>,
    #[serde(default)]
    form: Option<ScriptForm>,
    #[serde(default)]
    error: Option<String>,
}

/// Run one round: hand the script what is known, take back what it says.
///
/// `request` doubles as the completeness check — a `result` missing one
/// of the requested names is a script bug, and catching it here beats an
/// adapter later reporting a login failure for an absent field.
pub async fn run_round(
    script: &str,
    request: &[&str],
    input: &BTreeMap<String, String>,
    timeout: Duration,
) -> Result<ScriptRound, CredentialError> {
    let payload = serde_json::to_string(&ScriptRequest { request, input })
        .map_err(|e| CredentialError::ProviderError(format!("encoding script request: {e}")))?;
    let stdout = run(script, payload, timeout).await?;
    parse_round(script, request, &stdout)
}

fn parse_round(
    script: &str,
    request: &[&str],
    stdout: &str,
) -> Result<ScriptRound, CredentialError> {
    if stdout.trim().is_empty() {
        return Err(CredentialError::ProviderError(format!(
            "credential script `{script}` printed nothing"
        )));
    }
    let raw: RawRound = serde_json::from_str(stdout).map_err(|e| {
        CredentialError::ProviderError(format!(
            "credential script `{script}`: stdout is not a round answer ({e})"
        ))
    })?;

    let set = raw.result.is_some() as u8 + raw.form.is_some() as u8 + raw.error.is_some() as u8;
    if set != 1 {
        return Err(CredentialError::ProviderError(format!(
            "credential script `{script}`: expected exactly one of `result`, \
             `form` or `error`, got {set}"
        )));
    }

    if let Some(error) = raw.error {
        let error = error.trim().to_string();
        if error.is_empty() {
            return Err(CredentialError::ProviderError(format!(
                "credential script `{script}` reported an empty error"
            )));
        }
        return Ok(ScriptRound::Failed(error));
    }

    if let Some(form) = raw.form {
        if form.fields.is_empty() {
            // A dialog with no inputs would park the login on something
            // the user cannot answer. `result` and `error` are the ways
            // to end a round without asking.
            return Err(CredentialError::ProviderError(format!(
                "credential script `{script}` asked for a form without fields"
            )));
        }
        let mut seen: Vec<&str> = Vec::with_capacity(form.fields.len());
        for f in &form.fields {
            if f.name.is_empty() {
                return Err(CredentialError::ProviderError(format!(
                    "credential script `{script}`: a form field has an empty name"
                )));
            }
            if seen.contains(&f.name.as_str()) {
                return Err(CredentialError::ProviderError(format!(
                    "credential script `{script}`: duplicate form field `{}`",
                    f.name
                )));
            }
            seen.push(&f.name);
        }
        return Ok(ScriptRound::Form(form));
    }

    let values = raw.result.unwrap_or_default();
    let missing: Vec<&str> = request
        .iter()
        .copied()
        .filter(|f| !values.contains_key(*f))
        .collect();
    if !missing.is_empty() {
        return Err(CredentialError::Unavailable(format!(
            "credential script `{script}` returned no value for {}",
            missing
                .iter()
                .map(|f| format!("`{f}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(ScriptRound::Values(values))
}

/// Run one invocation under `sh -c`, feeding it the request on stdin.
async fn run(command: &str, stdin: String, timeout: Duration) -> Result<String, CredentialError> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| CredentialError::ProviderError(format!("spawn `{command}`: {e}")))?;

    let mut pipe = child
        .stdin
        .take()
        .ok_or_else(|| CredentialError::ProviderError("stdin pipe missing".into()))?;
    pipe.write_all(stdin.as_bytes())
        .await
        .map_err(|e| CredentialError::ProviderError(format!("writing to `{command}`: {e}")))?;
    // Dropping the handle closes the pipe — without it a script that
    // reads to EOF would wait for a writer that never goes away.
    drop(pipe);

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return Err(CredentialError::ProviderError(format!(
                "waiting for `{command}`: {e}"
            )));
        }
        Err(_) => {
            return Err(CredentialError::ProviderError(format!(
                "`{command}` timed out after {}s",
                timeout.as_secs()
            )));
        }
    };

    if !output.status.success() {
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "?".into());
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        return Err(CredentialError::ProviderError(if stderr.is_empty() {
            format!("`{command}` exited {code}")
        } else {
            format!("`{command}` exited {code}: {stderr}")
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write an executable script and return the path to run it by.
    fn script(dir: &std::path::Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod script");
        path.display().to_string()
    }

    /// The motivating shape: locked store → ask for the passphrase;
    /// unlocked (passphrase already in `input`) → hand over both values.
    const PASS_SCRIPT: &str = r#"#!/bin/sh
req=$(cat)
case "$req" in
  *'"passphrase"'*)
    printf '{"result":{"username":"alice","token":"t-42"}}'
    ;;
  *)
    printf '{"form":{"header":"Unlock the password store","fields":[{"name":"passphrase","label":"Passphrase","masked":true}]}}'
    ;;
esac
"#;

    fn no_input() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    #[tokio::test]
    async fn a_locked_store_asks_and_the_answer_completes_the_next_round() {
        let dir = tempfile::tempdir().unwrap();
        let s = script(dir.path(), "pass.sh", PASS_SCRIPT);
        let request = ["username", "token"];

        let first = run_round(&s, &request, &no_input(), Duration::from_secs(5))
            .await
            .expect("first round");
        let form = match first {
            ScriptRound::Form(f) => f,
            other => panic!("expected a form, got {other:?}"),
        };
        assert_eq!(form.header.as_deref(), Some("Unlock the password store"));
        assert_eq!(form.fields.len(), 1);
        assert!(form.fields[0].masked);

        let mut input = BTreeMap::new();
        input.insert("passphrase".to_string(), "hunter2".to_string());
        let second = run_round(&s, &request, &input, Duration::from_secs(5))
            .await
            .expect("second round");
        match second {
            ScriptRound::Values(v) => {
                assert_eq!(v["username"], "alice");
                assert_eq!(v["token"], "t-42");
            }
            other => panic!("expected values, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_request_names_the_wanted_fields_and_input_reaches_the_script() {
        let dir = tempfile::tempdir().unwrap();
        // Echoes its stdin back as an error message, so the test can see
        // exactly what the script was handed.
        let s = script(
            dir.path(),
            "echo.sh",
            "#!/bin/sh\nreq=$(cat)\nprintf '{\"error\":%s}' \"$(printf '%s' \"$req\" | sed 's/\"/\\\\\"/g;s/^/\"/;s/$/\"/')\"\n",
        );
        let mut input = BTreeMap::new();
        input.insert("passphrase".to_string(), "hunter2".to_string());
        let round = run_round(&s, &["username", "token"], &input, Duration::from_secs(5))
            .await
            .expect("round");
        let seen = match round {
            ScriptRound::Failed(msg) => msg,
            other => panic!("expected the echo, got {other:?}"),
        };
        assert!(seen.contains(r#""request":["username","token"]"#), "{seen}");
        assert!(
            seen.contains(r#""input":{"passphrase":"hunter2"}"#),
            "{seen}"
        );
    }

    /// A field the config asked for but the script left out is caught
    /// here, not three layers later as a login failure.
    #[test]
    fn a_result_missing_a_requested_field_is_unavailable() {
        let err = parse_round(
            "s.sh",
            &["username", "token"],
            r#"{"result":{"username":"a"}}"#,
        )
        .expect_err("must fail");
        assert!(matches!(err, CredentialError::Unavailable(_)));
        assert!(err.to_string().contains("`token`"), "got: {err}");
    }

    #[test]
    fn a_result_may_carry_more_than_was_asked_for() {
        let round = parse_round(
            "s.sh",
            &["token"],
            r#"{"result":{"token":"t","username":"a"}}"#,
        )
        .expect("extra keys are the script's business");
        assert!(matches!(round, ScriptRound::Values(_)));
    }

    #[test]
    fn two_answer_keys_at_once_are_rejected() {
        let err = parse_round(
            "s.sh",
            &["token"],
            r#"{"result":{"token":"t"},"error":"nope"}"#,
        )
        .expect_err("ambiguous answer");
        assert!(err.to_string().contains("exactly one"), "got: {err}");
    }

    #[test]
    fn an_answer_with_no_key_at_all_is_rejected() {
        let err = parse_round("s.sh", &["token"], "{}").expect_err("empty answer");
        assert!(err.to_string().contains("exactly one"), "got: {err}");
    }

    #[test]
    fn a_form_without_fields_is_rejected() {
        let err = parse_round("s.sh", &["token"], r#"{"form":{"fields":[]}}"#)
            .expect_err("nothing to answer");
        assert!(err.to_string().contains("without fields"), "got: {err}");
    }

    #[test]
    fn duplicate_form_fields_are_rejected() {
        let err = parse_round(
            "s.sh",
            &["token"],
            r#"{"form":{"fields":[{"name":"a"},{"name":"a"}]}}"#,
        )
        .expect_err("must fail");
        assert!(err.to_string().contains("duplicate"), "got: {err}");
    }

    #[test]
    fn a_form_field_without_a_label_falls_back_to_its_name() {
        let round = parse_round(
            "s.sh",
            &["token"],
            r#"{"form":{"fields":[{"name":"store_passphrase","masked":true}]}}"#,
        )
        .expect("valid form");
        match round {
            ScriptRound::Form(f) => {
                assert_eq!(f.fields[0].effective_label(), "Store Passphrase");
                assert!(f.error.is_none());
            }
            other => panic!("expected a form, got {other:?}"),
        }
    }

    /// "wrong passphrase, try again" — a re-ask, not an abort.
    #[test]
    fn a_form_can_carry_the_error_that_caused_the_re_ask() {
        let round = parse_round(
            "s.sh",
            &["token"],
            r#"{"form":{"error":"that passphrase was rejected","fields":[{"name":"p"}]}}"#,
        )
        .expect("valid form");
        match round {
            ScriptRound::Form(f) => {
                assert_eq!(f.error.as_deref(), Some("that passphrase was rejected"))
            }
            other => panic!("expected a form, got {other:?}"),
        }
    }

    #[test]
    fn a_typo_in_a_form_field_does_not_pass_silently() {
        let err = parse_round(
            "s.sh",
            &["token"],
            r#"{"form":{"fields":[{"name":"a","maskd":true}]}}"#,
        )
        .expect_err("must fail");
        assert!(err.to_string().contains("maskd"), "got: {err}");
    }

    #[tokio::test]
    async fn non_json_stdout_is_a_provider_error_naming_the_script() {
        let dir = tempfile::tempdir().unwrap();
        let s = script(
            dir.path(),
            "chatty.sh",
            "#!/bin/sh\ncat >/dev/null\necho hello\n",
        );
        let err = run_round(&s, &["token"], &no_input(), Duration::from_secs(5))
            .await
            .expect_err("must fail");
        assert!(matches!(err, CredentialError::ProviderError(_)));
        assert!(err.to_string().contains("chatty.sh"), "got: {err}");
    }

    #[tokio::test]
    async fn a_non_zero_exit_carries_the_scripts_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let s = script(
            dir.path(),
            "angry.sh",
            "#!/bin/sh\ncat >/dev/null\necho 'no password store' >&2\nexit 3\n",
        );
        let err = run_round(&s, &["token"], &no_input(), Duration::from_secs(5))
            .await
            .expect_err("must fail");
        assert!(err.to_string().contains("no password store"), "got: {err}");
        assert!(err.to_string().contains("exited 3"), "got: {err}");
    }

    #[tokio::test]
    async fn a_hanging_script_hits_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let s = script(dir.path(), "slow.sh", "#!/bin/sh\nsleep 99\n");
        let err = run_round(&s, &["token"], &no_input(), Duration::from_millis(50))
            .await
            .expect_err("must fail");
        assert!(err.to_string().contains("timed out"), "got: {err}");
    }
}
