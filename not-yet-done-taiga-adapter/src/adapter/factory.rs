//! Adapter factory: parses the YAML config, opens (or reuses) the cache
//! DB connection, and constructs a `TaigaAdapter` without ever blocking
//! the calling thread. Login orchestration runs as a background task on
//! the auth bridge — the TUI comes up immediately and the Taiga tab will
//! show either the cached session or a credentials popup.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sea_orm::DatabaseConnection;

use not_yet_done_content::{AdapterFactory, ContentAdapter, ContentError, Result};

use super::{TaigaAdapter, auth_bridge::AuthBridge, config::TaigaConfig};
use crate::cache_store::scope_id_for_url;

/// Owns a per-URL connection pool. Each unique `db.url` (after defaulting)
/// is opened + schema-synced exactly once and shared across all adapter
/// instances pointing at the same backing store.
#[derive(Default)]
pub struct TaigaAdapterFactory {
    connections: Mutex<HashMap<String, Arc<DatabaseConnection>>>,
}

impl TaigaAdapterFactory {
    pub fn new() -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
        }
    }

    fn connection_for(
        &self,
        db_url: &str,
    ) -> std::result::Result<Arc<DatabaseConnection>, String> {
        if let Some(existing) = self.connections.lock().unwrap().get(db_url).cloned() {
            return Ok(existing);
        }

        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| "TaigaAdapterFactory needs a Tokio runtime".to_string())?;
        if handle.runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
            return Err("TaigaAdapterFactory needs a multi-threaded Tokio runtime".into());
        }

        let url_owned = db_url.to_string();
        let db = tokio::task::block_in_place(|| {
            handle.block_on(async move { crate::db::connect(&url_owned).await })
        })
        .map_err(|e| format!("open taiga cache db ({db_url}): {e}"))?;

        let arc = Arc::new(db);
        self.connections
            .lock()
            .unwrap()
            .insert(db_url.to_string(), Arc::clone(&arc));
        Ok(arc)
    }
}

impl AdapterFactory for TaigaAdapterFactory {
    fn adapter_type(&self) -> &str {
        "taiga"
    }

    fn create(&self, instance_id: &str, config: &str) -> Result<Box<dyn ContentAdapter>> {
        let cfg: TaigaConfig = serde_yaml::from_str(config)
            .map_err(|e| ContentError::Other(format!("Invalid Taiga config: {e}").into()))?;

        cfg.auth
            .validate()
            .map_err(|e| ContentError::Other(format!("Invalid Taiga auth spec: {e}").into()))?;

        let db_url = match &cfg.db {
            Some(c) => c.url.clone(),
            None => crate::db::default_sqlite_url()
                .map_err(|e| ContentError::Other(e.into()))?,
        };
        let db = self
            .connection_for(&db_url)
            .map_err(|e| ContentError::Other(e.into()))?;

        let scope_id = scope_id_for_url(&cfg.url);
        let name = cfg.name.unwrap_or_else(|| cfg.url.clone());
        let request_secs = cfg
            .request_timeout_secs
            .unwrap_or(super::config::DEFAULT_REQUEST_TIMEOUT_SECS);
        // Default the connect ceiling to `min(request, cap)` so an
        // unreachable host fails fast; an explicit value lifts the cap.
        let connect_secs = cfg
            .connect_timeout_secs
            .unwrap_or_else(|| request_secs.min(super::config::DEFAULT_CONNECT_TIMEOUT_CAP_SECS));
        let timeouts = crate::client::HttpTimeouts { request_secs, connect_secs };

        let auth = AuthBridge::new(cfg.url, Arc::clone(&db), scope_id, cfg.auth, timeouts)
            .map_err(|e| ContentError::Other(e.into()))?;

        // Auth is exercised lazily by the first `root()` / `get_by_id()`
        // call. Previously the factory spawned an eager `get_client()`
        // warmup so the TUI saw status flips before the first list — but
        // that side-effect runs unconditionally, defeating the
        // `adapter.manual_connect: true` opt-out (interactive auth
        // would fire at TUI startup regardless). Status updates still
        // reach subscribers via `subscribe_status()`; they just arrive
        // on the load-triggered path instead of a separate warmup task.

        Ok(Box::new(TaigaAdapter::from_parts(
            auth,
            name,
            instance_id.to_string(),
            db,
            scope_id,
        )))
    }
}
