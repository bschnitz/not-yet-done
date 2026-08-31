//! Runtime resolvers for [`CredentialProvider`] variants.
//!
//! Each provider config builds into a `Box<dyn CredentialResolver>` that
//! the orchestrator consults during login. Resolvers cache the resolved
//! value so repeated `resolve()` calls return the same string until
//! `invalidate()` is called — which the orchestrator does after the
//! server rejects the current credentials, breaking the otherwise
//! infinite "send-cached → server-rejects → send-cached" loop.
//!
//! The `Prompt` provider is intentionally not constructed here: it
//! requires a status channel and a reply handle that only the
//! orchestrator owns. `CredentialProvider::build_resolver` therefore
//! returns an error for `Prompt`.
//!
//! The `Script` provider is the exception that proves the rule: it may
//! need the user too (a locked password store), but it carries its own
//! way to reach them — a [`CredentialPrompts`] handle onto the adapter's
//! [`PromptRequest`] stream, handed to `build_resolver_with`. That is
//! what lets a config *without* an auth block (a calendar connection, an
//! SSH hop) ask through the frontend instead of leaving `gpg` to open its
//! own `pinentry` window.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::{RwLock, mpsc, oneshot};

use super::CredentialProvider;
use super::credential_script::{self, MAX_SCRIPT_ROUNDS, ScriptForm, ScriptRound};
use crate::{ActionInput, FormFieldSpec, InputSpec, PromptAnswer, PromptRequest};

#[derive(Debug, Error)]
pub enum CredentialError {
    /// The underlying source has no value (env var unset, file missing,
    /// keyring entry absent). Retrying without invalidating won't help.
    #[error("credential unavailable: {0}")]
    Unavailable(String),
    /// The provider failed mid-flight (script crashed, keyring backend
    /// errored, IO failure). May be transient.
    #[error("provider error: {0}")]
    ProviderError(String),
}

#[async_trait]
pub trait CredentialResolver: Send + Sync {
    /// Returns the current value. After a successful call, subsequent
    /// calls return the same value until `invalidate()` is called.
    /// Errors are not cached.
    async fn resolve(&self) -> Result<String, CredentialError>;

    /// Drops any cached value so the next `resolve()` re-fetches.
    async fn invalidate(&self);
}

/// A resolver's way back to the user: the adapter's
/// [`PromptRequest`] stream plus the label the frontend shows as the
/// asking party (the connection, not the script — the user knows their
/// accounts, not our helpers).
///
/// Cloneable so one connection's providers share a stream.
#[derive(Clone)]
pub struct CredentialPrompts {
    source: String,
    tx: mpsc::Sender<PromptRequest>,
}

impl CredentialPrompts {
    pub fn new(source: impl Into<String>, tx: mpsc::Sender<PromptRequest>) -> Self {
        Self {
            source: source.into(),
            tx,
        }
    }
}

impl CredentialProvider {
    /// Build the runtime resolver for this provider. Returns an error
    /// for `Prompt`, which is wired up by the orchestrator instead.
    ///
    /// A `script` provider built this way can only serve an *unlocked*
    /// source: with no prompt channel it has nowhere to ask. Callers that
    /// have one use [`build_resolver_with`](Self::build_resolver_with).
    pub fn build_resolver(&self) -> Result<Box<dyn CredentialResolver>, String> {
        self.build_resolver_with(None)
    }

    /// As [`build_resolver`](Self::build_resolver), but with a channel a
    /// `script` provider may ask the user through.
    pub fn build_resolver_with(
        &self,
        prompts: Option<&CredentialPrompts>,
    ) -> Result<Box<dyn CredentialResolver>, String> {
        match self {
            CredentialProvider::Literal { value } => {
                Ok(Box::new(LiteralResolver::new(value.clone())))
            }
            CredentialProvider::Env { var } => Ok(Box::new(EnvResolver::new(var.clone()))),
            CredentialProvider::File { path, trim } => {
                Ok(Box::new(FileResolver::new(path.clone(), *trim)))
            }
            CredentialProvider::Command {
                script,
                timeout_secs,
                retries,
            } => Ok(Box::new(CommandResolver::new(
                script.clone(),
                Duration::from_secs(*timeout_secs),
                *retries,
            ))),
            CredentialProvider::Keyring { service, account } => Ok(Box::new(KeyringResolver::new(
                service.clone(),
                account.clone(),
            ))),
            CredentialProvider::Script {
                script,
                field,
                timeout_secs,
            } => Ok(Box::new(ScriptResolver::new(
                script.clone(),
                field.clone(),
                Duration::from_secs(*timeout_secs),
                prompts.cloned(),
            ))),
            // Both need a frontend in the loop (see
            // `CredentialProvider::needs_frontend`), which only the
            // orchestrator can reach.
            CredentialProvider::Prompt { .. } => Err(
                "prompt provider must be wired up by the auth orchestrator, \
                 not from build_resolver"
                    .into(),
            ),
            CredentialProvider::ScriptResult => Err(
                "script-result provider must be wired up by the auth orchestrator, \
                 not from build_resolver"
                    .into(),
            ),
        }
    }
}

