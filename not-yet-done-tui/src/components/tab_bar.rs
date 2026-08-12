//! TabBar component: top-level tab navigation with sub-view indicators.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};

use tuirealm::command::{Cmd, CmdResult};
use tuirealm::component::Component;
use tuirealm::props::{AttrValue, Attribute, QueryResult};
use tuirealm::state::{State, StateValue};

use crate::tabs::{MainTab, Tab, digit_for_index};
use crate::ui::theme::Theme;
use std::sync::Arc;

/// A single bar segment: label text + whether it's the active item.
struct BarItem {
    label: String,
    active: bool,
    /// Style patch for an unread tab, layered over the active/inactive
    /// style at render time. `None` for read tabs and for every subtab.
    unread: Option<Style>,
}

pub struct TabBarComponent {
    active_tab: Tab,
    content_count: usize,
    theme: Arc<Theme>,
    main_tab_labels: Vec<MainTab>,
    /// Per content-tab subtab labels with their active flag. Outer index
    /// is the Content tab index; inner is (label, is_active). Pushed in
    /// each frame by App from the corresponding `ContentView`.
    content_sub_tabs: Vec<Vec<(String, bool)>>,
    /// When true the subtab bar always occupies its own line beneath the
    /// main tabs, regardless of whether it would fit on the first line.
    /// Driven by `tabs.subtabs_own_line` in tui.yaml.
    subtabs_own_line: bool,
}

/// Info about a content tab for display in the tab bar.
pub struct ContentTabInfo {
    pub name: String,
    pub icon: String,
}

impl TabBarComponent {
    pub fn new(theme: Arc<Theme>, content_tabs: &[ContentTabInfo], subtabs_own_line: bool) -> Self {
        use crate::tabs::tab_label;
        // Initial labels use the natural slot autonumber; App overwrites
        // them each frame from the active layout via `set_main_tab_labels`.
        let mut main_tab_labels: Vec<MainTab> = Vec::new();
        for (i, info) in content_tabs.iter().enumerate() {
            let kb = digit_for_index(i)
                .map(|c| c.to_string())
                .unwrap_or_default();
            main_tab_labels.push(MainTab {
                tab: Tab::Content(i),
                label: tab_label(&info.icon, &kb, &info.name),
                unread: None,
            });
        }

        Self {
            active_tab: Tab::Content(0),
            content_count: content_tabs.len(),
            theme,
            main_tab_labels,
            content_sub_tabs: vec![Vec::new(); content_tabs.len()],
            subtabs_own_line,
        }
    }

    /// Push the dynamic subtab labels for one content tab. Called by App
    /// each frame from the corresponding ContentView.
    pub fn set_content_sub_tabs(&mut self, content_idx: usize, labels: Vec<(String, bool)>) {
        if let Some(slot) = self.content_sub_tabs.get_mut(content_idx) {
            *slot = labels;
        }
    }

    pub fn set_active_tab(&mut self, tab: Tab) {
        self.active_tab = tab;
    }

    /// Replace the main-tab label list (tab + rendered label incl. its
    /// key hint and unread emphasis). The App pushes this each frame from
    /// the active [`TabLayout`](crate::tabs::TabLayout) so the bar shows
    /// only the visible tabs, in configured order, with autonumber keys —
    /// and so an adapter that just went unread is emphasised on the very
    /// next frame, without the bar tracking any state of its own.
    pub fn set_main_tab_labels(&mut self, labels: Vec<MainTab>) {
        self.main_tab_labels = labels;
    }

    fn bar_width(items: &[BarItem]) -> usize {
        // Each item: border + "  label  " + border + 1 gap = label.chars() + 7
        // Inactive: "  label  " + 1 gap = label.chars() + 5
        // Rough: sum of display widths + 7 per item. Display width (not
        // `chars().count()`) so 2-cell emoji icons are measured correctly
        // for the one-line-vs-two-line decision.
        use unicode_width::UnicodeWidthStr;
        items.iter().map(|it| it.label.width() + 7).sum()
    }

    fn main_items(&self) -> Vec<BarItem> {
        self.main_tab_labels
            .iter()
            .map(|mt| BarItem {
                label: mt.label.clone(),
                active: mt.tab == self.active_tab,
                unread: mt.unread,
            })
            .collect()
    }

    fn sub_items(&self) -> Vec<BarItem> {
        let Tab::Content(idx) = self.active_tab;
        match self.content_sub_tabs.get(idx) {
            Some(labels) if !labels.is_empty() => labels
                .iter()
                .map(|(lab, active)| BarItem {
                    label: lab.clone(),
                    active: *active,
                    unread: None,
                })
                .collect(),
            _ => vec![BarItem {
                label: "tickets".to_string(),
                active: true,
                unread: None,
            }],
        }
    }

    pub fn required_height(&self, available_width: u16) -> u16 {
        if self.subtabs_own_line {
            return 2;
        }
        let main = Self::bar_width(&self.main_items());
        let sub = Self::bar_width(&self.sub_items()) + 4; // " 🮇▎ " separator
        if main + sub <= available_width as usize {
            1
        } else {
            2
        }
    }

