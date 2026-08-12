//! The `workflow` adapter's `config:` block. Every key is optional, so a bare
//! `adapter: { type: workflow }` is a valid instance that uses the default
//! on-disk definition directory and run database.
//!
//! ```yaml
//! adapter:
//!   type: workflow
//!   config:
//!     storage_path: ~/notes/workflows   # where the .md definitions live
//!     database: ~/.local/state/nyd/workflow-runs.sqlite
//!     ai_command: nyd-workflow-ai       # runner for `ai` steps (Phase 5)
//!     mode: manual                      # default step mode when a file sets none
//!     log_runs: true                    # default run-logging when a file sets none
//! ```

use fieldsmith::Buildable;
use serde::Deserialize;

/// The adapter-level configuration. All fields optional; see the module doc.
#[derive(Debug, Default, Deserialize, Buildable)]
#[serde(deny_unknown_fields)]
pub struct WorkflowConfig {
    /// Directory holding the workflow `.md` definitions. Defaults to
    /// [`crate::repo::default_root`] (`<data_dir>/not_yet_done/workflows`).
    #[serde(default)]
    pub storage_path: Option<String>,
    /// SQLite URL (or plain path) for the run/protocol store. Defaults to
    /// [`crate::store::default_sqlite_url`].
    #[serde(default)]
    pub database: Option<String>,
    /// Command that executes `ai` steps — it receives the step's instruction and
    /// drives the app's own CLI (Phase 5). Adapter-agnostic, like the calendar
    /// adapter's reminder command; no AI vendor is baked in.
    #[serde(default)]
    pub ai_command: Option<String>,
    /// Default step mode (`manual` / `auto` / `ai`) applied to a workflow whose
    /// frontmatter omits `mode:`. Defaults to `manual`.
    #[serde(default)]
    pub mode: Option<String>,
    /// Default for a workflow whose frontmatter omits `log_runs:`. Defaults to
    /// `true` (runs are recorded).
    #[serde(default)]
    pub log_runs: Option<bool>,
    /// Whether this instance's background trigger scheduler runs (Phase 6c) —
    /// the cron / event-bus watcher that starts runs on its own. Defaults to
    /// `true`; set `false` as a kill-switch to disable all triggers for the
    /// instance without editing the definitions.
    #[serde(default)]
    pub triggers_enabled: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_is_valid_and_all_none() {
        let cfg: WorkflowConfig = serde_yaml::from_str("{}").unwrap();
        assert!(cfg.storage_path.is_none());
        assert!(cfg.database.is_none());
        assert!(cfg.ai_command.is_none());
        assert!(cfg.mode.is_none());
        assert!(cfg.log_runs.is_none());
        assert!(cfg.triggers_enabled.is_none());
    }

    #[test]
    fn parses_all_fields_and_rejects_unknown() {
        let yaml = "\
storage_path: /tmp/wf
database: /tmp/wf.sqlite
ai_command: run-ai
mode: auto
log_runs: false
";
        let cfg: WorkflowConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.storage_path.as_deref(), Some("/tmp/wf"));
        assert_eq!(cfg.database.as_deref(), Some("/tmp/wf.sqlite"));
        assert_eq!(cfg.ai_command.as_deref(), Some("run-ai"));
        assert_eq!(cfg.mode.as_deref(), Some("auto"));
        assert_eq!(cfg.log_runs, Some(false));

        assert!(serde_yaml::from_str::<WorkflowConfig>("bogus: 1").is_err());
    }
}
