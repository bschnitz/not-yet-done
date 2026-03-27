//! Beispiel: "New Server" Formular
//!
//! Felder:
//!   • Hostname  — darf nicht leer sein, keine Leerzeichen
//!   • Port      — muss eine Zahl 1–65535 sein
//!   • API Key   — mindestens 8 Zeichen
//!
//! Tab / Shift+Tab wechselt das aktive Feld.
//! Enter zeigt die gesammelten Werte an (simulierter Submit).
//! Esc beendet.

use crossterm::{cursor::SetCursorStyle, event::{Event, KeyCode, KeyEventKind}, execute};
use not_yet_done_ratatui::{
    LineStyle, TextInput, TextInputEvent, TextInputKeymap, TextInputState, TextInputStyle,
};

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
    DefaultTerminal,
};

// ── Farben ────────────────────────────────────────────────────────────────────

const BG:            Color = Color::Rgb(10,  10,  20);   // fast schwarz
const PANEL_BG:      Color = Color::Rgb(18,  18,  35);   // Panel-Hintergrund
const ACCENT:        Color = Color::Rgb(100, 180, 255);  // hellblau — Prefix & Titel
const INPUT_FG:      Color = Color::Rgb(230, 230, 255);  // Eingabetext
const INPUT_BG:      Color = Color::Rgb(28,  28,  50);   // Eingabe-Hintergrund
const PLACEHOLDER:   Color = Color::Rgb(80,  80, 110);   // gedimmter Placeholder-Text

const ERROR_FG:      Color = Color::Rgb(255, 100,  80);  // Rot für Fehler
const ACTIVE_ACCENT: Color = Color::Rgb(140, 255, 180);  // Grün für aktives Feld
const SUBMIT_FG:     Color = Color::Rgb(30,   30,  50);
const SUBMIT_BG:     Color = Color::Rgb(140, 255, 180);
const DIM:           Color = Color::Rgb(80,   80, 110);

// ── App-State ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field { Hostname, Port, ApiKey }

impl Field {
    fn next(self) -> Self {
        match self { Self::Hostname => Self::Port, Self::Port => Self::ApiKey, Self::ApiKey => Self::Hostname }
    }
    fn prev(self) -> Self {
        match self { Self::Hostname => Self::ApiKey, Self::Port => Self::Hostname, Self::ApiKey => Self::Port }
    }
}

struct App {
    active: Field,
    hostname: TextInputState,
    port:     TextInputState,
    api_key:  TextInputState,
    submitted: Option<String>,  // gesammelter Submit-Text
}

impl App {
    fn new() -> Self {
        Self {
            active:    Field::Hostname,
            hostname:  TextInputState::new(),
            port:      TextInputState::new(),
            api_key:   TextInputState::new(),
            submitted: None,
        }
    }

    fn state_mut(&mut self, f: Field) -> &mut TextInputState {
        match f { Field::Hostname => &mut self.hostname, Field::Port => &mut self.port, Field::ApiKey => &mut self.api_key }
    }

    fn validate_all(&mut self) -> bool {
        let mut ok = true;

        // Hostname
        let h = self.hostname.value().to_string();
        if h.is_empty() {
            self.hostname.set_error("Hostname darf nicht leer sein");
            ok = false;
        } else if h.contains(' ') {
            self.hostname.set_error("Keine Leerzeichen erlaubt");
            ok = false;
        } else {
            self.hostname.clear_error();
        }

        // Port
        let p = self.port.value().to_string();
        match p.parse::<u16>() {
            Ok(n) if n >= 1 => self.port.clear_error(),
            _ => { self.port.set_error("Muss eine Zahl 1–65535 sein"); ok = false; }
        }

        // API Key
        let k = self.api_key.value().to_string();
        if k.len() < 8 {
            self.api_key.set_error("Mindestens 8 Zeichen erforderlich");
            ok = false;
        } else {
            self.api_key.clear_error();
        }

        ok
    }

    fn validate_field(&mut self, f: Field) {
        match f {
            Field::Hostname => {
                let h = self.hostname.value().to_string();
                if h.is_empty() || h.contains(' ') {
                    // Fehler erst beim Submit, beim Tippen nur löschen
                } else {
                    self.hostname.clear_error();
                }
            }
            Field::Port => {
                let p = self.port.value().to_string();
                if p.is_empty() {
                    self.port.clear_error();
                } else if p.parse::<u16>().map(|n| n < 1).unwrap_or(true) {
                    self.port.set_error("Muss eine Zahl 1–65535 sein");
                } else {
                    self.port.clear_error();
                }
            }
            Field::ApiKey => {
                let k = self.api_key.value().to_string();
                if k.is_empty() {
                    self.api_key.clear_error();
                } else if k.len() < 8 {
                    self.api_key.set_error("Mindestens 8 Zeichen erforderlich");
                } else {
                    self.api_key.clear_error();
                }
            }
        }
    }
}

// ── Styling helper ────────────────────────────────────────────────────────────

