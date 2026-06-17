//! TabBar component: top-level tab navigation with sub-view indicators.

use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::Frame;

use tuirealm::command::{Cmd, CmdResult};
use tuirealm::props::{Attribute, AttrValue, QueryResult};
use tuirealm::component::Component;
use tuirealm::state::{State, StateValue};

use crate::config::{GlobalAction, TrackingsAction};
use crate::tabs::{Tab, TrackingsSubView};
use std::sync::Arc;
use crate::ui::theme::Theme;

/// A single bar segment: label text + whether it's the active item.
struct BarItem {
    label: String,
    active: bool,
}

pub struct TabBarComponent {
    active_tab: Tab,
    trackings_sub_view: TrackingsSubView,
    content_count: usize,
    theme: Arc<Theme>,
    main_tab_labels: Vec<(Tab, String)>,
    trackings_sub_labels: Vec<(TrackingsSubView, String)>,
    /// Per content-tab subtab labels with their active flag. Outer index
    /// is the Content tab index; inner is (label, is_active). Pushed in
    /// each frame by App from the corresponding `ContentView`.
    content_sub_tabs: Vec<Vec<(String, bool)>>,
}

/// Info about a content tab for display in the tab bar.
pub struct ContentTabInfo {
    pub name: String,
    pub icon: String,
}

impl TabBarComponent {
    pub fn new(
        theme: Arc<Theme>,
        keybindings: &crate::config::KeyBindingConfig,
        content_tabs: &[ContentTabInfo],
    ) -> Self {
        let gkb = &keybindings.global;

        use crate::tabs::tab_label;
        let mut main_tab_labels = vec![
            (Tab::Trackings, tab_label("⏱️", &gkb.label(&GlobalAction::TabTrackings), "Trackings")),
        ];
        for (i, info) in content_tabs.iter().enumerate() {
            let kb = match i {
                0 => gkb.label(&GlobalAction::TabJira),
                1 => gkb.label(&GlobalAction::TabTaiga),
                2 => gkb.label(&GlobalAction::TabPostgres),
                3 => gkb.label(&GlobalAction::TabConfluence),
                _ => String::new(),
            };
            main_tab_labels.push((
                Tab::Content(i),
                tab_label(&info.icon, &kb, &info.name),
            ));
        }

        let trackings_sub_labels = vec![
            (TrackingsSubView::Normal, format!("{} {}", TrackingsSubView::Normal.title(), keybindings.trackings.label(&TrackingsAction::TrackingNormalToggle))),
            (TrackingsSubView::Condensed, format!("{} {}", TrackingsSubView::Condensed.title(), keybindings.trackings.label(&TrackingsAction::TrackingCondensedToggle))),
            (TrackingsSubView::Tree, format!("{} {}", TrackingsSubView::Tree.title(), keybindings.trackings.label(&TrackingsAction::TrackingTreeToggle))),
        ];

        Self {
            active_tab: Tab::Trackings,
            trackings_sub_view: TrackingsSubView::Normal,
            content_count: content_tabs.len(),
            theme,
            main_tab_labels,
            trackings_sub_labels,
            content_sub_tabs: vec![Vec::new(); content_tabs.len()],
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
    /// key hint). The App pushes this each frame from the active
    /// [`TabLayout`](crate::tabs::TabLayout) so the bar shows only the
    /// visible tabs, in constellation order, with autonumber keys.
    pub fn set_main_tab_labels(&mut self, labels: Vec<(Tab, String)>) {
        self.main_tab_labels = labels;
    }

    pub fn set_trackings_sub_view(&mut self, sv: TrackingsSubView) {
        self.trackings_sub_view = sv;
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
        self.main_tab_labels.iter().map(|(tab, label)| BarItem {
            label: label.clone(),
            active: *tab == self.active_tab,
        }).collect()
    }

    fn sub_items(&self) -> Vec<BarItem> {
        match self.active_tab {
            Tab::Trackings => self.trackings_sub_labels.iter().map(|(sv, label)| BarItem {
                label: label.clone(),
                active: *sv == self.trackings_sub_view,
            }).collect(),
            Tab::Content(idx) => match self.content_sub_tabs.get(idx) {
                Some(labels) if !labels.is_empty() => labels.iter()
                    .map(|(lab, active)| BarItem { label: lab.clone(), active: *active })
                    .collect(),
                _ => vec![BarItem { label: "tickets".to_string(), active: true }],
            },
        }
    }

    pub fn required_height(&self, available_width: u16) -> u16 {
        let main = Self::bar_width(&self.main_items());
        let sub = Self::bar_width(&self.sub_items()) + 4; // " 🮇▎ " separator
        if main + sub <= available_width as usize { 1 } else { 2 }
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
            let style = if item.active {
                Style::default()
                    .fg(active_fg)
                    .bg(active_bg.unwrap_or(bar_bg))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(inactive_fg).bg(bar_bg)
            };
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
        if area.height == 0 { return; }
        let t = &self.theme;
        let buf = frame.buffer_mut();

        let main_items = self.main_items();
        let sub_items = self.sub_items();
        let fits_one_line = Self::bar_width(&main_items) + Self::bar_width(&sub_items) + 2 <= area.width as usize;

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
            buf, &main_items, x, y, area.right(),
            main_bg, t.text_high(), Some(t.tab_active_bg()), t.text_dim(),
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
            buf, &sub_items, x, y, area.right(),
            sub_bg, t.text_high(), Some(t.sub_tab_active_bg()), t.text_dim(),
        );
    }

    fn query(&self, _attr: Attribute) -> Option<QueryResult<'_>> { None }
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
