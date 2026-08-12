//! Login flow orchestration: resolves credentials via the configured
//! providers, hands them to an adapter-supplied login function, and
//! caches the resulting session according to [`SessionCachePolicy`].
//!
//! The orchestrator splits the configured bindings into two groups:
//!
//! - **Value providers** (literal/env/file/command/keyring) build their
//!   resolvers up-front and hand back the value on demand.
//! - **Interactive providers** (`prompt`, `script-result`) are filled
//!   with the frontend in the loop. The orchestrator publishes an
//!   [`AdapterStatus::NeedsCreds`] form on the status channel and waits
//!   for [`AuthOrchestrator::submit_credentials`] to route the user's
//!   reply back — or for [`AuthOrchestrator::cancel_prompt`] to end the
//!   wait. Subsequent calls reuse the supplied values until
//!   [`AuthOrchestrator::re_authenticate`] or
//!   [`AuthOrchestrator::invalidate_credentials`] clears the cache.
//!
//! `prompt` bindings are asked in one dialog, up front. The credential
//! script (see [`credential_script`]) then runs in rounds: it returns the
//! values, or a form to render first, and the answers go back into the
//! next round. Whether it asks at all is only known once it has run —
//! which is the point, since an unlocked password store should cost the
//! user nothing. The round count is capped so a script that keeps asking
//! cannot hold the login open forever.
//!
//! The whole login flow is serialised via an internal mutex — concurrent
//! `ensure_session` callers queue up rather than racing the prompt form.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use thiserror::Error;
use tokio::sync::{Mutex, RwLock, oneshot, watch};

use super::credential_script::{self, ScriptRound};
use super::session_store::{SessionEntry, SessionStore};
use super::{
    AuthSpec, CredentialBinding, CredentialError, CredentialProvider, CredentialResolver,
    SessionCachePolicy,
};
use crate::{AdapterStatus, AuthField};

/// How often the credential script may come back asking for more input
/// before the login gives up. A script that has not converged by then is
/// looping, and the user is the one paying for it.
const MAX_SCRIPT_ROUNDS: usize = 5;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("credential resolution failed: {0}")]
    Credential(#[from] CredentialError),
    #[error("login failed: {0}")]
    LoginFailed(String),
    #[error("auth orchestrator misconfigured: {0}")]
    Misconfigured(String),
    #[error("user cancelled the credential prompt")]
    PromptCancelled,
    #[error("submit_credentials called without a pending prompt")]
    NoPromptPending,
}

/// Pluggable wall clock — exposed so tests can advance time deterministically.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Result of [`AuthOrchestrator::ensure_session`] — either the session
/// blob was just minted by the login function or it came from the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedSession {
    pub blob: String,
    pub from_cache: bool,
}

pub struct AuthOrchestrator {
    spec: AuthSpec,
    /// Per-field resolver, keyed by field name. Interactive fields are
    /// NOT in this map — they're filled via `submit_credentials`.
    resolvers: HashMap<String, Box<dyn CredentialResolver>>,
    /// Bindings that need a frontend prompt. Order matches `spec.bindings`.
    prompt_fields: Vec<CredentialBinding>,
    /// Mechanism fields the credential script supplies, in config order.
    /// They travel to the script as one `request` list.
    script_fields: Vec<String>,
    /// Cache of values supplied via the interactive path (prompts and
    /// finished forms alike), keyed by mechanism field. Cleared by
    /// `re_authenticate` / `invalidate_credentials` along with the
    /// resolver caches.
    interactive_cache: RwLock<HashMap<String, String>>,
    /// Pending prompt awaiter — set when the orchestrator publishes
    /// `NeedsCreds` and waits for `submit_credentials`.
    pending_prompt: Mutex<Option<oneshot::Sender<HashMap<String, String>>>>,
    /// Serialises the whole `ensure_session` / `re_authenticate` flow so
    /// concurrent callers don't race the prompt form.
    auth_mutex: Mutex<()>,
    session_store: Box<dyn SessionStore>,
    status_tx: watch::Sender<AdapterStatus>,
    clock: Arc<dyn Clock>,
}

impl AuthOrchestrator {
    pub fn from_spec(
        spec: AuthSpec,
        session_store: Box<dyn SessionStore>,
    ) -> Result<Self, AuthError> {
        // Mechanism and field coverage are checked against the adapter's
        // own descriptors (`AuthSpec::validate_against`) while its config
        // is read — this constructor has no descriptors and does not
        // second-guess them. The one invariant it does need is its own:
        // resolvers are keyed by field, so a duplicate would silently
        // drop one of them.
        let mut seen: Vec<&str> = Vec::with_capacity(spec.bindings.len());
        for binding in &spec.bindings {
            if seen.contains(&binding.field.as_str()) {
                return Err(AuthError::Misconfigured(format!(
                    "duplicate binding for field `{}`",
                    binding.field
                )));
            }
            seen.push(&binding.field);
        }

        let mut resolvers: HashMap<String, Box<dyn CredentialResolver>> = HashMap::new();
        let mut prompt_fields = Vec::new();
        let mut script_fields = Vec::new();
        for binding in &spec.bindings {
            match &binding.provider {
                CredentialProvider::Prompt { .. } => prompt_fields.push(binding.clone()),
                CredentialProvider::ScriptResult => script_fields.push(binding.field.clone()),
                other => {
                    let r = other.build_resolver().map_err(AuthError::Misconfigured)?;
                    resolvers.insert(binding.field.clone(), r);
                }
            }
        }
        // `AuthSpec::validate_against` rejects the pairing while the
        // config is read; this is the same invariant restated where the
        // loop below would otherwise have nothing to run.
        if !script_fields.is_empty() && spec.script.is_none() {
            return Err(AuthError::Misconfigured(
                "bindings use `script-result` but the auth block names no `script`".into(),
            ));
        }
        let (status_tx, _) = watch::channel(AdapterStatus::Idle);
        Ok(Self {
            spec,
            resolvers,
            prompt_fields,
            script_fields,
            interactive_cache: RwLock::new(HashMap::new()),
            pending_prompt: Mutex::new(None),
            auth_mutex: Mutex::new(()),
            session_store,
            status_tx,
            clock: Arc::new(SystemClock),
        })
    }

