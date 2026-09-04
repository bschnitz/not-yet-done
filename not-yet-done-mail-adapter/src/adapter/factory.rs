//! Adapter factory: parse the YAML, check every account's `auth:` block
//! against the mechanisms this adapter actually implements, and build the
//! adapter.
//!
//! Nothing is connected here. Six accounts must not become six logins at
//! startup — the connection actors are built on first use, and the auth
//! spec is the one thing worth rejecting up front, because a typo in a
//! mechanism name would otherwise only surface as a failed login much later.

use not_yet_done_content::{
    ContentAdapter, ContentError, HostContext, MechanismSpec, Result, TypedAdapterFactory,
};

use super::MailAdapter;
use crate::auth::MECHANISMS;
use crate::config::MailConfig;

#[derive(Default)]
pub struct MailAdapterFactory;

impl MailAdapterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl TypedAdapterFactory for MailAdapterFactory {
    type Config = MailConfig;

    fn adapter_type(&self) -> &str {
        "mail"
    }

    fn auth_mechanisms(&self) -> &'static [MechanismSpec] {
        MECHANISMS
    }

    fn build(
        &self,
        instance_id: &str,
        cfg: MailConfig,
        _ctx: &HostContext,
    ) -> Result<Box<dyn ContentAdapter>> {
        for account in &cfg.accounts {
            account.auth.validate_against(MECHANISMS).map_err(|e| {
                ContentError::Other(
                    format!("account `{}`: invalid auth spec: {e}", account.id).into(),
                )
            })?;
            // A submission block with its own credentials is checked against
            // the same table: the mechanisms are the account's, and a typo
            // there would otherwise only surface when a finished mail fails
            // to leave.
            if let Some(auth) = account.smtp.as_ref().and_then(|s| s.auth.as_ref()) {
                auth.validate_against(MECHANISMS).map_err(|e| {
                    ContentError::Other(
                        format!("account `{}`: invalid smtp auth spec: {e}", account.id).into(),
                    )
                })?;
            }
        }
        let adapter = MailAdapter::from_config(instance_id, cfg)
            .map_err(|e| ContentError::Other(e.into()))?;
        Ok(Box::new(adapter))
    }
}
