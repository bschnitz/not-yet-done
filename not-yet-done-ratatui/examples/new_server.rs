//! Example: "New Server" form
//!
//! Demonstrates the tui-realm TextInput MockComponent.
//!
//! Fields:
//!   • Hostname  — must not be empty or contain spaces
//!   • Port      — must be a number 1–65535
//!   • API Key   — at least 8 characters
//!
//! Tab / Shift+Tab switches the active field.
//! Enter shows the collected values (simulated submit).
//! Esc exits.

use crossterm::{
    cursor::SetCursorStyle,
    event::{Event, KeyCode, KeyEventKind},
    execute,
};
use not_yet_done_ratatui::{
    TextInput, TextInputEvent, TextInputStyle, TextInputStyleType, ATTR_ERROR,
};
use tuirealm::{
    AttrValue, Attribute, Component, MockComponent, State, StateValue,
    event::{Key, KeyEvent, NoUserEvent},
};

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
    DefaultTerminal,
};
use ratatui::prelude::Stylize;


// ── Colours ───────────────────────────────────────────────────────────────────

const BG: Color         = Color::Rgb(10, 10, 20);
const PANEL_BG: Color   = Color::Rgb(18, 18, 35);
const ACCENT: Color     = Color::Rgb(100, 180, 255);
const INPUT_FG: Color   = Color::Rgb(230, 230, 255);
const INPUT_BG: Color   = Color::Rgb(28, 28, 50);
const PLACEHOLDER: Color = Color::Rgb(80, 80, 110);
const ERROR_FG: Color   = Color::Rgb(255, 100, 80);
const ACTIVE_ACCENT: Color = Color::Rgb(140, 255, 180);
const SUBMIT_FG: Color  = Color::Rgb(30, 30, 50);
const SUBMIT_BG: Color  = Color::Rgb(140, 255, 180);
const DIM: Color        = Color::Rgb(80, 80, 110);
const INACTIVE_PH: Color = Color::Rgb(45, 45, 65);
const OVERLAY_BG: Color = Color::Rgb(20, 40, 30);

// ── Field enum ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Hostname,
    Port,
    ApiKey,
}

impl Field {
    fn next(self) -> Self {
        match self {
            Self::Hostname => Self::Port,
            Self::Port     => Self::ApiKey,
            Self::ApiKey   => Self::Hostname,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Hostname => Self::ApiKey,
            Self::Port     => Self::Hostname,
            Self::ApiKey   => Self::Port,
        }
    }
}

// ── Style constructors ────────────────────────────────────────────────────────

fn inactive_style() -> TextInputStyle {
    TextInputStyle::new()
        .prefix_color(ACCENT)
        .set_style(TextInputStyleType::Title, Style::default().fg(ACCENT))
        .set_style(TextInputStyleType::Input, Style::default().fg(INPUT_FG))
        .placeholder_color(INACTIVE_PH)
        .set_style(TextInputStyleType::Error, Style::default().fg(ERROR_FG))
}

fn active_style() -> TextInputStyle {
    TextInputStyle::new()
        .prefix_color(ACTIVE_ACCENT)
        .set_style(TextInputStyleType::Title,
            Style::default().fg(ACTIVE_ACCENT).bg(INPUT_BG))
        .set_style(TextInputStyleType::Input,
            Style::default().fg(INPUT_FG).bg(INPUT_BG))
        .placeholder_color(PLACEHOLDER)
        .set_style(TextInputStyleType::Error, Style::default().fg(ERROR_FG))
}

/// Creates a [`TextInput`] component with both focus styles already applied.
fn make_input(title: &str, placeholder: &str) -> TextInput {
    TextInput::default()
        .with_title(title)
        .with_placeholder(placeholder)
        .with_inactive_style(inactive_style())
        .with_active_style(active_style())
}

// ── App state ─────────────────────────────────────────────────────────────────

struct App {
    active:    Field,
    hostname:  TextInput,
    port:      TextInput,
    api_key:   TextInput,
    submitted: Option<String>,
}

