//! Resolving one account's credentials — and keeping several accounts from
//! asking at the same time.
//!
//! IMAP mints no session token: the password *is* the credential and stays
//! one, so the orchestrator here does what it does for postgres — resolve
//! the fields, hand them back, cache them — while the live session belongs
//! to the connection actor. `run_login` is therefore pure packing, with no
//! I/O of its own.
//!
//! The part that is specific to a multi-account instance is
//! [`LoginLane`]. `ContentAdapter::submit_credentials` carries the fields
//! and nothing else — no account — so if two accounts published a dialog at
//! once, the answer to the second would be routed to whichever one the
//! adapter guessed. The lane makes that unrepresentable: one account resolves
//! at a time, and while it does, it *is* the addressee. Six accounts
//! connecting at startup therefore ask one after another, each dialog naming
//! its own mailbox.
//!
//! With the usual `pass` setup none of this is visible: the credential
//! script answers without a dialog, and the lane is held for milliseconds.

use std::collections::HashMap;
use std::sync::Arc;

use not_yet_done_content::{AuthOrchestrator, AuthSpec, InMemorySessionStore, StatusReporter};
use tokio::sync::{Mutex, MutexGuard, RwLock};

use crate::error::{MailError, MailResult};

/// The single credential dialog an instance may have open, and who owns it.
#[derive(Default)]
pub(crate) struct LoginLane {
    gate: Mutex<()>,
    holder: RwLock<Option<String>>,
}

/// Held for as long as one account is resolving. Dropping it releases the
/// lane and clears the addressee, so an abandoned login cannot leave later
/// answers pointing at it.
pub(crate) struct LaneGuard<'a> {
    lane: &'a LoginLane,
    _gate: MutexGuard<'a, ()>,
}

impl LoginLane {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Wait for the lane and take it in the name of `account`.
    pub(crate) async fn enter(&self, account: &str) -> LaneGuard<'_> {
        let gate = self.gate.lock().await;
        *self.holder.write().await = Some(account.to_string());
        LaneGuard {
            lane: self,
            _gate: gate,
        }
    }

    /// Which account a `submit_credentials` belongs to, if any is asking.
    pub(crate) async fn holder(&self) -> Option<String> {
        self.holder.read().await.clone()
    }
}

impl Drop for LaneGuard<'_> {
    fn drop(&mut self) {
        // The write lock is uncontended in practice (readers only peek), but
        // Drop cannot await: clear it without blocking, and fall back to a
        // detached clear if a reader happens to hold it this instant.
        if let Ok(mut h) = self.lane.holder.try_write() {
            *h = None;
        }
    }
}

/// One account's credentials.
pub(crate) struct AccountCredentials {
    /// The account id — what the lane records as the addressee.
    id: String,
    orchestrator: Arc<AuthOrchestrator>,
    lane: Arc<LoginLane>,
}

impl AccountCredentials {
    pub(crate) fn new(
        id: impl Into<String>,
        spec: AuthSpec,
        status: StatusReporter,
        lane: Arc<LoginLane>,
    ) -> Result<Arc<Self>, String> {
        let orchestrator = AuthOrchestrator::from_spec_with_status(
            spec,
            Box::new(InMemorySessionStore::new()),
            status,
        )
        .map_err(|e| format!("auth orchestrator: {e}"))?;
        Ok(Arc::new(Self {
            id: id.into(),
            orchestrator: Arc::new(orchestrator),
            lane,
        }))
    }

    /// Every field of the account's `auth:` block, resolved. Cached, so the
    /// credential script runs once per process and not once per reconnect.
    ///
    /// Holds the instance-wide login lane for the whole resolution: while
    /// this account may be asking, no other one may.
    pub(crate) async fn fields(&self) -> MailResult<HashMap<String, String>> {
        let _lane = self.lane.enter(&self.id).await;
        let resolved = self
            .orchestrator
            .ensure_session(|creds| async move {
                serde_json::to_string(&creds).map_err(|e| format!("encode credentials: {e}"))
            })
            .await
            .map_err(|e| match e {
                not_yet_done_content::AuthError::PromptCancelled => MailError::Cancelled,
                other => MailError::Auth(other.to_string()),
            })?;
        serde_json::from_str(&resolved.blob)
            .map_err(|e| MailError::Auth(format!("decode credentials: {e}")))
    }

    pub(crate) async fn submit(&self, fields: HashMap<String, String>) -> Result<(), String> {
        self.orchestrator
            .submit_credentials(fields)
            .await
            .map_err(|e| e.to_string())
    }

    pub(crate) async fn cancel(&self) -> Result<(), String> {
        self.orchestrator
            .cancel_prompt()
            .await
            .map_err(|e| e.to_string())
    }

    /// Forget what was resolved; the next login resolves again, and asks if
    /// that is where the values come from. Called when the server rejected
    /// the credentials — replaying a wrong password until the account locks
    /// is the failure mode this prevents.
    pub(crate) async fn invalidate(&self) {
        self.orchestrator.invalidate_credentials().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::{CredentialBinding, CredentialProvider, SessionCachePolicy};
    use std::time::Duration;

    fn literal_spec() -> AuthSpec {
        AuthSpec {
            mechanism: "password".into(),
            session_cache: SessionCachePolicy::default(),
            script: None,
            script_timeout_secs: 120,
            plugins: Vec::new(),
            bindings: vec![
                CredentialBinding {
                    field: "username".into(),
                    provider: CredentialProvider::Literal { value: "u".into() },
                    label: None,
                    masked: None,
                },
                CredentialBinding {
                    field: "password".into(),
                    provider: CredentialProvider::Literal { value: "p".into() },
                    label: None,
                    masked: None,
                },
            ],
        }
    }

    #[tokio::test]
    async fn resolved_fields_come_back_by_name() {
        let creds = AccountCredentials::new(
            "work",
            literal_spec(),
            StatusReporter::new(),
            LoginLane::new(),
        )
        .expect("spec is valid");
        let fields = creds.fields().await.expect("literals resolve");
        assert_eq!(fields.get("username").map(String::as_str), Some("u"));
        assert_eq!(fields.get("password").map(String::as_str), Some("p"));
    }

    /// The invariant the lane exists for: while one account is resolving, it
    /// is the only possible addressee of a `submit_credentials`.
    #[tokio::test]
    async fn only_one_account_may_be_asking_at_a_time() {
        let lane = LoginLane::new();
        assert_eq!(lane.holder().await, None);

        let held = lane.enter("work").await;
        assert_eq!(lane.holder().await, Some("work".to_string()));

        let second = Arc::clone(&lane);
        let waiting = tokio::spawn(async move {
            let g = second.enter("private").await;
            let who = second.holder().await;
            drop(g);
            who
        });
        // The second account must not become the addressee while the first
        // holds the lane.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiting.is_finished(), "second login waits its turn");
        assert_eq!(lane.holder().await, Some("work".to_string()));

        drop(held);
        assert_eq!(waiting.await.expect("joins"), Some("private".to_string()));
        assert_eq!(lane.holder().await, None, "the lane is free again");
    }
}
