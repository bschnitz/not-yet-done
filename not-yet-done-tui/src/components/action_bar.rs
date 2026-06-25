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
//! Active state: each [`ActionHint`] carries its own `active` flag and the
//! bar renders active hints bold + underlined + accent. The component does
//! not know *why* a hint is active — the view stamps `active` while building
//! its hints (tracking running, cut armed, jump-mode open, editor focused,
//! …). This is the structural contract: the top action bar holds shortcuts
//! that can be momentarily active, the bottom status bar holds shortcuts that
//! never are. Carrying `active` on the hint enforces that the active-ness is
//! considered for every action-bar entry, rather than special-casing a few
//! hardcoded descriptions.

use std::sync::Arc;

use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;

use tuirealm::command::{Cmd, CmdResult};
use tuirealm::props::{Attribute, AttrValue, QueryResult};
use tuirealm::component::Component;
use tuirealm::state::{State, StateValue};

use crate::ui::theme::Theme;

/// Per-favorite entry shown in the bar: (name, shortcut).
pub type Favorite = (String, String);

/// An action-bar hint: a shortcut that can be *momentarily active* (a mode
/// is armed — tracking running, cut on the move-clipboard, jump-mode open,
/// an editor focused, …). The bar marks it while `active`.
///
/// This is deliberately distinct from the status bar's plain `(key, desc)`
/// tuple: the top bar holds activatable shortcuts, the bottom bar holds
/// shortcuts that are never active. Carrying `active` as a field makes the
/// component enforce that distinction instead of matching individual
/// descriptions. The `key` is shown dim, the `desc` bright (accent + bold +
/// underline while active).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActionHint {
    pub key: String,
    pub desc: String,
    pub active: bool,
}

impl ActionHint {
    pub fn new(key: impl Into<String>, desc: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            desc: desc.into(),
            active: false,
        }
    }

    /// Builder: set the active flag (mode armed → highlighted).
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }
}

