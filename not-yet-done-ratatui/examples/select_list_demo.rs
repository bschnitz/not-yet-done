//! Example: SelectList + MultiChoice showcase
//!
//! Demonstrates the `SelectList` widget with multiple marker styles and the
//! extended `MultiChoice` dropdown with markers, filter, and footer.
//!
//!   Lists shown (SelectList, always visible):
//!     1. Default (no marker)      — Programming languages
//!     2. Checkbox markers         — Toppings
//!     3. Radio markers (single)   — Difficulty
//!     4. Custom markers           — Status flags
//!     5. With filter + footer     — Countries
//!
//!   Dropdown (MultiChoice, opens/closes):
//!     6. Genres — checkbox, filter, footer, max_height=8
//!
//!   Navigation:
//!     Ctrl+L   — focus next
//!     Ctrl+H   — focus previous
//!     Ctrl+K/J — move cursor up/down (SelectList)
//!     ↑/↓      — move cursor up/down (MultiChoice)
//!     Space    — toggle selection
//!     Enter    — open/close MultiChoice dropdown
//!     Esc      — quit

use crossterm::event::{
    Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use not_yet_done_ratatui::{
    Keys, MultiChoice, MultiChoiceKeymap, MultiChoiceStyle, MultiChoiceStyleType,
    SelectList, SelectListKeymap, SelectListStyle, SelectListStyleType,
    SelectionMarker, SelectionMode,
};
use tuirealm::{
    command::Cmd,
    component::{AppComponent, Component},
    event::{Key, KeyEvent, NoUserEvent},
    props::{AttrValue, Attribute},
};

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    DefaultTerminal,
};

// ── Colours ───────────────────────────────────────────────────────────────────

const BG: Color = Color::Rgb(10, 10, 20);
const PANEL_BG: Color = Color::Rgb(18, 18, 35);
const ACCENT: Color = Color::Rgb(100, 180, 255);
const GREEN: Color = Color::Rgb(140, 255, 180);
const DIM: Color = Color::Rgb(60, 60, 90);
const TEXT: Color = Color::Rgb(210, 210, 230);
const TEXT_MUTED: Color = Color::Rgb(140, 140, 170);
const CURSOR_BG: Color = Color::Rgb(40, 50, 80);
const SELECTED_FG: Color = Color::Rgb(255, 220, 80);
const SELECTED_MUTED: Color = Color::Rgb(180, 160, 80);
const FILTER_FG: Color = Color::Rgb(180, 180, 200);
const FILTER_CURSOR_BG: Color = Color::Rgb(80, 100, 140);
const FOOTER_FG: Color = Color::Rgb(120, 120, 160);
const GROUP_FG: Color = Color::Rgb(180, 140, 255);
const INPUT_BG: Color = Color::Rgb(28, 28, 50);
const SELECTED_BG: Color = Color::Rgb(50, 50, 80);
const CURSOR_BG_MC: Color = Color::Rgb(35, 40, 60);

// ── SelectList styles ────────────────────────────────────────────────────────

fn sl_inactive_style() -> SelectListStyle {
    SelectListStyle::default()
        .set_style(SelectListStyleType::Item, Style::default().fg(TEXT_MUTED))
        .set_style(SelectListStyleType::ItemSelected, Style::default().fg(SELECTED_MUTED))
        .set_style(SelectListStyleType::ItemCursor, Style::default().fg(TEXT_MUTED))
        .set_style(SelectListStyleType::ItemCursorSelected, Style::default().fg(SELECTED_MUTED))
        .set_style(SelectListStyleType::GroupHeader, Style::default().fg(DIM))
        .set_style(SelectListStyleType::FilterInput, Style::default().fg(TEXT_MUTED))
        .set_style(SelectListStyleType::FilterCursor, Style::default().fg(TEXT_MUTED))
        .set_style(SelectListStyleType::Footer, Style::default().fg(DIM))
}

