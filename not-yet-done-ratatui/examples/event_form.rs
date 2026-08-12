//! Example: "New event" form — the calendar adapter's create action, standalone
//!
//! Built exactly like `new_server`: the individual `TextInput`, `SelectList` and
//! `Toggle` widgets composed by hand in the tuirealm component model. Each field
//! widget renders its own `▍`-prefixed title and filled `INPUT_BG` value bar —
//! the example never draws a title itself. No spec-driven `Form` driver, no TUI
//! app around it; just the form, decoupled so its UX can be iterated in isolation.
//!
//! The field set, order, defaults and choices match
//! `not-yet-done-calendar-adapter`'s `create_event_spec` one-for-one. The eight
//! fields are laid out in two columns so the whole form fits a normal terminal:
//!
//!   Left column                 Right column
//!     Title                       Calendar
//!     Start   (natural language)  All day
//!     End     (natural language)  Show as
//!     Location
//!     Notes
//!
//! The *Start* / *End* fields take a natural-language phrase (`tomorrow 9am`,
//! `in 2 hours`, `next friday 8pm`); with the default `natural-date` feature the
//! dim line under each previews the phrase resolved against the wall clock.
//!
//!   Ctrl+J / Ctrl+K  — next / previous field
//!   ↑ ↓ · Ctrl+L/H · Tab (up) — move cursor inside a select
//!   Space            — toggle (All day) / pick option (select)
//!   Ctrl+Enter       — submit (dry-run "Would create …" overlay)
//!   Esc              — quit / dismiss overlay
//!
//! ```sh
//! cargo run -p not-yet-done-ratatui --example event_form
//! ```

use crossterm::{
    cursor::SetCursorStyle,
    event::{
        Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
};
use not_yet_done_ratatui::{
    Keys, SELECT_LIST_ATTR_SELECTED, SelectList, SelectListKeymap, SelectListStyle,
    SelectListStyleType, SelectionMarker, SelectionMode, TextInput, TextInputStyle,
    TextInputStyleType, Toggle, ToggleStyle, ToggleStyleType,
};
use tuirealm::{
    command::Cmd,
    component::{AppComponent, Component},
    event::{Key, KeyEvent, NoUserEvent},
    props::{AttrValue, Attribute, PropPayload, PropValue},
    state::{State, StateValue},
};

use ratatui::{
    DefaultTerminal,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Wrap},
};

// ── Colours (same palette as new_server) ───────────────────────────────────────

const BG: Color = Color::Rgb(10, 10, 20);
const PANEL_BG: Color = Color::Rgb(18, 18, 35);
const ACCENT: Color = Color::Rgb(100, 180, 255);
const ACTIVE_ACCENT: Color = Color::Rgb(140, 255, 180);
const INPUT_FG: Color = Color::Rgb(230, 230, 255);
const INPUT_BG: Color = Color::Rgb(28, 28, 50);
const PLACEHOLDER: Color = Color::Rgb(80, 80, 110);
const INACTIVE_PH: Color = Color::Rgb(45, 45, 65);
const SELECTED_FG: Color = Color::Rgb(255, 220, 80);
const SELECTED_MUTED: Color = Color::Rgb(150, 135, 70);
const CURSOR_BG: Color = Color::Rgb(40, 50, 80);
const DIM: Color = Color::Rgb(80, 80, 110);
const SUBMIT_FG: Color = Color::Rgb(30, 30, 50);
const SUBMIT_BG: Color = Color::Rgb(140, 255, 180);
const OVERLAY_BG: Color = Color::Rgb(20, 40, 30);

// ── The event fields — mirror `create_event_spec` in the calendar adapter ──────

const ACCOUNTS: &[&str] = &["Work", "Personal"];
const SHOW_AS: &[&str] = &[
    "Busy",
    "Free",
    "Tentative",
    "Out of office",
    "Working elsewhere",
];

