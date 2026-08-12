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

    // ── Secondary / Tertiary accents ────────────────────────────────────
    pub fn secondary(&self) -> Color {
        self.cfg.secondary.to_ratatui()
    }
    pub fn tertiary(&self) -> Color {
        self.cfg.tertiary.to_ratatui()
    }

    // ── Tree ─────────────────────────────────────────────────────────────
    pub fn tree_connector(&self) -> Color {
        self.cfg.tree_connector.to_ratatui()
    }

    // ── Tracking taskpath ────────────────────────────────────────────────
    pub fn taskpath_separator(&self) -> Color {
        self.cfg.taskpath_separator.to_ratatui()
    }

    // ── Grouping ─────────────────────────────────────────────────────────
    /// Accent for group-header rows and the grand-total footer in grouped
    /// content views (M3).
    pub fn group_header(&self) -> Color {
        self.cfg.group_header.to_ratatui()
    }

    // ── Unread ───────────────────────────────────────────────────────────
    /// Accent for unread chat items: channel/category names with unread
    /// messages, and the header line of an unread message in the list.
    pub fn unread(&self) -> Color {
        self.cfg.unread.to_ratatui()
    }

    // ── Card mode ────────────────────────────────────────────────────────
    /// Frame glyphs around a card in card mode.
    pub fn card_border(&self) -> Color {
        self.cfg.card_border.to_ratatui()
    }
    /// Field labels inside a card.
    pub fn card_label(&self) -> Color {
        self.cfg.card_label.to_ratatui()
    }

    // ── Tab bar ─────────────────────────────────────────────────────────
    pub fn tab_active(&self) -> Color {
        self.cfg.tab_active.to_ratatui()
    }
    pub fn tab_active_bg(&self) -> Color {
        self.cfg.tab_active_bg.to_ratatui()
    }
    pub fn sub_tab_active(&self) -> Color {
        self.cfg.sub_tab_active.to_ratatui()
    }
    pub fn sub_tab_active_bg(&self) -> Color {
        self.cfg.sub_tab_active_bg.to_ratatui()
    }

    // ── Toolbar ──────────────────────────────────────────────────────────
    pub fn toolbar_bg(&self) -> Color {
        self.cfg.toolbar_bg.to_ratatui()
    }

    // ── Form ───────────────────────────────────────────────────────────────
    pub fn focused_bg(&self) -> Color {
        self.cfg.focused_bg.to_ratatui()
    }
    pub fn form_bg(&self) -> Color {
        self.cfg.form_bg.to_ratatui()
    }

    // ── Spec-form popup palette (each role overridable, else app-theme) ──────
    /// Focused label / prefix / cursor accent. Falls back to [`Self::primary`].
    pub fn form_accent(&self) -> Color {
        self.override_or(self.cfg.form.accent.as_ref(), || self.primary())
    }
    /// Unfocused label colour. Falls back to [`Self::text_dim`].
    pub fn form_label_idle(&self) -> Color {
        self.override_or(self.cfg.form.label_idle.as_ref(), || self.text_dim())
    }
    /// Focused input text. Falls back to [`Self::text_high`].
    pub fn form_text(&self) -> Color {
        self.override_or(self.cfg.form.text.as_ref(), || self.text_high())
    }
    /// Unfocused input text / option text. Falls back to [`Self::text_med`].
    pub fn form_text_idle(&self) -> Color {
        self.override_or(self.cfg.form.text_idle.as_ref(), || self.text_med())
    }
    /// Placeholder text. Falls back to [`Self::text_dim`].
    pub fn form_placeholder(&self) -> Color {
        self.override_or(self.cfg.form.placeholder.as_ref(), || self.text_dim())
    }
    /// Picked option / checked toggle marker. Falls back to [`Self::success`].
    pub fn form_selected(&self) -> Color {
        self.override_or(self.cfg.form.selected.as_ref(), || self.success())
    }
    /// Footer / preview hint text. Falls back to [`Self::text_dim`].
    pub fn form_hint(&self) -> Color {
        self.override_or(self.cfg.form.hint.as_ref(), || self.text_dim())
    }
    /// Error text. Falls back to [`Self::error`].
    pub fn form_error(&self) -> Color {
        self.override_or(self.cfg.form.error.as_ref(), || self.error())
    }
    /// Fill behind the focused field (the bar). `None` → no bar (the default).
    pub fn form_field_bg(&self) -> Option<Color> {
        self.cfg.form.field_bg.as_ref().map(|c| c.to_ratatui())
    }
    /// Fill behind unfocused fields. Usually `None`.
    pub fn form_field_bg_idle(&self) -> Option<Color> {
        self.cfg.form.field_bg_idle.as_ref().map(|c| c.to_ratatui())
    }
    /// Fill behind the floating panel (the `event_form` look). Falls back to
    /// [`Self::form_bg`] so a panel always has a solid backdrop.
    pub fn form_panel_bg(&self) -> Option<Color> {
        Some(
            self.cfg
                .form
                .panel_bg
                .as_ref()
                .map(|c| c.to_ratatui())
                .unwrap_or_else(|| self.form_bg()),
        )
    }

    // ── Built-in vim editor pane (each role overridable, else app-theme) ────
    /// Background of the pane and the surface the other roles sit on. Falls
    /// back to [`Self::surface`].
    pub fn vim_bg(&self) -> Color {
        self.override_or(self.cfg.vim.bg.as_ref(), || self.surface())
    }
    /// Buffer text. Falls back to [`Self::text_high`].
    pub fn vim_text(&self) -> Color {
        self.override_or(self.cfg.vim.text.as_ref(), || self.text_high())
    }
    /// The character under the block cursor. `None` → draw it reversed.
    pub fn vim_cursor(&self) -> Option<Color> {
        self.cfg.vim.cursor.as_ref().map(|c| c.to_ratatui())
    }
    /// The block cursor itself. `None` → draw it reversed.
    pub fn vim_cursor_bg(&self) -> Option<Color> {
        self.cfg.vim.cursor_bg.as_ref().map(|c| c.to_ratatui())
    }
    /// Line-number gutter. Falls back to [`Self::text_dim`].
    pub fn vim_gutter(&self) -> Color {
        self.override_or(self.cfg.vim.gutter.as_ref(), || self.text_dim())
    }
    /// Mode indicator. Falls back to [`Self::accent`].
    pub fn vim_mode(&self) -> Color {
        self.override_or(self.cfg.vim.mode.as_ref(), || self.accent())
    }
    /// Status line. Falls back to [`Self::text_med`].
    pub fn vim_status(&self) -> Color {
        self.override_or(self.cfg.vim.status.as_ref(), || self.text_med())
    }
    /// Command line and its messages. Falls back to [`Self::primary`].
    pub fn vim_command_line(&self) -> Color {
        self.override_or(self.cfg.vim.command_line.as_ref(), || self.primary())
    }
    /// Text inside a selection. Falls back to [`Self::text_high`].
    pub fn vim_selection(&self) -> Color {
        self.override_or(self.cfg.vim.selection.as_ref(), || self.text_high())
    }
    /// Fill behind a selection. Falls back to [`Self::focused_bg`].
    pub fn vim_selection_bg(&self) -> Color {
        self.override_or(self.cfg.vim.selection_bg.as_ref(), || self.focused_bg())
    }

    /// Resolve an optional per-role override, else compute the app-theme
    /// fallback lazily.
    fn override_or(
        &self,
        override_color: Option<&crate::config::color::HexColor>,
        fallback: impl Fn() -> Color,
    ) -> Color {
        match override_color {
            Some(c) => c.to_ratatui(),
            None => fallback(),
        }
    }

    // ── Alert bar ────────────────────────────────────────────────────────
    pub fn alert_fg(&self) -> Color {
        self.cfg.alert_fg.to_ratatui()
    }
    pub fn alert_bg(&self) -> Color {
        self.cfg.alert_bg.to_ratatui()
    }
}