    /// Override the clock — for tests.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub fn spec(&self) -> &AuthSpec {
        &self.spec
    }

    /// Subscribe to live status updates. Adapters forward this through
    /// `ContentAdapter::subscribe_status`.
    pub fn subscribe_status(&self) -> watch::Receiver<AdapterStatus> {
        self.status_tx.subscribe()
    }

    /// Return a fresh session, reusing the cached one if the policy and
    /// TTL allow. `login` is called only on cache miss / expiry; it
    /// receives all resolved credential fields and returns the
    /// adapter-defined session blob.
    pub async fn ensure_session<F, Fut>(&self, login: F) -> Result<ResolvedSession, AuthError>
    where
        F: FnOnce(HashMap<String, String>) -> Fut,
        Fut: std::future::Future<Output = Result<String, String>>,
    {
        let _guard = self.auth_mutex.lock().await;
        if let Some(entry) = self.session_store.load().await {
            if self.is_session_valid(&entry) {
                let _ = self.status_tx.send(AdapterStatus::Ready);
                return Ok(ResolvedSession {
                    blob: entry.blob,
                    from_cache: true,
                });
            }
            self.session_store.delete().await;
        }
        self.run_login(login).await
    }

    /// Re-authenticate from scratch: drop the stored session and ALL
    /// resolver caches, then run `ensure_session` again. Used when the
    /// server rejects the current credentials (HTTP 401/403). Wiping
    /// resolver caches prevents the cookie/keyring loop where the same
    /// stale value would otherwise be replayed.
    pub async fn re_authenticate<F, Fut>(&self, login: F) -> Result<ResolvedSession, AuthError>
    where
        F: FnOnce(HashMap<String, String>) -> Fut,
        Fut: std::future::Future<Output = Result<String, String>>,
    {
        let _guard = self.auth_mutex.lock().await;
        self.session_store.delete().await;
        for r in self.resolvers.values() {
            r.invalidate().await;
        }
        self.interactive_cache.write().await.clear();
        self.run_login(login).await
    }

    /// Reply path for `AdapterStatus::NeedsCreds`. Routes `fields` back
    /// to the in-flight `ensure_session` / `re_authenticate` call that
    /// published the prompt.
    pub async fn submit_credentials(
        &self,
        fields: HashMap<String, String>,
    ) -> Result<(), AuthError> {
        let tx = self
            .pending_prompt
            .lock()
            .await
            .take()
            .ok_or(AuthError::NoPromptPending)?;
        tx.send(fields).map_err(|_| AuthError::PromptCancelled)?;
        Ok(())
    }

    /// The user closed the credential form without answering it.
    ///
    /// Taking the sender and dropping it makes the login's await fail
    /// with [`AuthError::PromptCancelled`], which releases the auth mutex
    /// and lets the next attempt start from scratch. Without this the
    /// frontend can only stop *showing* the form while the login waits
    /// for an answer that will never come — and every later login queues
    /// up behind it.
    pub async fn cancel_prompt(&self) -> Result<(), AuthError> {
        let tx = self
            .pending_prompt
            .lock()
            .await
            .take()
            .ok_or(AuthError::NoPromptPending)?;
        drop(tx);
        Ok(())
    }

    /// Persist a freshly minted session blob. Adapters call this from
    /// `try_refresh_session` paths so refreshed tokens flow through the
    /// same persistence as the original login.
    pub async fn store_session(&self, blob: String) {
        if matches!(self.spec.session_cache, SessionCachePolicy::None) {
            return;
        }
        self.session_store
            .save(SessionEntry {
                blob,
                created_at: self.clock.now(),
            })
            .await;
    }

    /// Drop the persisted session. Resolver / prompt caches stay populated.
    pub async fn invalidate_session(&self) {
        self.session_store.delete().await;
    }

    /// Drop the persisted session AND every resolver / prompt cache.
    /// Equivalent to `re_authenticate` minus the actual login retry —
    /// useful for explicit "forget credentials" actions.
    pub async fn invalidate_credentials(&self) {
        self.session_store.delete().await;
        for r in self.resolvers.values() {
            r.invalidate().await;
        }
        self.interactive_cache.write().await.clear();
    }

    // --- internals ------------------------------------------------------

    fn is_session_valid(&self, entry: &SessionEntry) -> bool {
        match self.spec.session_cache {
            SessionCachePolicy::None => false,
            SessionCachePolicy::Ttl { ttl_secs } | SessionCachePolicy::TtlOrClose { ttl_secs } => {
                let age = self
                    .clock
                    .now()
                    .duration_since(entry.created_at)
                    .unwrap_or(Duration::ZERO);
                age < Duration::from_secs(ttl_secs)
            }
            SessionCachePolicy::UntilRejected | SessionCachePolicy::Explicit => true,
        }
    }

    async fn run_login<F, Fut>(&self, login: F) -> Result<ResolvedSession, AuthError>
    where
        F: FnOnce(HashMap<String, String>) -> Fut,
        Fut: std::future::Future<Output = Result<String, String>>,
    {
        let _ = self.status_tx.send(AdapterStatus::Connecting {
            retry: 1,
            max_retries: 1,
            timeout_secs: 30,
        });
        let credentials = self.resolve_credentials().await?;
        let blob = login(credentials).await.map_err(AuthError::LoginFailed)?;
        if !matches!(self.spec.session_cache, SessionCachePolicy::None) {
            self.session_store
                .save(SessionEntry {
                    blob: blob.clone(),
                    created_at: self.clock.now(),
                })
                .await;
        }
        let _ = self.status_tx.send(AdapterStatus::Ready);
        Ok(ResolvedSession {
            blob,
            from_cache: false,
        })
    }

    async fn resolve_credentials(&self) -> Result<HashMap<String, String>, AuthError> {
        let mut values = HashMap::new();

        for binding in &self.spec.bindings {
            if binding.provider.needs_frontend() {
                continue;
            }
            let r = self
                .resolvers
                .get(&binding.field)
                .expect("non-interactive resolver registered");
            let v = r.resolve().await?;
            values.insert(binding.field.clone(), v);
        }

        let interactive: Vec<&str> = self
            .prompt_fields
            .iter()
            .map(|b| b.field.as_str())
            .chain(self.script_fields.iter().map(String::as_str))
            .collect();
        if !interactive.is_empty() {
            let cached = self.interactive_cache.read().await;
            if interactive.iter().all(|f| cached.contains_key(*f)) {
                for f in interactive {
                    values.insert(f.to_string(), cached[f].clone());
                }
            } else {
                drop(cached);
                let collected = self.collect_interactive().await?;
                let mut cache = self.interactive_cache.write().await;
                for (k, v) in collected.iter() {
                    cache.insert(k.clone(), v.clone());
                }
                drop(cache);
                for (k, v) in collected {
                    values.insert(k, v);
                }
            }
        }
        Ok(values)
    }

    /// Fill every interactive field: the plain prompts in one dialog,
    /// then whatever the credential script needs.
    ///
    /// The two are deliberately not merged into one form. A prompt is
    /// known from the config, a script's form only after the script has
    /// run — and it may not need one at all. Waiting for the script
    /// before showing the prompts would delay a dialog that was never in
    /// question.
    async fn collect_interactive(&self) -> Result<HashMap<String, String>, AuthError> {
        let mut values = HashMap::new();

        if !self.prompt_fields.is_empty() {
            let fields: Vec<AuthField> = self
                .prompt_fields
                .iter()
                .map(|b| AuthField {
                    name: b.field.clone(),
                    label: b.effective_label(),
                    masked: b.effective_masked(),
                    // The config bound this field; leaving it blank is
                    // not a choice the frontend gets to offer.
                    optional: false,
                    prefill: match &b.provider {
                        CredentialProvider::Prompt { prefill } => prefill.clone(),
                        _ => None,
                    },
                })
                .collect();
            let answers = self.ask(fields, None, None).await?;
            for b in &self.prompt_fields {
                if let Some(v) = answers.get(&b.field) {
                    values.insert(b.field.clone(), v.clone());
                }
            }
        }

        if !self.script_fields.is_empty() {
            for (k, v) in self.run_credential_script().await? {
                values.insert(k, v);
            }
        }
        Ok(values)
    }

    /// Publish one form and wait for the frontend's reply.
    ///
    /// The pending sender is what `submit_credentials` and
    /// `cancel_prompt` reach for; dropping it (cancel) makes the await
    /// below fail, which is how Esc ends a login instead of leaving it
    /// parked on the auth mutex.
    async fn ask(
        &self,
        fields: Vec<AuthField>,
        header: Option<String>,
        error: Option<String>,
    ) -> Result<HashMap<String, String>, AuthError> {
        let (tx, rx) = oneshot::channel();
        *self.pending_prompt.lock().await = Some(tx);
        let _ = self.status_tx.send(AdapterStatus::NeedsCreds {
            fields,
            header,
            error,
        });
        rx.await.map_err(|_| AuthError::PromptCancelled)
    }

    /// Run the credential script until it hands over the values.
    ///
    /// Each round is a fresh process, so `input` carries every answer
    /// collected so far rather than just the newest one.
    async fn run_credential_script(&self) -> Result<HashMap<String, String>, AuthError> {
        let script = self.spec.script.as_deref().ok_or_else(|| {
            AuthError::Misconfigured("no `script` for the `script-result` bindings".into())
        })?;
        let timeout = Duration::from_secs(self.spec.script_timeout_secs);
        let request: Vec<&str> = self.script_fields.iter().map(String::as_str).collect();
        let mut input: BTreeMap<String, String> = BTreeMap::new();

        for _ in 0..MAX_SCRIPT_ROUNDS {
            match credential_script::run_round(script, &request, &input, timeout).await? {
                ScriptRound::Values(values) => {
                    // Completeness against `request` is checked while the
                    // round is parsed, so every key is present here; the
                    // script's extra keys are none of our business.
                    return Ok(request
                        .iter()
                        .map(|f| ((*f).to_string(), values[*f].clone()))
                        .collect());
                }
                ScriptRound::Failed(message) => {
                    return Err(AuthError::Credential(CredentialError::ProviderError(
                        format!("credential script `{script}`: {message}"),
                    )));
                }
                ScriptRound::Form(form) => {
                    let fields: Vec<AuthField> = form
                        .fields
                        .iter()
                        .map(|f| AuthField {
                            name: f.name.clone(),
                            label: f.effective_label(),
                            masked: f.masked,
                            optional: f.optional,
                            prefill: f.prefill.clone(),
                        })
                        .collect();
                    let answers = self.ask(fields, form.header, form.error).await?;
                    for f in &form.fields {
                        if let Some(v) = answers.get(&f.name) {
                            input.insert(f.name.clone(), v.clone());
                        }
                    }
                }
            }
        }
        Err(AuthError::Credential(CredentialError::ProviderError(
            format!(
                "credential script `{script}` still asked for input after \
                 {MAX_SCRIPT_ROUNDS} rounds"
            ),
        )))
    }
}