/// A field's kind, holding its widget. Heterogeneous, so dispatch is by variant.
enum Field {
    /// Free text; `datetime` marks Start/End so a resolved-preview line shows.
    Text { input: TextInput, datetime: bool },
    /// Single-choice radio list plus its choice labels (for value read + height).
    Select {
        list: SelectList,
        choices: &'static [&'static str],
    },
    /// Boolean.
    Toggle(Toggle),
}

/// A field: the key it submits under, the column it lives in, and its widget.
struct Row {
    key: &'static str,
    title: &'static str,
    col: u8,
    field: Field,
}

// ── Widget styles (new_server chrome, per widget) ──────────────────────────────

fn ti_inactive_style() -> TextInputStyle {
    TextInputStyle::new()
        .prefix_color(ACCENT)
        .set_style(TextInputStyleType::Title, Style::default().fg(ACCENT))
        .set_style(TextInputStyleType::Input, Style::default().fg(INPUT_FG))
        .placeholder_color(INACTIVE_PH)
}

fn ti_active_style() -> TextInputStyle {
    TextInputStyle::new()
        .prefix_color(ACTIVE_ACCENT)
        .set_style(
            TextInputStyleType::Title,
            Style::default().fg(ACTIVE_ACCENT).bg(INPUT_BG),
        )
        .set_style(
            TextInputStyleType::Input,
            Style::default().fg(INPUT_FG).bg(INPUT_BG),
        )
        .placeholder_color(PLACEHOLDER)
}

fn tg_inactive_style() -> ToggleStyle {
    ToggleStyle::new()
        .prefix_color(ACCENT)
        .set_style(ToggleStyleType::Title, Style::default().fg(ACCENT))
        .set_style(ToggleStyleType::Value, Style::default().fg(INPUT_FG))
}

fn tg_active_style() -> ToggleStyle {
    ToggleStyle::new()
        .prefix_color(ACTIVE_ACCENT)
        .set_style(
            ToggleStyleType::Title,
            Style::default().fg(ACTIVE_ACCENT).bg(INPUT_BG),
        )
        .set_style(
            ToggleStyleType::Value,
            Style::default().fg(SELECTED_FG).bg(INPUT_BG),
        )
}

fn sl_inactive_style() -> SelectListStyle {
    SelectListStyle::default()
        .set_style(SelectListStyleType::Item, Style::default().fg(INPUT_FG))
        .set_style(
            SelectListStyleType::ItemSelected,
            Style::default().fg(SELECTED_MUTED),
        )
        .set_style(
            SelectListStyleType::ItemCursor,
            Style::default().fg(INPUT_FG),
        )
        .set_style(
            SelectListStyleType::ItemCursorSelected,
            Style::default().fg(SELECTED_MUTED),
        )
}

fn sl_active_style() -> SelectListStyle {
    SelectListStyle::default()
        .set_style(
            SelectListStyleType::Item,
            Style::default().fg(INPUT_FG).bg(INPUT_BG),
        )
        .set_style(
            SelectListStyleType::ItemSelected,
            Style::default().fg(SELECTED_FG).bg(INPUT_BG),
        )
        .set_style(
            SelectListStyleType::ItemCursor,
            Style::default().fg(INPUT_FG).bg(CURSOR_BG),
        )
        .set_style(
            SelectListStyleType::ItemCursorSelected,
            Style::default().fg(SELECTED_FG).bg(CURSOR_BG),
        )
}

fn sl_keymap() -> SelectListKeymap {
    SelectListKeymap {
        move_up: Keys::plain(Key::Up)
            .or_ctrl(Key::Char('h'))
            .or_plain(Key::Tab),
        move_down: Keys::plain(Key::Down).or_ctrl(Key::Char('l')),
        toggle: Keys::plain(Key::Char(' ')).or_ctrl(Key::Char(' ')),
        ..SelectListKeymap::default()
    }
}

