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

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::RwLock;

use super::CredentialProvider;

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

impl CredentialProvider {
    /// Build the runtime resolver for this provider. Returns an error
    /// for `Prompt`, which is wired up by the orchestrator instead.
    pub fn build_resolver(&self) -> Result<Box<dyn CredentialResolver>, String> {
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
