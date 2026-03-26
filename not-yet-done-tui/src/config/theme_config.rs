use serde::{Deserialize, Serialize};

use super::color::HexColor;

/// All theme colours, fully configurable via `tui-theme.yaml`.
///
/// Each field maps directly to a named constant in the old `Theme` struct.
/// The YAML file uses `#rrggbb` hex strings:
///
/// ```yaml
/// name: Teal Dark
/// bg:          "#121212"
/// surface:     "#1e1e1e"
/// surface_2:   "#282828"
/// primary:     "#4db6ac"
/// primary_dim: "#26a69a"
/// on_primary:  "#002522"
/// accent:      "#ffca28"
/// accent_dim:  "#ffb300"
/// text_high:   "#e6e6e6"
/// text_med:    "#9e9e9e"
/// text_dim:    "#616161"
/// success:     "#66bb6a"
/// error:       "#ef5350"
/// warning:     "#ffa726"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    /// Human-readable name shown in error messages / future theme picker
    #[serde(default = "default_name")]
    pub name: String,

    // ── Surfaces ──────────────────────────────────────────────────────────
    #[serde(default = "d_bg")]
    pub bg: HexColor,
    #[serde(default = "d_surface")]
    pub surface: HexColor,
    #[serde(default = "d_surface_2")]
    pub surface_2: HexColor,

    // ── Primary (teal) ────────────────────────────────────────────────────
    #[serde(default = "d_primary")]
    pub primary: HexColor,
    #[serde(default = "d_primary_dim")]
    pub primary_dim: HexColor,
    #[serde(default = "d_on_primary")]
    pub on_primary: HexColor,

    // ── Accent (amber) ────────────────────────────────────────────────────
    #[serde(default = "d_accent")]
    pub accent: HexColor,
    #[serde(default = "d_accent_dim")]
    pub accent_dim: HexColor,

    // ── Text ──────────────────────────────────────────────────────────────
    #[serde(default = "d_text_high")]
    pub text_high: HexColor,
    #[serde(default = "d_text_med")]
    pub text_med: HexColor,
    #[serde(default = "d_text_dim")]
    pub text_dim: HexColor,

    // ── Status ────────────────────────────────────────────────────────────
    #[serde(default = "d_success")]
    pub success: HexColor,
    #[serde(default = "d_error")]
    pub error: HexColor,
    #[serde(default = "d_warning")]
    pub warning: HexColor,

    // ── Form ───────────────────────────────────────────────────────────────
    #[serde(default = "d_focused_bg")]
    pub focused_bg: HexColor,
    #[serde(default = "d_form_bg")]
    pub form_bg: HexColor,
}

// ---------------------------------------------------------------------------
// Default helpers — serde requires fn pointers, not closures
// ---------------------------------------------------------------------------

fn default_name() -> String {
    "Catppuccin Mocha".to_string()
}
fn hex(s: &str) -> HexColor {
    s.parse().expect("hardcoded hex is valid")
}

fn d_bg() -> HexColor {
    hex("#1e1e2e")
}
fn d_surface() -> HexColor {
    hex("#313244")
}
fn d_surface_2() -> HexColor {
    hex("#45475a")
}
fn d_primary() -> HexColor {
    hex("#cba6f7")
}
fn d_primary_dim() -> HexColor {
    hex("#f5c2e7")
}
fn d_on_primary() -> HexColor {
    hex("#1e1e2e")
}
fn d_accent() -> HexColor {
    hex("#f9e2af")
}
fn d_accent_dim() -> HexColor {
    hex("#fab387")
}
fn d_text_high() -> HexColor {
    hex("#cdd6f4")
}
fn d_text_med() -> HexColor {
    hex("#bac2de")
}
fn d_text_dim() -> HexColor {
    hex("#45475a")
}
fn d_success() -> HexColor {
    hex("#a6e3a1")
}
fn d_error() -> HexColor {
    hex("#f38ba8")
}
fn d_warning() -> HexColor {
    hex("#f9e2af")
}
fn d_focused_bg() -> HexColor {
    hex("#000000")
}
fn d_form_bg() -> HexColor {
    hex("#000000")
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            name: default_name(),
            bg: d_bg(),
            surface: d_surface(),
            surface_2: d_surface_2(),
            primary: d_primary(),
            primary_dim: d_primary_dim(),
            on_primary: d_on_primary(),
            accent: d_accent(),
            accent_dim: d_accent_dim(),
            text_high: d_text_high(),
            text_med: d_text_med(),
            text_dim: d_text_dim(),
            success: d_success(),
            error: d_error(),
            warning: d_warning(),
            focused_bg: d_focused_bg(),
            form_bg: d_form_bg(),
        }
    }
}
