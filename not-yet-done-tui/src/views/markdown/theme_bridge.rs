//! Bridge between `ratatui-markdown`'s [`RichTextTheme`] and our own
//! [`Theme`]. A newtype wrapper so the markdown renderer pulls every color
//! from the user's `tui-theme.yaml` instead of hardcoding any (see the
//! project's no-hardcoded-colors rule).
//!
//! `RichTextTheme` exposes ~15 named color slots; we map each onto the
//! closest existing [`Theme`] role. The `json_*` / `accent_yellow` slots are
//! only consulted by the tree/code renderers (which we don't enable — the
//! dependency is `default-features = false, features = ["markdown"]`), but the
//! trait still requires them, so they get sensible mappings too.

use ratatui::style::Color;
use ratatui_markdown::theme::{Generation, RichTextTheme};

use crate::ui::theme::Theme;

/// Wraps a borrowed [`Theme`] so it can be handed to
/// [`ratatui_markdown::markdown::MarkdownRenderer::render`].
pub struct MdTheme<'a>(pub &'a Theme);

impl RichTextTheme for MdTheme<'_> {
    /// Cache generation. We construct a fresh `MarkdownRenderer` per table
    /// rebuild and never reuse rendered blocks across theme changes, so the
    /// renderer's generation-keyed cache is irrelevant here — a constant is
    /// correct and avoids needing a mutable/global counter.
    fn generation(&self) -> Generation {
        Generation(1)
    }

    // ── Text ────────────────────────────────────────────────────────────
    fn get_text_color(&self) -> Color {
        self.0.text_high()
    }
    fn get_muted_text_color(&self) -> Color {
        self.0.text_dim()
    }

    // ── Structural / emphasis ───────────────────────────────────────────
    fn get_primary_color(&self) -> Color {
        self.0.primary()
    }
    fn get_secondary_color(&self) -> Color {
        self.0.secondary()
    }
    fn get_info_color(&self) -> Color {
        self.0.accent()
    }

    // ── Borders / selection ─────────────────────────────────────────────
    fn get_border_color(&self) -> Color {
        self.0.tree_connector()
    }
    fn get_focused_border_color(&self) -> Color {
        self.0.primary()
    }
    fn get_popup_selected_background(&self) -> Color {
        self.0.surface_2()
    }

    // ── JSON/TOML tree slots (unused with markdown-only features) ────────
    fn get_json_key_color(&self) -> Color {
        self.0.accent()
    }
    fn get_json_string_color(&self) -> Color {
        self.0.success()
    }
    fn get_json_number_color(&self) -> Color {
        self.0.tertiary()
    }
    fn get_json_bool_color(&self) -> Color {
        self.0.primary()
    }
    fn get_json_null_color(&self) -> Color {
        self.0.text_dim()
    }
    fn get_accent_yellow(&self) -> Color {
        self.0.accent()
    }
}
