//! Chrome for the floating "panel" popups — the look the calendar *add
//! event* form and the shortcut menu established: no border, a `Clear` plus
//! [`Theme::form_panel_bg`] fill, a heading row, the caller's body, and a
//! compact key-hint line at the bottom. Everything in the form palette.
//!
//! The panel is content-sized and centred in the area it is given: its width
//! fits the widest of body / heading / hint line, its height the requested
//! body rows plus the fixed chrome rows. Callers hand over what they want
//! shown ([`PanelChrome::heading`], [`PanelChrome::hints`],
//! [`PanelChrome::body`]) and get back the body rectangle to draw into:
//!
//! ```ignore
//! let body = PanelChrome::new(Line::from("\u{2726} title"))
//!     .hints(vec![("\u{2191}\u{2193}", "nav"), ("Esc", "close")])
//!     .body(content_width, rows)
//!     .render(frame, area, &theme);
//! if let Some(body) = body {
//!     list.view(frame, body);
//! }
//! ```
//!
//! Every popup in the TUI draws on this chrome — there is no second popup
//! look to keep in sync.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use not_yet_done_ratatui::{LeaderListStyle, LeaderListStyleType};

use crate::ui::theme::Theme;

/// Rows the chrome occupies on top of the body: outer padding (top and
/// bottom), the heading, a gap under it, and a gap plus the hint line above
/// the bottom edge.
const CHROME_ROWS: u16 = 6;

/// Horizontal padding on each side of the body inside the panel.
const PAD_X: u16 = 2;

/// A borderless floating panel in the form palette.
pub struct PanelChrome<'a> {
    heading: Line<'a>,
    hints: Vec<(&'a str, &'a str)>,
    body_width: usize,
    body_rows: u16,
    min_width: u16,
    min_height: u16,
}

impl<'a> PanelChrome<'a> {
    /// A panel with the given heading line, no hints and an empty body.
    pub fn new(heading: Line<'a>) -> Self {
        Self {
            heading,
            hints: Vec::new(),
            body_width: 0,
            body_rows: 0,
            min_width: 36,
            min_height: 8,
        }
    }

    /// Key hints for the bottom line, as `(key, description)` pairs.
    pub fn hints(mut self, hints: Vec<(&'a str, &'a str)>) -> Self {
        self.hints = hints;
        self
    }

    /// How much room the body wants: its natural width in cells and the
    /// number of rows it would like. Both are wishes — the panel never grows
    /// past the area it is rendered into.
    pub fn body(mut self, width: usize, rows: u16) -> Self {
        self.body_width = width;
        self.body_rows = rows;
        self
    }

    /// Smallest panel width, in cells (default 36).
    pub fn min_width(mut self, width: u16) -> Self {
        self.min_width = width;
        self
    }

    /// The panel rectangle this chrome would occupy inside `area`, without
    /// drawing anything. Useful for hit-testing (mouse) and for callers that
    /// need the geometry before deciding what to render.
    pub fn panel_rect(&self, area: Rect) -> Rect {
        let heading_w: usize = self
            .heading
            .spans
            .iter()
            .map(|s| s.content.chars().count())
            .sum();
        let hints_w = hints_width(&self.hints);
        let inner_w = self.body_width.max(heading_w).max(hints_w);
        let panel_w = ((inner_w as u16).saturating_add(2 * PAD_X))
            .max(self.min_width)
            .min(area.width);
        let panel_h = self
            .body_rows
            .saturating_add(CHROME_ROWS)
            .min(area.height)
            .max(self.min_height.min(area.height));

        let x = area.x + area.width.saturating_sub(panel_w) / 2;
        let y = area.y + area.height.saturating_sub(panel_h) / 2;
        Rect::new(x, y, panel_w, panel_h)
    }

    /// Draw the panel (fill, heading, hint line) and return the body area, or
    /// `None` when the area is too small for anything to fit.
    pub fn render(self, frame: &mut Frame, area: Rect, theme: &Theme) -> Option<Rect> {
        let panel = self.panel_rect(area);
        if panel.width == 0 || panel.height == 0 {
            return None;
        }

        // Floating panel: clear behind, then fill (no border).
        frame.render_widget(Clear, panel);
        if let Some(bg) = theme.form_panel_bg() {
            frame.render_widget(Block::default().style(Style::default().bg(bg)), panel);
        }

        let inner = Rect::new(
            panel.x + PAD_X,
            panel.y + 1,
            panel.width.saturating_sub(2 * PAD_X),
            panel.height.saturating_sub(2),
        );
        if inner.height == 0 || inner.width == 0 {
            return None;
        }

        frame.render_widget(Paragraph::new(self.heading), Rect { height: 1, ..inner });

        // Hint line on the last inner row; the body fills the space between
        // the heading (one row of air below it) and a one-row gap above the
        // hints.
        let hints_y = inner.bottom().saturating_sub(1);
        frame.render_widget(
            Paragraph::new(hint_line(&self.hints, theme)),
            Rect::new(inner.x, hints_y, inner.width, 1),
        );

        let body_y = inner.y + 2;
        let body_h = hints_y.saturating_sub(1).saturating_sub(body_y);
        if body_h == 0 {
            return None;
        }
        Some(Rect {
            x: inner.x,
            y: body_y,
            width: inner.width,
            height: body_h,
        })
    }
}

