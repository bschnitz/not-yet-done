//! Bridges the generic [`AuthOrchestrator`] to a live [`KimaiClient`].
//!
//! Kimai auth is static — the resolved credentials (username + API
//! password, or a bearer token) *are* the session, so `run_login` just
//! packs them into a JSON blob without any HTTP. Restored blobs are
//! validated against `/api/version` so a stale token triggers a
//! transparent re-authentication instead of bubbling a 401 up to the
//! first list call.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{RwLock, watch};

use not_yet_done_content::{
    AdapterStatus, AuthFieldSpec, AuthOrchestrator, AuthSpec, MechanismSpec,
};

use crate::client::{HttpTimeouts, KimaiClient, KimaiSession};

/// What this adapter can speak against a Kimai instance. The factory
/// publishes this table and validates the config against it;
/// [`AuthBridge::run_login`] below implements it. The three belong
/// together — a new mechanism is an entry here plus an arm there.
pub(crate) const MECHANISMS: &[MechanismSpec] = &[
    MechanismSpec {
        id: "user-api-token",
        label: "Username and API password",
        doc: "Kimai's classic API user: the username plus the separate API password from the \
              user's API settings. Sent as X-AUTH-USER / X-AUTH-TOKEN.",
        fields: &[
            AuthFieldSpec::required("username", "Username", false),
            AuthFieldSpec::required("token", "API password", true),
        ],
    },
    MechanismSpec {
        id: "bearer-token",
        label: "API token",
        doc: "A personal API token from Kimai 2.14 and newer, sent as an Authorization: Bearer \
              header. No username needed.",
        fields: &[AuthFieldSpec::required("token", "API token", true)],
    },
];

pub(super) struct AuthBridge {
    base_url: String,
    timeouts: HttpTimeouts,
    orchestrator: Arc<AuthOrchestrator>,
    client: RwLock<Option<Arc<KimaiClient>>>,
}

impl AuthBridge {
    pub(super) fn new(
        base_url: String,
        spec: AuthSpec,
        session_store: Box<dyn not_yet_done_content::SessionStore>,
        timeouts: HttpTimeouts,
    ) -> Result<Arc<Self>, String> {
        let orchestrator = AuthOrchestrator::from_spec(spec, session_store)
            .map_err(|e| format!("auth orchestrator: {e}"))?;
        Ok(Arc::new(Self {
            base_url,
            timeouts,
            orchestrator: Arc::new(orchestrator),
            client: RwLock::new(None),
        }))
    }

    pub(super) fn subscribe_status(&self) -> watch::Receiver<AdapterStatus> {
        self.orchestrator.subscribe_status()
    }

    /// Hand a frontend's answers back to the login that is waiting for
    /// them — a `prompt` binding, or a form the auth block's `script`
    /// asked for.
    pub(super) async fn submit_credentials(
        &self,
        fields: HashMap<String, String>,
    ) -> Result<(), String> {
        self.orchestrator
            .submit_credentials(fields)
            .await
            .map_err(|e| e.to_string())
    }

    /// The user closed the dialog. Dropping the pending sender fails the
    /// waiting login instead of leaving it holding the auth mutex.
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
    pub(super) async fn get_client(self: &Arc<Self>) -> Result<Arc<KimaiClient>, String> {
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

    /// Pack the resolved credentials into a JSON session blob. No HTTP —
    /// for both supported mechanisms the credential *is* the session.
    async fn run_login(&self, creds: HashMap<String, String>) -> Result<String, String> {
        let session = match self.orchestrator.spec().mechanism.as_str() {
            "user-api-token" => {
                let username = creds
                    .get("username")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "username is empty".to_string())?;
                let token = creds
                    .get("token")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "token is empty".to_string())?;
                KimaiSession {
                    username: Some(username),
                    token,
                }
            }
            "bearer-token" => {
                let token = creds
                    .get("token")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "token is empty".to_string())?;
                KimaiSession {
                    username: None,
                    token,
                }
            }
            // Unreachable via config: the factory validated the id
            // against MECHANISMS. Kept as a defensive assertion for a
            // spec built in code.
            other => {
                return Err(format!(
                    "Kimai adapter does not support mechanism `{other}`"
                ));
            }
        };
        serde_json::to_string(&session).map_err(|e| format!("serialize session: {e}"))
    }

    async fn build_and_validate(&self, blob: &str) -> Result<Arc<KimaiClient>, String> {
        let session: KimaiSession =
            serde_json::from_str(blob).map_err(|e| format!("parse session blob: {e}"))?;
        let client = Arc::new(KimaiClient::from_session(
            &self.base_url,
            &session,
            self.timeouts,
        )?);
        client.version().await?;
        Ok(client)
    }

    async fn fill(&self, client: Arc<KimaiClient>) -> Result<Arc<KimaiClient>, String> {
        *self.client.write().await = Some(Arc::clone(&client));
        Ok(client)
    }
}
