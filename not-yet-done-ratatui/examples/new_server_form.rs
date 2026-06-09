//! Example: "New Server" form — Form widget with custom keymaps
//!
//! Demonstrates the `Form` widget with:
//!   • Fully customised keybindings at all three levels
//!   • A `MultiChoice` dropdown placed in the middle of the form to show that
//!     the `Form` widget handles render order — the open drop-down correctly
//!     overlaps the fields below it without any manual z-ordering.
//!
//! Custom bindings used here (all differ from the defaults):
//!
//!   Form navigation:
//!     Ctrl+N   — next field         (default: Tab)
//!     Ctrl+P   — previous field     (default: Shift+Tab)
//!     Ctrl+S   — submit / confirm   (default: Enter)
//!
//!   TextInput editing:
//!     Ctrl+B   — move cursor left   (default: ←)
//!     Ctrl+F   — move cursor right  (default: →)
//!     Ctrl+H   — delete backwards   (default: Backspace)
//!     Ctrl+D   — delete forwards    (default: Delete)
//!     Ctrl+K   — clear field        (default: Ctrl+U)
//!
//!   MultiChoice (Protocol dropdown):
//!     ↑ / ↓   — move cursor        (default: Ctrl+K / Ctrl+J)
//!     Space   — toggle selection   (unchanged)
//!
//! Fields:
//!   • Hostname   — must not be empty or contain spaces
//!   • Protocol   — MultiChoice: HTTP | HTTPS | WS | WSS  ← in the MIDDLE
//!   • Port       — must be a number 1–65535
//!   • API Key    — at least 8 characters
//!
//! Esc exits.

use crossterm::{
    cursor::SetCursorStyle,
    event::{Event, KeyCode, KeyEventKind},
    execute,
};
use not_yet_done_ratatui::{
    FieldEvent, Form, FormEvent, FormField, FormFieldState, FormKeymap, FormState, FormStyle,
    FormWidgetStyle, KeyBinding, MultiChoice, MultiChoiceKeymap, MultiChoiceState, TextInput,
    TextInputEvent, TextInputKeymap, TextInputState
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
    DefaultTerminal,
};

// ── Colours ───────────────────────────────────────────────────────────────────

const BG: Color = Color::Rgb(10, 10, 20);
const PANEL_BG: Color = Color::Rgb(18, 18, 35);
const FIELD_BG: Color = Color::Rgb(28, 28, 50);
const ACCENT: Color = Color::Rgb(100, 180, 255);
const INPUT_FG: Color = Color::Rgb(230, 230, 255);
const INACTIVE_PH: Color = Color::Rgb(45, 45, 65);
const ACTIVE_ACCENT: Color = Color::Rgb(140, 255, 180);
const ACTIVE_PH: Color = Color::Rgb(70, 90, 70);
const ERROR_FG: Color = Color::Rgb(255, 100, 80);
const SUBMIT_FG: Color = Color::Rgb(30, 30, 50);
const SUBMIT_BG: Color = Color::Rgb(140, 255, 180);
const DIM: Color = Color::Rgb(80, 80, 110);
const OVERLAY_BG: Color = Color::Rgb(20, 40, 30);
const MC_SELECTED_BG: Color = Color::Rgb(30, 50, 45);
const ACTIVE_INPUT_FG: Color = Color::Rgb(255, 220, 80);

const PROTOCOLS: &[&str] = &["HTTP", "HTTPS", "WS", "WSS"];

// ── Field indices ─────────────────────────────────────────────────────────────
//
// Protocol (MultiChoice) is placed second — between Hostname and Port — so
// the Form's last-render guarantee is clearly visible: when the drop-down is
// open it overlaps the fields below it.

const IDX_HOSTNAME: usize = 0;
const IDX_PROTOCOL: usize = 1;
const IDX_PORT: usize = 2;
const IDX_APIKEY: usize = 3;

// ── Custom keymaps ────────────────────────────────────────────────────────────

/// Custom form-level keymap — replaces Tab/Shift+Tab/Enter with Ctrl+N/P/S.
///
/// Pass this to `Form::new().keymap(form_keymap())`.
fn form_keymap() -> FormKeymap {
    FormKeymap {
        focus_next: KeyBinding::ctrl(KeyCode::Char('n')), // default: Tab
        focus_prev: KeyBinding::ctrl(KeyCode::Char('p')), // default: Shift+Tab
        confirm:    KeyBinding::ctrl(KeyCode::Char('s')), // default: Enter
    }
}

