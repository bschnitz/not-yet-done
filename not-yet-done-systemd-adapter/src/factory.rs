//! [`SystemdAdapterFactory`] — lifts the `systemd` adapter into the registry.
//!
//! [`SystemdConfig`] is the single source of truth: the generic
//! [`TypedAdapterFactory`] deserialises it and reflects its schema for the
//! config wizard, so `manager:` and `timeout_secs:` are declared once.
//!
//! `build` does no I/O. A `--user` manager that is not running, or a session
//! bus that cannot be reached, must fail on the load that wanted it — where
//! there is a pane to show the message — not while the tab is still being
//! assembled and nothing is on screen.

use not_yet_done_content::{ContentAdapter, ContentError, HostContext, Result, TypedAdapterFactory};

use crate::adapter::SystemdAdapter;
use crate::config::SystemdConfig;

#[derive(Default)]
pub struct SystemdAdapterFactory;

impl SystemdAdapterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl TypedAdapterFactory for SystemdAdapterFactory {
    type Config = SystemdConfig;

    fn adapter_type(&self) -> &str {
        "systemd"
    }

    fn build(
        &self,
        instance_id: &str,
        cfg: SystemdConfig,
        _ctx: &HostContext,
    ) -> Result<Box<dyn ContentAdapter>> {
        // A misspelled `manager:` is caught here rather than quietly falling
        // back to the user manager: the two managers hold different units, and
        // silently showing the wrong one is worse than refusing the tab.
        cfg.validate().map_err(|e| ContentError::Other(e.into()))?;
        Ok(Box::new(SystemdAdapter::new(instance_id.to_string(), &cfg)))
    }
}