// --- Literal -------------------------------------------------------------

pub struct LiteralResolver {
    value: String,
}

impl LiteralResolver {
    pub fn new(value: String) -> Self {
        Self { value }
    }
}

#[async_trait]
impl CredentialResolver for LiteralResolver {
    async fn resolve(&self) -> Result<String, CredentialError> {
        Ok(self.value.clone())
    }

    async fn invalidate(&self) {
        // The value lives in config; nothing to drop.
    }
}

// --- Env -----------------------------------------------------------------

pub struct EnvResolver {
    var: String,
    cache: RwLock<Option<String>>,
}

impl EnvResolver {
    pub fn new(var: String) -> Self {
        Self {
            var,
            cache: RwLock::new(None),
        }
    }
}

#[async_trait]
impl CredentialResolver for EnvResolver {
    async fn resolve(&self) -> Result<String, CredentialError> {
        if let Some(v) = self.cache.read().await.clone() {
            return Ok(v);
        }
        let value = std::env::var(&self.var).map_err(|_| {
            CredentialError::Unavailable(format!("env var `{}` is not set", self.var))
        })?;
        if value.is_empty() {
            return Err(CredentialError::Unavailable(format!(
                "env var `{}` is empty",
                self.var
            )));
        }
        *self.cache.write().await = Some(value.clone());
        Ok(value)
    }

    async fn invalidate(&self) {
        *self.cache.write().await = None;
    }
}

// --- File ----------------------------------------------------------------

pub struct FileResolver {
    path: PathBuf,
    trim: bool,
    cache: RwLock<Option<String>>,
}

impl FileResolver {
    pub fn new(path: PathBuf, trim: bool) -> Self {
        Self {
            path,
            trim,
            cache: RwLock::new(None),
        }
    }
}

#[async_trait]
impl CredentialResolver for FileResolver {
    async fn resolve(&self) -> Result<String, CredentialError> {
        if let Some(v) = self.cache.read().await.clone() {
            return Ok(v);
        }
        let bytes = tokio::fs::read(&self.path)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => {
                    CredentialError::Unavailable(format!("file not found: {}", self.path.display()))
                }
                _ => CredentialError::ProviderError(format!("read {}: {e}", self.path.display())),
            })?;
        let value = String::from_utf8(bytes).map_err(|e| {
            CredentialError::ProviderError(format!(
                "{}: not valid UTF-8 ({e})",
                self.path.display()
            ))
        })?;
        let value = if self.trim {
            value.trim().to_string()
        } else {
            value
        };
        if value.is_empty() {
            return Err(CredentialError::Unavailable(format!(
                "file `{}` is empty",
                self.path.display()
            )));
        }
        *self.cache.write().await = Some(value.clone());
        Ok(value)
    }

    async fn invalidate(&self) {
        *self.cache.write().await = None;
    }
}

// --- Command -------------------------------------------------------------

pub struct CommandResolver {
    script: String,
    timeout: Duration,
    retries: u32,
    cache: RwLock<Option<String>>,
}

impl CommandResolver {
    pub fn new(script: String, timeout: Duration, retries: u32) -> Self {
        Self {
            script,
            timeout,
            retries: retries.max(1),
            cache: RwLock::new(None),
        }
    }

    async fn run_once(&self) -> Result<String, String> {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(&self.script)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;

        let output = match tokio::time::timeout(self.timeout, child.wait_with_output()).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => return Err(format!("wait failed: {e}")),
            Err(_) => return Err(format!("timeout after {}s", self.timeout.as_secs())),
        };

