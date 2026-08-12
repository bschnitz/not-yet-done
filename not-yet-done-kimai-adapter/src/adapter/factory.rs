//! `AdapterFactory` impl: parses the YAML config and constructs a
//! `KimaiAdapter`. No cache DB and no persisted session — credential
//! resolution (pass/env/literal) is cheap enough to redo per process, so
//! the bridge runs on an [`InMemorySessionStore`].

use not_yet_done_content::{
    ContentAdapter, ContentError, InMemorySessionStore, MechanismSpec, Result, TypedAdapterFactory,
};

use super::KimaiAdapter;
use super::auth_bridge::{AuthBridge, MECHANISMS};
use super::config::{
    DEFAULT_CONNECT_TIMEOUT_CAP_SECS, DEFAULT_LOOKBACK_DAYS, DEFAULT_REQUEST_TIMEOUT_SECS,
    KimaiConfig,
};
use crate::client::HttpTimeouts;

#[derive(Default)]
pub struct KimaiAdapterFactory;

impl KimaiAdapterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl TypedAdapterFactory for KimaiAdapterFactory {
    type Config = KimaiConfig;

    fn adapter_type(&self) -> &str {
        "kimai"
    }

    fn auth_mechanisms(&self) -> &'static [MechanismSpec] {
        MECHANISMS
    }

    fn build(
        &self,
        instance_id: &str,
        cfg: KimaiConfig,
        _ctx: &not_yet_done_content::HostContext,
    ) -> Result<Box<dyn ContentAdapter>> {
        cfg.auth
            .validate_against(MECHANISMS)
            .map_err(|e| ContentError::Other(format!("Invalid Kimai auth spec: {e}").into()))?;

        let request_secs = cfg
            .request_timeout_secs
            .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);
        let connect_secs = cfg
            .connect_timeout_secs
            .unwrap_or_else(|| request_secs.min(DEFAULT_CONNECT_TIMEOUT_CAP_SECS));
        let timeouts = HttpTimeouts {
            request_secs,
            connect_secs,
        };

        let name = cfg.name.unwrap_or_else(|| cfg.url.clone());
        let lookback_days = cfg.lookback_days.unwrap_or(DEFAULT_LOOKBACK_DAYS);

        let auth = AuthBridge::new(
            cfg.url,
            cfg.auth,
            Box::new(InMemorySessionStore::new()),
            timeouts,
        )
        .map_err(|e| ContentError::Other(e.into()))?;

        Ok(Box::new(KimaiAdapter::from_parts(
            auth,
            name,
            instance_id.to_string(),
            lookback_days,
        )))
    }
}
