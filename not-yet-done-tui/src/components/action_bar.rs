//! ActionBar component: pure renderer driven by view-supplied state.
//!
//! Each view owns its own instance and calls the granular setters
//! whenever its corresponding state changes. The bar has no idea what
//! tab/view it lives in.
//!
//! Layout (priority high → low):
//!   - cmdline active  → ":<query>"
//!   - search active   → "/ <query> [n/m]"
//!   - fuzzy active    → "󰈲 <query>           [⏎ accept  esc cancel]"
//!   - normal          → fuzzy_label │ hints │ active_filter_name │ favorites
//!
//! Visual style mirrors the historic Tasks-view bar.
//!
//! Active editor: hint matching `active_editor` description is rendered
//! bold + underlined. The "track" hint additionally highlights when
//! `tracking_active` is set; the "cut" hint highlights when `cut_active`
//! is set (a node is on the move-clipboard).

use std::sync::Arc;

use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::Frame;

use tuirealm::command::{Cmd, CmdResult};
use tuirealm::props::{Attribute, AttrValue, QueryResult};
use tuirealm::component::Component;
use tuirealm::state::{State, StateValue};

use crate::ui::theme::Theme;

/// Per-favorite entry shown in the bar: (name, shortcut).
pub type Favorite = (String, String);

/// (key_label, description). The key is shown dim, the description bright.
pub type Hint = (String, String);

#[derive(Default)]
struct FuzzyState {
    active: bool,
    query: String,
    cursor: usize,
}

struct SearchState {
    active: bool,
    query: String,
    cursor: usize,
    current: usize,
    total: usize,
    /// Optional prompt prefix (defaults to "/ "). Used to distinguish
    /// adapter-side searches from local `/`-search.
    prefix: Option<String>,
    placeholder: Option<String>,
    show_match_info: bool,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            active: false,
            query: String::new(),
            cursor: 0,
            current: 0,
            total: 0,
            prefix: None,
            placeholder: None,
            show_match_info: true,
        }
    }
}

#[derive(Default)]
struct CmdlineState {
    active: bool,
    query: String,
    cursor: usize,
}

pub struct ActionBarComponent {
    theme: Arc<Theme>,

    /// Hints for normal mode.
    hints: Vec<Hint>,
    /// Hint description currently bound to an open editor (highlighted).
    active_editor: Option<String>,
    /// Highlights the hint with description == "track" when set.
    tracking_active: bool,
    /// Highlights the hint with description == "cut" when set (a node is
    /// currently cut to the move-clipboard, awaiting paste).
    cut_active: bool,
    /// Active filter / saved-query name shown after the hints.
    active_filter_name: Option<String>,
    /// Favorites (name, shortcut).
    favorites: Vec<Favorite>,

    fuzzy: FuzzyState,
    search: SearchState,
    cmdline: CmdlineState,

    /// "{key} Fuzzy Filter" label shown before hints in normal mode.
    fuzzy_label: Option<String>,
    /// Hint shown at the right edge while fuzzy mode is active.
    fuzzy_exit_label: String,
    /// Mode label shown bold + accent at the very start of the bar in
    /// normal mode (e.g. "WINDOW" while the window-leader chord is
    /// pending). When set, takes precedence over `fuzzy_label`.
    mode_label: Option<String>,
}

impl ActionBarComponent {
    pub fn new(theme: Arc<Theme>) -> Self {
        Self {
            theme,
            hints: Vec::new(),
            active_editor: None,
            tracking_active: false,
            cut_active: false,
            active_filter_name: None,
            favorites: Vec::new(),
            fuzzy: FuzzyState::default(),
            search: SearchState::default(),
            cmdline: CmdlineState::default(),
            fuzzy_label: None,
            fuzzy_exit_label: "⏎ accept  esc cancel".to_string(),
            mode_label: None,
        }
    }

    /// Set a mode label (e.g. "WINDOW") shown bold + accent at the very
    /// start of the bar. Pass `None` to clear it.
    pub fn set_mode_label(&mut self, label: Option<String>) {
        self.mode_label = label;
    }

    /// Provide labels for fuzzy filter mode. Pass `None` to hide the
    /// "{key} Fuzzy Filter" prefix in normal mode (e.g. when the view
    /// doesn't expose a dedicated fuzzy keybinding through this label).
    pub fn set_fuzzy_label(&mut self, fuzzy_label: Option<String>, exit_label: Option<String>) {
        self.fuzzy_label = fuzzy_label;
        if let Some(label) = exit_label {
            self.fuzzy_exit_label = label;
        }
    }

    pub fn set_hints(&mut self, hints: Vec<Hint>) {
        self.hints = hints;
    }

    pub fn set_active_editor(&mut self, name: Option<&str>) {
        self.active_editor = name.map(|s| s.to_string());
    }