        if !output.status.success() {
            let code = output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".into());
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.trim();
            return Err(if stderr.is_empty() {
                format!("exit {code}")
            } else {
                format!("exit {code}: {stderr}")
            });
        }

        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if value.is_empty() {
            return Err("script produced empty stdout".into());
        }
        Ok(value)
    }
}

#[async_trait]
impl CredentialResolver for CommandResolver {
    async fn resolve(&self) -> Result<String, CredentialError> {
        if let Some(v) = self.cache.read().await.clone() {
            return Ok(v);
        }
        let mut last_err = String::new();
        for _ in 0..self.retries {
            match self.run_once().await {
                Ok(v) => {
                    *self.cache.write().await = Some(v.clone());
                    return Ok(v);
                }
                Err(e) => last_err = e,
            }
        }
        Err(CredentialError::ProviderError(format!(
            "command failed after {} attempt(s): {last_err}",
            self.retries
        )))
    }

    async fn invalidate(&self) {
        *self.cache.write().await = None;
    }
}

// --- Script --------------------------------------------------------------

/// One lock per credential script, so several slots pointing at the same
/// helper cost the user one dialog rather than one each.
///
/// Without it the four connections of a calendar adapter resolve in
/// parallel (each backend fetches in its own task) and a cold `gpg`
/// agent would raise four passphrase forms in a row. Serialised, the
/// first one unlocks the store and the rest find the agent warm. The key
/// is the script's *path* — the first shell word — because the arguments
/// are what differ between slots of the same helper.
fn script_lock(script: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
        OnceLock::new();
    let key = script
        .split_whitespace()
        .next()
        .unwrap_or(script)
        .to_string();
    let mut locks = LOCKS
        .get_or_init(|| StdMutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    Arc::clone(locks.entry(key).or_default())
}

/// Resolves one slot by running a credential script in rounds, asking the
/// user through [`CredentialPrompts`] whenever the script wants input.
pub struct ScriptResolver {
    script: String,
    /// Which key of the script's `result` this slot takes; `None` means
    /// "the only one it returns".
    field: Option<String>,
    timeout: Duration,
    prompts: Option<CredentialPrompts>,
    cache: RwLock<Option<String>>,
}

impl ScriptResolver {
    pub fn new(
        script: String,
        field: Option<String>,
        timeout: Duration,
        prompts: Option<CredentialPrompts>,
    ) -> Self {
        Self {
            script,
            field,
            timeout,
            prompts,
            cache: RwLock::new(None),
        }
    }

    /// The one value this slot wanted out of a finished round.
    fn pick(&self, mut values: BTreeMap<String, String>) -> Result<String, CredentialError> {
        if let Some(field) = &self.field {
            // `run_round` already checked a named field is present, so
            // this only fires for a script that ignores its request.
            return values.remove(field).ok_or_else(|| {
                CredentialError::Unavailable(format!(
                    "credential script `{}` returned no value for `{field}`",
                    self.script
                ))
            });
        }
        match values.len() {
            1 => Ok(values.into_values().next().expect("len checked")),
            0 => Err(CredentialError::Unavailable(format!(
                "credential script `{}` returned no values",
                self.script
            ))),
            _ => Err(CredentialError::ProviderError(format!(
                "credential script `{}` returned several values ({}) — name the \
                 one this slot takes with `field:`",
                self.script,
                values
                    .keys()
                    .map(|k| format!("`{k}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    /// Put the script's form in front of the user and wait for the answers.
    ///
    /// Every way of having no one to ask ends the same: an error naming
    /// the script, never a wait. A frontend that took no prompt stream,
    /// one that dropped it, and a user who dismissed the form are all
    /// "this credential is unavailable right now".
    async fn ask(&self, form: ScriptForm) -> Result<HashMap<String, String>, CredentialError> {
        let prompts = self.prompts.as_ref().ok_or_else(|| {
            CredentialError::Unavailable(format!(
                "credential script `{}` needs input, but this connection has no \
                 interactive frontend",
                self.script
            ))
        })?;

        let fields: Vec<FormFieldSpec> = form
            .fields
            .iter()
            .map(|f| FormFieldSpec {
                key: f.name.clone(),
                label: f.effective_label(),
                kind: crate::FormFieldKind::Text,
                required: !f.optional,
                default: f.prefill.clone(),
                masked: f.masked,
                visible_when: None,
            })
            .collect();

        let (tx, rx) = oneshot::channel();
        let request = PromptRequest {
            source: prompts.source.clone(),
            prompt: form
                .header
                .unwrap_or_else(|| "Credentials required".to_string()),
            detail: form.error,
            input: InputSpec::Form { fields },
            respond: tx,
        };
        prompts.tx.send(request).await.map_err(|_| {
            CredentialError::Unavailable(format!(
                "credential script `{}` needs input, but the frontend stopped \
                 listening for prompts",
                self.script
            ))
        })?;

        match rx.await {
            Ok(PromptAnswer::Provided(ActionInput::Form(values))) => Ok(values),
            Ok(PromptAnswer::Provided(_)) => Err(CredentialError::ProviderError(format!(
                "credential script `{}`: the frontend answered the form with \
                 something other than form values",
                self.script
            ))),
            Ok(PromptAnswer::Cancelled) => Err(CredentialError::Unavailable(format!(
                "credential script `{}`: the prompt was dismissed",
                self.script
            ))),
            Err(_) => Err(CredentialError::Unavailable(format!(
                "credential script `{}`: the prompt was dropped unanswered",
                self.script
            ))),
        }
    }

    /// The round loop: run, answer what it asks, run again.
    async fn run_rounds(&self) -> Result<String, CredentialError> {
        let request: Vec<&str> = self.field.iter().map(String::as_str).collect();
        let mut input: BTreeMap<String, String> = BTreeMap::new();

        for _ in 0..MAX_SCRIPT_ROUNDS {
            match credential_script::run_round(&self.script, &request, &input, self.timeout).await?
            {
                ScriptRound::Values(values) => return self.pick(values),
                ScriptRound::Failed(message) => {
                    return Err(CredentialError::ProviderError(format!(
                        "credential script `{}`: {message}",
                        self.script
                    )));
                }
                ScriptRound::Form(form) => {
                    // A fresh process every round remembers nothing, so
                    // the answers accumulate here rather than in the script.
                    for (name, value) in self.ask(form).await? {
                        input.insert(name, value);
                    }
                }
            }
        }
        Err(CredentialError::ProviderError(format!(
            "credential script `{}` still asked for input after \
             {MAX_SCRIPT_ROUNDS} rounds",
            self.script
        )))
    }
}

#[async_trait]
impl CredentialResolver for ScriptResolver {
    async fn resolve(&self) -> Result<String, CredentialError> {
        if let Some(v) = self.cache.read().await.clone() {
            return Ok(v);
        }
        // Held across the dialog on purpose: a sibling slot waiting here
        // is a sibling not asking the same question a second time.
        let lock = script_lock(&self.script);
        let _guard = lock.lock().await;
        if let Some(v) = self.cache.read().await.clone() {
            return Ok(v);
        }
        let value = self.run_rounds().await?;
        *self.cache.write().await = Some(value.clone());
        Ok(value)
    }

    async fn invalidate(&self) {
        *self.cache.write().await = None;
    }
}

// --- Keyring -------------------------------------------------------------

pub struct KeyringResolver {
    service: String,
    account: String,
    cache: RwLock<Option<String>>,
}

impl KeyringResolver {
    pub fn new(service: String, account: String) -> Self {
        Self {
            service,
            account,
            cache: RwLock::new(None),
        }
    }
}

#[async_trait]
impl CredentialResolver for KeyringResolver {
    async fn resolve(&self) -> Result<String, CredentialError> {
        if let Some(v) = self.cache.read().await.clone() {
            return Ok(v);
        }
        let service = self.service.clone();
        let account = self.account.clone();
        // The keyring crate is sync (DBus roundtrips block); spawn_blocking
        // keeps the tokio runtime non-blocking.
        let result = tokio::task::spawn_blocking(move || {
            let entry = keyring::Entry::new(&service, &account)
                .map_err(|e| format!("keyring entry [{service}/{account}]: {e}"))?;
            entry.get_password().map_err(|e| match e {
                keyring::Error::NoEntry => {
                    format!("no keyring entry for [{service}/{account}]")
                }
                other => format!("keyring read [{service}/{account}]: {other}"),
            })
        })
        .await
        .map_err(|e| CredentialError::ProviderError(format!("blocking task: {e}")))?;

        let value = result.map_err(|e| {
            if e.starts_with("no keyring entry") {
                CredentialError::Unavailable(e)
            } else {
                CredentialError::ProviderError(e)
            }
        })?;
        *self.cache.write().await = Some(value.clone());
        Ok(value)
    }

    async fn invalidate(&self) {
        *self.cache.write().await = None;
    }
}

// --- Tests ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Write an executable script and return the path to run it by.
    fn script_file(dir: &std::path::Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod script");
        path.display().to_string()
    }

    /// The motivating shape: a store that asks for its passphrase until it
    /// has one, then hands the secret over.
    const LOCKED_STORE: &str = r#"#!/bin/sh
req=$(cat)
case "$req" in
  *'"passphrase"'*) printf '{"result":{"password":"s3cret"}}' ;;
  *) printf '{"form":{"header":"Password store locked","fields":[{"name":"passphrase","masked":true}]}}' ;;
esac
"#;

    fn script_resolver(
        script: &str,
        field: Option<&str>,
        prompts: Option<CredentialPrompts>,
    ) -> ScriptResolver {
        ScriptResolver::new(
            script.to_string(),
            field.map(str::to_string),
            Duration::from_secs(10),
            prompts,
        )
    }

    /// Answer every form with the same values, counting how often we were
    /// asked. The count is the point: it is what tells one dialog from four.
    fn answering_frontend(
        values: Vec<(&'static str, &'static str)>,
    ) -> (CredentialPrompts, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (tx, mut rx) = mpsc::channel::<PromptRequest>(8);
        let asked = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&asked);
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                counter.fetch_add(1, Ordering::SeqCst);
                let answers: HashMap<String, String> = values
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect();
                let _ = req
                    .respond
                    .send(PromptAnswer::Provided(ActionInput::Form(answers)));
            }
        });
        (CredentialPrompts::new("synthetic account", tx), asked)
    }

    #[tokio::test]
    async fn script_resolver_costs_no_prompt_when_the_store_is_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = script_file(
            dir.path(),
            "open.sh",
            "#!/bin/sh\ncat >/dev/null\nprintf '{\"result\":{\"password\":\"s3cret\"}}'\n",
        );
        // No prompt channel at all — an unlocked store must still resolve.
        let r = script_resolver(&path, None, None);
        assert_eq!(r.resolve().await.unwrap(), "s3cret");
    }

    #[tokio::test]
    async fn script_resolver_asks_once_and_the_answer_completes_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = script_file(dir.path(), "locked.sh", LOCKED_STORE);
        let (prompts, asked) = answering_frontend(vec![("passphrase", "opensesame")]);

        let r = script_resolver(&path, Some("password"), Some(prompts));
        assert_eq!(r.resolve().await.unwrap(), "s3cret");
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
        // The value is cached: a second read asks nobody.
        assert_eq!(r.resolve().await.unwrap(), "s3cret");
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn script_resolver_without_a_frontend_fails_instead_of_hanging() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = script_file(dir.path(), "locked.sh", LOCKED_STORE);
        let r = script_resolver(&path, Some("password"), None);
        let err = r.resolve().await.expect_err("nothing can answer this");
        assert!(
            err.to_string().contains("no interactive frontend"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn script_resolver_reports_a_dismissed_prompt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = script_file(dir.path(), "locked.sh", LOCKED_STORE);
        let (tx, mut rx) = mpsc::channel::<PromptRequest>(8);
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let _ = req.respond.send(PromptAnswer::Cancelled);
            }
        });
        let r = script_resolver(
            &path,
            Some("password"),
            Some(CredentialPrompts::new("synthetic account", tx)),
        );
        let err = r.resolve().await.expect_err("the user said no");
        assert!(err.to_string().contains("dismissed"), "unexpected: {err}");
    }

    #[tokio::test]
    async fn script_resolver_needs_a_field_name_when_several_values_come_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body = "#!/bin/sh\ncat >/dev/null\nprintf '{\"result\":{\"password\":\"p\",\"token\":\"t\"}}'\n";
        let path = script_file(dir.path(), "two.sh", body);

        let err = script_resolver(&path, None, None)
            .resolve()
            .await
            .expect_err("ambiguous without a field");
        assert!(err.to_string().contains("`field:`"), "unexpected: {err}");

        // Named, it is unambiguous.
        let r = script_resolver(&path, Some("token"), None);
        assert_eq!(r.resolve().await.unwrap(), "t");
    }

    /// The four-connections case: several slots on one helper cost the
    /// user one dialog, not one each. The script asks only while the
    /// marker is absent, so a second *concurrent* run that skipped the
    /// lock would ask a second time.
    #[tokio::test]
    async fn slots_sharing_a_script_ask_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("unlocked");
        let body = format!(
            r#"#!/bin/sh
req=$(cat)
if [ -f {marker} ]; then
  printf '{{"result":{{"password":"s3cret"}}}}'
  exit 0
fi
case "$req" in
  *'"passphrase"'*) : > {marker}; printf '{{"result":{{"password":"s3cret"}}}}' ;;
  *) printf '{{"form":{{"header":"Password store locked","fields":[{{"name":"passphrase","masked":true}}]}}}}' ;;