fn sl_active_style() -> SelectListStyle {
    SelectListStyle::default()
        .set_style(SelectListStyleType::Item, Style::default().fg(TEXT))
        .set_style(SelectListStyleType::ItemSelected, Style::default().fg(SELECTED_FG))
        .set_style(SelectListStyleType::ItemCursor, Style::default().fg(TEXT).bg(CURSOR_BG))
        .set_style(SelectListStyleType::ItemCursorSelected, Style::default().fg(SELECTED_FG).bg(CURSOR_BG))
        .set_style(SelectListStyleType::GroupHeader, Style::default().fg(GROUP_FG).add_modifier(Modifier::BOLD))
        .set_style(SelectListStyleType::FilterInput, Style::default().fg(FILTER_FG))
        .set_style(SelectListStyleType::FilterCursor, Style::default().fg(TEXT).bg(FILTER_CURSOR_BG))
        .set_style(SelectListStyleType::Footer, Style::default().fg(FOOTER_FG))
}

// ── MultiChoice styles ───────────────────────────────────────────────────────

fn mc_inactive_style() -> MultiChoiceStyle {
    MultiChoiceStyle::new()
        .prefix_color(ACCENT)
        .set_style(MultiChoiceStyleType::Title, Style::default().fg(ACCENT))
        .set_style(MultiChoiceStyleType::Normal, Style::default().fg(TEXT_MUTED))
        .set_style(MultiChoiceStyleType::Selected, Style::default().fg(SELECTED_MUTED))
        .set_style(MultiChoiceStyleType::SelectedActive, Style::default().fg(SELECTED_MUTED))
}

fn mc_active_style() -> MultiChoiceStyle {
    MultiChoiceStyle::new()
        .prefix_color(GREEN)
        .set_style(MultiChoiceStyleType::Title, Style::default().fg(GREEN).bg(INPUT_BG))
        .set_style(MultiChoiceStyleType::Normal, Style::default().fg(TEXT).bg(INPUT_BG))
        .set_style(MultiChoiceStyleType::Active, Style::default().fg(SELECTED_FG).bg(INPUT_BG))
        .set_style(MultiChoiceStyleType::Selected, Style::default().fg(TEXT).bg(SELECTED_BG))
        .set_style(MultiChoiceStyleType::SelectedActive, Style::default().fg(SELECTED_FG).bg(SELECTED_BG))
        .set_style(MultiChoiceStyleType::LastLine, Style::default().bg(PANEL_BG))
        .set_style(MultiChoiceStyleType::FilterInput, Style::default().fg(FILTER_FG).bg(INPUT_BG))
        .set_style(MultiChoiceStyleType::FilterCursor, Style::default().fg(TEXT).bg(FILTER_CURSOR_BG))
        .set_style(MultiChoiceStyleType::Footer, Style::default().fg(FOOTER_FG).bg(INPUT_BG))
}

// ── Keymaps ──────────────────────────────────────────────────────────────────

fn sl_keymap() -> SelectListKeymap {
    SelectListKeymap {
        toggle: Keys::plain(tuirealm::event::Key::Char(' '))
            .or_ctrl(tuirealm::event::Key::Char(' ')),
        ..SelectListKeymap::default()
    }
}

fn mc_keymap() -> MultiChoiceKeymap {
    use tuirealm::event::Key;
    MultiChoiceKeymap {
        move_up: Keys::plain(Key::Up).or_ctrl(Key::Char('k')),
        move_down: Keys::plain(Key::Down).or_ctrl(Key::Char('j')),
        toggle: Keys::plain(Key::Char(' ')).or_ctrl(Key::Char(' ')),
        ..MultiChoiceKeymap::default()
    }
}

// ── List definitions ─────────────────────────────────────────────────────────

fn make_languages() -> SelectList {
    SelectList::default()
        .with_items(vec!["Rust", "Go", "TypeScript", "Python", "Zig", "C++", "Haskell", "Elixir"])
        .with_keymap(sl_keymap())
        .with_inactive_style(sl_inactive_style())
        .with_active_style(sl_active_style())
}

fn make_toppings() -> SelectList {
    SelectList::default()
        .with_items(vec!["Mozzarella", "Pepperoni", "Mushrooms", "Olives", "Basil", "Onions", "Peppers"])
        .with_marker(SelectionMarker::Checkbox)
        .with_keymap(sl_keymap())
        .with_inactive_style(sl_inactive_style())
        .with_active_style(sl_active_style())
}

