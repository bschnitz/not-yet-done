use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Split direction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitType {
    Vertical,
    Horizontal,
}

impl Default for SplitType {
    fn default() -> Self {
        SplitType::Vertical
    }
}

// ---------------------------------------------------------------------------
// Which pane appears first when split is active
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitPane {
    View,
    Form,
}

// ---------------------------------------------------------------------------
// SplitConfig — controls how a tab splits its area between view and form
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitConfig {
    /// Whether to split vertically (side-by-side) or horizontally (stacked).
    #[serde(rename = "type", default)]
    pub split_type: SplitType,

    /// Minimum terminal width (columns) required for a vertical split (side-by-side).
    /// Below this threshold the split collapses to a single pane.
    /// Named after the split type it controls.
    #[serde(alias = "vertical-threshold", default = "default_vertical_threshold")]
    pub vertical_threshold: u16,

    /// Minimum terminal height (rows) required for a horizontal split (stacked).
    /// Below this threshold the split collapses to a single pane.
    /// Named after the split type it controls.
    #[serde(
        alias = "horizontal-threshold",
        default = "default_horizontal_threshold"
    )]
    pub horizontal_threshold: u16,

    /// Render order of the two panes — first entry is rendered first
    /// (left in vertical split, top in horizontal split).
    #[serde(default = "default_order")]
    pub order: Vec<SplitPane>,
}

fn default_vertical_threshold() -> u16 {
    120
}
fn default_horizontal_threshold() -> u16 {
    30
}
fn default_order() -> Vec<SplitPane> {
    vec![SplitPane::View, SplitPane::Form]
}

impl Default for SplitConfig {
    fn default() -> Self {
        Self {
            split_type: SplitType::default(),
            vertical_threshold: default_vertical_threshold(),
            horizontal_threshold: default_horizontal_threshold(),
            order: default_order(),
        }
    }
}

// ---------------------------------------------------------------------------
// Per-tab layout configs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TasksLayoutConfig {
    #[serde(default)]
    pub split: SplitConfig,
}

// ---------------------------------------------------------------------------
// Top-level LayoutConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LayoutConfig {
    #[serde(default)]
    pub tasks: TasksLayoutConfig,
}
