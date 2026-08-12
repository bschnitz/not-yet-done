//! Bridges the generic [`AuthOrchestrator`] to a live [`TaigaClient`].
//!
//! The orchestrator owns the credential resolution + session-cache
//! lifecycle; this bridge translates the orchestrator's session blob
//! (adapter-side JSON containing the JWT pair plus user identity) into
//! a usable `TaigaClient` and caches it for the duration of the session.
//!
//! On the slow path it routes the orchestrator's login fn through
//! [`crate::client::perform_login`] and validates restored sessions
//! against `/users/me` so a stale cached JWT triggers a transparent
//! re-authentication instead of bubbling up a 401 to the first list call.

use std::collections::HashMap;
use std::sync::Arc;

use sea_orm::DatabaseConnection;
use tokio::sync::{Notify, RwLock, watch};
use uuid::Uuid;

use not_yet_done_content::{
    AdapterStatus, AuthFieldSpec, AuthOrchestrator, AuthSpec, MechanismSpec,
};

use crate::auth_session_store::SqlAuthSessionStore;
use crate::client::{TaigaClient, TaigaSession, perform_login};

/// What this adapter can speak against a Taiga instance. The factory
/// publishes this table and validates the config against it;
/// [`AuthBridge::run_login`] below implements it. The two belong
/// together — a new mechanism is an entry here plus a branch there.
pub(crate) const MECHANISMS: &[MechanismSpec] = &[MechanismSpec {
    id: "password-login",
    label: "Username and password",
    doc: "Log in with the Taiga account's username and password; the server hands back a JWT \
          pair the adapter caches and refreshes.",
    fields: &[
        AuthFieldSpec::required("username", "Username", false),
        AuthFieldSpec::required("password", "Password", true),
    ],
}];

pub(super) struct AuthBridge {
    base_url: String,
    db: Arc<DatabaseConnection>,
    scope_id: Uuid,
    /// HTTP timeout budget baked into every client this bridge builds
    /// (login round-trip and session-restored clients alike).
    timeouts: crate::client::HttpTimeouts,
    orchestrator: Arc<AuthOrchestrator>,
    client: RwLock<Option<Arc<TaigaClient>>>,
    ready: Notify,
}

impl AuthBridge {
    pub(super) fn new(
        base_url: String,
        db: Arc<DatabaseConnection>,
        scope_id: Uuid,
        spec: AuthSpec,
        timeouts: crate::client::HttpTimeouts,
    ) -> Result<Arc<Self>, String> {
        let store = SqlAuthSessionStore::new(Arc::clone(&db), scope_id);
        let orchestrator = AuthOrchestrator::from_spec(spec, Box::new(store))
            .map_err(|e| format!("auth orchestrator: {e}"))?;
        Ok(Arc::new(Self {
            base_url,
            db,
            scope_id,
            timeouts,
            orchestrator: Arc::new(orchestrator),
            client: RwLock::new(None),
            ready: Notify::new(),
        }))
    }

    pub(super) fn subscribe_status(&self) -> watch::Receiver<AdapterStatus> {
        self.orchestrator.subscribe_status()
    }

    pub(super) async fn submit_credentials(
        &self,
        fields: HashMap<String, String>,
    ) -> Result<(), String> {
        self.orchestrator
            .submit_credentials(fields)
            .await
            .map_err(|e| e.to_string())
    }

    pub(super) async fn cancel_credentials(&self) -> Result<(), String> {
        self.orchestrator
            .cancel_prompt()
            .await
            .map_err(|e| e.to_string())
    }

    pub(super) async fn invalidate_session(&self) {
        *self.client.write().await = None;
        self.orchestrator.invalidate_session().await;
    }

    pub(super) async fn invalidate_credentials(&self) {
        *self.client.write().await = None;
        self.orchestrator.invalidate_credentials().await;
    }

