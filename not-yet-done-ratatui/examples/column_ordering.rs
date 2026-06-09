//! Example: Column ordering with MultiChoice
//!
//! Demonstrates MultiChoice with ordering enabled. Two dropdowns side by side:
//!   1. Columns — select and reorder table columns (with order numbers)
//!   2. Toppings — reorder pizza toppings (without order numbers)
//!
//! Navigation:
//!   Ctrl+L / Ctrl+H — switch between dropdowns
//!   Ctrl+J / Ctrl+K — move cursor (also ↑/↓)
//!   Ctrl+D / Ctrl+F — reorder item up/down
//!   Space            — toggle selection
//!   Enter            — open/close dropdown
//!   type to filter   — fuzzy search
//!   Esc              — quit

use crossterm::event::{
    Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use not_yet_done_ratatui::{
    Keys, MultiChoice, MultiChoiceKeymap, MultiChoiceStyle, MultiChoiceStyleType,
    SelectionMarker,
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

// ── Colours ──────────────────────────────────────────────────────────────────

const BG: Color = Color::Rgb(10, 10, 20);
const PANEL_BG: Color = Color::Rgb(18, 18, 35);
const ACCENT: Color = Color::Rgb(100, 180, 255);
const GREEN: Color = Color::Rgb(140, 255, 180);
const DIM: Color = Color::Rgb(60, 60, 90);
const TEXT: Color = Color::Rgb(210, 210, 230);
const TEXT_MUTED: Color = Color::Rgb(140, 140, 170);
const INPUT_BG: Color = Color::Rgb(28, 28, 50);
const SELECTED_FG: Color = Color::Rgb(255, 220, 80);
const CURSOR_BG: Color = Color::Rgb(35, 40, 60);

// ── Styles ───────────────────────────────────────────────────────────────────

fn active_style() -> MultiChoiceStyle {
    MultiChoiceStyle::new()
        .prefix_color(GREEN)
        .set_style(MultiChoiceStyleType::Title, Style::default().fg(GREEN).bg(INPUT_BG))
        .set_style(MultiChoiceStyleType::Normal, Style::default().fg(TEXT).bg(INPUT_BG))
        .set_style(MultiChoiceStyleType::Active, Style::default().fg(TEXT).bg(CURSOR_BG))
        .set_style(MultiChoiceStyleType::Selected, Style::default().fg(SELECTED_FG).bg(INPUT_BG))
        .set_style(MultiChoiceStyleType::SelectedActive, Style::default().fg(SELECTED_FG).bg(CURSOR_BG))
        .set_style(MultiChoiceStyleType::LastLine, Style::default().bg(PANEL_BG))
        .set_style(MultiChoiceStyleType::Footer, Style::default().fg(DIM).bg(INPUT_BG))
}

fn inactive_style() -> MultiChoiceStyle {
    MultiChoiceStyle::new()
        .prefix_color(ACCENT)
        .set_style(MultiChoiceStyleType::Title, Style::default().fg(ACCENT))
        .set_style(MultiChoiceStyleType::Normal, Style::default().fg(TEXT_MUTED))
        .set_style(MultiChoiceStyleType::Selected, Style::default().fg(SELECTED_FG))
        .set_style(MultiChoiceStyleType::SelectedActive, Style::default().fg(SELECTED_FG))
}

// ── Keymap ───────────────────────────────────────────────────────────────────

fn demo_keymap() -> MultiChoiceKeymap {
    use tuirealm::event::Key;
    MultiChoiceKeymap {
        move_up:    Keys::plain(Key::Up).or_ctrl(Key::Char('k')),
        move_down:  Keys::plain(Key::Down).or_ctrl(Key::Char('j')),
        toggle:     Keys::plain(Key::Char(' ')).or_ctrl(Key::Char(' ')),
        order_up:   Keys::ctrl(Key::Char('d')),
        order_down: Keys::ctrl(Key::Char('f')),
        ..MultiChoiceKeymap::default()
    }
}

// ── Dropdowns ────────────────────────────────────────────────────────────────

fn make_columns() -> MultiChoice {
    MultiChoice::default()
        .with_title("Table Columns")
        .with_choices(vec![
            "Status", "Priority", "Tracking", "Description",
            "Created", "Updated", "Tags", "Project",
            "Assignee", "Due Date", "Estimate",
        ])
        .with_placeholder("Select columns…")
        .with_marker(SelectionMarker::Checkbox)
        .with_ordering(true)
        .with_show_order(true)
        .with_show_filter(true)
        .with_show_footer(true)
        .with_keymap(demo_keymap())
        .with_inactive_style(inactive_style())
        .with_active_style(active_style())
}

fn make_toppings() -> MultiChoice {
    MultiChoice::default()
        .with_title("Pizza Toppings (reorder)")
        .with_choices(vec![
            "Mozzarella", "Pepperoni", "Mushrooms", "Olives",
            "Basil", "Onions", "Peppers", "Anchovies",
        ])
        .with_placeholder("Build your pizza…")
        .with_marker(SelectionMarker::Custom { selected: "● ", unselected: "○ " })
        .with_ordering(true)
        .with_show_order(false)
        .with_show_filter(true)
        .with_show_footer(true)
        .with_keymap(demo_keymap())
        .with_inactive_style(inactive_style())
        .with_active_style(active_style())
}

// ── App ──────────────────────────────────────────────────────────────────────

const NUM: usize = 2;

struct App {
    active: usize,
    dropdowns: [MultiChoice; NUM],
    open: [bool; NUM],
}

impl App {
    fn new() -> Self {
        let mut app = Self {
            active: 0,
            dropdowns: [make_columns(), make_toppings()],
            open: [false; NUM],
        };
        app.dropdowns[0].attr(Attribute::Focus, AttrValue::Flag(true));
        app.open[0] = true;
        app
    }

    fn set_focus(&mut self, idx: usize) {
        for (i, dd) in self.dropdowns.iter_mut().enumerate() {
            dd.attr(Attribute::Focus, AttrValue::Flag(false));
            self.open[i] = false;
        }
        self.dropdowns[idx].attr(Attribute::Focus, AttrValue::Flag(true));
        self.open[idx] = true;
        self.active = idx;
    }
}

// ── Event conversion ─────────────────────────────────────────────────────────

fn to_tuirealm(k: &crossterm::event::KeyEvent) -> tuirealm::event::Event<NoUserEvent> {
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

    let panel_w = area.width.min(80);
    let panel_h = area.height.min(24);
    let px = area.x + area.width.saturating_sub(panel_w) / 2;
    let py = area.y + area.height.saturating_sub(panel_h) / 2;
    let panel = Rect::new(px, py, panel_w, panel_h);
    frame.render_widget(Block::default().style(Style::default().bg(PANEL_BG)), panel);

    let inner = Rect::new(panel.x + 2, panel.y + 1, panel.width.saturating_sub(4), panel.height.saturating_sub(3));

    // Heading
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("✦ ", Style::default().fg(GREEN)),
            Span::styled("MultiChoice Ordering", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        ])),
        Rect { height: 1, ..inner },
    );

    // Two dropdowns side by side
    let content = Rect { y: inner.y + 2, height: inner.height.saturating_sub(4), ..inner };
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(content);

    for (i, &col) in cols.iter().enumerate() {
        let mc_area = Rect { x: col.x + 1, width: col.width.saturating_sub(2), ..col };
        app.dropdowns[i].view(frame, mc_area);
    }

    // Help
    let help_y = panel.y + panel.height.saturating_sub(2);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" C-l/h", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" switch  ", Style::default().fg(DIM)),
            Span::styled("C-j/k", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" nav  ", Style::default().fg(DIM)),
            Span::styled("C-d/f", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" reorder  ", Style::default().fg(DIM)),
            Span::styled("Spc", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" toggle  ", Style::default().fg(DIM)),
            Span::styled("Esc", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(" quit", Style::default().fg(DIM)),
        ])),
        Rect::new(panel.x + 2, help_y, panel.width.saturating_sub(4), 1),
    );
}

// ── Main ─────────────────────────────────────────────────────────────────────

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
        if k.kind != KeyEventKind::Press { continue; }

        match (k.code, k.modifiers) {
            (KeyCode::Esc, KeyModifiers::NONE) => {
                if app.open[app.active] {
                    app.dropdowns[app.active].perform(Cmd::Cancel);
                    app.open[app.active] = false;
                } else {
                    break;
                }
            }
            (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                app.set_focus((app.active + 1) % NUM);
            }
            (KeyCode::Char('h'), KeyModifiers::CONTROL) => {
                app.set_focus(if app.active == 0 { NUM - 1 } else { app.active - 1 });
            }
            (KeyCode::Enter, KeyModifiers::NONE) => {
                let i = app.active;
                if app.open[i] {
                    app.dropdowns[i].perform(Cmd::Cancel);
                    app.open[i] = false;
                } else {
                    app.dropdowns[i].perform(Cmd::Submit);
                    app.open[i] = true;
                }
            }
            _ => {
                let ev = to_tuirealm(&k);
                let _ = app.dropdowns[app.active].on(&ev);
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
