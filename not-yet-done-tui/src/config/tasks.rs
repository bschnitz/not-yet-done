use serde::{Deserialize, Serialize};

/// Tab-specific configuration for the Tasks tab.
///
/// ```yaml
/// tasks:
///   tree:
///     default_expand_depth: 2
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TasksConfig {
    #[serde(default)]
    pub tree: TasksTreeConfig,
}

/// Tree-mode behaviour in the Tasks tab.
///
/// `default_expand_depth` controls how many levels are expanded by
/// default on startup (and after `zm`). 0 = only root level visible,
/// 1 = root + first level of children, 2 = three levels visible, …
///
/// The user can still expand/collapse individual nodes (`<space>`), open
/// everything (`zr`), or collapse back to this depth (`zm`). State is
/// kept in memory only — it does **not** persist across restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TasksTreeConfig {
    #[serde(default = "default_expand_depth")]
    pub default_expand_depth: u32,
}

fn default_expand_depth() -> u32 {
    0
}

impl Default for TasksTreeConfig {
    fn default() -> Self {
        Self {
            default_expand_depth: default_expand_depth(),
        }
    }
}
