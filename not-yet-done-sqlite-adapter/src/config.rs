//! Adapter YAML config.
//!
//! Unlike Postgres there is no server, no transport and no credentials —
//! a SQLite database *is* a file. What replaces all of that is
//! `sources:`, an arbitrarily long list of glob patterns. Every file a
//! pattern matches becomes one root child. A plain path is simply a glob
//! that matches itself, so single-file setups need no special syntax.

use fieldsmith::Buildable;
use serde::Deserialize;

#[derive(Deserialize, Buildable, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct SqliteConfig {
    /// Optional human-readable name for the connection (shown as the
    /// root label in the TUI). Defaults to the single source's file stem
    /// when exactly one literal path is configured, else `sqlite`.
    #[serde(default)]
    pub name: Option<String>,

    /// Glob patterns naming the database files to expose. Matched fresh
    /// on every reload, so a new file appearing on disk shows up without
    /// a restart. `~` expands to the user's home directory; patterns must
    /// be absolute afterwards (a relative pattern would silently depend
    /// on the working directory the TUI happened to start in).
    pub sources: Vec<String>,

    /// Open every database read-only. On by default: this adapter is a
    /// browser, and a stray `UPDATE` in a scratch script should not be
    /// able to damage a file the user only wanted to look at. Set to
    /// `false` deliberately when you want to write.
    #[serde(default = "default_read_only")]
    pub read_only: bool,

    /// `PRAGMA busy_timeout` for every connection. SQLite locks at file
    /// level, so a writer elsewhere (another process, another tab) makes
    /// reads fail instantly with `SQLITE_BUSY` unless we agree to wait.
    #[serde(default = "default_busy_timeout_ms")]
    pub busy_timeout_ms: u64,

    /// Per-call timeout for catalogue reads and row fetches. `None` (the
    /// default) means "wait forever". A local file rarely stalls, but a
    /// database on a network mount can, and `busy_timeout_ms` only covers
    /// lock contention — not a hung filesystem.
    #[serde(default)]
    pub query_timeout_secs: Option<u64>,
}

fn default_read_only() -> bool {
    true
}

fn default_busy_timeout_ms() -> u64 {
    5_000
}

impl SqliteConfig {
    /// Validate cross-field invariants: at least one non-empty source,
    /// and every pattern absolute once `~` is expanded.
    pub fn validate(&self) -> Result<(), String> {
        if self.sources.is_empty() {
            return Err("sources must list at least one path or glob pattern".into());
        }
        for pattern in &self.sources {
            if pattern.trim().is_empty() {
                return Err("sources must not contain an empty pattern".into());
            }
            let expanded = crate::sources::expand_home(pattern);
            if !std::path::Path::new(&expanded).is_absolute() {
                return Err(format!(
                    "source pattern '{pattern}' must be absolute (or start with ~/)"
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> SqliteConfig {
        serde_yaml::from_str(yaml).expect("yaml parses")
    }

    #[test]
    fn defaults_are_read_only_with_a_busy_timeout() {
        let cfg = parse(
            r#"
sources:
  - /srv/data/metrics.db
"#,
        );
        cfg.validate().expect("valid");
        assert!(cfg.read_only, "read_only must default to true");
        assert_eq!(cfg.busy_timeout_ms, 5_000);
        assert_eq!(cfg.query_timeout_secs, None);
    }

    #[test]
    fn many_patterns_are_allowed_side_by_side() {
        let cfg = parse(
            r#"
name: scratch
sources:
  - /srv/data/*.db
  - /srv/data/archive/**/*.sqlite
  - ~/notes/journal.db
read_only: false
busy_timeout_ms: 250
query_timeout_secs: 10
"#,
        );
        cfg.validate().expect("valid");
        assert_eq!(cfg.sources.len(), 3);
        assert!(!cfg.read_only);
        assert_eq!(cfg.busy_timeout_ms, 250);
        assert_eq!(cfg.query_timeout_secs, Some(10));
    }

    #[test]
    fn rejects_an_empty_source_list() {
        let cfg = parse("sources: []\n");
        let err = cfg.validate().expect_err("empty sources must fail");
        assert!(err.contains("at least one"), "{err}");
    }

    #[test]
    fn rejects_a_relative_pattern() {
        let cfg = parse("sources: [data/*.db]\n");
        let err = cfg.validate().expect_err("relative pattern must fail");
        assert!(err.contains("absolute"), "{err}");
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let res: Result<SqliteConfig, _> =
            serde_yaml::from_str("sources: [/srv/data/x.db]\nschema: main\n");
        assert!(res.is_err(), "unknown field must fail to parse");
    }

    #[test]
    fn shipped_example_adapter_config_parses_and_validates() {
        // The example referenced by docs/examples/views/sqlite.yaml must
        // stay schema-valid — `deny_unknown_fields` would otherwise let a
        // renamed field break the shipped example silently.
        let yaml = include_str!("../../docs/examples/views/sqlite-adapter.yaml");
        let cfg = parse(yaml);
        cfg.validate()
            .expect("example adapter config should validate");
        assert_eq!(cfg.name.as_deref(), Some("example-sqlite"));
    }
}
