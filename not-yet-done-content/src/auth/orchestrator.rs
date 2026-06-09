//! Login flow orchestration: resolves credentials via the configured
//! providers, hands them to an adapter-supplied login function, and
//! caches the resulting session according to [`SessionCachePolicy`].
//!
//! The orchestrator splits the configured bindings into two groups:
//!
//! - **Value providers** (literal/env/file/command/keyring) build their
//!   resolvers up-front and hand back the value on demand.
//! - **Prompt providers** are filled by the frontend. The orchestrator
//!   collects every prompt field into a single
//!   [`AdapterStatus::NeedsCreds`] form, publishes it on the status
//!   channel, and waits for [`AuthOrchestrator::submit_credentials`] to
//!   route the user's reply back. Subsequent calls reuse the supplied
//!   values until [`AuthOrchestrator::re_authenticate`] or
//!   [`AuthOrchestrator::invalidate_credentials`] clears the cache.
//!
//! The whole login flow is serialised via an internal mutex — concurrent
//! `ensure_session` callers queue up rather than racing the prompt form.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use thiserror::Error;
use tokio::sync::{Mutex, RwLock, oneshot, watch};

use super::session_store::{SessionEntry, SessionStore};
use super::{
    AuthSpec, CredentialBinding, CredentialError, CredentialProvider, CredentialResolver,
    SessionCachePolicy,
};
use crate::{AdapterStatus, AuthField};

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
    /// Per-field resolver, keyed by field name. Prompt fields are NOT in
    /// this map — they're filled via `submit_credentials`.
    resolvers: HashMap<String, Box<dyn CredentialResolver>>,
    /// Bindings that need a frontend prompt. Order matches `spec.bindings`.
    prompt_fields: Vec<CredentialBinding>,
    /// Cache of values supplied via the prompt path. Cleared by
    /// `re_authenticate` / `invalidate_credentials` along with the
    /// resolver caches.
    prompt_cache: RwLock<HashMap<String, String>>,
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
        spec.validate().map_err(AuthError::Misconfigured)?;
        let mut resolvers: HashMap<String, Box<dyn CredentialResolver>> = HashMap::new();
        let mut prompt_fields = Vec::new();
        for binding in &spec.bindings {
            match &binding.provider {
                CredentialProvider::Prompt { .. } => prompt_fields.push(binding.clone()),
                other => {
                    let r = other.build_resolver().map_err(AuthError::Misconfigured)?;
                    resolvers.insert(binding.field.clone(), r);
                }
            }
        }
        let (status_tx, _) = watch::channel(AdapterStatus::Idle);
        Ok(Self {
            spec,
            resolvers,
            prompt_fields,
            prompt_cache: RwLock::new(HashMap::new()),
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
        self.prompt_cache.write().await.clear();
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
        self.prompt_cache.write().await.clear();
    }

    // --- internals ------------------------------------------------------

    fn is_session_valid(&self, entry: &SessionEntry) -> bool {
        match self.spec.session_cache {
            SessionCachePolicy::None => false,
            SessionCachePolicy::Ttl { ttl_secs }
            | SessionCachePolicy::TtlOrClose { ttl_secs } => {
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
            if matches!(binding.provider, CredentialProvider::Prompt { .. }) {
                continue;
            }
            let r = self
                .resolvers
                .get(&binding.field)
                .expect("non-prompt resolver registered");
            let v = r.resolve().await?;
            values.insert(binding.field.clone(), v);
        }

        if !self.prompt_fields.is_empty() {
            let cached = self.prompt_cache.read().await;
            let all_cached = self
                .prompt_fields
                .iter()
                .all(|b| cached.contains_key(&b.field));
            if all_cached {
                for b in &self.prompt_fields {
                    values.insert(b.field.clone(), cached[&b.field].clone());
                }
            } else {
                drop(cached);
                let collected = self.prompt_for_credentials().await?;
                let mut cache = self.prompt_cache.write().await;
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

    async fn prompt_for_credentials(&self) -> Result<HashMap<String, String>, AuthError> {
        let (tx, rx) = oneshot::channel();
        *self.pending_prompt.lock().await = Some(tx);

        let fields: Vec<AuthField> = self
            .prompt_fields
            .iter()
            .map(|b| AuthField {
                name: b.field.clone(),
                label: b.effective_label(),
                masked: b.effective_masked(),
                prefill: match &b.provider {
                    CredentialProvider::Prompt { prefill } => prefill.clone(),
                    _ => None,
                },
            })
            .collect();
        let _ = self.status_tx.send(AdapterStatus::NeedsCreds { fields });

        rx.await.map_err(|_| AuthError::PromptCancelled)
    }
}

// --- Tests ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::session_store::InMemorySessionStore;
    use super::*;
    use crate::AuthMechanism;
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
            mechanism: AuthMechanism::BearerToken,
            session_cache: policy,
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
            mechanism: AuthMechanism::PasswordLogin,
            session_cache: policy,
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

    #[tokio::test]
    async fn invalid_spec_is_rejected_at_construction() {
        let bad = AuthSpec {
            mechanism: AuthMechanism::BearerToken,
            session_cache: SessionCachePolicy::None,
            bindings: vec![], // missing required `token` binding
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
            CredentialProvider::Literal {
                value: "x".into(),
            },
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
            CredentialProvider::Literal {
                value: "x".into(),
            },
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
            CredentialProvider::Literal {
                value: "x".into(),
            },
            SessionCachePolicy::None,
        );
        let store = Arc::new(InMemorySessionStore::new());
        let orch = AuthOrchestrator::from_spec(
            spec,
            Box::new(StoreHandle(store.clone())),
        )
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
            if let AdapterStatus::NeedsCreds { fields } = &*rx.borrow() {
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
            CredentialProvider::Literal {
                value: "x".into(),
            },
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
            CredentialProvider::Literal {
                value: "x".into(),
            },
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
            CredentialProvider::Literal {
                value: "x".into(),
            },
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
            CredentialProvider::Literal {
                value: "x".into(),
            },
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