// ── Field constructors ─────────────────────────────────────────────────────────

fn text(title: &'static str, col: u8, placeholder: &str, default: &str, datetime: bool) -> Row {
    let mut input = TextInput::default()
        .with_title(title)
        .with_placeholder(placeholder.to_string())
        .with_inactive_style(ti_inactive_style())
        .with_active_style(ti_active_style());
    if !default.is_empty() {
        input.attr(Attribute::Value, AttrValue::String(default.to_string()));
    }
    Row {
        key: title_key(title),
        title,
        col,
        field: Field::Text { input, datetime },
    }
}

fn select(title: &'static str, col: u8, choices: &'static [&'static str]) -> Row {
    let mut list = SelectList::default()
        .with_items(choices.to_vec())
        .with_marker(SelectionMarker::Radio)
        .with_mode(SelectionMode::Single)
        .with_keymap(sl_keymap())
        .with_inactive_style(sl_inactive_style())
        .with_active_style(sl_active_style());
    // Preselect the first option — the adapter's defaults (Work / Busy) lead.
    list.attr(
        Attribute::Custom(SELECT_LIST_ATTR_SELECTED),
        AttrValue::Payload(PropPayload::Vec(vec![PropValue::Usize(0)])),
    );
    Row {
        key: title_key(title),
        title,
        col,
        field: Field::Select { list, choices },
    }
}

fn toggle(title: &'static str, col: u8) -> Row {
    let t = Toggle::default()
        .with_title(title)
        .with_labels("Yes", "No")
        .with_value(false)
        .with_inactive_style(tg_inactive_style())
        .with_active_style(tg_active_style());
    Row {
        key: title_key(title),
        title,
        col,
        field: Field::Toggle(t),
    }
}

/// Maps a display title to the adapter's form key.
fn title_key(title: &str) -> &'static str {
    match title {
        "Title" => "title",
        "Calendar" => "account",
        "Start" => "start",
        "End" => "end",
        "All day" => "all_day",
        "Show as" => "show_as",
        "Location" => "location",
        "Notes" => "body",
        _ => "unknown",
    }
}

// ── App state ────────────────────────────────────────────────────────────────

struct App {
    active: usize,
    rows: Vec<Row>,
    overlay: Option<String>,
}

impl App {
    fn new() -> Self {
        let rows = vec![
            text("Title", 0, "Event title", "", false),
            text("Start", 0, "e.g. today 9:00", "today 9:00", true),
            text("End", 0, "e.g. today 10:00", "today 10:00", true),
            text("Location", 0, "Optional", "", false),
            text("Notes", 0, "Optional", "", false),
            select("Calendar", 1, ACCOUNTS),
            toggle("All day", 1),
            select("Show as", 1, SHOW_AS),
        ];
        let mut app = Self {
            active: 0,
            rows,
            overlay: None,
        };
        app.set_focus(0);
        app
    }

    fn set_focus(&mut self, idx: usize) {
        for (i, row) in self.rows.iter_mut().enumerate() {
            let flag = AttrValue::Flag(i == idx);
            match &mut row.field {
                Field::Text { input, .. } => input.attr(Attribute::Focus, flag),
                Field::Select { list, .. } => list.attr(Attribute::Focus, flag),
                Field::Toggle(t) => t.attr(Attribute::Focus, flag),
            }
        }
        self.active = idx;
    }

    fn focus_next(&mut self) {
        self.set_focus((self.active + 1) % self.rows.len());
    }

    fn focus_prev(&mut self) {
        let n = self.rows.len();
        self.set_focus(if self.active == 0 {
            n - 1
        } else {
            self.active - 1
        });
    }