fn make_difficulty() -> SelectList {
    SelectList::default()
        .with_items(vec!["Easy", "Medium", "Hard", "Expert"])
        .with_marker(SelectionMarker::Radio)
        .with_mode(SelectionMode::Single)
        .with_keymap(sl_keymap())
        .with_inactive_style(sl_inactive_style())
        .with_active_style(sl_active_style())
}

fn make_status() -> SelectList {
    SelectList::default()
        .with_items(vec!["Active", "Paused", "Archived", "Draft", "Review"])
        .with_marker(SelectionMarker::Custom { selected: "● ", unselected: "○ " })
        .with_keymap(sl_keymap())
        .with_inactive_style(sl_inactive_style())
        .with_active_style(sl_active_style())
}

fn make_countries() -> SelectList {
    SelectList::default()
        .with_items(vec![
            "Germany", "France", "Italy", "Spain", "Portugal",
            "Netherlands", "Belgium", "Austria", "Switzerland",
            "Sweden", "Norway", "Denmark", "Finland", "Poland",
            "Czech Republic", "Ireland", "Greece",
        ])
        .with_marker(SelectionMarker::Checkbox)
        .with_show_filter(true)
        .with_show_footer(true)
        .with_keymap(sl_keymap())
        .with_inactive_style(sl_inactive_style())
        .with_active_style(sl_active_style())
}

fn mc_dot_active_style() -> MultiChoiceStyle {
    MultiChoiceStyle::new()
        .prefix_color(GREEN)
        .set_style(MultiChoiceStyleType::Title, Style::default().fg(GREEN).bg(INPUT_BG))
        .set_style(MultiChoiceStyleType::Normal, Style::default().fg(TEXT_MUTED).bg(INPUT_BG))
        .set_style(MultiChoiceStyleType::Active, Style::default().fg(TEXT).bg(CURSOR_BG_MC))
        .set_style(MultiChoiceStyleType::Selected, Style::default().fg(SELECTED_FG).bg(INPUT_BG))
        .set_style(MultiChoiceStyleType::SelectedActive, Style::default().fg(SELECTED_FG).bg(CURSOR_BG_MC))
        .set_style(MultiChoiceStyleType::LastLine, Style::default().bg(PANEL_BG))
        .set_style(MultiChoiceStyleType::FilterInput, Style::default().fg(FILTER_FG).bg(INPUT_BG))
        .set_style(MultiChoiceStyleType::FilterCursor, Style::default().fg(TEXT).bg(FILTER_CURSOR_BG))
        .set_style(MultiChoiceStyleType::Footer, Style::default().fg(FOOTER_FG).bg(INPUT_BG))
}

fn mc_dot_inactive_style() -> MultiChoiceStyle {
    MultiChoiceStyle::new()
        .prefix_color(ACCENT)
        .set_style(MultiChoiceStyleType::Title, Style::default().fg(ACCENT))
        .set_style(MultiChoiceStyleType::Normal, Style::default().fg(TEXT_MUTED))
        .set_style(MultiChoiceStyleType::Selected, Style::default().fg(SELECTED_MUTED))
        .set_style(MultiChoiceStyleType::SelectedActive, Style::default().fg(SELECTED_MUTED))
}

fn make_genres_dropdown() -> MultiChoice {
    MultiChoice::default()
        .with_title("Genres (dropdown)")
        .with_choices(vec![
            "Rock", "Jazz", "Electronic", "Hip-Hop", "Classical",
            "Metal", "Folk", "R&B", "Reggae", "Blues",
            "Country", "Punk", "Soul", "Ambient",
        ])
        .with_placeholder("Select genres…")
        .with_marker(SelectionMarker::Checkbox)
        .with_show_filter(true)
        .with_show_footer(true)
        .with_max_height(8)
        .with_keymap(mc_keymap())
        .with_inactive_style(mc_inactive_style())
        .with_active_style(mc_active_style())
}

fn make_moods_dropdown() -> MultiChoice {
    MultiChoice::default()
        .with_title("Moods (●/○ style)")
        .with_choices(vec![
            "Energetic", "Relaxing", "Melancholic", "Upbeat",
            "Chill", "Dark", "Euphoric", "Nostalgic",
        ])
        .with_placeholder("Select moods…")
        .with_marker(SelectionMarker::Custom { selected: "● ", unselected: "○ " })
        .with_show_filter(true)
        .with_show_footer(true)
        .with_max_height(8)
        .with_keymap(mc_keymap())
        .with_inactive_style(mc_dot_inactive_style())
        .with_active_style(mc_dot_active_style())
}

