//! `AdapterFactory` impl: parses YAML and constructs a
//! `PostgresAdapter`. The SSH tunnel and the postgres session are
//! deferred to first-use inside `PostgresClient` — `create` itself is
//! cheap so the TUI starts without paying for SSH handshakes that the
//! user might not need this session.

use std::sync::Arc;

use not_yet_done_content::{AdapterFactory, ContentAdapter, ContentError, Result};

use crate::client::PostgresClient;
use crate::config::PostgresConfig;

use super::PostgresAdapter;

#[derive(Default)]
pub struct PostgresAdapterFactory;

impl PostgresAdapterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl AdapterFactory for PostgresAdapterFactory {
    fn adapter_type(&self) -> &str {
        "postgres"
    }

    fn create(
        &self,
        instance_id: &str,
        config: &str,
        _ctx: &not_yet_done_content::HostContext,
    ) -> Result<Box<dyn ContentAdapter>> {
        let cfg: PostgresConfig = serde_yaml::from_str(config).map_err(|e| {
            ContentError::Other(format!("Invalid Postgres config: {e}").into())
        })?;
        cfg.validate().map_err(|e| {
            ContentError::Other(format!("Invalid Postgres config: {e}").into())
        })?;

        let target_host = cfg.transport.target.host.clone();
        let name = cfg.name.unwrap_or_else(|| format!("postgres@{target_host}"));

        let query_timeout = cfg
            .query_timeout_secs
            .map(std::time::Duration::from_secs);
        let client = Arc::new(PostgresClient::new(
            cfg.transport,
            cfg.postgres,
            query_timeout,
        ));

        let warm = Arc::clone(&client);
        tokio::spawn(async move {
            // Warm tunnel + session in the background so the first user
            // interaction with the tab is fast. Errors are surfaced on
            // the next real call.
            let _ = warm.list_databases().await;
        });

        Ok(Box::new(PostgresAdapter::from_client(
            client,
            name,
            instance_id.to_string(),
        )))
    }
}
