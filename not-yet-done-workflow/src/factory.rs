//! [`WorkflowAdapterFactory`] — lifts the `workflow` adapter into the registry.
//!
//! The associated [`WorkflowConfig`] is the single source of truth: the generic
//! [`TypedFactory`](not_yet_done_content::TypedFactory) deserialises it and
//! reflects its schema. `build` opens the run store here (the sync `create`
//! path, bridged to async via [`store::open_blocking`]) so the adapter itself
//! stays free of the runtime-bridging concern.

use std::sync::Arc;

use not_yet_done_content::{ContentAdapter, HostContext, Result, TypedAdapterFactory};

use crate::adapter::WorkflowAdapter;
use crate::config::WorkflowConfig;
use crate::store::{self, RunStore};

#[derive(Default)]
pub struct WorkflowAdapterFactory;

impl WorkflowAdapterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl TypedAdapterFactory for WorkflowAdapterFactory {
    type Config = WorkflowConfig;

    fn adapter_type(&self) -> &str {
        "workflow"
    }

    fn build(
        &self,
        instance_id: &str,
        cfg: WorkflowConfig,
        ctx: &HostContext,
    ) -> Result<Box<dyn ContentAdapter>> {
        let store = Arc::new(open_store(cfg.database.as_deref()));
        let adapter = WorkflowAdapter::new(instance_id.to_string(), cfg, store);
        // Start the background trigger scheduler (cron / event bus) — a no-op
        // when disabled by config or there are no triggers to watch.
        adapter.spawn_triggers(Arc::clone(&ctx.event_bus));
        Ok(Box::new(adapter))
    }
}

/// Open the run store from the config's `database:` (or the default path),
/// degrading to an inert store on any failure so a bad path never breaks
/// adapter construction — the workflow surface then simply records no runs.
fn open_store(database: Option<&str>) -> RunStore {
    let url = match database.map(str::trim).filter(|s| !s.is_empty()) {
        Some(db) => store::normalize_db_url(db),
        None => match store::default_sqlite_url() {
            Ok(u) => u,
            Err(_) => return RunStore::inert(),
        },
    };
    match store::open_blocking(&url) {
        Ok(conn) => RunStore::new(conn),
        Err(_) => RunStore::inert(),
    }
}