fn make_style(is_active: bool) -> TextInputStyle {
    if is_active {
        TextInputStyle::new()
            .prefix_color(ACTIVE_ACCENT)
            .title_style(LineStyle::default().fg(ACTIVE_ACCENT).bg(INPUT_BG))
            .input_style(LineStyle::default().fg(INPUT_FG).bg(INPUT_BG))
            .placeholder_color(PLACEHOLDER)
            .error_style(LineStyle::default().fg(ERROR_FG))
    } else {
        TextInputStyle::new()
            .prefix_color(ACCENT)
            .title_style(LineStyle::default().fg(ACCENT))
            .input_style(LineStyle::default().fg(INPUT_FG))
            .placeholder_color(Color::Rgb(45, 45, 65))
            .error_style(LineStyle::default().fg(ERROR_FG))
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

fn render(app: &App, frame: &mut ratatui::Frame) {
    let area = frame.area();

    // Schwarzer Hintergrund
    frame.render_widget(
        Block::default().style(Style::default().bg(BG)),
        area,
    );

    // Zentriertes Panel: 52 breit, 24 hoch
    let panel_w = 52u16;
    let panel_h = 24u16;
    let px = area.x + area.width.saturating_sub(panel_w) / 2;
    let py = area.y + area.height.saturating_sub(panel_h) / 2;
    let panel = Rect::new(px, py, panel_w.min(area.width), panel_h.min(area.height));

    frame.render_widget(Block::default().style(Style::default().bg(PANEL_BG)), panel);

    let inner = Rect::new(panel.x + 2, panel.y + 1, panel.width.saturating_sub(4), panel.height.saturating_sub(2));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // Überschrift
            Constraint::Length(1),  // Leerzeile
            Constraint::Length(3),  // Hostname
            Constraint::Length(1),  // Abstand
            Constraint::Length(3),  // Port
            Constraint::Length(1),  // Abstand
            Constraint::Length(3),  // API Key
            Constraint::Length(1),  // Abstand
            Constraint::Length(1),  // Submit-Button
            Constraint::Min(0),     // Rest
        ])
        .split(inner);

    // Überschrift
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("✦ ", Style::default().fg(ACTIVE_ACCENT)),
            Span::styled("New Server", Style::default().fg(ACCENT).bold()),
        ])),
        chunks[0],
    );

    // Widgets rendern
    let keymap = TextInputKeymap::default();

    let widget_width = inner.width;

    TextInput::new("Hostname")
        .placeholder("z.B. api.example.com")
        .width(widget_width)
        .style(make_style(app.active == Field::Hostname))
        .keymap(keymap.clone())
        .render_with_state(chunks[2], frame.buffer_mut(), &app.hostname);

    TextInput::new("Port")
        .placeholder("z.B. 8080")
        .width(widget_width)
        .style(make_style(app.active == Field::Port))
        .keymap(keymap.clone())
        .render_with_state(chunks[4], frame.buffer_mut(), &app.port);

    TextInput::new("API Key")
        .placeholder("min. 8 Zeichen")
        .width(widget_width)
        .style(make_style(app.active == Field::ApiKey))
        .keymap(keymap.clone())
        .render_with_state(chunks[6], frame.buffer_mut(), &app.api_key);

    // Submit-Button
    let btn_text = "  [ Enter → Submit ]  ";
    frame.render_widget(
        Paragraph::new(btn_text).style(Style::default().fg(SUBMIT_FG).bg(SUBMIT_BG).bold()),
        chunks[8],
    );

    // Hilfezeile unten im Panel
    let help_y = panel.y + panel.height.saturating_sub(1);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Tab", Style::default().fg(ACCENT).bold()),
            Span::styled(" nächstes  ", Style::default().fg(DIM)),
            Span::styled("Esc", Style::default().fg(ACCENT).bold()),
            Span::styled(" beenden", Style::default().fg(DIM)),
        ])),
        Rect::new(panel.x + 2, help_y, panel.width.saturating_sub(4), 1),
    );

    // Submit-Overlay
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
                        .title(Span::styled(" ✓ Gespeichert ", Style::default().fg(ACTIVE_ACCENT).bold()))
                        .style(Style::default().fg(INPUT_FG).bg(Color::Rgb(20, 40, 30))),
                )
                .style(Style::default().fg(INPUT_FG)),
            overlay,
        );
    }

    // Aktives Widget nochmal konstruieren um cursor_position zu berechnen
    let active_state = match app.active {
        Field::Hostname => &app.hostname,
        Field::Port     => &app.port,
        Field::ApiKey   => &app.api_key,
    };

    if !active_state.value().is_empty() {
        let active_area = match app.active {
            Field::Hostname => chunks[2],
            Field::Port     => chunks[4],
            Field::ApiKey   => chunks[6],
        };
        let active_widget = TextInput::new("").width(inner.width);
        let pos = active_widget.cursor_position(active_area, active_state);
        frame.set_cursor_position(pos);
    }
}

// ── Main Loop ─────────────────────────────────────────────────────────────────

fn run(mut terminal: DefaultTerminal) -> std::io::Result<()> {
    // Cursor-Style einmalig setzen
    execute!(std::io::stdout(), SetCursorStyle::BlinkingBar)?;

    let keymap = TextInputKeymap::default();
    let mut app = App::new();

    loop {
        terminal.draw(|f| render(&app, f))?;

        let event = crossterm::event::read()?;

        // Overlay schließen
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

            match k.code {
                KeyCode::Esc => break,

                KeyCode::Tab => {
                    app.active = app.active.next();
                }
                KeyCode::BackTab => {
                    app.active = app.active.prev();
                }

                KeyCode::Enter => {
                    if app.validate_all() {
                        app.submitted = Some(format!(
                            "\n  Hostname : {}\n  Port     : {}\n  API Key  : {}\n\n  (beliebige Taste schließt)",
                            app.hostname.value(),
                            app.port.value(),
                            app.api_key.value(),
                        ));
                    }
                }

                _ => {
                    // Aktives Feld bekommt den Event
                    let active = app.active;
                    let state = app.state_mut(active);
                    if let TextInputEvent::Changed(_) = state.handle_event(&event, &keymap) {
                        // Live-Validierung bei jeder Änderung
                        app.validate_field(active);
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
    ratatui::restore();
    result
}
