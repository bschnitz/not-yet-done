pub mod color;
pub mod editor;
pub mod keybinding_edit;
pub mod keybindings;
pub mod script;
pub mod tabs;
pub mod theme_config;
pub mod tracking;
pub mod tui_config;
pub mod view_config;
pub mod yaml_edit;

// Legacy single-purpose services are superseded by TuiConfigService —
// kept as dead modules only if needed for migration; otherwise removed.

pub use keybindings::{
    CommonAction, ContentAction, FormAction, GlobalAction, KeyBindingConfig, QueryMenuAction,
    WindowAction,
};
pub use tabs::TabsConfig;
pub use theme_config::ThemeConfig;
pub use tui_config::{ShortcutScope, TuiConfig, TuiConfigService};