// --- Tests ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::session_store::InMemorySessionStore;
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestClock {
        now: StdMutex<SystemTime>,
    }

    impl TestClock {
        fn new(t: SystemTime) -> Arc<Self> {
            Arc::new(Self {
                now: StdMutex::new(t),
            })
        }

        fn advance(&self, d: Duration) {
            let mut n = self.now.lock().unwrap();
            *n += d;
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> SystemTime {
            *self.now.lock().unwrap()
        }
    }

    fn bearer_spec(provider: CredentialProvider, policy: SessionCachePolicy) -> AuthSpec {
        AuthSpec {
            mechanism: "bearer-token".into(),
            session_cache: policy,
            script: None,
            script_timeout_secs: 120,
            bindings: vec![CredentialBinding {
                field: "token".into(),
                provider,
                label: None,
                masked: None,
            }],
        }
    }

    fn password_spec_with_prompt(policy: SessionCachePolicy) -> AuthSpec {
        AuthSpec {
            mechanism: "password-login".into(),
            session_cache: policy,
            script: None,
            script_timeout_secs: 120,
            bindings: vec![
                CredentialBinding {
                    field: "username".into(),
                    provider: CredentialProvider::Literal {
                        value: "alice".into(),
                    },
                    label: None,
                    masked: None,
                },
                CredentialBinding {
                    field: "password".into(),
                    provider: CredentialProvider::Prompt { prefill: None },
                    label: None,
                    masked: None,
                },
            ],
        }
    }

    fn build(spec: AuthSpec, clock: Arc<dyn Clock>) -> AuthOrchestrator {
        AuthOrchestrator::from_spec(spec, Box::new(InMemorySessionStore::new()))
            .expect("spec valid")
            .with_clock(clock)
    }

    /// Field coverage is the descriptors' business (checked while the
    /// adapter's config is read), but a duplicate binding would silently
    /// drop a resolver from the map this constructor builds — so it stays
    /// a construction error.
    #[tokio::test]
    async fn duplicate_bindings_are_rejected_at_construction() {
        let binding = |provider| CredentialBinding {
            field: "token".into(),
            provider,
            label: None,
            masked: None,
        };
        let bad = AuthSpec {
            mechanism: "bearer-token".into(),
            session_cache: SessionCachePolicy::None,
            script: None,
            script_timeout_secs: 120,
            bindings: vec![
                binding(CredentialProvider::Literal { value: "a".into() }),
                binding(CredentialProvider::Literal { value: "b".into() }),
            ],
        };
        let res = AuthOrchestrator::from_spec(bad, Box::new(InMemorySessionStore::new()));
        assert!(matches!(res, Err(AuthError::Misconfigured(_))));
    }

    #[tokio::test]
    async fn ensure_session_calls_login_on_first_use() {
        let spec = bearer_spec(
            CredentialProvider::Literal {
                value: "synthetic-token".into(),
            },
            SessionCachePolicy::UntilRejected,
        );
        let orch = build(spec, TestClock::new(SystemTime::UNIX_EPOCH));
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in = calls.clone();
        let session = orch
            .ensure_session(|creds| {
                calls_in.fetch_add(1, Ordering::SeqCst);
                async move {
                    assert_eq!(creds.get("token").unwrap(), "synthetic-token");
                    Ok::<_, String>("session-blob-1".into())
                }
            })
            .await
            .expect("ok");
        assert_eq!(session.blob, "session-blob-1");
        assert!(!session.from_cache);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn ensure_session_reuses_cached_blob() {
        let spec = bearer_spec(
            CredentialProvider::Literal { value: "x".into() },
            SessionCachePolicy::UntilRejected,
        );
        let orch = build(spec, TestClock::new(SystemTime::UNIX_EPOCH));
        let calls = Arc::new(AtomicUsize::new(0));

        for expected_from_cache in [false, true, true] {
            let calls_in = calls.clone();
            let s = orch
                .ensure_session(|_| {
                    calls_in.fetch_add(1, Ordering::SeqCst);
                    async { Ok::<_, String>("blob".into()) }
                })
                .await
                .unwrap();
            assert_eq!(s.from_cache, expected_from_cache);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn ttl_policy_expires_after_ttl() {
        let clock = TestClock::new(SystemTime::UNIX_EPOCH);
        let spec = bearer_spec(
            CredentialProvider::Literal { value: "x".into() },
            SessionCachePolicy::Ttl { ttl_secs: 60 },
        );
        let orch = build(spec, clock.clone());
        let calls = Arc::new(AtomicUsize::new(0));

        let calls_in = calls.clone();
        orch.ensure_session(|_| {
            calls_in.fetch_add(1, Ordering::SeqCst);
            async { Ok::<_, String>("v1".into()) }
        })
        .await
        .unwrap();

        clock.advance(Duration::from_secs(30));
        let calls_in = calls.clone();
        let s = orch
            .ensure_session(|_| {
                calls_in.fetch_add(1, Ordering::SeqCst);
                async { Ok::<_, String>("v2".into()) }
            })
            .await
            .unwrap();
        assert!(s.from_cache, "still inside TTL window");
        assert_eq!(s.blob, "v1");

        clock.advance(Duration::from_secs(31)); // total 61s
        let calls_in = calls.clone();
        let s = orch
            .ensure_session(|_| {
                calls_in.fetch_add(1, Ordering::SeqCst);
                async { Ok::<_, String>("v2".into()) }
            })
            .await
            .unwrap();
        assert!(!s.from_cache, "TTL exceeded");
        assert_eq!(s.blob, "v2");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn none_policy_does_not_persist_session() {
        let spec = bearer_spec(
            CredentialProvider::Literal { value: "x".into() },
            SessionCachePolicy::None,
        );
        let store = Arc::new(InMemorySessionStore::new());
        let orch = AuthOrchestrator::from_spec(spec, Box::new(StoreHandle(store.clone())))
            .unwrap()
            .with_clock(TestClock::new(SystemTime::UNIX_EPOCH));

        orch.ensure_session(|_| async { Ok::<_, String>("blob".into()) })
            .await
            .unwrap();
        assert!(store.load().await.is_none(), "Policy::None must not save");

        // Second call also runs login — nothing was cached.
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in = calls.clone();
        orch.ensure_session(|_| {
            calls_in.fetch_add(1, Ordering::SeqCst);
            async { Ok::<_, String>("blob2".into()) }
        })
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// Forwards calls to a shared store so the test can also inspect it.
    struct StoreHandle(Arc<InMemorySessionStore>);
    #[async_trait::async_trait]
    impl SessionStore for StoreHandle {
        async fn load(&self) -> Option<SessionEntry> {
            self.0.load().await
        }
        async fn save(&self, e: SessionEntry) {
            self.0.save(e).await
        }
        async fn delete(&self) {
            self.0.delete().await
        }
    }

    #[tokio::test]
    async fn re_authenticate_drops_store_and_invalidates_resolvers() {
        // Use a file-backed token so we can prove the resolver cache was cleared.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        tokio::fs::write(&path, b"tok-v1\n").await.unwrap();

        let spec = bearer_spec(
            CredentialProvider::File {
                path: path.clone(),
                trim: true,
            },
            SessionCachePolicy::UntilRejected,
        );
        let orch = build(spec, TestClock::new(SystemTime::UNIX_EPOCH));

        let seen = Arc::new(StdMutex::new(Vec::<String>::new()));
        let seen_in = seen.clone();
        orch.ensure_session(|c| {
            seen_in.lock().unwrap().push(c["token"].clone());
            async { Ok::<_, String>("login-1".into()) }
        })
        .await
        .unwrap();

        // Rotate the on-disk token, then re-auth: resolver cache must be
        // dropped so we pick up the new value.
        tokio::fs::write(&path, b"tok-v2\n").await.unwrap();
        let seen_in = seen.clone();
        let s = orch
            .re_authenticate(|c| {
                seen_in.lock().unwrap().push(c["token"].clone());
                async { Ok::<_, String>("login-2".into()) }
            })
            .await
            .unwrap();
        assert!(!s.from_cache);
        assert_eq!(s.blob, "login-2");
        assert_eq!(*seen.lock().unwrap(), vec!["tok-v1", "tok-v2"]);
    }

    #[tokio::test]
    async fn prompt_flow_publishes_needs_creds_and_routes_reply() {
        let orch = Arc::new(build(
            password_spec_with_prompt(SessionCachePolicy::UntilRejected),
            TestClock::new(SystemTime::UNIX_EPOCH),
        ));
        let mut rx = orch.subscribe_status();

        let orch_in = orch.clone();
        let login = tokio::spawn(async move {
            orch_in
                .ensure_session(|creds| async move {
                    assert_eq!(creds.get("username").unwrap(), "alice");
                    assert_eq!(creds.get("password").unwrap(), "synthetic-pw");
                    Ok::<_, String>("blob".into())
                })
                .await
        });

        // Wait for NeedsCreds.
        loop {
            rx.changed().await.unwrap();
            if let AdapterStatus::NeedsCreds { fields, .. } = &*rx.borrow() {
                assert_eq!(fields.len(), 1);
                assert_eq!(fields[0].name, "password");
                assert!(fields[0].masked);
                break;
            }
        }

        let mut reply = HashMap::new();
        reply.insert("password".into(), "synthetic-pw".into());
        orch.submit_credentials(reply).await.unwrap();

        let session = login.await.unwrap().unwrap();
        assert_eq!(session.blob, "blob");
    }

    #[tokio::test]
    async fn prompt_values_are_cached_until_invalidate() {
        let orch = Arc::new(build(
            password_spec_with_prompt(SessionCachePolicy::None),
            TestClock::new(SystemTime::UNIX_EPOCH),
        ));

        // First call → prompt round-trip.
        let orch_in = orch.clone();
        let first = tokio::spawn(async move {
            orch_in
                .ensure_session(|c| async move {
                    assert_eq!(c["password"], "pw1");
                    Ok::<_, String>("s1".into())
                })
                .await
        });
        let mut rx = orch.subscribe_status();
        loop {
            rx.changed().await.unwrap();
            if matches!(&*rx.borrow(), AdapterStatus::NeedsCreds { .. }) {
                break;
            }
        }
        let mut reply = HashMap::new();
        reply.insert("password".into(), "pw1".into());
        orch.submit_credentials(reply).await.unwrap();
        first.await.unwrap().unwrap();

        // Second call → prompt cache hits, no NeedsCreds, no submit needed.
        let s = orch
            .ensure_session(|c| async move {
                assert_eq!(c["password"], "pw1");
                Ok::<_, String>("s2".into())
            })
            .await
            .unwrap();
        assert_eq!(s.blob, "s2");

        // After invalidate_credentials, prompt is needed again.
        orch.invalidate_credentials().await;
        let orch_in = orch.clone();
        let third = tokio::spawn(async move {
            orch_in
                .ensure_session(|c| async move {
                    assert_eq!(c["password"], "pw2");
                    Ok::<_, String>("s3".into())
                })
                .await
        });
        loop {
            rx.changed().await.unwrap();
            if matches!(&*rx.borrow(), AdapterStatus::NeedsCreds { .. }) {
                break;
            }
        }
        let mut reply = HashMap::new();
        reply.insert("password".into(), "pw2".into());
        orch.submit_credentials(reply).await.unwrap();
        third.await.unwrap().unwrap();
    }

    // --- credential script -------------------------------------------------

    /// Write an executable script and return the path to run it by.
    fn write_script(dir: &std::path::Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path.display().to_string()
    }

    /// The motivating script: hands over both values when the store is
    /// already unlocked (the passphrase is in `input`), asks for the
    /// passphrase when it is not.
    const PASS_SCRIPT: &str = r#"#!/bin/sh
req=$(cat)
case "$req" in
  *'"passphrase":"hunter2"'*)
    printf '{"result":{"username":"alice","token":"t-42"}}'
    ;;
  *'"passphrase"'*)
    printf '{"form":{"header":"Unlock the password store","error":"that passphrase was rejected","fields":[{"name":"passphrase","masked":true}]}}'
    ;;
  *)
    printf '{"form":{"header":"Unlock the password store","fields":[{"name":"passphrase","label":"Passphrase","masked":true}]}}'
    ;;
esac
"#;

    /// `user-api-token` with both fields off one script.
    fn script_spec(script: &str, policy: SessionCachePolicy) -> AuthSpec {
        let binding = |field: &str| CredentialBinding {
            field: field.into(),
            provider: CredentialProvider::ScriptResult,
            label: None,
            masked: None,
        };
        AuthSpec {
            mechanism: "user-api-token".into(),
            session_cache: policy,
            script: Some(script.to_string()),
            script_timeout_secs: 10,
            bindings: vec![binding("username"), binding("token")],
        }
    }

    /// The everyday case: the store is open, the script answers in one
    /// round, and the user is never asked anything.
    #[tokio::test]
    async fn an_unlocked_store_costs_no_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(
            dir.path(),
            "open.sh",
            "#!/bin/sh\ncat >/dev/null\nprintf '{\"result\":{\"username\":\"alice\",\"token\":\"t-42\"}}'\n",
        );
        let orch = build(
            script_spec(&script, SessionCachePolicy::None),
            TestClock::new(SystemTime::UNIX_EPOCH),
        );
        let mut rx = orch.subscribe_status();
        let session = orch
            .ensure_session(|creds| async move {
                assert_eq!(creds.len(), 2, "both fields from one run: {creds:?}");
                assert_eq!(creds["username"], "alice");
                assert_eq!(creds["token"], "t-42");
                Ok::<_, String>("blob".into())
            })
            .await
            .expect("no prompt needed");
        assert_eq!(session.blob, "blob");
        assert!(
            !matches!(&*rx.borrow_and_update(), AdapterStatus::NeedsCreds { .. }),
            "no form was published"
        );
    }

    /// The locked case: the script's form reaches the frontend with its
    /// header, and the answer comes back to the script as `input`.
    #[tokio::test]
    async fn a_locked_store_asks_once_and_the_answer_completes_the_login() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(dir.path(), "pass.sh", PASS_SCRIPT);
        let orch = Arc::new(build(
            script_spec(&script, SessionCachePolicy::None),
            TestClock::new(SystemTime::UNIX_EPOCH),
        ));
        let mut rx = orch.subscribe_status();

        let orch_in = orch.clone();
        let login = tokio::spawn(async move {
            orch_in
                .ensure_session(|creds| async move {
                    assert_eq!(creds["username"], "alice");
                    assert_eq!(creds["token"], "t-42");
                    Ok::<_, String>("blob".into())
                })
                .await
        });

        loop {
            rx.changed().await.unwrap();
            if let AdapterStatus::NeedsCreds {
                fields,
                header,
                error,
            } = &*rx.borrow()
            {
                assert_eq!(fields.len(), 1);
                assert_eq!(fields[0].name, "passphrase");
                assert_eq!(fields[0].label, "Passphrase");
                assert!(fields[0].masked);
                assert_eq!(header.as_deref(), Some("Unlock the password store"));
                assert!(error.is_none(), "nothing has failed yet");
                break;
            }
        }

        let mut reply = HashMap::new();
        reply.insert("passphrase".to_string(), "hunter2".to_string());
        orch.submit_credentials(reply).await.unwrap();
        assert_eq!(login.await.unwrap().unwrap().blob, "blob");
    }

    /// A rejected passphrase is re-asked with the script's own message —
    /// and the answers pile up, because each round is a fresh process
    /// that remembers nothing.
    #[tokio::test]
    async fn a_re_ask_carries_the_error_and_input_accumulates() {
        let dir = tempfile::tempdir().unwrap();
        // Asks for `a`, then for `b`, then reports what it still sees.
        let script = write_script(
            dir.path(),
            "two-step.sh",
            r#"#!/bin/sh
req=$(cat)
case "$req" in
  *'"a":"1"'*'"b":"2"'*)
    printf '{"result":{"username":"alice","token":"saw-both"}}' ;;
  *'"a":"1"'*)
    printf '{"form":{"error":"one more","fields":[{"name":"b"}]}}' ;;
  *)
    printf '{"form":{"fields":[{"name":"a"}]}}' ;;
