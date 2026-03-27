//! Beispiel: "New User Profile" Formular
//!
//! Felder:
//!   • Rolle       — Mehrfachauswahl aus ["Admin", "Editor", "Viewer", "Guest"]
//!   • Benutzername — mindestens 3 Zeichen
//!   • E-Mail      — muss ein gültiges E-Mail-Format haben
//!
//! Tab / Shift+Tab wechselt das aktive Feld.
//! Enter zeigt die gesammelten Werte an (simulierter Submit).
//! Esc beendet.

use crossterm::{
    cursor::SetCursorStyle,
    event::{Event, KeyCode, KeyEventKind},
    execute,
};
use not_yet_done_ratatui::{
    MultiChoice, MultiChoiceEvent, MultiChoiceKeymap, MultiChoiceState, MultiChoiceStyle,
    MultiChoiceStyleType, TextInput, TextInputEvent, TextInputKeymap, TextInputState,
    TextInputStyle, TextInputStyleType,
};

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
    DefaultTerminal,
};

// ── Farben ────────────────────────────────────────────────────────────────────

const BG: Color = Color::Rgb(10, 10, 20); // fast schwarz
const PANEL_BG: Color = Color::Rgb(18, 18, 35); // Panel-Hintergrund
const ACCENT: Color = Color::Rgb(100, 180, 255); // hellblau — Prefix & Titel
const INPUT_FG: Color = Color::Rgb(230, 230, 255); // Eingabetext
const INPUT_BG: Color = Color::Rgb(28, 28, 50); // Eingabe-Hintergrund
const PLACEHOLDER: Color = Color::Rgb(80, 80, 110); // gedimmter Placeholder-Text
const SELECTED_MC_BG: Color = Color::Rgb(35, 45, 65);
const ACTIVE_INPUT_FG: Color = Color::Rgb(255, 215, 0);

const ERROR_FG: Color = Color::Rgb(255, 100, 80); // Rot für Fehler
const ACTIVE_ACCENT: Color = Color::Rgb(140, 255, 180); // Grün für aktives Feld
const SUBMIT_FG: Color = Color::Rgb(30, 30, 50);
const SUBMIT_BG: Color = Color::Rgb(140, 255, 180);
const DIM: Color = Color::Rgb(80, 80, 110);

// ── App-State ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Role,
    Username,
    Email,
}

impl Field {
    fn next(self) -> Self {
        match self {
            Self::Role => Self::Username,
            Self::Username => Self::Email,
            Self::Email => Self::Role,
        }
    }
    fn prev(self) -> Self {
        match self {
            Self::Role => Self::Email,
            Self::Username => Self::Role,
            Self::Email => Self::Username,
        }
    }
}

struct App {
    active: Field,
    role: MultiChoiceState,
    username: TextInputState,
    email: TextInputState,
    submitted: Option<String>, // gesammelter Submit-Text
}

impl App {
    fn new() -> Self {
        Self {
            active: Field::Role,
            role: MultiChoiceState::new(4),
            username: TextInputState::new(),
            email: TextInputState::new(),
            submitted: None,
        }
    }

    fn state_mut(&mut self, f: Field) -> &mut dyn std::any::Any {
        match f {
            Field::Role => &mut self.role as &mut dyn std::any::Any,
            Field::Username => &mut self.username as &mut dyn std::any::Any,
            Field::Email => &mut self.email as &mut dyn std::any::Any,
        }
    }

    fn validate_all(&mut self) -> bool {
        let mut ok = true;

        // Benutzername
        let u = self.username.value().to_string();
        if u.len() < 3 {
            self.username.set_error("Mindestens 3 Zeichen erforderlich");
            ok = false;
        } else {
            self.username.clear_error();
        }

        // E-Mail
        let e = self.email.value().to_string();
        if !e.contains('@') || !e.contains('.') {
            self.email.set_error("Ungültiges E-Mail-Format");
            ok = false;
        } else {
            self.email.clear_error();
        }

        ok
    }

