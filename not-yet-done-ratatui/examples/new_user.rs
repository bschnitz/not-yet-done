//! Example: "New User Profile" form
//!
//! Fields:
//!   • Role      — multiple choice from ["Admin", "Editor", "Viewer", "Guest"]
//!   • Username  — at least 3 characters
//!   • E-mail    — must contain '@' and '.'
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

// ── Colours ───────────────────────────────────────────────────────────────────

const BG: Color = Color::Rgb(10, 10, 20);
const PANEL_BG: Color = Color::Rgb(18, 18, 35);
const ACCENT: Color = Color::Rgb(100, 180, 255);
const INPUT_FG: Color = Color::Rgb(230, 230, 255);
const INPUT_BG: Color = Color::Rgb(28, 28, 50);
const PLACEHOLDER: Color = Color::Rgb(80, 80, 110);
const SELECTED_MC_BG: Color = Color::Rgb(35, 45, 65);
const ACTIVE_INPUT_FG: Color = Color::Rgb(255, 215, 0);

const ERROR_FG: Color = Color::Rgb(255, 100, 80);
const ACTIVE_ACCENT: Color = Color::Rgb(140, 255, 180);
const SUBMIT_FG: Color = Color::Rgb(30, 30, 50);
const SUBMIT_BG: Color = Color::Rgb(140, 255, 180);
const DIM: Color = Color::Rgb(80, 80, 110);
const INACTIVE_PH: Color = Color::Rgb(45, 45, 65);
const OVERLAY_BG: Color = Color::Rgb(20, 40, 30);

// ── App state ─────────────────────────────────────────────────────────────────

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
    submitted: Option<String>,
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

        let u = self.username.value().to_string();
        if u.len() < 3 {
            self.username.set_error("At least 3 characters required");
            ok = false;
        } else {
            self.username.clear_error();
        }

        let e = self.email.value().to_string();
        if !e.contains('@') || !e.contains('.') {
            self.email.set_error("Invalid e-mail format");
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
                    self.username.set_error("At least 3 characters required");
                } else {
                    self.username.clear_error();
                }
            }
            Field::Email => {
                let e = self.email.value().to_string();
                if e.is_empty() {
                    self.email.clear_error();
                } else if !e.contains('@') || !e.contains('.') {
                    self.email.set_error("Invalid e-mail format");
                } else {
                    self.email.clear_error();
                }
            }
            Field::Role => {}
        }
    }
}

// ── Style helpers ─────────────────────────────────────────────────────────────

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
            .placeholder_color(INACTIVE_PH)
            .set_style(TextInputStyleType::Error, Style::default().fg(ERROR_FG))
    }
}

fn make_mc_style(is_active: bool) -> MultiChoiceStyle {
    if is_active {
        MultiChoiceStyle::new()
            .prefix_color(ACTIVE_ACCENT)
            .set_style(
                MultiChoiceStyleType::Title,
                Style::default().fg(ACTIVE_ACCENT).bg(INPUT_BG),
            )
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
            .set_style(MultiChoiceStyleType::Title, Style::default().fg(ACCENT))
    }
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
            Constraint::Length(3), // role (MC, always 3 rows when closed)
            Constraint::Length(1), // spacer
            Constraint::Length(3), // username
            Constraint::Length(1), // spacer
            Constraint::Length(3), // e-mail
            Constraint::Length(1), // spacer
            Constraint::Length(1), // submit button
            Constraint::Min(0),
        ])
        .split(inner);

    // Heading
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("✦ ", Style::default().fg(ACTIVE_ACCENT)),
            Span::styled("New User Profile", Style::default().fg(ACCENT).bold()),
        ])),
        chunks[0],
    );

    let keymap = TextInputKeymap::default();
    let mc_keymap = MultiChoiceKeymap::default();
    let widget_width = inner.width;
    let roles = ["Admin", "Editor", "Viewer", "Guest"];

    // Inactive fields first, then Role last (so its drop-down overlays the rest).
    TextInput::new("Username")
        .placeholder("min. 3 characters")
        .width(widget_width)
        .style(make_style(app.active == Field::Username))
        .keymap(keymap.clone())
        .render_with_state(chunks[4], frame.buffer_mut(), &app.username);

    TextInput::new("E-mail")
        .placeholder("e.g. user@example.com")
        .width(widget_width)
        .style(make_style(app.active == Field::Email))
        .keymap(keymap.clone())
        .render_with_state(chunks[6], frame.buffer_mut(), &app.email);

    MultiChoice::new("Role", &roles)
        .placeholder("Choose one or more roles")
        .style(make_mc_style(app.active == Field::Role))
        .keymap(mc_keymap.clone())
        .render_with_state(chunks[2], frame.buffer_mut(), &app.role);

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

    // Cursor position for active text input fields
    if app.active != Field::Role {
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
            let pos = TextInput::new("").width(inner.width).cursor_position(active_area, active_state);
            frame.set_cursor_position(pos);
        }
    }
}

// ── Main loop ─────────────────────────────────────────────────────────────────

fn run(mut terminal: DefaultTerminal) -> std::io::Result<()> {
    execute!(std::io::stdout(), SetCursorStyle::BlinkingBar)?;

    let keymap = TextInputKeymap::default();
    let mc_keymap = MultiChoiceKeymap::default();
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
                            "\n  Role    : {}\n  Username: {}\n  E-mail  : {}\n\n  (any key closes)",
                            selected_roles,
                            app.username.value(),
                            app.email.value(),
                        ));
                    }
                }

                _ => {
                    let active = app.active;
                    if active == Field::Role {
                        if let MultiChoiceEvent::SelectionChanged(_) =
                            app.role.handle_event(&event, &mc_keymap)
                        {}
                    } else {
                        let state = app
                            .state_mut(active)
                            .downcast_mut::<TextInputState>()
                            .unwrap();
                        if let TextInputEvent::Changed(_) = state.handle_event(&event, &keymap) {
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
