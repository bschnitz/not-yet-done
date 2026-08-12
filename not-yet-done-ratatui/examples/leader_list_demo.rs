//! Example: LeaderList showcase (table of contents & aligned key→value lines)
//!
//! The `LeaderList` widget maps `a → b` pairs onto one line each, laid out as
//! `a + post + n·f + pre + b`, where the filler `f` repeats to push `b` flush
//! against the right edge — a "dot leader", as in a printed table of contents.
//!
//!   Panels shown:
//!     1. Table of contents  — selectable, dot leader `……`, page numbers right
//!     2. Keyboard shortcuts  — filler `·`, rendered at the widget's min width
//!     3. Nutrition facts     — filler spaces, prefix unit, fixed width
//!
//!   Navigation (panel 1 only):
//!     ↑/↓ or C-k/C-j — move cursor
//!     Enter          — "select" the entry (updates the status line)
//!     Esc            — quit

use crossterm::event::{
    Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use not_yet_done_ratatui::{
    Keys, LeaderList, LeaderListEvent, LeaderListKeymap, LeaderListStyle, LeaderListStyleType,
    LeaderWidth,
};
use tuirealm::{
    component::{AppComponent, Component},
    event::{Key, KeyEvent, NoUserEvent},
    props::{AttrValue, Attribute},
};

use ratatui::{
    DefaultTerminal,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

// ── Colours ───────────────────────────────────────────────────────────────────

const BG: Color = Color::Rgb(10, 10, 20);
const PANEL_BG: Color = Color::Rgb(18, 18, 35);
const ACCENT: Color = Color::Rgb(100, 180, 255);
const GREEN: Color = Color::Rgb(140, 255, 180);
const DIM: Color = Color::Rgb(70, 70, 100);
const TEXT: Color = Color::Rgb(210, 210, 230);
const TEXT_MUTED: Color = Color::Rgb(140, 140, 170);
const CURSOR_BG: Color = Color::Rgb(40, 50, 80);
const RIGHT_FG: Color = Color::Rgb(255, 220, 80);

// ── LeaderList styles ──────────────────────────────────────────────────────────

fn toc_style() -> LeaderListStyle {
    LeaderListStyle::new()
        .set_style(
            LeaderListStyleType::Title,
            Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
        )
        .set_style(LeaderListStyleType::Left, Style::default().fg(TEXT))
        .set_style(LeaderListStyleType::Filler, Style::default().fg(DIM))
        .set_style(
            LeaderListStyleType::Right,
            Style::default().fg(RIGHT_FG).add_modifier(Modifier::BOLD),
        )
        .set_style(LeaderListStyleType::Cursor, Style::default().bg(CURSOR_BG))
        .set_style(
            LeaderListStyleType::Status,
            Style::default()
                .fg(TEXT_MUTED)
                .add_modifier(Modifier::ITALIC),
        )
}

fn muted_style() -> LeaderListStyle {
    LeaderListStyle::new()
        .set_style(LeaderListStyleType::Left, Style::default().fg(TEXT_MUTED))
        .set_style(LeaderListStyleType::Filler, Style::default().fg(DIM))
        .set_style(LeaderListStyleType::Right, Style::default().fg(ACCENT))
}

// ── Panel definitions ────────────────────────────────────────────────────────

/// The 30 table-of-contents entries, shared by the panel and the status line.
fn toc_entries() -> Vec<(String, String)> {
    const SECTIONS: [&str; 30] = [
        "Introduction",
        "Getting Started",
        "Installation",
        "Configuration",
        "Core Concepts",
        "The Component Model",
        "Rendering Basics",
        "Layout & Constraints",
        "Styling & Themes",
        "Event Handling",
        "Building Widgets",
        "The LeaderList Widget",
        "Tables & Grids",
        "Forms & Inputs",
        "Popups & Overlays",
        "State Management",
        "Keymaps & Shortcuts",
        "Focus & Navigation",
        "Scrolling & Paging",
        "Advanced Topics",
        "Performance Tuning",
        "Unicode & Wide Glyphs",
        "Testing Widgets",
        "Debugging the TUI",
        "Integration Patterns",
        "Migration Guide",
        "Appendix A — Glossary",
        "Appendix B — Key Codes",
        "Appendix C — Colours",
        "Index",
    ];
    let mut page = 1usize;
    SECTIONS
        .iter()
        .map(|name| {
            let entry = ((*name).to_string(), page.to_string());
            page += name.len() % 7 + 4; // pseudo page numbers, monotonically up
            entry
        })
        .collect()
}

fn make_toc() -> LeaderList {
    LeaderList::default()
        .with_title("1 · Table of contents (selectable, dot leader)")
        .with_entries(toc_entries())
        // post = " ", filler = "." (dot leader), pre = " "
        .with_affixes(" ", ".", " ")
        .with_selectable(true)
        // Only 4 of the 30 entries are shown → scrollable with a status line.
        .with_max_rows(4)
        .with_status_line(true)
        .with_style(toc_style())
        .with_keymap(LeaderListKeymap {
            move_up: Keys::plain(Key::Up).or_ctrl(Key::Char('k')),
            move_down: Keys::plain(Key::Down).or_ctrl(Key::Char('j')),
            ..LeaderListKeymap::default()
        })
}

fn make_shortcuts() -> LeaderList {
    LeaderList::default()
        .with_entries(vec![
            ("Move cursor", "j / k"),
            ("Toggle", "Space"),
            ("Confirm", "Enter"),
            ("Cancel", "Esc"),
            ("Quit", "Ctrl+C"),
        ])
        .with_affixes(" ", "…", " ")
        // Render at the tightest width in which nothing is truncated.
        .with_width(LeaderWidth::Min)
        .with_style(muted_style())
}

fn make_nutrition() -> LeaderList {
    LeaderList::default()
        .with_entries(vec![
            ("Energy", "512"),
            ("Protein", "9"),
            ("Carbohydrate", "63"),
            ("of which sugars", "4"),
            ("Fat", "27"),
            ("Salt", "1"),
        ])
        // post = ":", filler = spaces, pre = right-aligns the value + unit.
        .with_affixes(":", " ", "  ")
        .with_width(LeaderWidth::Fixed(34))
        .with_style(muted_style())
}

// ── App state ────────────────────────────────────────────────────────────────

struct App {
    toc: LeaderList,
    shortcuts: LeaderList,
    nutrition: LeaderList,
    status: String,
}

impl App {
    fn new() -> Self {
        let mut toc = make_toc();
        toc.attr(Attribute::Focus, AttrValue::Flag(true));
        Self {
            toc,
            shortcuts: make_shortcuts(),
            nutrition: make_nutrition(),
            status: "Move with ↑/↓, press Enter to select an entry.".into(),
        }
    }
}

// ── Event conversion ────────────────────────────────────────────────────────

fn to_tuirealm_event(k: &crossterm::event::KeyEvent) -> tuirealm::event::Event<NoUserEvent> {
    let code = match k.code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        _ => Key::Null,
    };
    tuirealm::event::Event::Keyboard(KeyEvent {
        code,
        modifiers: k.modifiers.into(),
    })
}

// ── Render ───────────────────────────────────────────────────────────────────

fn panel_title(frame: &mut ratatui::Frame, area: Rect, title: &str) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            title,
            Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
        ))),
        Rect { height: 1, ..area },
    );
}

