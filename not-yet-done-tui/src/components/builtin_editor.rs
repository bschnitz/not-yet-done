//! The builtin editor pane: an in-process modal editor hosted by the TUI.
//!
//! An editor profile with `builtin: true` (see
//! [`crate::config::editor::EditorConfig`]) edits here instead of spawning
//! `$EDITOR`. The pane is laid out at the bottom of the screen, above the
//! message bars, and swallows every key while open.
//!
//! This module is the seam between the App and [`vimrealm`]: it owns the
//! geometry, maps the [`Theme`] onto the editor's style slots, and turns the
//! editor's [`VimEvent`]s into the same three things the external-editor
//! lifecycle already knows — an intermediate save, a save-and-close, and a
//! cancel. Everything modal lives in the crate; nothing here knows what a
//! motion is.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use tuirealm::component::Component;
use vimrealm::{VimEditor, VimEvent, VimStyle, VimStyleType};

use crate::ui::theme::Theme;

/// What a keypress did to the pane. Mirrors the external editor's
/// lifecycle so the App can reuse `live_apply` / `commit` unchanged.
pub enum BuiltinEditorOutcome {
    /// The editor handled the key; nothing for the App to do.
    Consumed,
    /// `:w` — persist and keep editing (the session's `live_apply`).
    Save(String),
    /// `:wq` / `:x` — persist and close (the session's `commit`).
    SaveAndClose(String),
    /// `:q` on a clean buffer, or `:q!` — close, discarding the buffer.
    Cancel,
}

/// How tall the pane should be, as configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaneHeight {
    /// A share of the terminal height, in percent.
    Percent(u16),
    /// A fixed number of rows.
    Rows(u16),
}

/// Smallest pane worth drawing: border, one text row, status line.
const MIN_HEIGHT: u16 = 4;

/// Parse the profile's `height` field. `"30%"` is a share of the terminal,
/// a bare number is a row count; anything unparseable falls back to the
/// default share rather than failing an edit the user just asked for.
fn parse_height(spec: &str) -> PaneHeight {
    let spec = spec.trim();
    match spec.strip_suffix('%') {
        Some(pct) => pct
            .trim()
            .parse::<u16>()
            .map(|p| PaneHeight::Percent(p.clamp(1, 100)))
            .unwrap_or(PaneHeight::Percent(40)),
        None => spec
            .parse::<u16>()
            .map(|rows| PaneHeight::Rows(rows.max(MIN_HEIGHT)))
            .unwrap_or(PaneHeight::Percent(40)),
    }
}

/// Map the app theme onto the editor's style slots. Every colour comes from
/// the theme — either from the optional `vim:` block in `tui-theme.yaml` or,
/// role by role, from the app palette it falls back to. The crate's own
/// fallbacks are modifier-only, so a slot left unset would inherit the
/// surrounding palette instead of a hardcoded hue.
fn style_from_theme(theme: &Theme) -> VimStyle {
    let bg = theme.vim_bg();
    VimStyle::new()
        .with(
            VimStyleType::Text,
            Style::default().fg(theme.vim_text()).bg(bg),
        )
        .with(VimStyleType::Cursor, cursor_style(theme, bg))
        // A selection gets a background rather than a reverse, so the block
        // cursor inside it stays the brighter of the two.
        .with(
            VimStyleType::Selection,
            Style::default()
                .fg(theme.vim_selection())
                .bg(theme.vim_selection_bg()),
        )
        .with(
            VimStyleType::Gutter,
            Style::default().fg(theme.vim_gutter()).bg(bg),
        )
        .with(
            VimStyleType::Mode,
            Style::default()
                .fg(theme.vim_mode())
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        )
        .with(
            VimStyleType::Status,
            Style::default().fg(theme.vim_status()).bg(bg),
        )
        .with(
            VimStyleType::CommandLine,
            Style::default().fg(theme.vim_command_line()).bg(bg),
        )
}