    /// Collects the field values into (key, value) form, mirroring the
    /// `ActionInput::Form` map the adapter receives.
    fn values(&self) -> Vec<(&'static str, String)> {
        self.rows
            .iter()
            .map(|row| {
                let value = match &row.field {
                    Field::Text { input, .. } => match input.state() {
                        State::Single(StateValue::String(s)) => s,
                        _ => String::new(),
                    },
                    Field::Select { list, choices } => {
                        let idx = match list.state() {
                            State::Vec(v) => v.first().and_then(|sv| match sv {
                                StateValue::Usize(i) => Some(*i),
                                _ => None,
                            }),
                            _ => None,
                        };
                        idx.and_then(|i| choices.get(i))
                            .map(|s| s.to_string())
                            .unwrap_or_default()
                    }
                    Field::Toggle(t) => t.is_on().to_string(),
                };
                (row.key, value)
            })
            .collect()
    }
}

// ── Layout helpers ─────────────────────────────────────────────────────────────

/// Height a field occupies (title + value rows, plus preview / list items).
fn field_height(field: &Field) -> u16 {
    match field {
        Field::Text { datetime, .. } => {
            if *datetime {
                3
            } else {
                2
            }
        }
        Field::Toggle(_) => 2,
        Field::Select { choices, .. } => 1 + choices.len() as u16,
    }
}

// ── Event conversion (same as new_server) ──────────────────────────────────────

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

// ── Dry-run summary (mirrors the adapter's `execute_create`) ───────────────────

fn dry_run_summary(values: &[(&str, String)]) -> String {
    let get = |key: &str| {
        values
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.trim().to_string())
            .unwrap_or_default()
    };
    let title = get("title");
    let title = if title.is_empty() {
        "(untitled)".to_string()
    } else {
        title
    };
    let account = get("account");
    let show_as = {
        let s = get("show_as");
        if s.is_empty() { "Busy".to_string() } else { s }
    };
    let all_day = get("all_day") == "true";
    let location = get("location");

    let when = resolve_when(&get("start"), &get("end"), all_day);
    let mut msg =
        format!("\n  Would create \u{201c}{title}\u{201d}\n  in {account}: {when} [{show_as}]");
    if !location.is_empty() {
        msg.push_str(&format!("\n  @ {location}"));
    }
    msg.push_str("\n\n  Prototype — no write backend yet.\n  (any key closes)");
    msg
}

#[cfg(feature = "natural-date")]
fn resolve_when(start: &str, end: &str, all_day: bool) -> String {
    use chrono::Local;
    let now = Local::now();
    let s = natural_date::resolve_datetime(start, now);
    let e = natural_date::resolve_datetime(end, now);
    match (s, e) {
        (Some(s), _) if all_day => s
            .with_timezone(&Local)
            .format("%Y-%m-%d (all day)")
            .to_string(),
        (Some(s), Some(e)) => format!(
            "{} \u{2013} {}",
            s.with_timezone(&Local).format("%Y-%m-%d %H:%M"),
            e.with_timezone(&Local).format("%H:%M"),
        ),
        _ => format!("{start} \u{2013} {end} (unresolved)"),
    }
}

#[cfg(not(feature = "natural-date"))]
fn resolve_when(start: &str, end: &str, all_day: bool) -> String {
    if all_day {
        format!("{start} (all day)")
    } else {
        format!("{start} \u{2013} {end}")
    }
}

/// The dim resolved-instant preview shown under a Start/End field.
#[cfg(feature = "natural-date")]
fn datetime_preview_line(value: &str) -> Option<String> {
    not_yet_done_ratatui::datetime_preview(value, true, chrono::Local::now())
}

#[cfg(not(feature = "natural-date"))]
fn datetime_preview_line(_value: &str) -> Option<String> {
    None
}

// ── Render ───────────────────────────────────────────────────────────────────

