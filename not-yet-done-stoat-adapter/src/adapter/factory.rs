//! Adapter factory: parses the YAML config, opens (or reuses) the
//! backing-store connection, and constructs a `StoatAdapter` without
//! blocking. Login + WS connect run lazily in the background on the
//! first `root()` call (see [`super::StoatAdapter::root`]), so the TUI
//! comes up immediately and the Stoat tab shows the live connection
//! status banner.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sea_orm::DatabaseConnection;

use not_yet_done_content::{AdapterFactory, ContentAdapter, ContentError, Result};

use super::{StoatAdapter, auth_bridge::AuthBridge, config::StoatConfig};
use crate::auth_session_store::SqlAuthSessionStore;
use crate::db::scope_id_for_url;

/// Owns a per-URL connection pool. Each unique `db.url` (after
/// defaulting) is opened + schema-synced exactly once and shared across
/// all adapter instances pointing at the same backing store.
#[derive(Default)]
pub struct StoatAdapterFactory {
    connections: Mutex<HashMap<String, Arc<DatabaseConnection>>>,
}

impl StoatAdapterFactory {
    pub fn new() -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
        }
    }

    fn connection_for(&self, db_url: &str) -> std::result::Result<Arc<DatabaseConnection>, String> {
        if let Some(existing) = self.connections.lock().unwrap().get(db_url).cloned() {
            return Ok(existing);
        }

        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| "StoatAdapterFactory needs a Tokio runtime".to_string())?;
        if handle.runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
            return Err("StoatAdapterFactory needs a multi-threaded Tokio runtime".into());
        }

        let url_owned = db_url.to_string();
        let db = tokio::task::block_in_place(|| {
            handle.block_on(async move { crate::db::connect(&url_owned).await })
        })
        .map_err(|e| format!("open stoat db ({db_url}): {e}"))?;

        let arc = Arc::new(db);
        self.connections
            .lock()
            .unwrap()
            .insert(db_url.to_string(), Arc::clone(&arc));
        Ok(arc)
    }
}

impl AdapterFactory for StoatAdapterFactory {
    fn adapter_type(&self) -> &str {
        "stoat"
    }

    fn create(&self, instance_id: &str, config: &str) -> Result<Box<dyn ContentAdapter>> {
        let cfg: StoatConfig = serde_yaml::from_str(config)
            .map_err(|e| ContentError::Other(format!("Invalid Stoat config: {e}").into()))?;

        cfg.auth
            .validate()
            .map_err(|e| ContentError::Other(format!("Invalid Stoat auth spec: {e}").into()))?;

        let db_url = match &cfg.db {
            Some(c) => c.url.clone(),
            None => crate::db::default_sqlite_url().map_err(|e| ContentError::Other(e.into()))?,
        };
        let db = self
            .connection_for(&db_url)
            .map_err(|e| ContentError::Other(e.into()))?;

        let scope_id = scope_id_for_url(&cfg.url);
        let name = cfg.name.unwrap_or_else(|| cfg.url.clone());

        let store = SqlAuthSessionStore::new(Arc::clone(&db), scope_id);
        let auth = AuthBridge::new(cfg.url, Box::new(store), cfg.auth)
            .map_err(|e| ContentError::Other(e.into()))?;

        Ok(Box::new(StoatAdapter::from_parts(
            auth,
            name,
            instance_id.to_string(),
        )))
    }
}
