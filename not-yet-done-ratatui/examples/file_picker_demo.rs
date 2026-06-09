//! Example: FilePicker walk-through
//!
//! A standalone demo of the composite `FilePicker` widget. Pick files by
//! typing a directory + glob, toggle entries in the Files pane, then
//! confirm to print the chosen paths and exit.
//!
//! Run:
//!     cargo run --release -p not-yet-done-ratatui --example file_picker_demo
//!
//! Keys are taken from `FilePickerKeymap::default()` and rendered into
//! the help bar from the picker's live keymap, so the displayed shortcut
//! list always matches whatever you have configured. Every action in
//! `FilePickerKeymap` accepts multiple bindings (chain with
//! `Keys::or`/`or_plain`/`or_ctrl`).
//!
//! Directory input accepts a leading `~` / `~/` (expanded against `$HOME`).
//! Glob input is a comma-separated list of gitignore-style patterns
//! (e.g. `*.rs, **/*.toml`).

use std::path::PathBuf;

use crossterm::event::{
    Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use not_yet_done_ratatui::{
    FilePicker, FilePickerEvent, FilePickerKeymap, FilePickerStyle, SelectListStyle,
    SelectListStyleType, TextInputStyle, TextInputStyleType,
};
use ratatui::{
    DefaultTerminal,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};
use tuirealm::{
    component::{AppComponent, Component},
    event::{Key, KeyEvent, NoUserEvent},
    props::{AttrValue, Attribute},
};

// ── Colour palette (mirrors playlist_builder.rs) ─────────────────────────────

const BG: Color = Color::Rgb(20, 20, 30);
const PANEL_BG: Color = Color::Rgb(25, 25, 40);
const ACCENT: Color = Color::Rgb(180, 130, 255);
const ACTIVE_ACCENT: Color = Color::Rgb(200, 150, 255);
const INPUT_FG: Color = Color::Rgb(230, 230, 255);
const INPUT_BG: Color = Color::Rgb(35, 35, 55);
const PLACEHOLDER: Color = Color::Rgb(90, 90, 120);
const INACTIVE_PH: Color = Color::Rgb(45, 45, 65);
const SELECTED_BG: Color = Color::Rgb(50, 50, 80);
const ACTIVE_FG: Color = Color::Rgb(255, 210, 90);
const DIM: Color = Color::Rgb(100, 100, 130);

// ── TextInput styles (Directory + Glob) ──────────────────────────────────────

fn inactive_text_style() -> TextInputStyle {
    TextInputStyle::new()
        .prefix_color(ACCENT)
        .set_style(TextInputStyleType::Title, Style::default().fg(ACCENT))
        .set_style(TextInputStyleType::Input, Style::default().fg(INPUT_FG))
        .placeholder_color(INACTIVE_PH)
}

fn active_text_style() -> TextInputStyle {
    TextInputStyle::new()
        .prefix_color(ACTIVE_ACCENT)
        .set_style(
            TextInputStyleType::Title,
            Style::default().fg(ACTIVE_ACCENT).bg(INPUT_BG).add_modifier(Modifier::BOLD),
        )
        .set_style(
            TextInputStyleType::Input,
            Style::default().fg(INPUT_FG).bg(INPUT_BG),
        )
        .placeholder_color(PLACEHOLDER)
}

// ── SelectList styles (Files + Selected) ─────────────────────────────────────

fn inactive_select_list_style() -> SelectListStyle {
    SelectListStyle::default()
        .prefix_color(ACCENT)
        .placeholder_color(INACTIVE_PH)
        .set_style(SelectListStyleType::Item, Style::default().fg(INPUT_FG))
        .set_style(
            SelectListStyleType::ItemSelected,
            Style::default().fg(INPUT_FG).bg(SELECTED_BG),
        )
        .set_style(SelectListStyleType::ItemCursor, Style::default().fg(INPUT_FG))
        .set_style(
            SelectListStyleType::ItemCursorSelected,
            Style::default().fg(INPUT_FG).bg(SELECTED_BG),
        )
        .set_style(SelectListStyleType::FilterInput, Style::default().fg(DIM))
        .set_style(SelectListStyleType::FilterCursor, Style::default().fg(DIM))
        .set_style(SelectListStyleType::Footer, Style::default().fg(DIM))
}

fn active_select_list_style() -> SelectListStyle {
    SelectListStyle::default()
        .prefix_color(ACTIVE_ACCENT)
        .placeholder_color(PLACEHOLDER)
        .set_style(
            SelectListStyleType::Item,
            Style::default().fg(INPUT_FG).bg(INPUT_BG),
        )
        .set_style(
            SelectListStyleType::ItemSelected,
            Style::default().fg(INPUT_FG).bg(SELECTED_BG),
        )
        .set_style(
            SelectListStyleType::ItemCursor,
            Style::default()
                .fg(ACTIVE_FG)
                .bg(INPUT_BG)
                .add_modifier(Modifier::BOLD),
        )
        .set_style(
            SelectListStyleType::ItemCursorSelected,
            Style::default()
                .fg(ACTIVE_FG)
                .bg(SELECTED_BG)
                .add_modifier(Modifier::BOLD),
        )
        .set_style(
            SelectListStyleType::FilterInput,
            Style::default().fg(INPUT_FG).bg(INPUT_BG),
        )
        .set_style(
            SelectListStyleType::FilterCursor,
            Style::default()
                .fg(INPUT_BG)
                .bg(ACTIVE_FG)
                .add_modifier(Modifier::BOLD),
        )
        .set_style(
            SelectListStyleType::Footer,
            Style::default().fg(DIM).bg(INPUT_BG),
        )
}

fn picker_style() -> FilePickerStyle {
    FilePickerStyle::new()
        .with_text_input_inactive(inactive_text_style())
        .with_text_input_active(active_text_style())
        .with_select_list_inactive(inactive_select_list_style())
        .with_select_list_active(active_select_list_style())
}

// ── crossterm → tuirealm event bridge ────────────────────────────────────────

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

// ── Chrome (title + help bar) ────────────────────────────────────────────────

fn render_chrome(frame: &mut ratatui::Frame, panel: Rect, help_lines: &[Line<'static>]) {
    frame.render_widget(
        Block::default().style(Style::default().bg(BG)),
        frame.area(),
    );
    frame.render_widget(Block::default().style(Style::default().bg(PANEL_BG)), panel);

    let title = Paragraph::new(Line::from(vec![
        Span::styled("✦ ", Style::default().fg(ACTIVE_ACCENT)),
        Span::styled(
            "FilePicker Demo",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
    ]));
    // Title sits one row below the panel's top edge so the panel keeps
    // a blank gutter at the top.
    frame.render_widget(
        title,
        Rect::new(panel.x + 2, panel.y + 1, panel.width.saturating_sub(4), 1),
    );

    // Help bar sits one row above the panel's bottom edge so the panel
    // keeps a matching blank gutter at the bottom.
    let help_h = help_lines.len() as u16;
    let help_top = panel
        .y
        .saturating_add(panel.height.saturating_sub(help_h + 1));
    let help_w = panel.width.saturating_sub(4);
    for (i, line) in help_lines.iter().enumerate() {
        let paragraph = Paragraph::new(line.clone());
        frame.render_widget(
            paragraph,
            Rect::new(panel.x + 2, help_top + i as u16, help_w, 1),
        );
    }
}

/// Build the help-bar lines from the live keymap, wrapping greedily so
/// each hint pair (keys + label) stays on one line. Returns at least one
/// line even when the budget is zero.
fn help_lines(keymap: &FilePickerKeymap, max_width: u16) -> Vec<Line<'static>> {
    let entries: Vec<(String, &'static str)> = vec![
        (
            format!("{}/{}", keymap.focus_next.display(), keymap.focus_prev.display()),
            "focus",
        ),
        (
            format!("{}/{}", keymap.browse_down.display(), keymap.browse_up.display()),
            "nav",
        ),
        (keymap.toggle.display(), "toggle"),
        (keymap.tab_complete.display(), "complete"),
        (keymap.filter_clear.display(), "clear filter"),
        (keymap.remove_selected.display(), "remove"),
        (keymap.paste.display(), "paste"),
        (keymap.submit.display(), "submit"),
        (keymap.cancel.display(), "cancel"),
    ];

    // Greedy line-pack. Each entry renders as "<keys> <label>" with no
    // leading margin (the outer Rect already inset 2 chars from the
    // panel edge, matching every other component). Consecutive entries
    // on the same line are joined by a 2-space gutter.
    const SEP: usize = 2;
    let mut rows: Vec<Vec<(String, String)>> = vec![Vec::new()];
    let mut current_width: usize = 0;
    let budget = max_width as usize;

    for (keys, label) in entries {
        let entry_width = keys.chars().count() + 1 + label.chars().count();
        let last_empty = rows.last().map(|r| r.is_empty()).unwrap_or(true);
        let tentative = if last_empty {
            entry_width
        } else {
            current_width + SEP + entry_width
        };
        if !last_empty && tentative > budget {
            rows.push(Vec::new());
            current_width = entry_width;
        } else {
            current_width = tentative;
        }
        rows.last_mut().unwrap().push((keys, label.to_string()));
    }

    rows.into_iter()
        .map(|pairs| {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (i, (keys, label)) in pairs.into_iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw("  "));
                }
                spans.push(Span::styled(
                    keys,
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(
                    format!(" {label}"),
                    Style::default().fg(DIM),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

// ── Main loop ────────────────────────────────────────────────────────────────

enum Outcome {
    Confirmed(Vec<PathBuf>),
    Cancelled,
}

fn run(mut terminal: DefaultTerminal) -> std::io::Result<Outcome> {
    crossterm::execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )?;

    let initial_dir = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "~".to_string());

    let mut picker = FilePicker::default()
        .with_style(picker_style())
        .with_initial_directory(initial_dir)
        .with_initial_glob("*")
        .with_paste_provider(|| {
            arboard::Clipboard::new()
                .and_then(|mut cb| cb.get_text())
                .ok()
        });
    picker.attr(Attribute::Focus, AttrValue::Flag(true));

    let outcome = loop {
        terminal.draw(|f| {
            let area = f.area();
            let panel_w = area.width.min(90);
            let panel_h = area.height.min(30);
            let px = area.x + area.width.saturating_sub(panel_w) / 2;
            let py = area.y + area.height.saturating_sub(panel_h) / 2;
            let panel = Rect::new(px, py, panel_w, panel_h);

            // Compute the wrapped help bar once — both the chrome
            // renderer and the picker area sizing depend on its height.
            let lines = help_lines(picker.keymap(), panel.width.saturating_sub(4));
            let help_h = lines.len() as u16;
            render_chrome(f, panel, &lines);

            // Panel vertical layout:
            //   blank, title, blank, …picker…, blank, …help…, blank
            // → picker height = panel.height − 5 − help_h
            // → picker top    = panel.y + 3 (skip top-blank + title + blank)
            let picker_area = Rect::new(
                panel.x + 2,
                panel.y + 3,
                panel.width.saturating_sub(4),
                panel.height.saturating_sub(5 + help_h),
            );
            picker.view(f, picker_area);
        })?;

        let event = crossterm::event::read()?;
        let Event::Key(k) = event else { continue };
        if k.kind != KeyEventKind::Press {
            continue;
        }
        // Hard kill switch for the demo wrapper — Ctrl+C exits without
        // routing through the picker. Useful if a state ever wedges.
        if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
            break Outcome::Cancelled;
        }

        match picker.on(&to_tuirealm_event(&k)) {
            Some(FilePickerEvent::Confirmed(paths)) => break Outcome::Confirmed(paths),
            Some(FilePickerEvent::Cancelled) => break Outcome::Cancelled,
            _ => {}
        }
    };

    Ok(outcome)
}

fn main() -> std::io::Result<()> {
    let terminal = ratatui::init();
    let result = run(terminal);
    let _ = crossterm::execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    ratatui::restore();

    match result? {
        Outcome::Confirmed(paths) if paths.is_empty() => {
            println!("(submitted with no selections)");
        }
        Outcome::Confirmed(paths) => {
            println!("Selected {} path(s):", paths.len());
            for p in paths {
                println!("  {}", p.display());
            }
        }
        Outcome::Cancelled => {
            println!("(cancelled)");
        }
    }
    Ok(())
}