/// Custom TextInput keymap — Emacs-style editing keys.
///
/// Pass this to each `TextInput::new(…).keymap(text_input_keymap())`.
fn text_input_keymap() -> TextInputKeymap {
    TextInputKeymap {
        move_left:   KeyBinding::ctrl(KeyCode::Char('b')), // default: ←
        move_right:  KeyBinding::ctrl(KeyCode::Char('f')), // default: →
        delete_back: KeyBinding::ctrl(KeyCode::Char('h')), // default: Backspace
        delete_fwd:  KeyBinding::ctrl(KeyCode::Char('d')), // default: Delete
        clear:       KeyBinding::ctrl(KeyCode::Char('k')), // default: Ctrl+U
    }
}

/// Custom MultiChoice keymap — arrow keys instead of Ctrl+J/K.
///
/// Pass this to `MultiChoice::new(…).keymap(multi_choice_keymap())`.
fn multi_choice_keymap() -> MultiChoiceKeymap {
    MultiChoiceKeymap {
        move_down: KeyBinding::new(KeyCode::Down),  // default: Ctrl+J
        move_up:   KeyBinding::new(KeyCode::Up),    // default: Ctrl+K
        toggle:    KeyBinding::new(KeyCode::Char(' ')), // default: Space (unchanged)
    }
}

// ── App state ─────────────────────────────────────────────────────────────────

struct App {
    form_state: FormState,
    submitted: Option<String>,
}

impl App {
    fn new() -> Self {
        Self {
            form_state: FormState::new(vec![
                FormFieldState::TextInput(TextInputState::new()),
                FormFieldState::MultiChoice(MultiChoiceState::new(PROTOCOLS.len())),
                FormFieldState::TextInput(TextInputState::new()),
                FormFieldState::TextInput(TextInputState::new()),
            ]),
            submitted: None,
        }
    }

    fn validate_all(&mut self) -> bool {
        let mut ok = true;

        if let Some(ts) = self.form_state.field_mut(IDX_HOSTNAME).and_then(|f| f.as_text_input_mut()) {
            let v = ts.value().to_string();
            if v.is_empty() {
                ts.set_error("Must not be empty");
                ok = false;
            } else if v.contains(' ') {
                ts.set_error("No spaces allowed");
                ok = false;
            } else {
                ts.clear_error();
            }
        }

        if let Some(ts) = self.form_state.field_mut(IDX_PORT).and_then(|f| f.as_text_input_mut()) {
            let v = ts.value().to_string();
            match v.parse::<u16>() {
                Ok(n) if n >= 1 => ts.clear_error(),
                _ => {
                    ts.set_error("Must be a number 1–65535");
                    ok = false;
                }
            }
        }

        if let Some(ts) = self.form_state.field_mut(IDX_APIKEY).and_then(|f| f.as_text_input_mut()) {
            let v = ts.value().to_string();
            if v.len() < 8 {
                ts.set_error("At least 8 characters required");
                ok = false;
            } else {
                ts.clear_error();
            }
        }

        ok
    }

    fn validate_field(&mut self, index: usize) {
        match index {
            IDX_HOSTNAME => {
                if let Some(ts) = self.form_state.field_mut(IDX_HOSTNAME).and_then(|f| f.as_text_input_mut()) {
                    let v = ts.value().to_string();
                    if v.is_empty() {
                        ts.clear_error();
                    } else if v.contains(' ') {
                        ts.set_error("No spaces allowed");
                    } else {
                        ts.clear_error();
                    }
                }
            }
            IDX_PORT => {
                if let Some(ts) = self.form_state.field_mut(IDX_PORT).and_then(|f| f.as_text_input_mut()) {
                    let v = ts.value().to_string();
                    if v.is_empty() {
                        ts.clear_error();
                    } else if v.parse::<u16>().map(|n| n < 1).unwrap_or(true) {
                        ts.set_error("Must be a number 1–65535");
                    } else {
                        ts.clear_error();
                    }
                }
            }
            IDX_APIKEY => {
                if let Some(ts) = self.form_state.field_mut(IDX_APIKEY).and_then(|f| f.as_text_input_mut()) {
                    let v = ts.value().to_string();
                    if v.is_empty() {
                        ts.clear_error();
                    } else if v.len() < 8 {
                        ts.set_error("At least 8 characters required");
                    } else {
                        ts.clear_error();
                    }
                }
            }
            _ => {}
        }
    }