esac
"#,
            marker = marker.display()
        );
        let path = script_file(dir.path(), "agent.sh", &body);
        let (prompts, asked) = answering_frontend(vec![("passphrase", "opensesame")]);

        let a = script_resolver(&path, Some("password"), Some(prompts.clone()));
        let b = script_resolver(&path, Some("password"), Some(prompts));
        let (ra, rb) = tokio::join!(a.resolve(), b.resolve());
        assert_eq!(ra.unwrap(), "s3cret");
        assert_eq!(rb.unwrap(), "s3cret");
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn literal_resolver_returns_value() {
        let r = LiteralResolver::new("synthetic".into());
        assert_eq!(r.resolve().await.unwrap(), "synthetic");
        r.invalidate().await; // no-op for literal
        assert_eq!(r.resolve().await.unwrap(), "synthetic");
    }

    #[tokio::test]
    async fn env_resolver_reads_var_and_caches() {
        let var = "NYD_AUTH_TEST_ENV_RESOLVER_OK";
        // SAFETY (Rust 2024): test sets a uniquely-named env var; no
        // concurrent test reads or writes it.
        unsafe {
            std::env::set_var(var, "secret-synthetic");
        }
        let r = EnvResolver::new(var.into());
        assert_eq!(r.resolve().await.unwrap(), "secret-synthetic");
        // Cache holds even after the env var is unset.
        unsafe {
            std::env::remove_var(var);
        }
        assert_eq!(r.resolve().await.unwrap(), "secret-synthetic");
        // After invalidate, the now-missing env makes resolve fail.
        r.invalidate().await;
        let err = r.resolve().await.expect_err("must fail when unset");
        assert!(matches!(err, CredentialError::Unavailable(_)));
    }

    #[tokio::test]
    async fn env_resolver_rejects_empty_string() {
        let var = "NYD_AUTH_TEST_ENV_RESOLVER_EMPTY";
        unsafe {
            std::env::set_var(var, "");
        }
        let r = EnvResolver::new(var.into());
        let err = r.resolve().await.expect_err("empty must fail");
        assert!(matches!(err, CredentialError::Unavailable(_)));
        unsafe {
            std::env::remove_var(var);
        }
    }

    #[tokio::test]
    async fn file_resolver_reads_and_trims() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("token");
        tokio::fs::write(&path, b"  secret-synthetic\n")
            .await
            .unwrap();
        let r = FileResolver::new(path, true);
        assert_eq!(r.resolve().await.unwrap(), "secret-synthetic");
    }

    #[tokio::test]
    async fn file_resolver_keeps_raw_bytes_when_trim_false() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("token");
        tokio::fs::write(&path, b"  raw  ").await.unwrap();
        let r = FileResolver::new(path, false);
        assert_eq!(r.resolve().await.unwrap(), "  raw  ");
    }

    #[tokio::test]
    async fn file_resolver_invalidate_picks_up_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("token");
        tokio::fs::write(&path, b"v1\n").await.unwrap();
        let r = FileResolver::new(path.clone(), true);
        assert_eq!(r.resolve().await.unwrap(), "v1");
        // File edit goes unnoticed until we invalidate.
        tokio::fs::write(&path, b"v2\n").await.unwrap();
        assert_eq!(r.resolve().await.unwrap(), "v1");
        r.invalidate().await;
        assert_eq!(r.resolve().await.unwrap(), "v2");
    }

    #[tokio::test]
    async fn file_resolver_missing_path_is_unavailable() {
        let r = FileResolver::new("/nonexistent/synthetic/path".into(), true);
        let err = r.resolve().await.expect_err("must fail");
        assert!(matches!(err, CredentialError::Unavailable(_)));
    }

    #[tokio::test]
    async fn file_resolver_empty_file_is_unavailable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("empty");
        tokio::fs::write(&path, b"\n  \n").await.unwrap();
        let r = FileResolver::new(path, true);
        let err = r.resolve().await.expect_err("must fail");
        assert!(matches!(err, CredentialError::Unavailable(_)));
    }

    #[tokio::test]
    async fn command_resolver_runs_script_and_caches() {
        let r = CommandResolver::new("echo synthetic-stdout".into(), Duration::from_secs(5), 1);
        assert_eq!(r.resolve().await.unwrap(), "synthetic-stdout");
        // Cache holds across calls.
        assert_eq!(r.resolve().await.unwrap(), "synthetic-stdout");
    }

    #[tokio::test]
    async fn command_resolver_times_out() {
        let r = CommandResolver::new("sleep 99".into(), Duration::from_millis(50), 1);
        let err = r.resolve().await.expect_err("must time out");
        assert!(matches!(err, CredentialError::ProviderError(_)));
        assert!(
            err.to_string().contains("timeout"),
            "expected timeout: {err}"
        );
    }

    #[tokio::test]
    async fn command_resolver_retries_on_transient_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let counter = dir.path().join("count");
        tokio::fs::write(&counter, b"0").await.unwrap();
        let script = format!(
            r#"n=$(cat {p}); if [ "$n" = "0" ]; then echo 1 > {p}; exit 1; else echo synthetic-ok; fi"#,
            p = counter.display(),
        );
        let r = CommandResolver::new(script, Duration::from_secs(5), 3);
        assert_eq!(r.resolve().await.unwrap(), "synthetic-ok");
    }

    #[tokio::test]
    async fn command_resolver_fails_after_max_retries() {
        let r = CommandResolver::new("exit 1".into(), Duration::from_secs(5), 2);
        let err = r.resolve().await.expect_err("must fail");
        assert!(
            err.to_string().contains("2 attempt"),
            "mentions attempt count: {err}"
        );
    }

    #[tokio::test]
    async fn command_resolver_empty_stdout_is_error() {
        let r = CommandResolver::new("true".into(), Duration::from_secs(5), 1);
        let err = r.resolve().await.expect_err("must fail");
        assert!(matches!(err, CredentialError::ProviderError(_)));
    }

    // The keyring resolver needs an active D-Bus session bus and a
    // running secret-service backend (gnome-keyring, kwallet, …) — not
    // generally available in CI. Run manually with `cargo test --
    // --ignored`.
    #[tokio::test]
    #[ignore]
    async fn keyring_resolver_roundtrip() {
        let service = "nyd-auth-test-synthetic";
        let account = "synthetic-account";
        tokio::task::spawn_blocking(move || {
            let e = keyring::Entry::new(service, account).unwrap();
            e.set_password("secret-roundtrip").unwrap();
        })
        .await
        .unwrap();
        let r = KeyringResolver::new(service.into(), account.into());
        assert_eq!(r.resolve().await.unwrap(), "secret-roundtrip");
        tokio::task::spawn_blocking(move || {
            let e = keyring::Entry::new(service, account).unwrap();
            let _ = e.delete_credential();
        })
        .await
        .unwrap();
    }

    /// Both interactive providers need a frontend the resolver layer
    /// cannot reach; the orchestrator wires them up instead.
    #[test]
    fn build_resolver_for_interactive_providers_errors() {
        for p in [
            CredentialProvider::Prompt { prefill: None },
            CredentialProvider::ScriptResult,
        ] {
            assert!(p.needs_frontend());
            assert!(p.build_resolver().is_err(), "must not build: {p:?}");
        }
    }

    #[test]
    fn build_resolver_for_each_supported_kind() {
        let providers = [
            CredentialProvider::Literal { value: "x".into() },
            CredentialProvider::Env { var: "X".into() },
            CredentialProvider::File {
                path: "/tmp/x".into(),
                trim: true,
            },
            CredentialProvider::Command {
                script: "true".into(),
                timeout_secs: 1,
                retries: 1,
            },
            CredentialProvider::Keyring {
                service: "x".into(),
                account: "y".into(),
            },
        ];
        for p in providers {
            assert!(p.build_resolver().is_ok(), "provider should build: {p:?}");
        }
    }
}