    pub fn set_tracking_active(&mut self, active: bool) {
        self.tracking_active = active;
    }

    pub fn set_cut_active(&mut self, active: bool) {
        self.cut_active = active;
    }

    pub fn set_active_filter_name(&mut self, name: Option<String>) {
        self.active_filter_name = name;
    }

    pub fn set_favorites(&mut self, favorites: Vec<Favorite>) {
        self.favorites = favorites;
    }

    pub fn set_fuzzy(&mut self, active: bool, query: &str, cursor: usize) {
        self.fuzzy.active = active;
        self.fuzzy.query = query.to_string();
        self.fuzzy.cursor = cursor;
    }

    pub fn set_search(
        &mut self,
        active: bool,
        query: &str,
        cursor: usize,
        current: usize,
        total: usize,
    ) {
        self.search.active = active;
        self.search.query = query.to_string();
        self.search.cursor = cursor;
        self.search.current = current;
        self.search.total = total;
    }

    /// Override the search-bar prompt prefix and placeholder. Pass `None` for
    /// either to fall back to the defaults ("/ " and "type to search…").
    /// `show_match_info = false` hides the `[n/m]` counter (adapter-side
    /// searches don't have a meaningful match count).
    pub fn set_search_chrome(
        &mut self,
        prefix: Option<String>,
        placeholder: Option<String>,
        show_match_info: bool,
    ) {
        self.search.prefix = prefix;
        self.search.placeholder = placeholder;
        self.search.show_match_info = show_match_info;
    }

    pub fn set_cmdline(&mut self, active: bool, query: &str, cursor: usize) {
        self.cmdline.active = active;
        self.cmdline.query = query.to_string();
        self.cmdline.cursor = cursor;
    }

    /// Calculate how many rows the bar needs at the given width.
    /// Takeover modes (cmdline/search/fuzzy) always use one row.
    pub fn required_height(&self, available_width: u16) -> u16 {
        if self.cmdline.active || self.search.active || self.fuzzy.active {
            return 1;
        }
        if self.hints.is_empty()
            && self.fuzzy_label.is_none()
            && self.active_filter_name.is_none()
            && self.favorites.is_empty()
            && self.active_editor.is_none()
            && self.mode_label.is_none()
        {
            return 0;
        }

        let prefix = if let Some(s) = self.mode_label.as_ref() {
            s.chars().count() + 5
        } else if let Some(s) = self.fuzzy_label.as_ref() {
            s.chars().count() + 5
        } else {
            0
        };
        let hint_widths: Vec<usize> = self
            .hints
            .iter()
            .map(|(key, desc)| key.chars().count() + 1 + desc.chars().count() + 2)
            .collect();
        let total: usize = 1 + prefix + hint_widths.iter().sum::<usize>();
        let w = available_width as usize;
        if total <= w {
            return 1;
        }

        let mut lines: u16 = 1;
        let mut line_used = 1 + prefix;
        for &hw in &hint_widths {
            if line_used + hw > w && line_used > 1 {
                lines += 1;
                line_used = 1;
            }
            line_used += hw;
        }
        lines
    }
}

// ── Rendering helpers ────────────────────────────────────────────────

fn fill_bg(buf: &mut ratatui::buffer::Buffer, area: Rect, bg: ratatui::style::Color) {
    for row in 0..area.height {
        let y = area.top() + row;
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                cell.set_char(' ');
                cell.set_bg(bg);
            }
        }
    }
}

fn write_run(
    buf: &mut ratatui::buffer::Buffer,
    x: &mut u16,
    y: &mut u16,
    right: u16,
    bottom: u16,
    text: &str,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
    mods: Modifier,
) {
    let style = Style::default().fg(fg).bg(bg).add_modifier(mods);
    for ch in text.chars() {
        if *x >= right || *y >= bottom {
            return;
        }
        if let Some(cell) = buf.cell_mut(Position::new(*x, *y)) {
            cell.set_char(ch);
            cell.set_style(style);
        }
        *x += 1;
    }
}

impl ActionBarComponent {
    fn render_cmdline(&self, frame: &mut Frame, area: Rect) -> Option<Position> {
        let t = &self.theme;
        let bg = t.toolbar_bg();
        let buf = frame.buffer_mut();
        let mut x = area.left() + 1;
        let mut y = area.top();
        let right = area.right();
        let bottom = area.bottom();

        write_run(buf, &mut x, &mut y, right, bottom, ":", t.accent(), bg, Modifier::BOLD);

        let max_w = right.saturating_sub(x) as usize;
        let chars: Vec<char> = self.cmdline.query.chars().collect();
        let view_start = if self.cmdline.cursor >= max_w {
            self.cmdline.cursor + 1 - max_w
        } else {
            0
        };
        let text_start_x = x;

        if chars.is_empty() {
            write_run(buf, &mut x, &mut y, right, bottom, "type command…", t.text_dim(), bg, Modifier::empty());
        } else {
            for (screen_idx, char_idx) in (view_start..chars.len()).enumerate() {
                if screen_idx >= max_w {
                    break;
                }
                if x < right {
                    if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                        cell.set_char(chars[char_idx]);
                        cell.set_style(Style::default().fg(t.text_high()).bg(bg));
                    }
                    x += 1;
                }
            }
        }