    fn validate_field(&mut self, f: Field) {
        match f {
            Field::Username => {
                let u = self.username.value().to_string();
                if u.is_empty() {
                    self.username.clear_error();
                } else if u.len() < 3 {
                    self.username.set_error("Mindestens 3 Zeichen erforderlich");
                } else {
                    self.username.clear_error();
                }
            }
            Field::Email => {
                let e = self.email.value().to_string();
                if e.is_empty() {
                    self.email.clear_error();
                } else if !e.contains('@') || !e.contains('.') {
                    self.email.set_error("Ungültiges E-Mail-Format");
                } else {
                    self.email.clear_error();
                }
            }
            Field::Role => {}
        }
    }
}

// ── Styling helper ────────────────────────────────────────────────────────────

fn make_style(is_active: bool) -> TextInputStyle {
    if is_active {
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
            .set_style(TextInputStyleType::Error, Style::default().fg(ERROR_FG))
    } else {
        TextInputStyle::new()
            .prefix_color(ACCENT)
            .set_style(TextInputStyleType::Title, Style::default().fg(ACCENT))
            .set_style(TextInputStyleType::Input, Style::default().fg(INPUT_FG))
            .placeholder_color(Color::Rgb(45, 45, 65))
            .set_style(TextInputStyleType::Error, Style::default().fg(ERROR_FG))
    }
}

fn make_mc_style(is_active: bool) -> MultiChoiceStyle {
    if is_active {
        MultiChoiceStyle::new()
            .prefix_color(ACTIVE_ACCENT)
            // Titel
            .set_style(
                MultiChoiceStyleType::Title,
                Style::default().fg(ACTIVE_ACCENT).bg(INPUT_BG),
            )
            // Einträge
            .set_style(
                MultiChoiceStyleType::Normal,
                Style::default().fg(INPUT_FG).bg(INPUT_BG),
            )
            .set_style(
                MultiChoiceStyleType::Active,
                Style::default().fg(ACTIVE_INPUT_FG).bg(INPUT_BG),
            )
            .set_style(
                MultiChoiceStyleType::Selected,
                Style::default().fg(INPUT_FG).bg(SELECTED_MC_BG),
            )
            .set_style(
                MultiChoiceStyleType::SelectedActive,
                Style::default().fg(ACTIVE_INPUT_FG).bg(SELECTED_MC_BG),
            )
    } else {
        MultiChoiceStyle::new()
            .prefix_color(ACCENT)
            // Titel
            .set_style(MultiChoiceStyleType::Title, Style::default().fg(ACCENT))
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

fn render(app: &App, frame: &mut ratatui::Frame) {
    let area = frame.area();

    // Schwarzer Hintergrund
    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);

    // Zentriertes Panel: 52 breit, 24 hoch
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
            Constraint::Length(1), // Überschrift
            Constraint::Length(1), // Leerzeile
            Constraint::Length(3), // Rolle (MC, immer 3 Zeilen!)
            Constraint::Length(1), // Abstand
            Constraint::Length(3), // Benutzername
            Constraint::Length(1), // Abstand
            Constraint::Length(3), // E-Mail
            Constraint::Length(1), // Abstand
            Constraint::Length(1), // Submit-Button
            Constraint::Min(0),    // Rest
        ])
        .split(inner);

    // Überschrift
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("✦ ", Style::default().fg(ACTIVE_ACCENT)),
            Span::styled("New User Profile", Style::default().fg(ACCENT).bold()),
        ])),
        chunks[0],
    );

    // Widgets rendern
    let keymap = TextInputKeymap::default();
    let mc_keymap = MultiChoiceKeymap::default();
    let widget_width = inner.width;

    // Rolle (MultiChoice)
    let roles = ["Admin", "Editor", "Viewer", "Guest"];
    let mc_area = chunks[2];

    // Benutzername
    TextInput::new("Benutzername")
        .placeholder("mind. 3 Zeichen")
        .width(widget_width)
        .style(make_style(app.active == Field::Username))
        .keymap(keymap.clone())
        .render_with_state(chunks[4], frame.buffer_mut(), &app.username);

    MultiChoice::new("Rolle", &roles)
        .placeholder("Wähle eine oder mehrere Rollen")
        .style(make_mc_style(app.active == Field::Role))
        .keymap(mc_keymap.clone())
        .render_with_state(
            mc_area.x,
            mc_area.y,
            mc_area.width,
            frame.buffer_mut(),
            &app.role,
        );

    // E-Mail
    TextInput::new("E-Mail")
        .placeholder("z.B. user@example.com")
        .width(widget_width)
        .style(make_style(app.active == Field::Email))
        .keymap(keymap.clone())
        .render_with_state(chunks[6], frame.buffer_mut(), &app.email);

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
                        .title(Span::styled(
                            " ✓ Gespeichert ",
                            Style::default().fg(ACTIVE_ACCENT).bold(),
                        ))
                        .style(Style::default().fg(INPUT_FG).bg(Color::Rgb(20, 40, 30))),
                )
                .style(Style::default().fg(INPUT_FG)),
            overlay,
        );
    }

    // Aktives Widget: Cursor-Position
    if app.active == Field::Role && app.role.open {
        // TODO: MC-Cursor-Position
    } else if app.active != Field::Role {
        let active_state = match app.active {
            Field::Username => &app.username,
            Field::Email => &app.email,
            _ => return,
        };
        if !active_state.value().is_empty() {
            let active_area = match app.active {
                Field::Username => chunks[4],
                Field::Email => chunks[6],
                _ => return,
            };
            let active_widget = TextInput::new("").width(inner.width);
            let pos = active_widget.cursor_position(active_area, active_state);
            frame.set_cursor_position(pos);
        }
    }
}