/// The block cursor. Unconfigured it *reverses* whatever it sits on — that
/// keeps it visible on the pane background and inside a selection alike, so it
/// only needs a background to reverse into. Naming either cursor colour opts
/// out of that and paints the pair as given, reverse included would then just
/// swap the two back.
fn cursor_style(theme: &Theme, bg: ratatui::style::Color) -> Style {
    match (theme.vim_cursor(), theme.vim_cursor_bg()) {
        (None, None) => Style::default().bg(bg).add_modifier(Modifier::REVERSED),
        (fg, cursor_bg) => Style::default()
            .fg(fg.unwrap_or(bg))
            .bg(cursor_bg.unwrap_or_else(|| theme.vim_text())),
    }
}

/// The mounted editor pane.
pub struct BuiltinEditorPane {
    pub editor: VimEditor,
    height: PaneHeight,
}

impl BuiltinEditorPane {
    /// Mount an editor for `content`. `label` names the action being
    /// edited (`"edit"`, `"new"`) and ends up in the pane's title.
    pub fn new(
        theme: &Theme,
        label: &str,
        content: &str,
        height_spec: &str,
        line_numbers: bool,
    ) -> Self {
        Self {
            editor: VimEditor::default()
                .with_text(content)
                .with_title(format!(" {label} — :wq save and close · :q! discard "))
                .with_line_numbers(line_numbers)
                .with_style(style_from_theme(theme)),
            height: parse_height(height_spec),
        }
    }

    /// Rows the pane wants out of a terminal `available` rows tall. Never
    /// takes the whole screen: the content view keeps at least a third, so
    /// the row being edited stays visible above the pane.
    pub fn required_height(&self, available: u16) -> u16 {
        if available <= MIN_HEIGHT {
            return 0;
        }
        let wanted = match self.height {
            PaneHeight::Percent(p) => (u32::from(available) * u32::from(p) / 100) as u16,
            PaneHeight::Rows(rows) => rows,
        };
        let cap = available - available / 3;
        wanted.clamp(MIN_HEIGHT, cap)
    }

