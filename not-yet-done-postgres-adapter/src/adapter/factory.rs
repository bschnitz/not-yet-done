//! `AdapterFactory` impl: parses YAML and constructs a
//! `PostgresAdapter`. The SSH tunnel and the postgres session are
//! deferred to first-use inside `PostgresClient` — `create` itself is
//! cheap so the TUI starts without paying for SSH handshakes that the
//! user might not need this session.

use std::sync::Arc;

use not_yet_done_content::{
    ContentAdapter, ContentError, MechanismSpec, Result, TypedAdapterFactory,
};

use crate::client::PostgresClient;
use crate::config::PostgresConfig;

use super::PostgresAdapter;
use super::auth::{MECHANISMS, PostgresCredentials};

#[derive(Default)]
pub struct PostgresAdapterFactory;

impl PostgresAdapterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl TypedAdapterFactory for PostgresAdapterFactory {
    type Config = PostgresConfig;

    fn adapter_type(&self) -> &str {
        "postgres"
    }

    fn auth_mechanisms(&self) -> &'static [MechanismSpec] {
        MECHANISMS
    }

    fn build(
        &self,
        instance_id: &str,
        cfg: PostgresConfig,
        _ctx: &not_yet_done_content::HostContext,
    ) -> Result<Box<dyn ContentAdapter>> {
        cfg.validate()
            .map_err(|e| ContentError::Other(format!("Invalid Postgres config: {e}").into()))?;

        let target_host = cfg.transport.target.host.clone();
        let name = cfg
            .name
            .unwrap_or_else(|| format!("postgres@{target_host}"));

        let query_timeout = cfg.query_timeout_secs.map(std::time::Duration::from_secs);
        // The auth block, when some provider slot delegates to it. Built
        // here rather than lazily so a mechanism this adapter cannot
        // speak fails at config time, next to every other config error.
        let credentials = match cfg.auth {
            Some(spec) => {
                spec.validate_against(MECHANISMS).map_err(|e| {
                    ContentError::Other(format!("Invalid Postgres auth spec: {e}").into())
                })?;
                Some(PostgresCredentials::new(spec).map_err(|e| ContentError::Other(e.into()))?)
            }
            None => None,
        };

        let client = Arc::new(PostgresClient::new(
            cfg.transport,
            cfg.postgres,
            query_timeout,
            credentials.clone(),
        ));

        // No eager warmup. Building an adapter happens for every
        // configured view at TUI startup, so a background
        // `list_databases()` here ran unconditionally — and its first
        // step is resolving the password, which for a `command` provider
        // means running `pass` and putting a pinentry dialog on screen
        // before the user has even left the tasks view. That defeats the
        // view-level `adapter.manual_connect: true` opt-out (same
        // trade-off the Jira and Confluence factories already made).
        // The tunnel and session are established lazily by the first
        // real call instead.

        Ok(Box::new(PostgresAdapter::from_client(
            client,
            name,
            instance_id.to_string(),
            credentials,
        )))
    }
}