// ── App state ────────────────────────────────────────────────────────────────

const NUM_LISTS: usize = 5;
const NUM_DROPDOWNS: usize = 2;
const TOTAL_FIELDS: usize = NUM_LISTS + NUM_DROPDOWNS;

struct App {
    active: usize,
    lists: [SelectList; NUM_LISTS],
    list_titles: [&'static str; NUM_LISTS],
    dropdowns: [MultiChoice; NUM_DROPDOWNS],
    dropdown_open: [bool; NUM_DROPDOWNS],
}

impl App {
    fn new() -> Self {
        let mut app = Self {
            active: 0,
            lists: [
                make_languages(),
                make_toppings(),
                make_difficulty(),
                make_status(),
                make_countries(),
            ],
            list_titles: [
                "Languages (default)",
                "Toppings (checkbox)",
                "Difficulty (radio/single)",
                "Status (custom ●/○)",
                "Countries (filter+footer)",
            ],
            dropdowns: [make_genres_dropdown(), make_moods_dropdown()],
            dropdown_open: [false; NUM_DROPDOWNS],
        };
        app.lists[0].attr(Attribute::Focus, AttrValue::Flag(true));
        app
    }

    fn set_focus(&mut self, idx: usize) {
        for list in &mut self.lists {
            list.attr(Attribute::Focus, AttrValue::Flag(false));
        }
        for (i, dd) in self.dropdowns.iter_mut().enumerate() {
            dd.attr(Attribute::Focus, AttrValue::Flag(false));
            self.dropdown_open[i] = false;
        }

        if idx < NUM_LISTS {
            self.lists[idx].attr(Attribute::Focus, AttrValue::Flag(true));
        } else {
            let dd_idx = idx - NUM_LISTS;
            self.dropdowns[dd_idx].attr(Attribute::Focus, AttrValue::Flag(true));
            self.dropdown_open[dd_idx] = true;
        }
        self.active = idx;
    }

    fn focus_next(&mut self) {
        self.set_focus((self.active + 1) % TOTAL_FIELDS);
    }

    fn focus_prev(&mut self) {
        self.set_focus(if self.active == 0 { TOTAL_FIELDS - 1 } else { self.active - 1 });
    }
}

// ── Event conversion ────────────────────────────────────────────────────────

fn to_tuirealm_event(k: &crossterm::event::KeyEvent) -> tuirealm::event::Event<NoUserEvent> {
    let code = match k.code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::F(n) => Key::Function(n),
        _ => Key::Null,
    };
    tuirealm::event::Event::Keyboard(KeyEvent {
        code,
        modifiers: k.modifiers.into(),
    })
}

// ── Render ───────────────────────────────────────────────────────────────────

