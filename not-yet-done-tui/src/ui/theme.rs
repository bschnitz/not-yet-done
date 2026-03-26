use ratatui::style::Color;

use crate::config::ThemeConfig;

/// Runtime theme — wraps `ThemeConfig` and exposes `ratatui::Color` values.
///
/// Every widget receives `&Theme` instead of referencing `Theme::CONSTANT`,
/// which allows the theme to change at startup based on `tui-theme.yaml`.
pub struct Theme {
    cfg: ThemeConfig,
}

impl Theme {
    pub fn new(cfg: ThemeConfig) -> Self {
        Self { cfg }
    }

    // ── Surfaces ──────────────────────────────────────────────────────────
    pub fn bg(&self) -> Color {
        self.cfg.bg.to_ratatui()
    }
    pub fn surface(&self) -> Color {
        self.cfg.surface.to_ratatui()
    }
    pub fn surface_2(&self) -> Color {
        self.cfg.surface_2.to_ratatui()
    }

    // ── Primary ───────────────────────────────────────────────────────────
    pub fn primary(&self) -> Color {
        self.cfg.primary.to_ratatui()
    }
    pub fn primary_dim(&self) -> Color {
        self.cfg.primary_dim.to_ratatui()
    }
    pub fn on_primary(&self) -> Color {
        self.cfg.on_primary.to_ratatui()
    }

    // ── Accent ────────────────────────────────────────────────────────────
    pub fn accent(&self) -> Color {
        self.cfg.accent.to_ratatui()
    }
    #[allow(dead_code)]
    pub fn accent_dim(&self) -> Color {
        self.cfg.accent_dim.to_ratatui()
    }

    // ── Text ──────────────────────────────────────────────────────────────
    pub fn text_high(&self) -> Color {
        self.cfg.text_high.to_ratatui()
    }
    pub fn text_med(&self) -> Color {
        self.cfg.text_med.to_ratatui()
    }
    pub fn text_dim(&self) -> Color {
        self.cfg.text_dim.to_ratatui()
    }

    // ── Status ────────────────────────────────────────────────────────────
    #[allow(dead_code)]
    pub fn success(&self) -> Color {
        self.cfg.success.to_ratatui()
    }
    #[allow(dead_code)]
    pub fn error(&self) -> Color {
        self.cfg.error.to_ratatui()
    }
    #[allow(dead_code)]
    pub fn warning(&self) -> Color {
        self.cfg.warning.to_ratatui()
    }

    // ── Form ───────────────────────────────────────────────────────────────
    pub fn focused_bg(&self) -> Color {
        self.cfg.focused_bg.to_ratatui()
    }
    pub fn form_bg(&self) -> Color {
        self.cfg.form_bg.to_ratatui()
    }
}