esac
"#,
        );
        let orch = Arc::new(build(
            script_spec(&script, SessionCachePolicy::None),
            TestClock::new(SystemTime::UNIX_EPOCH),
        ));
        let mut rx = orch.subscribe_status();

        let orch_in = orch.clone();
        let login = tokio::spawn(async move {
            orch_in
                .ensure_session(|creds| async move {
                    // Only reachable if round three still saw the answer
                    // given in round one.
                    assert_eq!(creds["token"], "saw-both");
                    Ok::<_, String>("blob".into())
                })
                .await
        });

        for (field, expected_error) in [("a", None), ("b", Some("one more"))] {
            loop {
                rx.changed().await.unwrap();
                if let AdapterStatus::NeedsCreds { fields, error, .. } = &*rx.borrow() {
                    assert_eq!(fields[0].name, field);
                    assert_eq!(error.as_deref(), expected_error);
                    break;
                }
            }
            let mut reply = HashMap::new();
            reply.insert(
                field.to_string(),
                if field == "a" { "1" } else { "2" }.to_string(),
            );
            orch.submit_credentials(reply).await.unwrap();
        }
        assert_eq!(login.await.unwrap().unwrap().blob, "blob");
    }

    /// Esc must end the login, not just hide the form: the auth mutex is
    /// held while the round waits, so a login left waiting blocks every
    /// later attempt.
    #[tokio::test]
    async fn cancelling_the_form_fails_the_login_and_frees_the_next_one() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(dir.path(), "pass.sh", PASS_SCRIPT);
        let orch = Arc::new(build(
            script_spec(&script, SessionCachePolicy::None),
            TestClock::new(SystemTime::UNIX_EPOCH),
        ));
        let mut rx = orch.subscribe_status();

        let orch_in = orch.clone();
        let login = tokio::spawn(async move {
            orch_in
                .ensure_session(|_| async { panic!("login must not run") })
                .await
        });
        loop {
            rx.changed().await.unwrap();
            if matches!(&*rx.borrow(), AdapterStatus::NeedsCreds { .. }) {
                break;
            }
        }
        orch.cancel_prompt().await.expect("a prompt was pending");

        let err = login.await.unwrap().expect_err("must fail");
        assert!(matches!(err, AuthError::PromptCancelled), "got: {err}");

        // The mutex is free again: a second attempt gets as far as its
        // own form instead of queueing behind the abandoned one.
        let orch_in = orch.clone();
        let second = tokio::spawn(async move {
            orch_in
                .ensure_session(|_| async { Ok::<_, String>("blob".into()) })
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                rx.changed().await.unwrap();
                if matches!(&*rx.borrow(), AdapterStatus::NeedsCreds { .. }) {
                    return;
                }
            }
        })
        .await
        .expect("second attempt reaches its own form");
        orch.cancel_prompt().await.unwrap();
        let _ = second.await.unwrap();
    }

    #[tokio::test]
    async fn cancelling_without_a_pending_prompt_errors() {
        let orch = build(
            bearer_spec(
                CredentialProvider::Literal { value: "t".into() },
                SessionCachePolicy::None,
            ),
            TestClock::new(SystemTime::UNIX_EPOCH),
        );
        let err = orch.cancel_prompt().await.expect_err("must fail");
        assert!(matches!(err, AuthError::NoPromptPending), "got: {err}");
    }

    /// A script that keeps asking must not hold the login open forever.
    #[tokio::test]
    async fn a_script_that_never_stops_asking_hits_the_round_cap() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(
            dir.path(),
            "greedy.sh",
            "#!/bin/sh\ncat >/dev/null\nprintf '{\"form\":{\"fields\":[{\"name\":\"again\"}]}}'\n",
        );
        let orch = Arc::new(build(
            script_spec(&script, SessionCachePolicy::None),
            TestClock::new(SystemTime::UNIX_EPOCH),
        ));
        let mut rx = orch.subscribe_status();

        let orch_in = orch.clone();
        let login = tokio::spawn(async move {
            orch_in
                .ensure_session(|_| async { panic!("login must not run") })
                .await
        });
        for _ in 0..MAX_SCRIPT_ROUNDS {
            loop {
                rx.changed().await.unwrap();
                if matches!(&*rx.borrow(), AdapterStatus::NeedsCreds { .. }) {
                    break;
                }
            }
            let mut reply = HashMap::new();
            reply.insert("again".to_string(), "sure".to_string());
            // The last round is the one that gives up, so by then there
            // may be no prompt left to answer.
            let _ = orch.submit_credentials(reply).await;
        }
        let err = login.await.unwrap().expect_err("must fail");
        assert!(err.to_string().contains("after 5 rounds"), "got: {err}");
    }

    /// A script's own error ends the login and keeps its wording — the
    /// script knows why better than we do.
    #[tokio::test]
    async fn a_script_error_fails_the_login_with_its_message() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(
            dir.path(),
            "angry.sh",
            "#!/bin/sh\ncat >/dev/null\nprintf '{\"error\":\"no password store here\"}'\n",
        );
        let orch = build(
            script_spec(&script, SessionCachePolicy::None),
            TestClock::new(SystemTime::UNIX_EPOCH),
        );
        let mut rx = orch.subscribe_status();
        let err = orch
            .ensure_session(|_| async { panic!("login must not run") })
            .await
            .expect_err("must fail");
        assert!(
            err.to_string().contains("no password store here"),
            "got: {err}"
        );
        assert!(
            !matches!(&*rx.borrow_and_update(), AdapterStatus::NeedsCreds { .. }),
            "nothing was asked"
        );
    }

    /// Prompts are known from the config, the script's form only after it
    /// has run — so they are two dialogs, prompts first.
    #[tokio::test]
    async fn prompt_bindings_are_asked_before_the_script_runs() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(dir.path(), "pass.sh", PASS_SCRIPT);
        let spec = AuthSpec {
            mechanism: "user-api-token".into(),
            session_cache: SessionCachePolicy::None,
            script: Some(script),
            script_timeout_secs: 10,
            bindings: vec![
                CredentialBinding {
                    field: "username".into(),
                    provider: CredentialProvider::Prompt {
                        prefill: Some("bob".into()),
                    },
                    label: None,
                    masked: None,
                },
                CredentialBinding {
                    field: "token".into(),
                    provider: CredentialProvider::ScriptResult,
                    label: None,
                    masked: None,
                },
            ],
        };
        let orch = Arc::new(build(spec, TestClock::new(SystemTime::UNIX_EPOCH)));
        let mut rx = orch.subscribe_status();

        let orch_in = orch.clone();
        let login = tokio::spawn(async move {
            orch_in
                .ensure_session(|creds| async move {
                    assert_eq!(creds["username"], "bob");
                    assert_eq!(creds["token"], "t-42");
                    Ok::<_, String>("blob".into())
                })
                .await
        });

        // First dialog: the configured prompt, with its prefill.
        loop {
            rx.changed().await.unwrap();
            if let AdapterStatus::NeedsCreds { fields, header, .. } = &*rx.borrow() {
                assert_eq!(fields.len(), 1);
                assert_eq!(fields[0].name, "username");
                assert_eq!(fields[0].prefill.as_deref(), Some("bob"));
                assert!(header.is_none(), "a plain prompt has nothing to announce");
                break;
            }
        }
        let mut reply = HashMap::new();
        reply.insert("username".to_string(), "bob".to_string());
        orch.submit_credentials(reply).await.unwrap();

        // Second dialog: the script's, with its own header.
        loop {
            rx.changed().await.unwrap();
            if let AdapterStatus::NeedsCreds { fields, header, .. } = &*rx.borrow() {
                assert_eq!(fields[0].name, "passphrase");
                assert_eq!(header.as_deref(), Some("Unlock the password store"));
                break;
            }
        }
        let mut reply = HashMap::new();
        reply.insert("passphrase".to_string(), "hunter2".to_string());
        orch.submit_credentials(reply).await.unwrap();
        assert_eq!(login.await.unwrap().unwrap().blob, "blob");
    }

    /// Re-authenticating runs the script again — the whole point is that
    /// its values may have gone stale.
    #[tokio::test]
    async fn re_authenticate_runs_the_script_again() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(dir.path(), "pass.sh", PASS_SCRIPT);
        let orch = Arc::new(build(
            script_spec(&script, SessionCachePolicy::None),
            TestClock::new(SystemTime::UNIX_EPOCH),
        ));

        for round in 0..2 {
            // Subscribe before the login starts, so the form cannot be
            // published between the spawn and the first `changed()`.
            let mut rx = orch.subscribe_status();
            let orch_in = orch.clone();
            let run = tokio::spawn(async move {
                let call =
                    |_: HashMap<String, String>| async move { Ok::<_, String>("blob".into()) };
                if round == 0 {
                    orch_in.ensure_session(call).await
                } else {
                    orch_in.re_authenticate(call).await
                }
            });
            loop {
                rx.changed().await.unwrap();
                if matches!(&*rx.borrow(), AdapterStatus::NeedsCreds { .. }) {
                    break;
                }
            }
            let mut reply = HashMap::new();
            reply.insert("passphrase".to_string(), "hunter2".to_string());
            orch.submit_credentials(reply).await.unwrap();
            run.await.unwrap().unwrap();
        }
    }

    /// The pairing is checked while the config is read, but a spec that
    /// slips past that must not leave the orchestrator with a loop it
    /// cannot run.
    #[tokio::test]
    async fn script_result_without_a_script_is_rejected_at_construction() {
        let spec = AuthSpec {
            mechanism: "bearer-token".into(),
            session_cache: SessionCachePolicy::None,
            script: None,
            script_timeout_secs: 120,
            bindings: vec![CredentialBinding {
                field: "token".into(),
                provider: CredentialProvider::ScriptResult,
                label: None,
                masked: None,
            }],
        };
        let err = AuthOrchestrator::from_spec(spec, Box::new(InMemorySessionStore::new()))
            .err()
            .expect("must be rejected");
        assert!(matches!(err, AuthError::Misconfigured(_)), "got: {err}");
    }

    #[tokio::test]
    async fn submit_credentials_without_pending_prompt_errors() {
        let orch = build(
            password_spec_with_prompt(SessionCachePolicy::None),
            TestClock::new(SystemTime::UNIX_EPOCH),
        );
        let mut reply = HashMap::new();
        reply.insert("password".into(), "x".into());
        let err = orch.submit_credentials(reply).await.expect_err("must fail");
        assert!(matches!(err, AuthError::NoPromptPending));
    }

    #[tokio::test]
    async fn login_failure_propagates_and_does_not_cache() {
        let spec = bearer_spec(
            CredentialProvider::Literal { value: "x".into() },
            SessionCachePolicy::UntilRejected,
        );
        let orch = build(spec, TestClock::new(SystemTime::UNIX_EPOCH));
        let err = orch
            .ensure_session(|_| async { Err::<String, _>("server says no".into()) })
            .await
            .expect_err("must fail");
        assert!(matches!(err, AuthError::LoginFailed(ref m) if m == "server says no"));

        // Next call retries (no cached session).
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in = calls.clone();
        orch.ensure_session(|_| {
            calls_in.fetch_add(1, Ordering::SeqCst);
            async { Ok::<_, String>("blob".into()) }
        })
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn invalidate_session_keeps_resolver_caches() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token");
        tokio::fs::write(&path, b"tok-v1\n").await.unwrap();
        let spec = bearer_spec(
            CredentialProvider::File {
                path: path.clone(),
                trim: true,
            },
            SessionCachePolicy::UntilRejected,
        );
        let orch = build(spec, TestClock::new(SystemTime::UNIX_EPOCH));

        let seen = Arc::new(StdMutex::new(Vec::<String>::new()));
        let seen_in = seen.clone();
        orch.ensure_session(|c| {
            seen_in.lock().unwrap().push(c["token"].clone());
            async { Ok::<_, String>("login-1".into()) }
        })
        .await
        .unwrap();

        tokio::fs::write(&path, b"tok-v2\n").await.unwrap();
        orch.invalidate_session().await;

        let seen_in = seen.clone();
        orch.ensure_session(|c| {
            seen_in.lock().unwrap().push(c["token"].clone());
            async { Ok::<_, String>("login-2".into()) }
        })
        .await
        .unwrap();
        // The on-disk file changed, but the resolver cache was kept —
        // invalidate_session only drops the *session*, not credentials.
        assert_eq!(*seen.lock().unwrap(), vec!["tok-v1", "tok-v1"]);
    }

    #[tokio::test]
    async fn store_session_persists_externally_minted_blob() {
        let store = Arc::new(InMemorySessionStore::new());
        let spec = bearer_spec(
            CredentialProvider::Literal { value: "x".into() },
            SessionCachePolicy::UntilRejected,
        );
        let orch = AuthOrchestrator::from_spec(spec, Box::new(StoreHandle(store.clone())))
            .unwrap()
            .with_clock(TestClock::new(SystemTime::UNIX_EPOCH));
        orch.store_session("refreshed-blob".into()).await;
        let entry = store.load().await.expect("saved");
        assert_eq!(entry.blob, "refreshed-blob");

        // Subsequent ensure_session sees the externally-stored blob.
        let s = orch
            .ensure_session(|_| async { panic!("login must not run") })
            .await
            .unwrap();
        assert!(s.from_cache);
        assert_eq!(s.blob, "refreshed-blob");
    }

    #[tokio::test]
    async fn store_session_skipped_for_none_policy() {
        let store = Arc::new(InMemorySessionStore::new());
        let spec = bearer_spec(
            CredentialProvider::Literal { value: "x".into() },
            SessionCachePolicy::None,
        );
        let orch = AuthOrchestrator::from_spec(spec, Box::new(StoreHandle(store.clone())))
            .unwrap()
            .with_clock(TestClock::new(SystemTime::UNIX_EPOCH));
        orch.store_session("blob".into()).await;
        assert!(store.load().await.is_none());
    }

    #[tokio::test]
    async fn explicit_policy_keeps_session_indefinitely() {
        let clock = TestClock::new(SystemTime::UNIX_EPOCH);
        let spec = bearer_spec(
            CredentialProvider::Literal { value: "x".into() },
            SessionCachePolicy::Explicit,
        );
        let orch = build(spec, clock.clone());
        orch.ensure_session(|_| async { Ok::<_, String>("blob".into()) })
            .await
            .unwrap();
        clock.advance(Duration::from_secs(60 * 60 * 24 * 365));
        let s = orch
            .ensure_session(|_| async { panic!("login must not run") })
            .await
            .unwrap();
        assert!(s.from_cache);
        assert_eq!(s.blob, "blob");
    }

    #[tokio::test]
    async fn missing_credential_propagates_error() {
        let spec = bearer_spec(
            CredentialProvider::Env {
                var: "NYD_AUTH_ORCH_TEST_DEFINITELY_UNSET".into(),
            },
            SessionCachePolicy::UntilRejected,
        );
        let orch = build(spec, TestClock::new(SystemTime::UNIX_EPOCH));
        let err = orch
            .ensure_session(|_| async { panic!("login must not run") })
            .await
            .expect_err("must fail");
        assert!(matches!(err, AuthError::Credential(_)));
    }
}