    /// Feed one canonical key string (the App's key pipeline) to the
    /// editor. Keys the App's own `&str` encoding cannot express are
    /// swallowed rather than leaking into global dispatch — while the pane
    /// is open it owns the keyboard.
    pub fn handle_key(&mut self, key: &str) -> BuiltinEditorOutcome {
        let Some(ev) = crate::events::key_string_to_tuirealm(key) else {
            return BuiltinEditorOutcome::Consumed;
        };
        match self.editor.on_key(ev) {
            Some(VimEvent::Save) => BuiltinEditorOutcome::Save(self.editor.text()),
            Some(VimEvent::SaveAndClose) => BuiltinEditorOutcome::SaveAndClose(self.editor.text()),
            Some(VimEvent::Cancel) => BuiltinEditorOutcome::Cancel,
            Some(VimEvent::Changed) | None => BuiltinEditorOutcome::Consumed,
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        self.editor.view(frame, area);
    }

    /// Show `msg` on the editor's own status line — used for outcomes the
    /// user needs to see *without* the pane closing (a failed live save).
    pub fn set_message(&mut self, msg: impl Into<String>) {
        self.editor.set_message(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThemeConfig;

    fn pane(height: &str) -> BuiltinEditorPane {
        let theme = Theme::new(ThemeConfig::default());
        BuiltinEditorPane::new(&theme, "edit", "hello", height, false)
    }

    #[test]
    fn a_percentage_is_a_share_of_the_terminal() {
        assert_eq!(parse_height("30%"), PaneHeight::Percent(30));
        assert_eq!(parse_height(" 30 % "), PaneHeight::Percent(30));
        assert_eq!(pane("30%").required_height(40), 12);
    }

    #[test]
    fn a_bare_number_is_a_row_count() {
        assert_eq!(parse_height("12"), PaneHeight::Rows(12));
        assert_eq!(pane("12").required_height(40), 12);
    }

    #[test]
    fn nonsense_falls_back_instead_of_refusing_to_open() {
        assert_eq!(parse_height("tall"), PaneHeight::Percent(40));
        assert_eq!(parse_height(""), PaneHeight::Percent(40));
    }

    #[test]
    fn the_pane_never_swallows_the_whole_screen() {
        // 90 % of 30 rows would leave the content view 3 rows; the cap
        // reserves a third of the terminal for it.
        assert_eq!(pane("90%").required_height(30), 20);
        assert_eq!(pane("100").required_height(30), 20);
    }

    #[test]
    fn a_terminal_too_short_to_host_the_pane_gets_no_pane() {
        assert_eq!(pane("50%").required_height(4), 0);
        // …and an almost-too-short one gets the minimum, not a sliver.
        assert_eq!(pane("5%").required_height(20), MIN_HEIGHT);
    }

    #[test]
    fn ex_commands_map_onto_the_editor_lifecycle() {
        let mut p = pane("30%");
        for key in ["i", "h", "i", "esc"] {
            assert!(matches!(p.handle_key(key), BuiltinEditorOutcome::Consumed));
        }
        assert!(matches!(p.handle_key(":"), BuiltinEditorOutcome::Consumed));
        assert!(matches!(p.handle_key("w"), BuiltinEditorOutcome::Consumed));
        match p.handle_key("enter") {
            BuiltinEditorOutcome::Save(text) => assert_eq!(text, "hihello"),
            _ => panic!("`:w` must report an intermediate save"),
        }
        for key in [":", "w", "q"] {
            p.handle_key(key);
        }
        match p.handle_key("enter") {
            BuiltinEditorOutcome::SaveAndClose(text) => assert_eq!(text, "hihello"),
            _ => panic!("`:wq` must report a save-and-close"),
        }
    }

    #[test]
    fn a_discard_reports_a_cancel() {
        let mut p = pane("30%");
        for key in [":", "q", "!"] {
            p.handle_key(key);
        }
        assert!(matches!(
            p.handle_key("enter"),
            BuiltinEditorOutcome::Cancel
        ));
    }

    #[test]
    fn keys_the_app_cannot_encode_do_not_escape_the_pane() {
        let mut p = pane("30%");
        assert!(matches!(
            p.handle_key("ctrl+alt+nonsense"),
            BuiltinEditorOutcome::Consumed
        ));
    }

    #[test]
    fn an_empty_vim_block_keeps_the_app_palette() {
        let theme = Theme::new(ThemeConfig::default());
        let style = style_from_theme(&theme);
        let text = style.get(VimStyleType::Text).expect("text is mapped");
        assert_eq!(text.fg, Some(theme.text_high()));
        assert_eq!(text.bg, Some(theme.surface()));
        let cursor = style.get(VimStyleType::Cursor).expect("cursor is mapped");
        assert!(
            cursor.add_modifier.contains(Modifier::REVERSED),
            "unconfigured, the cursor reverses what it sits on"
        );
    }

    #[test]
    fn a_configured_role_overrides_only_itself() {
        let mut cfg = ThemeConfig::default();
        cfg.vim.selection_bg = Some("#504945".parse().unwrap());
        let theme = Theme::new(cfg);
        let style = style_from_theme(&theme);
        assert_eq!(
            style.get(VimStyleType::Selection).and_then(|s| s.bg),
            Some(ratatui::style::Color::Rgb(0x50, 0x49, 0x45))
        );
        assert_eq!(
            style.get(VimStyleType::Text).and_then(|s| s.bg),
            Some(Theme::new(ThemeConfig::default()).surface()),
            "the other roles keep their fallbacks"
        );
    }

    #[test]
    fn naming_a_cursor_colour_drops_the_reverse() {
        let mut cfg = ThemeConfig::default();
        cfg.vim.cursor_bg = Some("#fabd2f".parse().unwrap());
        let style = style_from_theme(&Theme::new(cfg));
        let cursor = style.get(VimStyleType::Cursor).expect("cursor is mapped");
        assert!(!cursor.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(
            cursor.bg,
            Some(ratatui::style::Color::Rgb(0xfa, 0xbd, 0x2f))
        );
    }
}
