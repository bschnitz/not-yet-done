//! `AdapterFactory` impl: parse the YAML config, open (or reuse) the event
//! store's database, build one backend per connection via the compile-time
//! registry, and hand them to a [`CalendarAdapter`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sea_orm::DatabaseConnection;

use not_yet_done_content::{
    ContentAdapter, ContentError, HostContext, Result, TypedAdapterFactory,
};

use crate::adapter::CalendarAdapter;
use crate::config::CalendarConfig;
use crate::persist::Persisted;
use crate::store::{EventStore, connect, default_sqlite_url};

/// Owns a per-URL connection pool: each distinct `db.url` (after defaulting)
/// is opened and schema-synced once and shared by every instance using it.
#[derive(Default)]
pub struct CalendarAdapterFactory {
    connections: Mutex<HashMap<String, Arc<DatabaseConnection>>>,
}

impl CalendarAdapterFactory {
    pub fn new() -> Self {
        Self::default()
    }

    fn connection_for(&self, db_url: &str) -> std::result::Result<Arc<DatabaseConnection>, String> {
        if let Some(existing) = self.connections.lock().unwrap().get(db_url).cloned() {
            return Ok(existing);
        }
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| "the calendar adapter needs a Tokio runtime".to_string())?;
        if handle.runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
            return Err("the calendar adapter needs a multi-threaded Tokio runtime".into());
        }
        let url_owned = db_url.to_string();
        let db = tokio::task::block_in_place(|| handle.block_on(connect(&url_owned)))
            .map_err(|e| format!("open calendar event store ({db_url}): {e}"))?;
        let arc = Arc::new(db);
        self.connections
            .lock()
            .unwrap()
            .insert(db_url.to_string(), Arc::clone(&arc));
        Ok(arc)
    }
}

impl TypedAdapterFactory for CalendarAdapterFactory {
    type Config = CalendarConfig;

    fn adapter_type(&self) -> &str {
        "calendar"
    }

    fn build(
        &self,
        instance_id: &str,
        cfg: CalendarConfig,
        ctx: &HostContext,
    ) -> Result<Box<dyn ContentAdapter>> {
        let db_url = match &cfg.db {
            Some(db) => db.url.clone(),
            None => default_sqlite_url().map_err(|e| ContentError::Other(e.into()))?,
        };
        let db = self
            .connection_for(&db_url)
            .map_err(|e| ContentError::Other(e.into()))?;
        let persisted = Arc::new(Persisted::new(EventStore::new(db, instance_id)));

        let adapter =
            CalendarAdapter::from_config(instance_id.to_string(), cfg, Some(persisted), ctx)
                .map_err(|e| ContentError::Other(e.into()))?;

        Ok(Box::new(adapter))
    }
}