impl App {
    fn new() -> Self {
        let mut app = Self {
            active:    Field::Hostname,
            hostname:  make_input("Hostname", "e.g. api.example.com"),
            port:      make_input("Port", "e.g. 8080"),
            api_key:   make_input("API Key", "min. 8 characters"),
            submitted: None,
        };
        // Grant initial focus to the first field.
        app.hostname.attr(Attribute::Focus, AttrValue::Flag(true));
        app
    }

    /// Moves focus to `field`, updating both the internal flag and the
    /// components' focus attributes.
    fn set_focus(&mut self, field: Field) {
        self.hostname.attr(Attribute::Focus, AttrValue::Flag(false));
        self.port    .attr(Attribute::Focus, AttrValue::Flag(false));
        self.api_key .attr(Attribute::Focus, AttrValue::Flag(false));
        self.component_mut(field).attr(Attribute::Focus, AttrValue::Flag(true));
        self.active = field;
    }

    fn component_mut(&mut self, field: Field) -> &mut TextInput {
        match field {
            Field::Hostname => &mut self.hostname,
            Field::Port     => &mut self.port,
            Field::ApiKey   => &mut self.api_key,
        }
    }

    /// Reads the current string value from a component.
    fn value_of(&self, field: Field) -> String {
        let component = match field {
            Field::Hostname => &self.hostname,
            Field::Port     => &self.port,
            Field::ApiKey   => &self.api_key,
        };
        match component.state() {
            State::One(StateValue::String(s)) => s,
            _ => String::new(),
        }
    }

    fn set_error(&mut self, field: Field, msg: &str) {
        self.component_mut(field)
            .attr(Attribute::Custom(ATTR_ERROR), AttrValue::String(msg.into()));
    }

    fn clear_error(&mut self, field: Field) {
        self.component_mut(field)
            .attr(Attribute::Custom(ATTR_ERROR), AttrValue::Flag(false));
    }

    /// Validates all fields. Returns `true` when every field is valid.
    fn validate_all(&mut self) -> bool {
        let mut ok = true;

        let h = self.value_of(Field::Hostname);
        if h.is_empty() {
            self.set_error(Field::Hostname, "Hostname must not be empty");
            ok = false;
        } else if h.contains(' ') {
            self.set_error(Field::Hostname, "No spaces allowed");
            ok = false;
        } else {
            self.clear_error(Field::Hostname);
        }

        let p = self.value_of(Field::Port);
        match p.parse::<u16>() {
            Ok(n) if n >= 1 => self.clear_error(Field::Port),
            _ => {
                self.set_error(Field::Port, "Must be a number 1–65535");
                ok = false;
            }
        }

        let k = self.value_of(Field::ApiKey);
        if k.len() < 8 {
            self.set_error(Field::ApiKey, "At least 8 characters required");
            ok = false;
        } else {
            self.clear_error(Field::ApiKey);
        }

        ok
    }

    /// Live-validates the currently active field after each keystroke.
    fn validate_active(&mut self) {
        let field = self.active;
        match field {
            Field::Hostname => {
                let h = self.value_of(Field::Hostname);
                if !h.is_empty() && !h.contains(' ') {
                    self.clear_error(Field::Hostname);
                }
            }
            Field::Port => {
                let p = self.value_of(Field::Port);
                if p.is_empty() {
                    self.clear_error(Field::Port);
                } else if p.parse::<u16>().map(|n| n < 1).unwrap_or(true) {
                    self.set_error(Field::Port, "Must be a number 1–65535");
                } else {
                    self.clear_error(Field::Port);
                }
            }
            Field::ApiKey => {
                let k = self.value_of(Field::ApiKey);
                if k.is_empty() {
                    self.clear_error(Field::ApiKey);
                } else if k.len() < 8 {
                    self.set_error(Field::ApiKey, "At least 8 characters required");
                } else {
                    self.clear_error(Field::ApiKey);
                }
            }
        }
    }
}

// ── Event conversion ──────────────────────────────────────────────────────────

