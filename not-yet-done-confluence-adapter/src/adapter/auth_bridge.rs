//! Bridges the generic [`AuthOrchestrator`] to a live [`ConfluenceClient`].
//!
//! Mirrors the Jira adapter's bridge — the orchestrator owns the
//! credential-resolution state machine (literal / shell command / env /
//! interactive prompt) and we just translate the resulting session blob
//! into a usable HTTP client.
//!
//! Restored sessions are validated against `/rest/api/user/current` so a
//! stale cached cookie triggers a transparent re-authentication instead
//! of bubbling up a 401 to the first list call. Confluence Server only
//! supports the `cookie` mechanism today (Crowd SSO + `JSESSIONID`);
//! other mechanisms in the spec are surfaced as configuration errors.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Notify, RwLock, watch};

use not_yet_done_content::{AdapterStatus, AuthMechanism, AuthOrchestrator, AuthSpec};

use crate::client::{ConfluenceClient, ConfluenceSession};

pub(super) struct AuthBridge {
    base_url: String,
    accept_invalid_certs: bool,
    orchestrator: Arc<AuthOrchestrator>,
    client: RwLock<Option<Arc<ConfluenceClient>>>,
    ready: Notify,
}

impl AuthBridge {
    pub(super) fn new(
        base_url: String,
        accept_invalid_certs: bool,
        spec: AuthSpec,
        session_store: Box<dyn not_yet_done_content::SessionStore>,
    ) -> Result<Arc<Self>, String> {
        let orchestrator = AuthOrchestrator::from_spec(spec, session_store)
            .map_err(|e| format!("auth orchestrator: {e}"))?;
        Ok(Arc::new(Self {
            base_url,
            accept_invalid_certs,
            orchestrator: Arc::new(orchestrator),
            client: RwLock::new(None),
            ready: Notify::new(),
        }))
    }

    pub(super) fn subscribe_status(&self) -> watch::Receiver<AdapterStatus> {
        self.orchestrator.subscribe_status()
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

    /// Return a live client. Fast path on cache hit; slow path drives
    /// the orchestrator and validates restored sessions, retrying with
    /// `re_authenticate` if the cached blob no longer works.
    pub(super) async fn get_client(self: &Arc<Self>) -> Result<Arc<ConfluenceClient>, String> {
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
    /// for `cookie` the credential *is* the session.
    async fn run_login(&self, creds: HashMap<String, String>) -> Result<String, String> {
        let session = match self.orchestrator.spec().mechanism {
            AuthMechanism::Cookie => {
                let cookie = creds
                    .get("cookie")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "cookie credential is empty".to_string())?;
                ConfluenceSession {
                    cookie: Some(cookie),
                }
            }
            other => {
                return Err(format!(
                    "Confluence adapter does not support mechanism {other:?}"
                ));
            }
        };
        serde_json::to_string(&session).map_err(|e| format!("serialize session: {e}"))
    }

    async fn build_and_validate(&self, blob: &str) -> Result<Arc<ConfluenceClient>, String> {
        let session: ConfluenceSession =
            serde_json::from_str(blob).map_err(|e| format!("parse session blob: {e}"))?;
        let client = Arc::new(ConfluenceClient::from_session(
            &self.base_url,
            session,
            self.accept_invalid_certs,
        )?);
        client.current_user().await?;
        Ok(client)
    }

    async fn fill(&self, client: Arc<ConfluenceClient>) -> Result<Arc<ConfluenceClient>, String> {
        *self.client.write().await = Some(Arc::clone(&client));
        self.ready.notify_waiters();
        Ok(client)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::{
        AuthMechanism, AuthSpec, CredentialBinding, CredentialProvider, InMemorySessionStore,
        SessionCachePolicy,
    };

    fn cookie_spec_literal(value: &str) -> AuthSpec {
        AuthSpec {
            mechanism: AuthMechanism::Cookie,
            bindings: vec![CredentialBinding {
                field: "cookie".to_string(),
                provider: CredentialProvider::Literal {
                    value: value.to_string(),
                },
                label: None,
                masked: None,
            }],
            session_cache: SessionCachePolicy::UntilRejected,
        }
    }

    #[tokio::test]
    async fn run_login_packs_cookie_into_session_blob() {
        let bridge = AuthBridge::new(
            "https://wiki.example.invalid".to_string(),
            false,
            cookie_spec_literal("JSESSIONID=synthetic; crowd.token_key=abc"),
            Box::new(InMemorySessionStore::new()),
        )
        .expect("bridge");

        let mut creds = HashMap::new();
        creds.insert(
            "cookie".to_string(),
            "JSESSIONID=synthetic; crowd.token_key=abc".to_string(),
        );
        let blob = bridge.run_login(creds).await.expect("blob");

        let session: ConfluenceSession =
            serde_json::from_str(&blob).expect("blob parses as ConfluenceSession");
        assert_eq!(
            session.cookie.as_deref(),
            Some("JSESSIONID=synthetic; crowd.token_key=abc")
        );
    }

    #[tokio::test]
    async fn run_login_rejects_empty_cookie() {
        let bridge = AuthBridge::new(
            "https://wiki.example.invalid".to_string(),
            false,
            cookie_spec_literal("placeholder"),
            Box::new(InMemorySessionStore::new()),
        )
        .expect("bridge");

        let mut creds = HashMap::new();
        creds.insert("cookie".to_string(), "   ".to_string());
        let err = bridge.run_login(creds).await.expect_err("empty rejected");
        assert!(err.contains("cookie"), "error mentions cookie: {err}");
    }

    #[tokio::test]
    async fn run_login_rejects_unsupported_mechanism() {
        let spec = AuthSpec {
            mechanism: AuthMechanism::BasicAuth,
            bindings: vec![
                CredentialBinding {
                    field: "username".to_string(),
                    provider: CredentialProvider::Literal {
                        value: "u".to_string(),
                    },
                    label: None,
                    masked: None,
                },
                CredentialBinding {
                    field: "token".to_string(),
                    provider: CredentialProvider::Literal {
                        value: "t".to_string(),
                    },
                    label: None,
                    masked: None,
                },
            ],
            session_cache: SessionCachePolicy::UntilRejected,
        };
        let bridge = AuthBridge::new(
            "https://wiki.example.invalid".to_string(),
            false,
            spec,
            Box::new(InMemorySessionStore::new()),
        )
        .expect("bridge");

        let mut creds = HashMap::new();
        creds.insert("username".to_string(), "u".to_string());
        creds.insert("token".to_string(), "t".to_string());
        let err = bridge
            .run_login(creds)
            .await
            .expect_err("unsupported mechanism rejected");
        assert!(
            err.contains("does not support"),
            "error explains mechanism rejection: {err}"
        );
    }

    #[tokio::test]
    async fn invalidate_session_clears_cached_client() {
        let bridge = AuthBridge::new(
            "https://wiki.example.invalid".to_string(),
            false,
            cookie_spec_literal("JSESSIONID=x"),
            Box::new(InMemorySessionStore::new()),
        )
        .expect("bridge");

        let synthetic = Arc::new(
            ConfluenceClient::new("https://wiki.example.invalid", "JSESSIONID=x", false)
                .expect("client"),
        );
        *bridge.client.write().await = Some(synthetic);
        assert!(bridge.client.read().await.is_some());

        bridge.invalidate_session().await;
        assert!(bridge.client.read().await.is_none());
    }
}