fn render(app: &mut App, frame: &mut ratatui::Frame) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);

    let panel_w = 78u16.min(area.width);
    let panel_h = 26u16.min(area.height);
    let px = area.x + area.width.saturating_sub(panel_w) / 2;
    let py = area.y + area.height.saturating_sub(panel_h) / 2;
    let panel = Rect::new(px, py, panel_w, panel_h);
    frame.render_widget(Block::default().style(Style::default().bg(PANEL_BG)), panel);

    let inner = Rect::new(
        panel.x + 2,
        panel.y + 1,
        panel.width.saturating_sub(4),
        panel.height.saturating_sub(2),
    );

    // Heading
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("\u{2726} ", Style::default().fg(ACTIVE_ACCENT)),
            Span::styled(
                "New event",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
        ])),
        Rect { height: 1, ..inner },
    );

    // Two columns below the heading, leaving room for submit + help at the bottom.
    let content_top = inner.y + 2;
    let content_bottom = panel.y + panel.height.saturating_sub(3); // submit(1)+help(1)+pad
    let gap = 3u16;
    let col_w = inner.width.saturating_sub(gap) / 2;
    let col_x = [inner.x, inner.x + col_w + gap];
    let mut col_y = [content_top, content_top];

    for (i, row) in app.rows.iter_mut().enumerate() {
        let c = row.col as usize;
        let x = col_x[c];
        let mut y = col_y[c];
        let active = i == app.active;
        let h = field_height(&row.field);
        if y + h > content_bottom {
            continue; // out of vertical room — should not happen at design sizes
        }

        match &mut row.field {
            Field::Text { input, datetime } => {
                input.view(
                    frame,
                    Rect {
                        x,
                        y,
                        width: col_w,
                        height: 2,
                    },
                );
                if *datetime {
                    let raw = match input.state() {
                        State::Single(StateValue::String(s)) => s,
                        _ => String::new(),
                    };
                    let line = match datetime_preview_line(&raw) {
                        Some(p) => format!("  \u{2192} {p}"),
                        None => "  \u{2192} …".to_string(),
                    };
                    frame.render_widget(
                        Paragraph::new(Span::styled(line, Style::default().fg(DIM))),
                        Rect {
                            x,
                            y: y + 2,
                            width: col_w,
                            height: 1,
                        },
                    );
                }
            }
            Field::Toggle(t) => {
                t.view(
                    frame,
                    Rect {
                        x,
                        y,
                        width: col_w,
                        height: 2,
                    },
                );
            }
            Field::Select { list, choices } => {
                // Field label in the same `▍`-prefixed style the widgets use.
                let (prefix_fg, label_bg) = if active {
                    (ACTIVE_ACCENT, Some(INPUT_BG))
                } else {
                    (ACCENT, None)
                };
                let mut label_style = Style::default().fg(prefix_fg).add_modifier(Modifier::BOLD);
                if let Some(bg) = label_bg {
                    label_style = label_style.bg(bg);
                }
                let mut para = Paragraph::new(Line::from(vec![
                    Span::styled("\u{258d} ", Style::default().fg(prefix_fg)),
                    Span::styled(row.title, label_style),
                ]));
                if let Some(bg) = label_bg {
                    para = para.style(Style::default().bg(bg));
                }
                frame.render_widget(
                    para,
                    Rect {
                        x,
                        y,
                        width: col_w,
                        height: 1,
                    },
                );

                // The list itself has no prefix bar — draw the `▍ ` gutter here,
                // shifting the items right so they align under the label text and
                // pick up the same side stripe the other widgets have.
                let gutter = 2u16;
                let n = choices.len() as u16;
                list.view(
                    frame,
                    Rect {
                        x: x + gutter,
                        y: y + 1,
                        width: col_w.saturating_sub(gutter),
                        height: n,
                    },
                );
                let mut bar_style = Style::default().fg(prefix_fg);
                if let Some(bg) = label_bg {
                    bar_style = bar_style.bg(bg);
                }
                for iy in 0..n {
                    frame.render_widget(
                        Paragraph::new(Span::styled("\u{258d} ", bar_style)),
                        Rect {
                            x,
                            y: y + 1 + iy,
                            width: gutter,
                            height: 1,
                        },
                    );
                }
            }
        }

        y += h + 1; // field + one spacer row
        col_y[c] = y;
    }

    // Submit button (panel-wide, above the help line)
    let submit_y = panel.y + panel.height.saturating_sub(3);
    frame.render_widget(
        Paragraph::new("  [ Ctrl+\u{21b5} → Save ]  ").style(
            Style::default()
                .fg(SUBMIT_FG)
                .bg(SUBMIT_BG)
                .add_modifier(Modifier::BOLD),
        ),
        Rect::new(inner.x, submit_y, inner.width, 1),
    );

    // Help line
    let help_y = panel.y + panel.height.saturating_sub(1);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " C-j",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" next  ", Style::default().fg(DIM)),
            Span::styled(
                "C-k",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" prev  ", Style::default().fg(DIM)),
            Span::styled(
                "\u{2191}\u{2193}/C-l/h",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" nav  ", Style::default().fg(DIM)),
            Span::styled(
                "Spc",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" pick  ", Style::default().fg(DIM)),
            Span::styled(
                "Esc",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" quit", Style::default().fg(DIM)),
        ])),
        Rect::new(panel.x + 2, help_y, panel.width.saturating_sub(4), 1),
    );

    // Submit overlay
    if let Some(msg) = &app.overlay {
        let ow = 54u16.min(area.width);
        let oh = 11u16.min(area.height);
        let ox = area.x + area.width.saturating_sub(ow) / 2;
        let oy = area.y + area.height.saturating_sub(oh) / 2;
        let overlay = Rect::new(ox, oy, ow, oh);
        frame.render_widget(Clear, overlay);
        frame.render_widget(
            Paragraph::new(msg.as_str())
                .block(
                    Block::bordered()
                        .title(Span::styled(
                            " \u{2713} New event ",
                            Style::default()
                                .fg(ACTIVE_ACCENT)
                                .add_modifier(Modifier::BOLD),
                        ))
                        .style(Style::default().fg(INPUT_FG).bg(OVERLAY_BG)),
                )
                .style(Style::default().fg(INPUT_FG))
                .wrap(Wrap { trim: false }),
            overlay,
        );
    }
}

