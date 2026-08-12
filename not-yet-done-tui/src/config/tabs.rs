//! Top-level tab order — a single curated, ordered list of tabs.
//!
//! `tabs.order` names the tabs to show, in display order, referencing
//! each content view's `tab.name`. The order also assigns the switch
//! keys `1`..`9`, then `0` for a tenth tab; an eleventh and beyond get
//! no digit (only `Tab`/`Shift+Tab` cycling reaches them).
//!
//! Two tabs sharing a name is a hard configuration error — the name
//! would no longer uniquely identify a tab — surfaced as a startup
//! modal.
//!
//! When no order is configured (`order` empty) every configured tab is
//! shown in its natural slot order, still autonumbered.
//!
//! By default the subtab row shares the first line with the main tabs and
//! only wraps onto a second line when it no longer fits. Setting
//! `subtabs_own_line: true` forces the subtabs onto their own line
//! unconditionally.
//!
//! ```yaml
//! tabs:
//!   order:
//!     - Tasks
//!     - Trackings
//!     - Jira
//!     - Taiga
//!   subtabs_own_line: true
//! ```

use serde::{Deserialize, Serialize};

/// The `tabs:` section of `tui.yaml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TabsConfig {
    /// Ordered list of tab display names to show. Empty (the default)
    /// shows every configured tab in its natural slot order.
    #[serde(default)]
    pub order: Vec<String>,

    /// When true the subtab row always occupies its own line beneath the
    /// main tabs instead of sharing the first line when it fits. Default
    /// false (dynamic wrapping).
    #[serde(default)]
    pub subtabs_own_line: bool,
}

impl TabsConfig {
    /// True when the user has curated an explicit tab order. When false
    /// the layout falls back to every tab in slot order.
    pub fn is_active(&self) -> bool {
        !self.order.is_empty()
    }

    /// The configured tab-name order.
    pub fn order(&self) -> &[String] {
        &self.order
    }
}