// ── Main Loop ─────────────────────────────────────────────────────────────────

fn run(mut terminal: DefaultTerminal) -> std::io::Result<()> {
    // Cursor-Style einmalig setzen
    execute!(std::io::stdout(), SetCursorStyle::BlinkingBar)?;

    let keymap = TextInputKeymap::default();
    let mc_keymap = MultiChoiceKeymap::default();
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
                    if app.active == Field::Role {
                        app.role.open();
                    } else {
                        app.role.close();
                    }
                }
                KeyCode::BackTab => {
                    app.active = app.active.prev();
                    if app.active == Field::Role {
                        app.role.open();
                    } else {
                        app.role.close();
                    }
                }

                KeyCode::Enter => {
                    if app.active == Field::Role {
                        app.role.open = !app.role.open;
                    } else if app.validate_all() {
                        let selected_roles = app
                            .role
                            .selected_indices()
                            .into_iter()
                            .map(|i| ["Admin", "Editor", "Viewer", "Guest"][i])
                            .collect::<Vec<_>>()
                            .join(", ");
                        app.submitted = Some(format!(
                            "\n  Rolle      : {}\n  Benutzername: {}\n  E-Mail     : {}\n\n  (beliebige Taste schließt)",
                            selected_roles,
                            app.username.value(),
                            app.email.value(),
                        ));
                    }
                }

                _ => {
                    // Aktives Feld bekommt den Event
                    let active = app.active;
                    if active == Field::Role {
                        if let MultiChoiceEvent::SelectionChanged(_) =
                            app.role.handle_event(&event, &mc_keymap)
                        {
                            // Auswahl geändert
                        }
                    } else {
                        let state = app
                            .state_mut(active)
                            .downcast_mut::<TextInputState>()
                            .unwrap();
                        if let TextInputEvent::Changed(_) = state.handle_event(&event, &keymap) {
                            // Live-Validierung bei jeder Änderung
                            app.validate_field(active);
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
    ratatui::restore();
    result
}
