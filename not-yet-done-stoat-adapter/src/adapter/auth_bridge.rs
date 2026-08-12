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

use not_yet_done_content::{
    AdapterStatus, AuthFieldSpec, AuthOrchestrator, AuthSpec, MechanismSpec, SessionStore,
};

use crate::client::{StoatClient, StoatSession, perform_login};

/// What this adapter can speak against a Stoat server. The factory
/// publishes this table and validates the config against it;
/// [`AuthBridge::run_login`] below implements it. The two belong
/// together — a new mechanism is an entry here plus a branch there.
pub(crate) const MECHANISMS: &[MechanismSpec] = &[MechanismSpec {
    id: "password-login",
    label: "E-mail and password",
    doc: "Log in with the account's e-mail address and password; the server hands back a \
          session token the adapter caches. Accounts with MFA are not supported yet.",
    fields: &[
        AuthFieldSpec::required("username", "E-mail address", false),
        AuthFieldSpec::required("password", "Password", true),
    ],
}];

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
        let orchestrator = AuthOrchestrator::from_spec(spec, store)
            .map_err(|e| format!("auth orchestrator: {e}"))?;
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
        // No match on the mechanism: `password-login` is the only entry
        // in MECHANISMS, and the factory validated the config against
        // it. Stoat logs in by e-mail, so the `username` field carries
        // the e-mail address (see the field label in MECHANISMS).
        let email = creds
            .get("username")
            .map(String::as_str)
            .unwrap_or("")
            .trim();
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

    /// Build a bridge with a pre-filled client for in-crate tests. Uses a
    /// minimal password-login spec + volatile session store; `get_client`
    /// never touches the network because the client cache is primed.
    #[cfg(test)]
    pub(super) fn for_test(base_url: impl Into<String>, client: Arc<StoatClient>) -> Arc<Self> {
        use not_yet_done_content::{
            AuthSpec, CredentialBinding, CredentialProvider, InMemorySessionStore,
        };
        let spec = AuthSpec {
            mechanism: "password-login".into(),
            session_cache: Default::default(),
            script: None,
            script_timeout_secs: 120,
            bindings: vec![
                CredentialBinding {
                    field: "username".into(),
                    provider: CredentialProvider::Prompt { prefill: None },
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
        };
        let orchestrator = AuthOrchestrator::from_spec(spec, Box::new(InMemorySessionStore::new()))
            .expect("test auth spec is valid");
        Arc::new(Self {
            base_url: base_url.into(),
            orchestrator: Arc::new(orchestrator),
            client: RwLock::new(Some(client)),
            ready: Notify::new(),
        })
    }
}