    fn collect_values(&self) -> String {
        let hostname = self.form_state.field(IDX_HOSTNAME)
            .and_then(|f| f.as_text_input())
            .map(|ts| ts.value().to_string())
            .unwrap_or_default();
        let protocols = self.form_state.field(IDX_PROTOCOL)
            .and_then(|f| f.as_multi_choice())
            .map(|ms| {
                ms.selected_indices().iter()
                    .filter_map(|&i| PROTOCOLS.get(i))
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let port = self.form_state.field(IDX_PORT)
            .and_then(|f| f.as_text_input())
            .map(|ts| ts.value().to_string())
            .unwrap_or_default();
        let api_key = self.form_state.field(IDX_APIKEY)
            .and_then(|f| f.as_text_input())
            .map(|ts| ts.value().to_string())
            .unwrap_or_default();
        format!(
            "\n  Hostname : {}\n  Protocol : {}\n  Port     : {}\n  API Key  : {}\n\n  (any key closes)",
            hostname,
            if protocols.is_empty() { "—".to_string() } else { protocols },
            port,
            api_key,
        )
    }
}

// ── Form factory ──────────────────────────────────────────────────────────────

/// Builds the `Form` widget.
///
/// The custom `TextInputKeymap` is applied to each field individually via
/// `TextInput::keymap()`.  The custom `FormKeymap` is applied to the form
/// itself via `Form::keymap()`.
fn make_form() -> Form<'static> {
    let global_style = FormStyle::new()
        .active(
            FormWidgetStyle::new()
                .prefix_color(ACTIVE_ACCENT)
                .title(Style::default().fg(ACTIVE_ACCENT).bg(FIELD_BG))
                .body(Style::default().fg(INPUT_FG).bg(FIELD_BG))
                .placeholder(ACTIVE_PH)
                .error(Style::default().fg(ERROR_FG))
                .mc_cursor(Style::default().fg(ACTIVE_INPUT_FG).bg(FIELD_BG))
                .mc_selected(Style::default().fg(INPUT_FG).bg(MC_SELECTED_BG))
                .mc_selected_cursor(Style::default().fg(ACTIVE_INPUT_FG).bg(MC_SELECTED_BG))
                .mc_closing_line(Style::default().bg(PANEL_BG)),
        )
        .inactive(
            FormWidgetStyle::new()
                .prefix_color(ACCENT)
                .title(Style::default().fg(ACCENT))
                .body(Style::default().fg(INPUT_FG))
                .placeholder(INACTIVE_PH)
                .error(Style::default().fg(ERROR_FG)),
        );

    // Each TextInput gets the custom keymap.  Style slots are left as None
    // so the form global style fills them all in.
    let km = text_input_keymap();

    Form::new()
        .style(global_style)
        .spacing(1)
        .keymap(form_keymap()) // ← custom form-level keymap
        .field(FormField::text_input(
            TextInput::new("Hostname")
                .placeholder("e.g. api.example.com")
                .keymap(km.clone()), // ← custom TextInput keymap
        ))
        .field(FormField::multi_choice(
            // Protocol is placed second (not last!) to demonstrate that the
            // Form renders the active field last — the open drop-down will
            // correctly overlap Port and API Key below it.
            MultiChoice::new("Protocol", PROTOCOLS)
                .placeholder("Choose protocol(s)")
                .keymap(multi_choice_keymap()) // ← custom MC keymap
        )
        .with_height(3)) // same height as TextInput fields
        .field(FormField::text_input(
            TextInput::new("Port")
                .placeholder("e.g. 8080")
                .keymap(km.clone()),
        ))
        .field(FormField::text_input(
            TextInput::new("API Key")
                .placeholder("min. 8 characters")
                .keymap(km),
        ))
}

// ── Render ────────────────────────────────────────────────────────────────────

fn render(app: &App, frame: &mut ratatui::Frame) {
    let area = frame.area();

    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);