fn render(app: &mut App, frame: &mut ratatui::Frame) {
    let area = frame.area();

    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);

    let panel_w = area.width.min(90);
    let panel_h = area.height.min(40);
    let px = area.x + area.width.saturating_sub(panel_w) / 2;
    let py = area.y + area.height.saturating_sub(panel_h) / 2;
    let panel = Rect::new(px, py, panel_w, panel_h);

    frame.render_widget(Block::default().style(Style::default().bg(PANEL_BG)), panel);

    let inner = Rect::new(
        panel.x + 2,
        panel.y + 1,
        panel.width.saturating_sub(4),
        panel.height.saturating_sub(3),
    );

    // Heading
    let heading_area = Rect { height: 1, ..inner };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("✦ ", Style::default().fg(GREEN)),
            Span::styled(
                "SelectList + MultiChoice Demo",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
        ])),
        heading_area,
    );

    // Main layout: SelectLists grid + MultiChoice dropdown below
    let content_area = Rect {
        y: inner.y + 2,
        height: inner.height.saturating_sub(4),
        ..inner
    };

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),      // SelectList grid
            Constraint::Length(1),    // spacer
            Constraint::Length(12),   // MultiChoice dropdown area
        ])
        .split(content_area);

    // SelectList grid: 2 rows x 3 columns (top row) + 2 columns (bottom row)
    let sl_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(sections[0]);

    let top_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(33), Constraint::Percentage(34), Constraint::Percentage(33)])
        .split(sl_rows[0]);

    let bottom_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(sl_rows[1]);

    let list_areas = [top_cols[0], top_cols[1], top_cols[2], bottom_cols[0], bottom_cols[1]];

    for (i, &col_area) in list_areas.iter().enumerate() {
        let padded = Rect {
            x: col_area.x + 1,
            width: col_area.width.saturating_sub(2),
            ..col_area
        };
        let title_area = Rect { height: 1, ..padded };
        let is_active = i == app.active;
        let title_fg = if is_active { GREEN } else { TEXT_MUTED };
        let indicator = if is_active { "▸ " } else { "  " };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(indicator, Style::default().fg(title_fg)),
                Span::styled(
                    app.list_titles[i],
                    Style::default().fg(title_fg).add_modifier(Modifier::BOLD),
                ),
            ])),
            title_area,
        );

        let list_area = Rect {
            y: padded.y + 1,
            height: padded.height.saturating_sub(1),
            ..padded
        };
        app.lists[i].view(frame, list_area);
    }

    // MultiChoice dropdowns side by side
    let mc_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(sections[2]);

    for (i, &col) in mc_cols.iter().enumerate() {
        let mc_area = Rect {
            x: col.x + 1,
            width: col.width.saturating_sub(2),
            ..col
        };
        app.dropdowns[i].view(frame, mc_area);
    }

    // Help line
    let help_y = panel.y + panel.height.saturating_sub(2);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" C-l", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" next  ", Style::default().fg(DIM)),
            Span::styled("C-h", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" prev  ", Style::default().fg(DIM)),
            Span::styled("↑↓/C-j/k", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" nav  ", Style::default().fg(DIM)),
            Span::styled("Spc", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" toggle  ", Style::default().fg(DIM)),
            Span::styled("Enter", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" open/close  ", Style::default().fg(DIM)),
            Span::styled("Esc", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" quit", Style::default().fg(DIM)),
        ])),
        Rect::new(panel.x + 2, help_y, panel.width.saturating_sub(4), 1),
    );
}

// ── Main loop ────────────────────────────────────────────────────────────────

fn run(mut terminal: DefaultTerminal) -> std::io::Result<()> {
    crossterm::execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )?;

    let mut app = App::new();

    loop {
        terminal.draw(|f| render(&mut app, f))?;

        let event = crossterm::event::read()?;

        let Event::Key(k) = event else { continue };
        if k.kind != KeyEventKind::Press {
            continue;
        }

        match (k.code, k.modifiers) {
            (KeyCode::Esc, KeyModifiers::NONE) => {
                if app.active >= NUM_LISTS {
                    let dd_idx = app.active - NUM_LISTS;
                    if app.dropdown_open[dd_idx] {
                        app.dropdowns[dd_idx].perform(Cmd::Cancel);
                        app.dropdown_open[dd_idx] = false;
                        continue;
                    }
                }
                break;
            }
            (KeyCode::Char('l'), KeyModifiers::CONTROL) => app.focus_next(),
            (KeyCode::Char('h'), KeyModifiers::CONTROL) => app.focus_prev(),
            (KeyCode::Enter, KeyModifiers::NONE) if app.active >= NUM_LISTS => {
                let dd_idx = app.active - NUM_LISTS;
                if app.dropdown_open[dd_idx] {
                    app.dropdowns[dd_idx].perform(Cmd::Cancel);
                    app.dropdown_open[dd_idx] = false;
                } else {
                    app.dropdowns[dd_idx].perform(Cmd::Submit);
                    app.dropdown_open[dd_idx] = true;
                }
            }
            _ => {
                let tui_ev = to_tuirealm_event(&k);
                if app.active < NUM_LISTS {
                    let _ = app.lists[app.active].on(&tui_ev);
                } else {
                    let dd_idx = app.active - NUM_LISTS;
                    let _ = app.dropdowns[dd_idx].on(&tui_ev);
                }
            }
        }
    }

    Ok(())
}

fn main() -> std::io::Result<()> {
    let terminal = ratatui::init();
    let result = run(terminal);
    let _ = crossterm::execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    ratatui::restore();
    result
}
