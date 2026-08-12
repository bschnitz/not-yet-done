//! `AdapterFactory` impl: parses YAML and constructs a
//! [`SqliteAdapter`]. Glob expansion and the first connection are
//! deferred to first use inside [`SqliteClient`], so `build` stays cheap
//! even when `sources:` points at a slow network mount.

use std::sync::Arc;
use std::time::Duration;

use not_yet_done_content::{ContentAdapter, ContentError, Result, TypedAdapterFactory};

use crate::client::SqliteClient;
use crate::config::SqliteConfig;
use crate::sources::expand_home;

use super::SqliteAdapter;

#[derive(Default)]
pub struct SqliteAdapterFactory;

impl SqliteAdapterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl TypedAdapterFactory for SqliteAdapterFactory {
    type Config = SqliteConfig;

    fn adapter_type(&self) -> &str {
        "sqlite"
    }

    fn build(
        &self,
        instance_id: &str,
        cfg: SqliteConfig,
        _ctx: &not_yet_done_content::HostContext,
    ) -> Result<Box<dyn ContentAdapter>> {
        cfg.validate()
            .map_err(|e| ContentError::Other(format!("Invalid SQLite config: {e}").into()))?;

        let name = cfg
            .name
            .clone()
            .unwrap_or_else(|| default_name(&cfg.sources));
        let client = Arc::new(SqliteClient::new(
            cfg.sources,
            cfg.read_only,
            Duration::from_millis(cfg.busy_timeout_ms),
            cfg.query_timeout_secs.map(Duration::from_secs),
        ));

        let warm = Arc::clone(&client);
        tokio::spawn(async move {
            // Expand the globs in the background so the first interaction
            // with the tab doesn't wait on a directory walk. Errors surface
            // on the next real call.
            let _ = warm.list_databases().await;
        });

        Ok(Box::new(SqliteAdapter::from_client(
            client,
            name,
            instance_id.to_string(),
        )))
    }
}

/// Root label when the config names none: the file stem for a single
/// literal source (the common case — one database, and its name is the
/// most useful thing to show), else the generic adapter name.
fn default_name(sources: &[String]) -> String {
    let [only] = sources else {
        return "sqlite".to_string();
    };
    if only.contains(['*', '?', '[', '{']) {
        return "sqlite".to_string();
    }
    std::path::Path::new(&expand_home(only))
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "sqlite".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_literal_source_names_the_root_after_the_file() {
        assert_eq!(default_name(&["/srv/data/metrics.db".into()]), "metrics");
    }

    #[test]
    fn globs_and_multiple_sources_fall_back_to_the_generic_name() {
        assert_eq!(default_name(&["/srv/data/*.db".into()]), "sqlite");
        assert_eq!(
            default_name(&["/srv/a.db".into(), "/srv/b.db".into()]),
            "sqlite"
        );
        assert_eq!(default_name(&[]), "sqlite");
    }
}