    /// Return a live client. Fast path on cache hit; slow path drives
    /// the orchestrator and validates restored sessions, retrying with
    /// `re_authenticate` if the cached blob no longer works.
    pub(super) async fn get_client(self: &Arc<Self>) -> Result<Arc<TaigaClient>, String> {
        if let Some(c) = self.client.read().await.clone() {
            return Ok(c);
        }

        let me = Arc::clone(self);
        let resolved = self
            .orchestrator
            .ensure_session(move |creds| {
                let me = Arc::clone(&me);
                async move { me.run_login(creds).await }
            })
            .await
            .map_err(|e| e.to_string())?;

        match self.build_and_validate(&resolved.blob).await {
            Ok(client) => self.fill(client).await,
            Err(_) if resolved.from_cache => {
                let me = Arc::clone(self);
                let fresh = self
                    .orchestrator
                    .re_authenticate(move |creds| {
                        let me = Arc::clone(&me);
                        async move { me.run_login(creds).await }
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                let client = self.build_and_validate(&fresh.blob).await?;
                self.fill(client).await
            }
            Err(e) => Err(e),
        }
    }

    async fn run_login(&self, creds: HashMap<String, String>) -> Result<String, String> {
        // No match on the mechanism: `password-login` is the only entry
        // in MECHANISMS, and the factory validated the config against it.
        let username = creds
            .get("username")
            .map(String::as_str)
            .unwrap_or("")
            .trim();
        let password = creds.get("password").map(String::as_str).unwrap_or("");
        if username.is_empty() || password.is_empty() {
            return Err("username and password are required".into());
        }
        let session = perform_login(&self.base_url, username, password, self.timeouts).await?;
        serde_json::to_string(&session).map_err(|e| format!("serialize session: {e}"))
    }

    async fn build_and_validate(&self, blob: &str) -> Result<Arc<TaigaClient>, String> {
        let session: TaigaSession =
            serde_json::from_str(blob).map_err(|e| format!("parse session blob: {e}"))?;
        let client = TaigaClient::from_session(
            &self.base_url,
            session,
            Arc::clone(&self.db),
            self.scope_id,
            self.timeouts,
        )?;
        client.myself().await?;
        Ok(client)
    }

    async fn fill(&self, client: Arc<TaigaClient>) -> Result<Arc<TaigaClient>, String> {
        *self.client.write().await = Some(Arc::clone(&client));
        self.ready.notify_waiters();
        Ok(client)
    }

    /// Run `op` against a live client. If it fails with what looks like
    /// an expired/invalid Taiga token, invalidate the cached session and
    /// retry once with a fresh client.
    ///
    /// `op` must be `Fn` because we may call it twice; it returns a fresh
    /// future each invocation. Captures should be cheap to clone — the
    /// retry is rare, but the closure pays the clone cost on the happy
    /// path too.
    pub(super) async fn with_client<T, F, Fut>(self: &Arc<Self>, op: F) -> Result<T, String>
    where
        F: Fn(Arc<TaigaClient>) -> Fut,
        Fut: std::future::Future<Output = Result<T, String>>,
    {
        let client = self.get_client().await?;
        match op(client).await {
            Ok(v) => return Ok(v),
            Err(e) if looks_like_token_failure(&e) => {}
            Err(e) => return Err(e),
        }
        self.invalidate_session().await;
        let fresh = self.get_client().await?;
        op(fresh).await
    }
}

/// Heuristic: does this error string indicate that the cached JWT was
/// rejected by Taiga? Taiga's DRF-SimpleJWT layer returns a JSON body
/// with `"code": "token_not_valid"` on 401, and the client wraps the
/// upstream status in the message. Matching on the literal string is
/// pragmatic — typed errors would need a wider refactor across every
/// HTTP call site.
fn looks_like_token_failure(err: &str) -> bool {
    err.contains("401") || err.contains("token_not_valid") || err.contains("Unauthorized")
}