// ── Main loop ────────────────────────────────────────────────────────────────

fn run(mut terminal: DefaultTerminal) -> std::io::Result<()> {
    execute!(std::io::stdout(), SetCursorStyle::BlinkingBar)?;
    execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )?;

    let mut app = App::new();

    loop {
        terminal.draw(|f| render(&mut app, f))?;

        let Event::Key(k) = crossterm::event::read()? else {
            continue;
        };
        if k.kind != KeyEventKind::Press {
            continue;
        }

        // The overlay swallows everything: any key dismisses it.
        if app.overlay.is_some() {
            app.overlay = None;
            continue;
        }

        match (k.code, k.modifiers) {
            (KeyCode::Esc, _) => break,
            (KeyCode::Char('j'), KeyModifiers::CONTROL) => app.focus_next(),
            (KeyCode::Char('k'), KeyModifiers::CONTROL) => app.focus_prev(),
            (KeyCode::Enter, KeyModifiers::CONTROL) => {
                app.overlay = Some(dry_run_summary(&app.values()));
            }
            _ => {
                let active = app.active;
                match &mut app.rows[active].field {
                    Field::Text { input, .. } => {
                        let _ = input.on(&to_tuirealm_event(&k));
                    }
                    Field::Select { list, .. } => {
                        let _ = list.on(&to_tuirealm_event(&k));
                    }
                    Field::Toggle(t) => {
                        if k.code == KeyCode::Char(' ') {
                            t.perform(Cmd::Toggle);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn main() -> std::io::Result<()> {
    let terminal = ratatui::init();
    let result = run(terminal);
    let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    ratatui::restore();
    result
}
