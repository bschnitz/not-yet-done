//! `AdapterFactory` impl: parses YAML config, resolves the cache DB
//! connection (with per-URL pooling), and constructs a `JiraAdapter`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sea_orm::DatabaseConnection;

use not_yet_done_content::{AdapterFactory, ContentAdapter, ContentError, Result};

use super::JiraAdapter;
use super::auth_bridge::AuthBridge;
use super::config::JiraConfig;
use crate::auth_session_store::SqlAuthSessionStore;
use crate::cache_store::scope_id_for_url;

/// Owns a per-URL connection pool. Each unique `db.url` (after defaulting)
/// is opened + schema-synced exactly once and shared across all adapter
/// instances pointing at the same backing store.
#[derive(Default)]
pub struct JiraAdapterFactory {
    connections: Mutex<HashMap<String, Arc<DatabaseConnection>>>,
}

impl JiraAdapterFactory {
    pub fn new() -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
        }
    }

    /// Look up — or open + sync schema — the connection for `db_url`.
    /// Bridges async `Database::connect` into the sync `AdapterFactory::create`
    /// path via `block_in_place`; requires a multi-threaded Tokio runtime.
    fn connection_for(&self, db_url: &str) -> std::result::Result<Arc<DatabaseConnection>, String> {
        if let Some(existing) = self.connections.lock().unwrap().get(db_url).cloned() {
            return Ok(existing);
        }

        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| "JiraAdapterFactory needs a Tokio runtime".to_string())?;
        if handle.runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
            return Err("JiraAdapterFactory needs a multi-threaded Tokio runtime".into());
        }

        let url_owned = db_url.to_string();
        let db = tokio::task::block_in_place(|| {
            handle.block_on(async move { crate::db::connect(&url_owned).await })
        })
        .map_err(|e| format!("open jira cache db ({db_url}): {e}"))?;

        let arc = Arc::new(db);
        self.connections
            .lock()
            .unwrap()
            .insert(db_url.to_string(), Arc::clone(&arc));
        Ok(arc)
    }
}

impl AdapterFactory for JiraAdapterFactory {
    fn adapter_type(&self) -> &str {
        "jira"
    }

    fn create(&self, instance_id: &str, config: &str) -> Result<Box<dyn ContentAdapter>> {
        let cfg: JiraConfig = serde_yaml::from_str(config)
            .map_err(|e| ContentError::Other(format!("Invalid Jira config: {e}").into()))?;

        cfg.auth
            .validate()
            .map_err(|e| ContentError::Other(format!("Invalid Jira auth spec: {e}").into()))?;

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

        let store = SqlAuthSessionStore::new(Arc::clone(&db), scope_id);
        let auth = AuthBridge::new(
            cfg.url.clone(),
            cfg.accept_invalid_certs,
            cfg.auth,
            Box::new(store),
        )
        .map_err(|e| ContentError::Other(e.into()))?;

        // Auth is exercised lazily by the first `root()` / `get_by_id()`
        // call. Previously the factory spawned an eager `get_client()`
        // warmup so the TUI saw status flips before the first list — but
        // that side-effect runs unconditionally, defeating the
        // `adapter.manual_connect: true` opt-out (the browser-based
        // OAuth flow would fire at TUI startup regardless). Status
        // updates still reach subscribers via `subscribe_status()`; they
        // just arrive on the load-triggered path instead of a separate
        // warmup task.

        Ok(Box::new(JiraAdapter::from_parts(
            auth,
            name,
            instance_id.to_string(),
            db,
            scope_id,
        )))
    }
}
