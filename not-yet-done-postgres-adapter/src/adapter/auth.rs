//! Credentials that come from the adapter's `auth:` block rather than
//! from a provider sitting directly in the transport / postgres section.
//!
//! Why this exists: a `command` provider running `pass` resolves in a
//! child process, so a locked GPG store opens *its own* pinentry window
//! — outside the TUI, once per secret. Routed through the
//! [`AuthOrchestrator`] instead, one credential script supplies every
//! field in a single invocation and asks the frontend for the store
//! passphrase through the normal
//! [`NeedsCreds`](AdapterStatus::NeedsCreds) contract, i.e. the same
//! dialog every other adapter uses.
//!
//! Postgres derives no session token from its credentials — the
//! password *is* the credential, forever. `run_login` therefore does no
//! I/O at all; it only packs the resolved fields into the session blob
//! the orchestrator wants to hand back. The store is in-memory, so the
//! secrets never touch the disk.

use std::collections::HashMap;
use std::sync::Arc;

use not_yet_done_content::{
    AdapterStatus, AuthFieldSpec, AuthOrchestrator, AuthSpec, InMemorySessionStore, MechanismSpec,
};
use tokio::sync::watch;

/// The mechanism field filling `postgres.password`.
pub(crate) const FIELD_PASSWORD: &str = "password";
/// The mechanism field filling an SSH hop's `kind: password`.
pub(crate) const FIELD_SSH_PASSWORD: &str = "ssh_password";
/// The mechanism field filling an encrypted SSH key's passphrase.
pub(crate) const FIELD_SSH_KEY_PASSPHRASE: &str = "ssh_key_passphrase";

/// What this adapter can be told from the outside. One mechanism: a
/// connection needs a database password and, depending on the transport,
/// a secret for the tunnel as well.
///
/// Every field is *optional* here on purpose. Which of them a given
/// connection actually needs is decided by the config's own shape — a
/// tunnel on public-key auth needs no SSH password, and a database
/// password may still come straight from a `command` provider. The
/// binding-to-slot invariant is therefore checked in
/// [`PostgresConfig::validate`](crate::config::PostgresConfig::validate),
/// which can see both halves; `validate_against` only guards the
/// vocabulary.
pub(crate) const MECHANISMS: &[MechanismSpec] = &[MechanismSpec {
    id: "password",
    label: "Password (database, and the tunnel if it needs one)",
    doc: "Supplies the secrets this connection needs from one place: the database password, and \
          for an SSH tunnel the hop password or key passphrase. Point the slot at \
          `{type: script-result}` and one credential script decrypts them together.",
    fields: &[
        AuthFieldSpec::optional(FIELD_PASSWORD, "Database password", true),
        AuthFieldSpec::optional(FIELD_SSH_PASSWORD, "SSH password", true),
        AuthFieldSpec::optional(FIELD_SSH_KEY_PASSPHRASE, "SSH key passphrase", true),
    ],
}];

/// Resolves the `auth:` block's fields, asking the frontend when a
/// binding or the credential script needs it.
pub struct PostgresCredentials {
    orchestrator: Arc<AuthOrchestrator>,
}

impl PostgresCredentials {
    pub(crate) fn new(spec: AuthSpec) -> Result<Arc<Self>, String> {
        let orchestrator = AuthOrchestrator::from_spec(spec, Box::new(InMemorySessionStore::new()))
            .map_err(|e| format!("auth orchestrator: {e}"))?;
        Ok(Arc::new(Self {
            orchestrator: Arc::new(orchestrator),
        }))
    }

    /// Interactive progress of the credential round — `NeedsCreds` while
    /// a dialog is up, `Failed` when the user gave up. The client mirrors
    /// this into its own status channel for the duration of a resolve;
    /// see [`PostgresClient`](crate::client::PostgresClient).
    pub(crate) fn subscribe_status(&self) -> watch::Receiver<AdapterStatus> {
        self.orchestrator.subscribe_status()
    }

    /// Every field of the auth block, resolved. Cached by the
    /// orchestrator's session store, so the credential script runs once
    /// per process rather than once per connect.
    pub(crate) async fn fields(&self) -> Result<HashMap<String, String>, String> {
        let session = self
            .orchestrator
            .ensure_session(|creds| async move {
                serde_json::to_string(&creds).map_err(|e| format!("encode credentials: {e}"))
            })
            .await
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&session.blob).map_err(|e| format!("decode credentials: {e}"))
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

    /// Forget the resolved fields; the next connect resolves again (and
    /// may ask). Used when postgres rejected the password we handed it.
    pub(crate) async fn invalidate(&self) {
        self.orchestrator.invalidate_credentials().await;
    }
}