    /// Render a bar (list of items) on a uniform background.
    /// Returns the x position after the last item.
    fn render_bar(
        buf: &mut ratatui::buffer::Buffer,
        items: &[BarItem],
        mut x: u16,
        y: u16,
        right: u16,
        bar_bg: ratatui::style::Color,
        active_fg: ratatui::style::Color,
        active_bg: Option<ratatui::style::Color>,
        inactive_fg: ratatui::style::Color,
    ) -> u16 {
        for item in items {
            if x >= right {
                break;
            }
            let body = format!("  {}  ", item.label);
            let mut style = if item.active {
                Style::default()
                    .fg(active_fg)
                    .bg(active_bg.unwrap_or(bar_bg))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(inactive_fg).bg(bar_bg)
            };
            // Unread emphasis is a *patch*, not a replacement: it adds the
            // configured modifiers (and only recolors when the config names
            // a color), so the bar's own active/inactive palette survives.
            if let Some(patch) = item.unread {
                style = style.patch(patch);
            }
            // `set_stringn` is grapheme- and display-width-aware: a 2-cell
            // emoji (incl. VS16 sequences like ⏱️) advances `x` by 2 and
            // blanks its trailing half, so labels after an emoji stay
            // aligned. A naive `x += 1` per `char` would misplace them.
            let max = right.saturating_sub(x) as usize;
            let (nx, _) = buf.set_stringn(x, y, &body, max, style);
            x = nx;
        }
        x
    }
}

impl Component for TabBarComponent {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        if area.height == 0 {
            return;
        }
        let t = &self.theme;
        let buf = frame.buffer_mut();

        let main_items = self.main_items();
        let sub_items = self.sub_items();
        let fits_one_line = !self.subtabs_own_line
            && Self::bar_width(&main_items) + Self::bar_width(&sub_items) + 2
                <= area.width as usize;

        let main_bg = t.bg();
        let sub_bg = t.surface();

        // Fill backgrounds.
        if fits_one_line {
            for fx in area.left()..area.right() {
                if let Some(cell) = buf.cell_mut(Position::new(fx, area.top())) {
                    cell.set_char(' ');
                    cell.set_bg(main_bg);
                }
            }
        } else {
            // Line 1: main bg, Line 2: sub bg.
            for fx in area.left()..area.right() {
                if let Some(cell) = buf.cell_mut(Position::new(fx, area.top())) {
                    cell.set_char(' ');
                    cell.set_bg(main_bg);
                }
            }
            if area.height > 1 {
                for fx in area.left()..area.right() {
                    if let Some(cell) = buf.cell_mut(Position::new(fx, area.top() + 1)) {
                        cell.set_char(' ');
                        cell.set_bg(sub_bg);
                    }
                }
            }
        }

        let mut x = area.left();
        let mut y = area.top();

        // Main bar: dark olive bg, cream fg.
        x = Self::render_bar(
            buf,
            &main_items,
            x,
            y,
            area.right(),
            main_bg,
            t.text_high(),
            Some(t.tab_active_bg()),
            t.text_dim(),
        );

        if fits_one_line {
            // Space + separator + space.
            let sep_fg = t.text_dim();
            if x < area.right() {
                if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                    cell.set_char(' ');
                    cell.set_style(Style::default().bg(main_bg));
                }
                x += 1;
            }
            if x < area.right() {
                if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                    cell.set_char('🮇');
                    cell.set_style(Style::default().fg(sep_fg).bg(main_bg));
                }
                x += 1;
            }
            if x < area.right() {
                if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                    cell.set_char('▎');
                    cell.set_style(Style::default().fg(sep_fg).bg(sub_bg));
                }
                x += 1;
            }
            if x < area.right() {
                if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                    cell.set_char(' ');
                    cell.set_style(Style::default().bg(sub_bg));
                }
                x += 1;
            }
            // Fill rest with sub_bg.
            for fx in x..area.right() {
                if let Some(cell) = buf.cell_mut(Position::new(fx, y)) {
                    cell.set_char(' ');
                    cell.set_bg(sub_bg);
                }
            }
        } else {
            y = area.top() + 1;
            x = area.left();
        }

        // Sub bar: dark magenta bg, cream fg.
        Self::render_bar(
            buf,
            &sub_items,
            x,
            y,
            area.right(),
            sub_bg,
            t.text_high(),
            Some(t.sub_tab_active_bg()),
            t.text_dim(),
        );
    }

    fn query(&self, _attr: Attribute) -> Option<QueryResult<'_>> {
        None
    }
    fn attr(&mut self, _attr: Attribute, _value: AttrValue) {}

    fn state(&self) -> State {
        State::Single(StateValue::String(format!("{:?}", self.active_tab)))
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        match cmd {
            Cmd::Move(tuirealm::command::Direction::Right) => {
                self.active_tab = self.active_tab.next(self.content_count);
                CmdResult::Changed(self.state())
            }
            Cmd::Move(tuirealm::command::Direction::Left) => {
                self.active_tab = self.active_tab.prev(self.content_count);
                CmdResult::Changed(self.state())
            }
            _ => CmdResult::NoChange,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn theme() -> Arc<Theme> {
        Arc::new(Theme::new(crate::config::ThemeConfig::default()))
    }

    fn tabs() -> Vec<ContentTabInfo> {
        vec![
            ContentTabInfo {
                name: "Jira".into(),
                icon: String::new(),
            },
            ContentTabInfo {
                name: "Tasks".into(),
                icon: String::new(),
            },
        ]
    }

    fn render(bar: &mut TabBarComponent, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| bar.view(f, f.area())).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn renders_tab_labels_without_any_menu_hint() {
        // The shortcut-menu hint now lives in the content action bar (with the
        // other activatable shortcuts), never in the tab bar — the tab bar
        // only shows tab labels.
        let mut bar = TabBarComponent::new(theme(), &tabs(), false);
        let text = render(&mut bar, 80, 1);
        assert!(text.contains("Jira"), "expected tab label: {text:?}");
        assert!(
            !text.contains("menu"),
            "tab bar must not show a menu hint: {text:?}"
        );
    }
}
