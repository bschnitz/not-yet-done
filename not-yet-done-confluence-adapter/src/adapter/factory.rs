//! `AdapterFactory` impl: parses YAML config, resolves the cache DB
//! connection (with per-URL pooling), and constructs a `ConfluenceAdapter`.
//!
//! CF-2b slice: wires an [`AuthBridge`] backed by the unified
//! [`AuthOrchestrator`] + the SQLite-persisted session store. No eager
//! warmup — auth is exercised lazily by the first `root()` / `get_by_id()`
//! call (same trade-off as the Jira factory: an eager get_client() would
//! defeat the view-level `manual_connect: true` opt-out).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sea_orm::DatabaseConnection;

use not_yet_done_content::{
    ContentAdapter, ContentError, MechanismSpec, Result, TypedAdapterFactory,
};

use super::ConfluenceAdapter;
use super::auth_bridge::{AuthBridge, MECHANISMS};
use crate::auth_session_store::SqlAuthSessionStore;
use crate::cache_store::scope_id_for_url;
use crate::config::ConfluenceConfig;

/// Owns a per-URL connection pool. Each unique `db.url` (after defaulting)
/// is opened + schema-synced exactly once and shared across all adapter
/// instances pointing at the same backing store.
#[derive(Default)]
pub struct ConfluenceAdapterFactory {
    connections: Mutex<HashMap<String, Arc<DatabaseConnection>>>,
}

impl ConfluenceAdapterFactory {
    pub fn new() -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
        }
    }

    /// Look up — or open + sync schema — the connection for `db_url`.
    /// Bridges async `Database::connect` into the sync
    /// `AdapterFactory::create` path via `block_in_place`; requires a
    /// multi-threaded Tokio runtime.
    fn connection_for(&self, db_url: &str) -> std::result::Result<Arc<DatabaseConnection>, String> {
        if let Some(existing) = self.connections.lock().unwrap().get(db_url).cloned() {
            return Ok(existing);
        }

        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| "ConfluenceAdapterFactory needs a Tokio runtime".to_string())?;
        if handle.runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
            return Err("ConfluenceAdapterFactory needs a multi-threaded Tokio runtime".into());
        }

        let url_owned = db_url.to_string();
        let db = tokio::task::block_in_place(|| {
            handle.block_on(async move { crate::db::connect(&url_owned).await })
        })
        .map_err(|e| format!("open confluence cache db ({db_url}): {e}"))?;

        let arc = Arc::new(db);
        self.connections
            .lock()
            .unwrap()
            .insert(db_url.to_string(), Arc::clone(&arc));
        Ok(arc)
    }
}

impl TypedAdapterFactory for ConfluenceAdapterFactory {
    type Config = ConfluenceConfig;

    fn adapter_type(&self) -> &str {
        "confluence"
    }

    fn auth_mechanisms(&self) -> &'static [MechanismSpec] {
        MECHANISMS
    }

    fn build(
        &self,
        instance_id: &str,
        cfg: ConfluenceConfig,
        _ctx: &not_yet_done_content::HostContext,
    ) -> Result<Box<dyn ContentAdapter>> {
        cfg.auth.validate_against(MECHANISMS).map_err(|e| {
            ContentError::Other(format!("Invalid Confluence auth spec: {e}").into())
        })?;

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
        let auth = AuthBridge::new(
            cfg.url.clone(),
            cfg.accept_invalid_certs,
            cfg.auth,
            Box::new(store),
        )
        .map_err(|e| ContentError::Other(e.into()))?;

        Ok(Box::new(ConfluenceAdapter::from_parts(
            auth,
            instance_id.to_string(),
            name,
            cfg.url.clone(),
            db,
            scope_id,
            cfg.space_keys,
        )))
    }
}
