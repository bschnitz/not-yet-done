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
}

// ---------------------------------------------------------------------------
// Default helpers — serde requires fn pointers, not closures
// ---------------------------------------------------------------------------

fn default_name() -> String {
    "Teal Dark".to_string()
}
fn hex(s: &str) -> HexColor {
    s.parse().expect("hardcoded hex is valid")
}

fn d_bg() -> HexColor {
    hex("#121212")
}
fn d_surface() -> HexColor {
    hex("#1e1e1e")
}
fn d_surface_2() -> HexColor {
    hex("#282828")
}
fn d_primary() -> HexColor {
    hex("#4db6ac")
}
fn d_primary_dim() -> HexColor {
    hex("#26a69a")
}
fn d_on_primary() -> HexColor {
    hex("#002522")
}
fn d_accent() -> HexColor {
    hex("#ffca28")
}
fn d_accent_dim() -> HexColor {
    hex("#ffb300")
}
fn d_text_high() -> HexColor {
    hex("#e6e6e6")
}
fn d_text_med() -> HexColor {
    hex("#9e9e9e")
}
fn d_text_dim() -> HexColor {
    hex("#616161")
}
fn d_success() -> HexColor {
    hex("#66bb6a")
}
fn d_error() -> HexColor {
    hex("#ef5350")
}
fn d_warning() -> HexColor {
    hex("#ffa726")
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
        }
    }
}
