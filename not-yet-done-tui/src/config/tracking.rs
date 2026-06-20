use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackingConfig {
    #[serde(default = "default_allow_parallel")]
    pub allow_parallel: bool,
}

fn default_allow_parallel() -> bool {
    false
}

impl Default for TrackingConfig {
    fn default() -> Self {
        Self {
            allow_parallel: default_allow_parallel(),
        }
    }
}