/// Converts a crossterm [`KeyEvent`](crossterm::event::KeyEvent) into a
/// tuirealm [`Event`] so it can be dispatched to a [`Component`].
fn to_tuirealm_event(
    k: &crossterm::event::KeyEvent,
) -> tuirealm::event::Event<NoUserEvent> {
    let code = match k.code {
        KeyCode::Char(c)  => Key::Char(c),
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete   => Key::Delete,
        KeyCode::Left     => Key::Left,
        KeyCode::Right    => Key::Right,
        KeyCode::Up       => Key::Up,
        KeyCode::Down     => Key::Down,
        KeyCode::Enter    => Key::Enter,
        KeyCode::Esc      => Key::Esc,
        KeyCode::Tab      => Key::Tab,
        KeyCode::BackTab  => Key::BackTab,
        KeyCode::Home     => Key::Home,
        KeyCode::End      => Key::End,
        KeyCode::PageUp   => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::F(n)     => Key::Function(n),
        _                 => Key::Null,
    };
    tuirealm::event::Event::Keyboard(KeyEvent { code, modifiers: k.modifiers.into() })
}

// ── Render ────────────────────────────────────────────────────────────────────

/// Renders the entire UI.
///
/// `app` must be `&mut` because [`MockComponent::view`] takes `&mut self`.
fn render(app: &mut App, frame: &mut ratatui::Frame) {
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
            Constraint::Length(3), // hostname
            Constraint::Length(1), // spacer
            Constraint::Length(3), // port
            Constraint::Length(1), // spacer
            Constraint::Length(3), // api key
            Constraint::Length(1), // spacer
            Constraint::Length(1), // submit button
            Constraint::Min(0),
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

    // Fields — view() selects active/inactive style internally and places the cursor.
    app.hostname.view(frame, chunks[2]);
    app.port    .view(frame, chunks[4]);
    app.api_key .view(frame, chunks[6]);

    // Submit button
    frame.render_widget(
        Paragraph::new("  [ Enter → Submit ]  ")
            .style(Style::default().fg(SUBMIT_FG).bg(SUBMIT_BG).bold()),
        chunks[8],
    );

    // Help line
    let help_y = panel.y + panel.height.saturating_sub(1);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Tab", Style::default().fg(ACCENT).bold()),
            Span::styled(" next  ", Style::default().fg(DIM)),
            Span::styled("Esc", Style::default().fg(ACCENT).bold()),
            Span::styled(" quit", Style::default().fg(DIM)),
        ])),
        Rect::new(panel.x + 2, help_y, panel.width.saturating_sub(4), 1),
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
        terminal.draw(|f| render(&mut app, f))?;

        let event = crossterm::event::read()?;

        if app.submitted.is_some() {
            if let Event::Key(k) = &event {
                if k.kind == KeyEventKind::Press {
                    app.submitted = None;
                }
            }
            continue;
        }

        let Event::Key(k) = event else { continue };
        if k.kind != KeyEventKind::Press {
            continue;
        }

        match k.code {
            KeyCode::Esc => break,

            KeyCode::Tab => {
                let next = app.active.next();
                app.set_focus(next);
            }
            KeyCode::BackTab => {
                let prev = app.active.prev();
                app.set_focus(prev);
            }

            KeyCode::Enter => {
                if app.validate_all() {
                    app.submitted = Some(format!(
                        "\n  Hostname : {}\n  Port     : {}\n  API Key  : {}\n\n  (any key closes)",
                        app.value_of(Field::Hostname),
                        app.value_of(Field::Port),
                        app.value_of(Field::ApiKey),
                    ));
                }
            }

            _ => {
                // Dispatch all other keys to the active component.
                // on() returns Some(TextInputEvent::Changed) when the value changed.
                let tui_ev = to_tuirealm_event(&k);
                if let Some(TextInputEvent::Changed(_)) =
                    app.component_mut(app.active).on(tui_ev)
                {
                    app.validate_active();
                }
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