fn render(app: &mut App, frame: &mut ratatui::Frame) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);

    let panel_w = area.width.min(74);
    let panel_h = area.height.min(34);
    let px = area.x + area.width.saturating_sub(panel_w) / 2;
    let py = area.y + area.height.saturating_sub(panel_h) / 2;
    let panel = Rect::new(px, py, panel_w, panel_h);
    frame.render_widget(Block::default().style(Style::default().bg(PANEL_BG)), panel);

    let inner = Rect::new(
        panel.x + 3,
        panel.y + 1,
        panel.width.saturating_sub(6),
        panel.height.saturating_sub(2),
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("✦ ", Style::default().fg(GREEN)),
            Span::styled(
                "LeaderList — table of contents & aligned pairs",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
        ])),
        Rect { height: 1, ..inner },
    );

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // heading + spacer
            Constraint::Length(8), // TOC (title + 6 rows + spacer)
            Constraint::Length(7), // shortcuts
            Constraint::Length(8), // nutrition
            Constraint::Min(0),    // filler
            Constraint::Length(2), // status + help
        ])
        .split(inner);

    // Panel 1 — TOC (selectable). The title is drawn by the widget itself
    // (set via LeaderList::with_title), so no external panel_title here.
    app.toc.view(frame, sections[1]);

    // Panel 2 — shortcuts at min width
    panel_title(frame, sections[2], "2 · Shortcuts (filler '…', min width)");
    app.shortcuts.view(
        frame,
        Rect {
            y: sections[2].y + 1,
            height: sections[2].height.saturating_sub(1),
            ..sections[2]
        },
    );

    // Panel 3 — nutrition at fixed width
    panel_title(
        frame,
        sections[3],
        "3 · Nutrition (space filler, fixed width 34)",
    );
    app.nutrition.view(
        frame,
        Rect {
            y: sections[3].y + 1,
            height: sections[3].height.saturating_sub(1),
            ..sections[3]
        },
    );

    // Status + help
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            &*app.status,
            Style::default().fg(TEXT),
        ))),
        Rect {
            height: 1,
            ..sections[5]
        },
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "↑↓/C-k/C-j",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" move  ", Style::default().fg(DIM)),
            Span::styled(
                "PgUp/PgDn",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" page  ", Style::default().fg(DIM)),
            Span::styled(
                "Enter",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" select  ", Style::default().fg(DIM)),
            Span::styled(
                "Esc",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" quit", Style::default().fg(DIM)),
        ])),
        Rect {
            y: sections[5].y + 1,
            height: 1,
            ..sections[5]
        },
    );
}

// ── Main loop ────────────────────────────────────────────────────────────────

fn run(mut terminal: DefaultTerminal) -> std::io::Result<()> {
    crossterm::execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )?;

    let mut app = App::new();
    let toc_titles: Vec<String> = toc_entries().into_iter().map(|(name, _)| name).collect();

    loop {
        terminal.draw(|f| render(&mut app, f))?;

        let Event::Key(k) = crossterm::event::read()? else {
            continue;
        };
        if k.kind != KeyEventKind::Press {
            continue;
        }

        if let (KeyCode::Esc, KeyModifiers::NONE) = (k.code, k.modifiers) {
            break;
        }

        let ev = to_tuirealm_event(&k);
        match app.toc.on(&ev) {
            Some(LeaderListEvent::CursorChanged(i)) => {
                app.status = format!("Cursor on \"{}\".", toc_titles[i]);
            }
            Some(LeaderListEvent::Selected(i)) => {
                app.status = format!("▶ Selected \"{}\".", toc_titles[i]);
            }
            Some(LeaderListEvent::Scrolled(top)) => {
                app.status = format!("Scrolled — top entry #{}.", top + 1);
            }
            Some(LeaderListEvent::Cancelled) => break,
            None => {}
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
