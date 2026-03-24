pub mod color;
pub mod keybindings;
pub mod layout;
pub mod theme_config;
pub mod tui_config;

// Legacy single-purpose services are superseded by TuiConfigService —
// kept as dead modules only if needed for migration; otherwise removed.

pub use keybindings::{FormAction, GlobalAction, KeyBindingConfig, TasksAction};
pub use layout::{SplitPane, SplitType};
pub use theme_config::ThemeConfig;
pub use tui_config::{TuiConfig, TuiConfigService};
