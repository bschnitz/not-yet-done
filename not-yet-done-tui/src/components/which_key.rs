//! "Which-key" chord-completion preview.
//!
//! While a multi-step chord is half-typed (`App::pending_key` is `Some`),
//! this borderless popup lists every binding that continues the pressed
//! prefix — action name on the left, the full combo on the right, in the
//! same visual style as the Ctrl+Y shortcut menu.
//!
//! It is *passive*: it captures no keys. The pressed keys keep flowing
//! through the normal chord dispatcher in [`crate::app::App::handle_key`],
//! so completing the chord runs its action and an unmapped key aborts it.
//! The popup is a pure view over the pending prefix and closes as soon as
//! the chord resolves (see [`crate::app::App::reconcile_which_key`]).

use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use tuirealm::component::Component;
use tuirealm::props::{AttrValue, Attribute};

use not_yet_done_ratatui::{LeaderList, LeaderListStyle, LeaderListStyleType};

use crate::ui::theme::Theme;

/// The chord-preview popup. Holds the (name, combo) rows for the current
/// pending prefix and a passive [`LeaderList`] rendering them.
pub struct WhichKeyMenu {
    theme: Arc<Theme>,
    open: bool,
    /// The surface form of the chord typed so far (e.g. `g` or `g l`).
    prefix: String,
    /// (action name, full combo) pairs that continue `prefix`.
    rows: Vec<(String, String)>,
    list: LeaderList,
}

impl WhichKeyMenu {
    pub fn new(theme: Arc<Theme>) -> Self {
        Self {
            theme,
            open: false,
            prefix: String::new(),
            rows: Vec::new(),
            list: LeaderList::default(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// Show (or refresh) the popup for `prefix` with the given continuation
    /// rows. Called both on the reveal timer and on every subsequent chord
    /// step so the list narrows live as the user types deeper.
    pub fn open(&mut self, prefix: String, rows: Vec<(String, String)>) {
        self.prefix = prefix;
        self.rows = rows;
        self.rebuild_list();
        self.open = true;
    }

    /// Mirror the shortcut menu's form-palette styling so the two popups
    /// look like siblings.
    fn style(&self) -> LeaderListStyle {
        let t = &self.theme;
        LeaderListStyle::new()
            .set_style(
                LeaderListStyleType::Left,
                Style::default().fg(t.form_text()),
            )
            .set_style(
                LeaderListStyleType::Filler,
                Style::default().fg(t.form_hint()),
            )
            .set_style(
                LeaderListStyleType::Right,
                Style::default()
                    .fg(t.form_accent())
                    .add_modifier(Modifier::BOLD),
            )
    }

    fn rebuild_list(&mut self) {
        let entries = self.rows.clone();
        let mut list = LeaderList::default()
            .with_entries(entries)
            .with_affixes(" ", ".", " ")
            .with_selectable(false)
            .with_status_line(false)
            .with_search(false)
            .with_style(self.style());
        list.attr(Attribute::Focus, AttrValue::Flag(true));
        self.list = list;
    }

    /// Borderless floating panel: `Clear` + `form_panel_bg` fill, a
    /// `✦ {prefix}…` heading, the passive list, and a compact hint line.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if !self.open || self.rows.is_empty() {
            return;
        }
        let t = Arc::clone(&self.theme);
        let count = self.rows.len();

        let content_w = self.list.min_width() as usize;
        let heading_text = format!("\u{2726} {}\u{2026}", self.prefix);
        let heading_w = heading_text.chars().count();
        let hint = "unmapped key cancels";
        let hint_w = hint.chars().count();

        let inner_w_needed = content_w.max(heading_w).max(hint_w);
        let panel_w = ((inner_w_needed as u16) + 4).max(28).min(area.width);
        // heading(1) + gap(1) + list(count) + gap(1) + hint(1) + pad(2).
        let wanted_h = count as u16 + 6;
        let panel_h = wanted_h.min(area.height).max(6);

        let px = area.x + area.width.saturating_sub(panel_w) / 2;
        let py = area.y + area.height.saturating_sub(panel_h) / 2;
        let panel = Rect::new(px, py, panel_w, panel_h);

        frame.render_widget(Clear, panel);
        if let Some(bg) = t.form_panel_bg() {
            frame.render_widget(Block::default().style(Style::default().bg(bg)), panel);
        }

        let inner = Rect::new(
            panel.x + 2,
            panel.y + 1,
            panel.width.saturating_sub(4),
            panel.height.saturating_sub(2),
        );
        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let heading = Line::from(vec![Span::styled(
            heading_text,
            Style::default()
                .fg(t.form_accent())
                .add_modifier(Modifier::BOLD),
        )]);
        frame.render_widget(Paragraph::new(heading), Rect { height: 1, ..inner });

        let hint_y = inner.bottom().saturating_sub(1);
        let list_y = inner.y + 2;
        let list_h = hint_y.saturating_sub(1).saturating_sub(list_y);
        if list_h > 0 {
            let list_area = Rect {
                x: inner.x,
                y: list_y,
                width: inner.width,
                height: list_h,
            };
            self.list.view(frame, list_area);
        }

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                hint,
                Style::default()
                    .fg(t.form_hint())
                    .add_modifier(Modifier::ITALIC),
            ))),
            Rect::new(inner.x, hint_y, inner.width, 1),
        );
    }
}