/// Pads a row of spans with blanks so `style` (typically the cursor-row
/// background) reaches the right edge of a `width`-wide body.
pub fn pad_row<'a>(spans: Vec<Span<'a>>, width: u16, style: Style) -> Line<'a> {
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let mut spans = spans;
    let pad = (width as usize).saturating_sub(used);
    if pad > 0 {
        spans.push(Span::styled(" ".repeat(pad), style));
    }
    Line::from(spans)
}

/// The background a cursor row sits on, in the form palette.
pub fn cursor_bg(theme: &Theme) -> ratatui::style::Color {
    theme.form_field_bg().unwrap_or_else(|| theme.surface_2())
}

/// Rendered width of a hint line, in cells.
pub fn hints_width(hints: &[(&str, &str)]) -> usize {
    hints
        .iter()
        .map(|(k, d)| k.chars().count() + 1 + d.chars().count() + 2)
        .sum()
}

/// The [`LeaderListStyle`] every panel popup uses for its list: form palette
/// throughout, the cursor row on the form field background, tagged rows in
/// the warning colour so a batch selection reads at a glance as "staged for
/// an action".
pub fn panel_leader_style(theme: &Theme) -> LeaderListStyle {
    let cursor_bg = cursor_bg(theme);
    LeaderListStyle::new()
        .set_style(
            LeaderListStyleType::Left,
            Style::default().fg(theme.form_text()),
        )
        .set_style(
            LeaderListStyleType::Filler,
            Style::default().fg(theme.form_hint()),
        )
        .set_style(
            LeaderListStyleType::Right,
            Style::default()
                .fg(theme.form_accent())
                .add_modifier(Modifier::BOLD),
        )
        .set_style(LeaderListStyleType::Cursor, Style::default().bg(cursor_bg))
        .set_style(
            LeaderListStyleType::Status,
            Style::default()
                .fg(theme.form_hint())
                .add_modifier(Modifier::ITALIC),
        )
        .set_style(
            LeaderListStyleType::Search,
            Style::default()
                .fg(theme.form_accent())
                .add_modifier(Modifier::BOLD),
        )
        .set_style(
            LeaderListStyleType::Marked,
            Style::default()
                .fg(theme.warning())
                .add_modifier(Modifier::BOLD),
        )
}

/// The compact `key description  key description` line, form palette.
pub fn hint_line<'a>(hints: &[(&'a str, &'a str)], theme: &Theme) -> Line<'a> {
    let key = Style::default()
        .fg(theme.form_accent())
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(theme.form_hint());
    let mut spans: Vec<Span> = Vec::new();
    for (k, d) in hints {
        spans.push(Span::styled(*k, key));
        spans.push(Span::styled(format!(" {d}  "), dim));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThemeConfig;

    fn theme() -> Theme {
        Theme::new(ThemeConfig::default())
    }

    #[test]
    fn panel_is_centred_and_content_sized() {
        let area = Rect::new(0, 0, 100, 40);
        let panel = PanelChrome::new(Line::from("title"))
            .hints(vec![("Esc", "close")])
            .body(50, 10)
            .panel_rect(area);
        assert_eq!(panel.width, 54);
        assert_eq!(panel.height, 16);
        assert_eq!(panel.x, 23);
        assert_eq!(panel.y, 12);
    }

    #[test]
    fn panel_never_exceeds_the_area() {
        let area = Rect::new(0, 0, 20, 6);
        let panel = PanelChrome::new(Line::from("a very long heading indeed"))
            .body(200, 100)
            .panel_rect(area);
        assert_eq!(panel.width, 20);
        assert_eq!(panel.height, 6);
    }

    #[test]
    fn body_area_sits_between_heading_and_hints() {
        let area = Rect::new(0, 0, 60, 20);
        let mut term =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(area.width, area.height))
                .expect("terminal");
        let t = theme();
        let mut body = None;
        term.draw(|f| {
            body = PanelChrome::new(Line::from("\u{2726} title"))
                .hints(vec![("Esc", "close")])
                .body(30, 5)
                .render(f, area, &t);
        })
        .expect("draw");
        let body = body.expect("body area");
        let panel = PanelChrome::new(Line::from("\u{2726} title"))
            .hints(vec![("Esc", "close")])
            .body(30, 5)
            .panel_rect(area);
        assert_eq!(body.height, 5);
        assert_eq!(body.y, panel.y + 3);
        assert_eq!(body.bottom(), panel.bottom() - 3);
    }
}
