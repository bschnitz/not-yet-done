//! `AdapterFactory` impl: parse the YAML config, build one backend per
//! connection via the compile-time registry, and hand them to a
//! [`CalendarAdapter`].

use not_yet_done_content::{
    ContentAdapter, ContentError, HostContext, Result, TypedAdapterFactory,
};

use crate::adapter::CalendarAdapter;
use crate::config::CalendarConfig;

#[derive(Default)]
pub struct CalendarAdapterFactory;

impl CalendarAdapterFactory {
    pub fn new() -> Self {
        Self
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
        let adapter = CalendarAdapter::from_config(instance_id.to_string(), cfg, ctx)
            .map_err(|e| ContentError::Other(e.into()))?;

        Ok(Box::new(adapter))
    }
}
