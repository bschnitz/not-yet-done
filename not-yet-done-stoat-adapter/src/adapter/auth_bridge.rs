//! Bridges the generic [`AuthOrchestrator`] to a live [`StoatClient`].
//!
//! The orchestrator owns credential resolution + the session-cache
//! lifecycle; this bridge translates its session blob (adapter-side
//! JSON holding the `X-Session-Token` plus user identity) into a usable
//! `StoatClient` and caches it for the session. On the slow path it
//! routes the orchestrator's login fn through [`crate::client::perform_login`]
//! and validates restored sessions against `/users/@me`, so a stale
//! token triggers a transparent re-login instead of a 401 bubbling up.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Notify, RwLock, watch};

use not_yet_done_content::{AdapterStatus, AuthOrchestrator, AuthSpec, SessionStore};

use crate::client::{StoatClient, StoatSession, perform_login};

pub(super) struct AuthBridge {
    base_url: String,
    orchestrator: Arc<AuthOrchestrator>,
    client: RwLock<Option<Arc<StoatClient>>>,
    ready: Notify,
}

impl AuthBridge {
    pub(super) fn new(
        base_url: String,
        store: Box<dyn SessionStore>,
        spec: AuthSpec,
    ) -> Result<Arc<Self>, String> {
        let orchestrator =
            AuthOrchestrator::from_spec(spec, store).map_err(|e| format!("auth orchestrator: {e}"))?;
        Ok(Arc::new(Self {
            base_url,
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
    pub(super) async fn get_client(self: &Arc<Self>) -> Result<Arc<StoatClient>, String> {
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
        // The unified `PasswordLogin` mechanism fixes the binding field
        // names to `username` + `password`. Stoat logs in by email, so
        // the `username` field carries the email address (see config docs).
        let email = creds.get("username").map(String::as_str).unwrap_or("").trim();
        let password = creds.get("password").map(String::as_str).unwrap_or("");
        if email.is_empty() || password.is_empty() {
            return Err("email (username field) and password are required".into());
        }
        let session = perform_login(&self.base_url, email, password).await?;
        serde_json::to_string(&session).map_err(|e| format!("serialize session: {e}"))
    }

    async fn build_and_validate(&self, blob: &str) -> Result<Arc<StoatClient>, String> {
        let session: StoatSession =
            serde_json::from_str(blob).map_err(|e| format!("parse session blob: {e}"))?;
        let client = StoatClient::from_session(&self.base_url, session)?;
        client.me().await?;
        Ok(client)
    }

    async fn fill(&self, client: Arc<StoatClient>) -> Result<Arc<StoatClient>, String> {
        *self.client.write().await = Some(Arc::clone(&client));
        self.ready.notify_waiters();
        Ok(client)
    }
}
