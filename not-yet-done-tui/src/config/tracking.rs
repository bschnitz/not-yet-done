use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingConfig {
    #[serde(default = "default_allow_parallel")]
    pub allow_parallel: bool,
    /// Separator string rendered between path segments in the `taskpath`
    /// column of the trackings views (Normal + Condensed). The string can
    /// be any width — color is configured via `theme.taskpath_separator`.
    #[serde(default = "default_taskpath_separator")]
    pub taskpath_separator: String,
    /// Scaffold inserted into a new script created via the `:script`
    /// menu on the Trackings tab. When `None`, falls back to
    /// `script.template`. Set this when the Trackings JSON shape
    /// (`{tracking_ids, filter_min_date, filter_max_date}`) needs a
    /// dedicated starter different from the generic node scaffold.
    #[serde(default)]
    pub script_template: Option<String>,
}

fn default_allow_parallel() -> bool {
    false
}

fn default_taskpath_separator() -> String {
    "/".to_string()
}

impl Default for TrackingConfig {
    fn default() -> Self {
        Self {
            allow_parallel: default_allow_parallel(),
            taskpath_separator: default_taskpath_separator(),
            script_template: None,
        }
    }
}
