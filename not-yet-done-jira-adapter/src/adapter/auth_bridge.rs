//! Bridges the generic [`AuthOrchestrator`] to a live [`JiraClient`].
//!
//! Translates the orchestrator's session blob (adapter-side JSON
//! containing either a `cookie` value or an `email` + `token` pair,
//! depending on the configured mechanism) into a usable `JiraClient`
//! and caches it for the duration of the session.
//!
//! On the slow path it routes the orchestrator's login fn through the
//! configured providers (literal cookie / shell command / static token /
//! …) and validates restored sessions against `/myself` so a stale
//! cached cookie triggers a transparent re-authentication instead of
//! bubbling up a 401 to the first list call.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Notify, RwLock, watch};

use not_yet_done_content::{
    AdapterStatus, AuthFieldSpec, AuthOrchestrator, AuthSpec, MechanismSpec, StatusReporter,
};

use crate::client::{JiraClient, JiraSession};

/// What this adapter can speak against a Jira Server / Data-Center
/// instance. The factory publishes this table and validates the config
/// against it; [`AuthBridge::run_login`] below implements it. The three
/// belong together — a new mechanism is an entry here plus an arm there.
pub(crate) const MECHANISMS: &[MechanismSpec] = &[
    MechanismSpec {
        id: "cookie",
        label: "Session cookie",
        doc: "Send a ready-made Cookie header — what an SSO login (Crowd, SAML) leaves behind. \
              Fetch it with a script; the adapter never talks to a browser itself.",
        fields: &[AuthFieldSpec::required("cookie", "Cookie header", true)],
    },
    MechanismSpec {
        id: "basic-auth",
        label: "Username and API token",
        doc: "HTTP Basic with a username (or e-mail) and an API token.",
        fields: &[
            AuthFieldSpec::required("username", "Username or e-mail", false),
            AuthFieldSpec::required("token", "API token", true),
        ],
    },
];

pub(super) struct AuthBridge {
    base_url: String,
    accept_invalid_certs: bool,
    /// Config `story_points_field`, handed to every client this bridge
    /// builds so the column skips discovery on an instance where the
    /// lookup would guess wrong.
    story_points_field: Option<String>,
    /// Config `retry:`, handed to every client this bridge builds.
    retry: not_yet_done_content::RetryConfig,
    orchestrator: Arc<AuthOrchestrator>,
    /// The one status channel of this connection: the orchestrator's login
    /// steps and the adapter's own request announcements both go here, so a
    /// frontend follows a single line from "running the cookie script" to
    /// "Loading issues" without stitching two streams together.
    status: StatusReporter,
    client: RwLock<Option<Arc<JiraClient>>>,
    ready: Notify,
}

impl AuthBridge {
    pub(super) fn new(
        base_url: String,
        accept_invalid_certs: bool,
        spec: AuthSpec,
        story_points_field: Option<String>,
        retry: not_yet_done_content::RetryConfig,
        session_store: Box<dyn not_yet_done_content::SessionStore>,
    ) -> Result<Arc<Self>, String> {
        let status = StatusReporter::new();
        let orchestrator =
            AuthOrchestrator::from_spec_with_status(spec, session_store, status.clone())
                .map_err(|e| format!("auth orchestrator: {e}"))?;
        Ok(Arc::new(Self {
            base_url,
            accept_invalid_certs,
            story_points_field,
            retry,
            orchestrator: Arc::new(orchestrator),
            status,
            client: RwLock::new(None),
            ready: Notify::new(),
        }))
    }

    pub(super) fn subscribe_status(&self) -> watch::Receiver<AdapterStatus> {
        self.status.subscribe()
    }

    /// The connection's status channel, for the adapter's own reports about
    /// the requests it sends once the login is behind it.
    pub(super) fn status(&self) -> StatusReporter {
        self.status.clone()
    }

    #[allow(dead_code)]
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

    /// The cached client, unless the server has rejected its session in the
    /// meantime — a rejected client is dropped here, which sends the caller
    /// down the slow path where the stale session fails validation and
    /// `re_authenticate` fetches a fresh one.
    async fn live_client(&self) -> Option<Arc<JiraClient>> {
        let cached = self.client.read().await.clone()?;
        if !cached.auth_rejected() {
            return Some(cached);
        }
        let mut slot = self.client.write().await;
        if slot.as_ref().is_some_and(|c| c.auth_rejected()) {
            *slot = None;
        }
        None
    }

    /// Return a live client. Fast path on cache hit; slow path drives
    /// the orchestrator and validates restored sessions, retrying with
    /// `re_authenticate` if the cached blob no longer works.
    ///
    /// The slow path is also where this connection's `Connecting` line
    /// ends — on either outcome. Nothing below reports a terminal status:
    /// the orchestrator is done once it has a session blob, while the
    /// connection is only usable after the validation round trip here.
    pub(super) async fn get_client(self: &Arc<Self>) -> Result<Arc<JiraClient>, String> {
        if let Some(c) = self.live_client().await {
            return Ok(c);
        }
        match self.connect().await {
            Ok(client) => {
                self.status.connected();
                Ok(client)
            }
            Err(e) => {
                self.status.failed(e.clone());
                Err(e)
            }
        }
    }

    /// Drive the orchestrator until a validated client exists.
    async fn connect(self: &Arc<Self>) -> Result<Arc<JiraClient>, String> {
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
    /// for `cookie`/`basic-auth` the credential *is* the session.
    async fn run_login(&self, creds: HashMap<String, String>) -> Result<String, String> {
        let session = match self.orchestrator.spec().mechanism.as_str() {
            "cookie" => {
                let cookie = creds
                    .get("cookie")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "cookie credential is empty".to_string())?;
                JiraSession {
                    cookie: Some(cookie),
                    ..JiraSession::default()
                }
            }
            "basic-auth" => {
                let email = creds
                    .get("username")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "username/email is empty".to_string())?;
                let token = creds
                    .get("token")
                    .cloned()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "token is empty".to_string())?;
                JiraSession {
                    email: Some(email),
                    token: Some(token),
                    ..JiraSession::default()
                }
            }
            // Unreachable via config: the factory validated the id
            // against MECHANISMS. Kept as a defensive assertion for a
            // spec built in code.
            other => {
                return Err(format!("Jira adapter does not support mechanism `{other}`"));
            }
        };
        serde_json::to_string(&session).map_err(|e| format!("serialize session: {e}"))
    }

    async fn build_and_validate(&self, blob: &str) -> Result<Arc<JiraClient>, String> {
        let session: JiraSession =
            serde_json::from_str(blob).map_err(|e| format!("parse session blob: {e}"))?;
        let client = Arc::new(
            JiraClient::from_session(&self.base_url, session, self.accept_invalid_certs)?
                .with_story_points_field(self.story_points_field.as_deref())
                .with_retry(self.retry.clone()),
        );
        // A restored cookie is only as good as Jira says it is, and asking
        // costs a round trip the user is otherwise left guessing about.
        self.status.connect_step("checking the session");
        client.current_user().await?;
        Ok(client)
    }

    async fn fill(&self, client: Arc<JiraClient>) -> Result<Arc<JiraClient>, String> {
        *self.client.write().await = Some(Arc::clone(&client));
        self.ready.notify_waiters();
        Ok(client)
    }
}