        let cursor_screen_x = text_start_x + (self.cmdline.cursor.saturating_sub(view_start)) as u16;
        if cursor_screen_x < right {
            Some(Position::new(cursor_screen_x, y))
        } else {
            None
        }
    }

    fn render_search(&self, frame: &mut Frame, area: Rect) -> Option<Position> {
        let t = &self.theme;
        let bg = t.toolbar_bg();
        let buf = frame.buffer_mut();
        let mut x = area.left() + 1;
        let mut y = area.top();
        let right = area.right();
        let bottom = area.bottom();

        let prefix = self.search.prefix.as_deref().unwrap_or("/ ");
        write_run(buf, &mut x, &mut y, right, bottom, prefix, t.accent(), bg, Modifier::BOLD);

        let match_info = if !self.search.show_match_info || self.search.query.is_empty() {
            String::new()
        } else if self.search.total == 0 {
            " [no matches]".to_string()
        } else {
            format!(" [{}/{}]", self.search.current + 1, self.search.total)
        };
        let info_width = match_info.chars().count() as u16 + 2;
        let max_w = right.saturating_sub(x).saturating_sub(info_width) as usize;

        let chars: Vec<char> = self.search.query.chars().collect();
        let view_start = if self.search.cursor >= max_w {
            self.search.cursor + 1 - max_w
        } else {
            0
        };
        let text_start_x = x;

        if chars.is_empty() {
            let placeholder = self.search.placeholder.as_deref().unwrap_or("type to search…");
            write_run(buf, &mut x, &mut y, right, bottom, placeholder, t.text_dim(), bg, Modifier::empty());
        } else {
            for (screen_idx, char_idx) in (view_start..chars.len()).enumerate() {
                if screen_idx >= max_w {
                    break;
                }
                if x < right {
                    if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                        cell.set_char(chars[char_idx]);
                        cell.set_style(Style::default().fg(t.text_high()).bg(bg));
                    }
                    x += 1;
                }
            }
        }

        let info_fg = if self.search.total == 0 && !self.search.query.is_empty() {
            t.error()
        } else {
            t.text_dim()
        };
        write_run(buf, &mut x, &mut y, right, bottom, &match_info, info_fg, bg, Modifier::empty());

        let cx = text_start_x + (self.search.cursor.saturating_sub(view_start)) as u16;
        if cx < right {
            Some(Position::new(cx, y))
        } else {
            None
        }
    }

    fn render_fuzzy(&self, frame: &mut Frame, area: Rect) -> Option<Position> {
        let t = &self.theme;
        let bg = t.toolbar_bg();
        let buf = frame.buffer_mut();
        let mut x = area.left() + 1;
        let mut y = area.top();
        let right = area.right();
        let bottom = area.bottom();

        write_run(buf, &mut x, &mut y, right, bottom, "󰈲 ", t.accent(), bg, Modifier::BOLD);

        let exit_w = self.fuzzy_exit_label.chars().count() as u16 + 2;
        let max_w = right.saturating_sub(x).saturating_sub(exit_w) as usize;
        let chars: Vec<char> = self.fuzzy.query.chars().collect();
        let view_start = if self.fuzzy.cursor >= max_w {
            self.fuzzy.cursor + 1 - max_w
        } else {
            0
        };
        let text_start_x = x;

        if chars.is_empty() {
            write_run(buf, &mut x, &mut y, right, bottom, "type to filter…", t.text_dim(), bg, Modifier::empty());
        } else {
            for (screen_idx, char_idx) in (view_start..chars.len()).enumerate() {
                if screen_idx >= max_w {
                    break;
                }
                if x < right {
                    if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                        cell.set_char(chars[char_idx]);
                        cell.set_style(Style::default().fg(t.text_high()).bg(bg));
                    }
                    x += 1;
                }
            }
        }

        let cursor_pos = if !chars.is_empty() {
            let cx = text_start_x + (self.fuzzy.cursor.saturating_sub(view_start)) as u16;
            (cx < right).then(|| Position::new(cx, y))
        } else {
            None
        };

        let exit_label_w = self.fuzzy_exit_label.chars().count() as u16;
        let hint_x = right.saturating_sub(exit_label_w + 1);
        let mut hx = hint_x;
        let mut hy = y;
        write_run(buf, &mut hx, &mut hy, right, bottom, &self.fuzzy_exit_label, t.text_dim(), bg, Modifier::empty());

        cursor_pos
    }

    fn render_normal(&self, frame: &mut Frame, area: Rect) {
        let t = &self.theme;
        let bg = t.toolbar_bg();
        let buf = frame.buffer_mut();
        let left = area.left();
        let right = area.right();
        let bottom = area.bottom();
        let mut x = left + 1;
        let mut y = area.top();

        if let Some(label) = self.mode_label.as_deref() {
            write_run(buf, &mut x, &mut y, right, bottom, label, t.accent(), bg, Modifier::BOLD);
            write_run(buf, &mut x, &mut y, right, bottom, "  │  ", t.text_dim(), bg, Modifier::empty());
        } else if let Some(label) = self.fuzzy_label.as_deref() {
            write_run(buf, &mut x, &mut y, right, bottom, label, t.primary_dim(), bg, Modifier::empty());
            write_run(buf, &mut x, &mut y, right, bottom, "  │  ", t.text_dim(), bg, Modifier::empty());
        }

        for (key_label, desc) in &self.hints {
            let hint_width =
                (key_label.chars().count() + 1 + desc.chars().count() + 2) as u16;
            if x + hint_width > right && x > left + 1 && y + 1 < bottom {
                y += 1;
                x = left + 1;
            }

            let is_editor_active = self.active_editor.as_deref() == Some(desc.as_str());
            let is_tracking_active = desc == "track" && self.tracking_active;
            let is_cut_active = desc == "cut" && self.cut_active;
            let is_active = is_editor_active || is_tracking_active || is_cut_active;
            let fg = if is_active { t.accent() } else { t.secondary() };
            let mods = if is_active {
                Modifier::UNDERLINED | Modifier::BOLD
            } else {
                Modifier::empty()
            };

            write_run(buf, &mut x, &mut y, right, bottom, key_label, t.text_dim(), bg, Modifier::empty());
            write_run(buf, &mut x, &mut y, right, bottom, " ", fg, bg, Modifier::empty());
            write_run(buf, &mut x, &mut y, right, bottom, desc, fg, bg, mods);
            write_run(buf, &mut x, &mut y, right, bottom, "  ", t.text_dim(), bg, Modifier::empty());
        }

        if let Some(name) = self.active_filter_name.as_deref() {
            write_run(buf, &mut x, &mut y, right, bottom, " │  ", t.text_dim(), bg, Modifier::empty());
            write_run(buf, &mut x, &mut y, right, bottom, name, t.accent(), bg, Modifier::ITALIC);
        }
        if !self.favorites.is_empty() {
            write_run(buf, &mut x, &mut y, right, bottom, "  │  ", t.text_dim(), bg, Modifier::empty());
            for (name, shortcut) in &self.favorites {
                let is_active = self.active_filter_name.as_deref() == Some(name.as_str());
                let fg = if is_active { t.accent() } else { t.secondary() };
                let mods = if is_active { Modifier::BOLD } else { Modifier::empty() };
                write_run(buf, &mut x, &mut y, right, bottom, name, fg, bg, mods);
                write_run(buf, &mut x, &mut y, right, bottom, " ", t.text_dim(), bg, Modifier::empty());
                let bracket = format!("[{shortcut}]");
                write_run(buf, &mut x, &mut y, right, bottom, &bracket, t.text_dim(), bg, Modifier::empty());
                write_run(buf, &mut x, &mut y, right, bottom, "  ", t.text_dim(), bg, Modifier::empty());
            }
        }
    }
}

impl Component for ActionBarComponent {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        if area.height == 0 {
            return;
        }
        let bg = self.theme.toolbar_bg();
        fill_bg(frame.buffer_mut(), area, bg);

        let cursor = if self.cmdline.active {
            self.render_cmdline(frame, area)
        } else if self.search.active {
            self.render_search(frame, area)
        } else if self.fuzzy.active {
            self.render_fuzzy(frame, area)
        } else {
            self.render_normal(frame, area);
            None
        };

        if let Some(pos) = cursor {
            frame.set_cursor_position(pos);
        }
    }

    fn query(&self, _attr: Attribute) -> Option<QueryResult<'_>> {
        None
    }
    fn attr(&mut self, _attr: Attribute, _value: AttrValue) {}
    fn state(&self) -> State {
        let mode = if self.cmdline.active {
            "cmdline"
        } else if self.search.active {
            "search"
        } else if self.fuzzy.active {
            "fuzzy"
        } else {
            "normal"
        };
        State::Single(StateValue::String(mode.to_string()))
    }
    fn perform(&mut self, _cmd: Cmd) -> CmdResult {
        CmdResult::NoChange
    }
}
