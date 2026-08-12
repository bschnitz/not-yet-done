//! Process-global session registry.
//!
//! Sessions are keyed by [`SessionConfig::account_key`]. Consumers that pass
//! the same key share one browser session (one MFA login serves a calendar and
//! a future mail backend alike); a different key yields a separate session.
//!
//! Entries are held weakly: the shared [`SessionInner`] lives only as long as a
//! [`SessionHandle`] exists somewhere, so a session (and its browser) shuts
//! down automatically once its last consumer drops.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, Weak};

use tokio::sync::Mutex;

use crate::error::MsOfficeError;
use crate::session::{SessionConfig, SessionHandle, SessionInner};

type Registry = Mutex<HashMap<String, Weak<SessionInner>>>;

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Entry point to the crate: get or create the shared session for an account.
pub struct MsOfficeWeb;

impl MsOfficeWeb {
    /// Return the session for `config.account_key`, creating it if none is
    /// live. Calling this repeatedly with the same key is cheap and always
    /// returns a handle to the same underlying session.
    ///
    /// The `async` signature only reflects the registry lock; no browser work
    /// happens here — the sidecar is launched lazily on the first operation.
    pub async fn session(config: SessionConfig) -> Result<SessionHandle, MsOfficeError> {
        // Hold the lock across the (cheap) create so two racing callers with
        // the same key can't spawn two sessions.
        let mut map = registry().lock().await;

        if let Some(inner) = map.get(&config.account_key).and_then(Weak::upgrade) {
            return Ok(SessionHandle { inner });
        }

        let inner = SessionInner::spawn(config.clone());
        map.insert(config.account_key.clone(), Arc::downgrade(&inner));
        Ok(SessionHandle { inner })
    }
}