impl From<(String, String)> for ActionHint {
    fn from((key, desc): (String, String)) -> Self {
        Self {
            key,
            desc,
            active: false,
        }
    }
}

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

    /// Hints for normal mode. Each carries its own `active` flag, stamped
    /// by the owning view while building its hints.
    hints: Vec<ActionHint>,
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

    pub fn set_hints(&mut self, hints: Vec<ActionHint>) {
        self.hints = hints;
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
    ///
    /// Drives off the exact same [`normal_units`](Self::normal_units) layout
    /// the renderer consumes, so the allocated height always matches the rows
    /// `render_normal` actually wraps into — hints *and* the active-filter name
    /// *and* the favorites (which previously weren't counted and so got
    /// truncated at the right edge instead of wrapping).
    pub fn required_height(&self, available_width: u16) -> u16 {
        if self.cmdline.active || self.search.active || self.fuzzy.active {
            return 1;
        }
        let (prefix, units) = self.normal_units();
        if prefix.is_none() && units.is_empty() {
            return 0;
        }
        let w = available_width as usize;
        if w == 0 {
            return 1;
        }
        let prefix_w = prefix.as_ref().map(BarUnit::width).unwrap_or(0);
        let mut lines: u16 = 1;
        let mut line_used = 1 + prefix_w;
        for unit in &units {
            let uw = unit.width();
            if line_used + uw > w && line_used > 1 {
                lines += 1;
                line_used = 1;
            }
            line_used += uw;
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

/// Draw a pre-styled run (used by the unit-based normal-mode layout, where
/// each run already carries its full [`Style`]). Stops at `right`/`bottom`.
fn write_run_styled(
    buf: &mut ratatui::buffer::Buffer,
    x: &mut u16,
    y: &mut u16,
    right: u16,
    bottom: u16,
    text: &str,
    style: Style,
) {
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

/// One atomic wrap unit of the normal-mode bar: a sequence of styled runs that
/// must stay together on a line (a hint `key+desc`, the active-filter name, one
/// favorite `name [shortcut]`). The renderer wraps to the next line before a
/// unit that wouldn't fit; it never splits a unit. Group separators (` │ `) are
/// folded into the unit they precede so they wrap along with it.
struct BarUnit {
    runs: Vec<(String, Style)>,
}

impl BarUnit {
    fn width(&self) -> usize {
        self.runs.iter().map(|(s, _)| s.chars().count()).sum()
    }
}

impl ActionBarComponent {
    /// Build the normal-mode bar as an optional leading prefix unit (mode /
    /// fuzzy label) plus the ordered wrap units (hints → active-filter name →
    /// favorites). Single source of truth shared by [`required_height`] and
    /// [`render_normal`] so the height calc and the draw never disagree.
    ///
    /// [`required_height`]: Self::required_height
    fn normal_units(&self) -> (Option<BarUnit>, Vec<BarUnit>) {
        let t = &self.theme;
        let bg = t.toolbar_bg();
        let run = |text: &str, fg: Color, mods: Modifier| {
            (text.to_string(), Style::default().fg(fg).bg(bg).add_modifier(mods))
        };

        let prefix = if let Some(label) = self.mode_label.as_deref() {
            Some(BarUnit {
                runs: vec![
                    run(label, t.accent(), Modifier::BOLD),
                    run("  │  ", t.text_dim(), Modifier::empty()),
                ],
            })
        } else if let Some(label) = self.fuzzy_label.as_deref() {
            Some(BarUnit {
                runs: vec![
                    run(label, t.primary_dim(), Modifier::empty()),
                    run("  │  ", t.text_dim(), Modifier::empty()),
                ],
            })
        } else {
            None
        };

        let mut units: Vec<BarUnit> = Vec::new();

        for hint in &self.hints {
            let fg = if hint.active { t.accent() } else { t.secondary() };
            let mods = if hint.active {
                Modifier::UNDERLINED | Modifier::BOLD
            } else {
                Modifier::empty()
            };
            units.push(BarUnit {
                runs: vec![
                    run(&hint.key, t.text_dim(), Modifier::empty()),
                    run(" ", fg, Modifier::empty()),
                    run(&hint.desc, fg, mods),
                    run("  ", t.text_dim(), Modifier::empty()),
                ],
            });
        }

        if let Some(name) = self.active_filter_name.as_deref() {
            units.push(BarUnit {
                runs: vec![
                    run(" │  ", t.text_dim(), Modifier::empty()),
                    run(name, t.accent(), Modifier::ITALIC),
                ],
            });
        }

        for (i, (name, shortcut)) in self.favorites.iter().enumerate() {
            let is_active = self.active_filter_name.as_deref() == Some(name.as_str());
            let fg = if is_active { t.accent() } else { t.secondary() };
            let mods = if is_active {
                Modifier::BOLD
            } else {
                Modifier::empty()
            };
            let mut runs: Vec<(String, Style)> = Vec::new();
            // The favorites group separator rides with the first favorite so it
            // wraps as a group marker rather than dangling at a line's end.
            if i == 0 {
                runs.push(run("  │  ", t.text_dim(), Modifier::empty()));
            }
            runs.push(run(name, fg, mods));
            runs.push(run(" ", t.text_dim(), Modifier::empty()));
            runs.push(run(&format!("[{shortcut}]"), t.text_dim(), Modifier::empty()));
            runs.push(run("  ", t.text_dim(), Modifier::empty()));
            units.push(BarUnit { runs });
        }

        (prefix, units)
    }

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
        let (prefix, units) = self.normal_units();
        let buf = frame.buffer_mut();
        let left = area.left();
        let right = area.right();
        let bottom = area.bottom();
        let mut x = left + 1;
        let mut y = area.top();

        // The prefix (mode/fuzzy label) always anchors the first line.
        if let Some(unit) = &prefix {
            for (text, style) in &unit.runs {
                write_run_styled(buf, &mut x, &mut y, right, bottom, text, *style);
            }
        }

        // Each unit wraps as a whole: if it wouldn't fit and we're past the
        // line start (and there's a line below), advance to the next row.
        for unit in &units {
            let uw = unit.width() as u16;
            if x + uw > right && x > left + 1 && y + 1 < bottom {
                y += 1;
                x = left + 1;
            }
            for (text, style) in &unit.runs {
                write_run_styled(buf, &mut x, &mut y, right, bottom, text, *style);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn bar() -> ActionBarComponent {
        ActionBarComponent::new(Arc::new(Theme::new(crate::config::ThemeConfig::default())))
    }

    #[test]
    fn fits_on_one_line_when_wide_enough() {
        let mut b = bar();
        b.set_hints(vec![ActionHint::new("f", "fuzzy filter"), ActionHint::new("/", "search")]);
        assert_eq!(b.required_height(200), 1);
    }

    #[test]
    fn favorites_are_counted_and_force_a_wrap() {
        // Regression: favorites used to be ignored by `required_height`, so a
        // bar whose favorites overflowed the width was allocated a single row
        // and the trailing favorites were truncated at the right edge instead
        // of wrapping onto a new line.
        let mut b = bar();
        b.set_hints(vec![ActionHint::new("f", "fuzzy filter")]);
        b.set_active_filter_name(Some("My Tickets".into()));
        b.set_favorites(vec![
            ("Mentioned In".into(), "ctrl+m".into()),
            ("My Tickets".into(), "ctrl+i".into()),
            ("Watched Tickets".into(), "ctrl+w".into()),
        ]);
        // Comfortably wide → everything on one line.
        assert_eq!(b.required_height(200), 1);
        // Narrow enough that the favorites no longer fit → must wrap, so the
        // bar reports more than one row (previously stuck at 1).
        assert!(b.required_height(40) > 1, "favorites must force a wrap");
    }

    #[test]
    fn takeover_modes_are_always_one_row() {
        let mut b = bar();
        b.set_hints(vec![ActionHint::new("f", "fuzzy filter")]);
        b.set_favorites(vec![("A".into(), "x".into()); 20]);
        b.set_search(true, "needle", 6, 0, 0);
        assert_eq!(b.required_height(10), 1);
    }

    #[test]
    fn empty_bar_needs_no_rows() {
        assert_eq!(bar().required_height(80), 0);
    }
}