    let panel_w = 52u16;
    let panel_h = 24u16;
    let px = area.x + area.width.saturating_sub(panel_w) / 2;
    let py = area.y + area.height.saturating_sub(panel_h) / 2;
    let panel = Rect::new(px, py, panel_w.min(area.width), panel_h.min(area.height));

    frame.render_widget(Block::default().style(Style::default().bg(PANEL_BG)), panel);

    let inner = Rect::new(
        panel.x + 2,
        panel.y + 1,
        panel.width.saturating_sub(4),
        panel.height.saturating_sub(2),
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // heading
            Constraint::Length(1), // spacer
            Constraint::Min(0),    // form area
            Constraint::Length(1), // spacer
            Constraint::Length(1), // submit button
            Constraint::Length(1), // spacer
            Constraint::Length(1), // help line
        ])
        .split(inner);

    // Heading
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("✦ ", Style::default().fg(ACTIVE_ACCENT)),
            Span::styled("New Server", Style::default().fg(ACCENT).bold()),
        ])),
        chunks[0],
    );

    // Form
    let form = make_form();
    let form_area = chunks[2];
    form.render_with_state(form_area, frame.buffer_mut(), &app.form_state);

    if let Some(pos) = form.cursor_position(form_area, &app.form_state) {
        frame.set_cursor_position(pos);
    }

    // Submit button
    frame.render_widget(
        Paragraph::new("  [ Ctrl+S → Submit ]  ")
            .style(Style::default().fg(SUBMIT_FG).bg(SUBMIT_BG).bold()),
        chunks[4],
    );

    // Help line
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Ctrl+N", Style::default().fg(ACCENT).bold()),
            Span::styled(" next  ", Style::default().fg(DIM)),
            Span::styled("Ctrl+P", Style::default().fg(ACCENT).bold()),
            Span::styled(" prev  ", Style::default().fg(DIM)),
            Span::styled("Ctrl+S", Style::default().fg(ACCENT).bold()),
            Span::styled(" submit  ", Style::default().fg(DIM)),
            Span::styled("Esc", Style::default().fg(ACCENT).bold()),
            Span::styled(" quit", Style::default().fg(DIM)),
        ])),
        chunks[6],
    );

    // Submit overlay
    if let Some(msg) = &app.submitted {
        let ow = 50u16;
        let oh = 10u16;
        let ox = area.x + area.width.saturating_sub(ow) / 2;
        let oy = area.y + area.height.saturating_sub(oh) / 2;
        let overlay = Rect::new(ox, oy, ow.min(area.width), oh.min(area.height));

        frame.render_widget(Clear, overlay);
        frame.render_widget(
            Paragraph::new(msg.as_str())
                .block(
                    Block::bordered()
                        .title(Span::styled(
                            " ✓ Saved ",
                            Style::default().fg(ACTIVE_ACCENT).bold(),
                        ))
                        .style(Style::default().fg(INPUT_FG).bg(OVERLAY_BG)),
                )
                .style(Style::default().fg(INPUT_FG)),
            overlay,
        );
    }
}

// ── Main loop ─────────────────────────────────────────────────────────────────

fn run(mut terminal: DefaultTerminal) -> std::io::Result<()> {
    execute!(std::io::stdout(), SetCursorStyle::BlinkingBar)?;

    let mut app = App::new();

    loop {
        terminal.draw(|f| render(&app, f))?;

        let event = crossterm::event::read()?;

        if app.submitted.is_some() {
            if let Event::Key(k) = &event {
                if k.kind == KeyEventKind::Press {
                    app.submitted = None;
                }
            }
            continue;
        }

        if let Event::Key(k) = &event {
            if k.kind != KeyEventKind::Press {
                continue;
            }

            if k.code == KeyCode::Esc {
                break;
            }

            match make_form().handle_event(&event, &mut app.form_state) {
                FormEvent::Submit => {
                    if app.validate_all() {
                        app.submitted = Some(app.collect_values());
                    }
                }
                FormEvent::FocusChanged { .. } => {}
                FormEvent::FieldEvent { index, event } => {
                    if let FieldEvent::TextInput(TextInputEvent::Changed(_)) = event {
                        app.validate_field(index);
                    }
                }
                FormEvent::Ignored => {}
            }
        }
    }

    Ok(())
}

fn main() -> std::io::Result<()> {
    let terminal = ratatui::init();
    let result = run(terminal);
    ratatui::restore();
    result
}
