//! Example: live-preview panel (left) + form (right) via `TwoColumnLayout`.
//!
//! Layout:
//!   • No borders on either panel
//!   • Outer padding: 2 on all sides
//!   • Inner (centre) padding: 1 on the touching edges of each panel
//!   • Left panel: slightly lighter background, updates in real time
//!   • Right panel: form with Hostname / Protocol / Port / API Key
//!
//! Colour scheme mirrors new_server_form.rs.
//!
//! Keys: Tab / Shift+Tab — next / prev field   Enter — submit   Esc — quit

use crossterm::{
    cursor::SetCursorStyle,
    event::{Event, KeyCode, KeyEventKind},
    execute,
};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
    DefaultTerminal,
};

use not_yet_done_ratatui::widgets::two_column::{
    HalfBorders, HalfPadding, TwoColumnLayout, TwoColumnStyle,
};
use not_yet_done_ratatui::{
    FieldEvent, Form, FormEvent, FormField, FormFieldState, FormState, FormStyle, FormWidgetStyle,
    MultiChoice, MultiChoiceState, TextInput, TextInputEvent, TextInputState,
};

// ── colours ───────────────────────────────────────────────────────────────────

const BG: Color = Color::Rgb(10, 10, 20);
const RIGHT_BG: Color = Color::Rgb(18, 18, 35); // form panel
const LEFT_BG: Color = Color::Rgb(30, 30, 55); // preview panel — lighter
const FIELD_BG: Color = Color::Rgb(28, 28, 50);
const ACCENT: Color = Color::Rgb(100, 180, 255);
const INPUT_FG: Color = Color::Rgb(230, 230, 255);
const INACTIVE_PH: Color = Color::Rgb(45, 45, 65);
const ACTIVE_ACCENT: Color = Color::Rgb(140, 255, 180);
const ACTIVE_PH: Color = Color::Rgb(70, 90, 70);
const ERROR_FG: Color = Color::Rgb(255, 100, 80);
const MC_SELECTED_BG: Color = Color::Rgb(30, 50, 45);
const ACTIVE_INPUT_FG: Color = Color::Rgb(255, 220, 80);

const PREVIEW_HEADING: Color = Color::Rgb(160, 200, 255);
const PREVIEW_LABEL: Color = Color::Rgb(90, 115, 145);
const PREVIEW_VALUE: Color = Color::Rgb(200, 220, 255);
const PREVIEW_URL: Color = Color::Rgb(140, 255, 180);
const PREVIEW_MASKED: Color = Color::Rgb(75, 90, 115);
const PREVIEW_OK: Color = Color::Rgb(100, 220, 120);
const PREVIEW_WARN: Color = Color::Rgb(255, 180, 60);
const PREVIEW_DIM: Color = Color::Rgb(60, 70, 100);

const PROTOCOLS: &[&str] = &["HTTP", "HTTPS", "WS", "WSS"];

const IDX_HOSTNAME: usize = 0;
const IDX_PROTOCOL: usize = 1;
const IDX_PORT: usize = 2;
const IDX_APIKEY: usize = 3;

// ── app state ─────────────────────────────────────────────────────────────────

struct App {
    form_state: FormState,
    submitted: bool,
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
            submitted: false,
        }
    }

    // ── field accessors ───────────────────────────────────────────────────────

    fn hostname(&self) -> &str {
        self.form_state
            .field(IDX_HOSTNAME)
            .and_then(|f| f.as_text_input())
            .map(|s| s.value())
            .unwrap_or("")
    }

    fn protocols(&self) -> Vec<&'static str> {
        self.form_state
            .field(IDX_PROTOCOL)
            .and_then(|f| f.as_multi_choice())
            .map(|ms| {
                ms.selected_indices()
                    .iter()
                    .filter_map(|&i| PROTOCOLS.get(i).copied())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn port(&self) -> &str {
        self.form_state
            .field(IDX_PORT)
            .and_then(|f| f.as_text_input())
            .map(|s| s.value())
            .unwrap_or("")
    }

    fn api_key(&self) -> &str {
        self.form_state
            .field(IDX_APIKEY)
            .and_then(|f| f.as_text_input())
            .map(|s| s.value())
            .unwrap_or("")
    }

    fn is_complete(&self) -> bool {
        !self.hostname().is_empty()
            && !self.protocols().is_empty()
            && self.port().parse::<u16>().map(|n| n >= 1).unwrap_or(false)
            && self.api_key().len() >= 8
    }

    // ── validation ────────────────────────────────────────────────────────────

    fn validate_all(&mut self) -> bool {
        let mut ok = true;

        if let Some(ts) = self
            .form_state
            .field_mut(IDX_HOSTNAME)
            .and_then(|f| f.as_text_input_mut())
        {
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

        if let Some(ts) = self
            .form_state
            .field_mut(IDX_PORT)
            .and_then(|f| f.as_text_input_mut())
        {
            let v = ts.value().to_string();
            match v.parse::<u16>() {
                Ok(n) if n >= 1 => ts.clear_error(),
                _ => {
                    ts.set_error("Must be 1–65535");
                    ok = false;
                }
            }
        }

        if let Some(ts) = self
            .form_state
            .field_mut(IDX_APIKEY)
            .and_then(|f| f.as_text_input_mut())
        {
            let v = ts.value().to_string();
            if v.len() < 8 {
                ts.set_error("At least 8 characters");
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
                if let Some(ts) = self
                    .form_state
                    .field_mut(IDX_HOSTNAME)
                    .and_then(|f| f.as_text_input_mut())
                {
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
                if let Some(ts) = self
                    .form_state
                    .field_mut(IDX_PORT)
                    .and_then(|f| f.as_text_input_mut())
                {
                    let v = ts.value().to_string();
                    if v.is_empty() {
                        ts.clear_error();
                    } else if v.parse::<u16>().map(|n| n < 1).unwrap_or(true) {
                        ts.set_error("Must be 1–65535");
                    } else {
                        ts.clear_error();
                    }
                }
            }
            IDX_APIKEY => {
                if let Some(ts) = self
                    .form_state
                    .field_mut(IDX_APIKEY)
                    .and_then(|f| f.as_text_input_mut())
                {
                    let v = ts.value().to_string();
                    if v.is_empty() {
                        ts.clear_error();
                    } else if v.len() < 8 {
                        ts.set_error("At least 8 characters");
                    } else {
                        ts.clear_error();
                    }
                }
            }
            _ => {}
        }
    }
}

// ── form factory ──────────────────────────────────────────────────────────────

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
                .mc_closing_line(Style::default().bg(RIGHT_BG)),
        )
        .inactive(
            FormWidgetStyle::new()
                .prefix_color(ACCENT)
                .title(Style::default().fg(ACCENT))
                .body(Style::default().fg(INPUT_FG))
                .placeholder(INACTIVE_PH)
                .error(Style::default().fg(ERROR_FG)),
        );

    Form::new()
        .style(global_style)
        .spacing(1)
        .field(FormField::text_input(
            TextInput::new("Hostname").placeholder("e.g. api.example.com"),
        ))
        .field(
            FormField::multi_choice(
                MultiChoice::new("Protocol", PROTOCOLS).placeholder("Choose protocol(s)"),
            )
            .with_height(3),
        )
        .field(FormField::text_input(
            TextInput::new("Port").placeholder("e.g. 8080"),
        ))
        .field(FormField::text_input(
            TextInput::new("API Key").placeholder("min. 8 characters"),
        ))
}

// ── preview panel ─────────────────────────────────────────────────────────────

/// Renders the live connection preview into `area`.
fn render_preview(area: Rect, buf: &mut Buffer, app: &App) {
    if area.height == 0 {
        return;
    }

    let hostname = app.hostname();
    let protocols = app.protocols();
    let port = app.port();
    let api_key = app.api_key();

    // Connection URL (uses first selected protocol as scheme).
    let url: String = if hostname.is_empty() {
        String::new()
    } else {
        let scheme = protocols.first().copied().unwrap_or("…").to_lowercase();
        let port_part = if port.is_empty() {
            String::new()
        } else {
            format!(":{port}")
        };
        format!("{scheme}://{hostname}{port_part}")
    };

    // Masked API key: show first 4 chars, then bullets.
    let key_line: Line = if api_key.is_empty() {
        Line::from(Span::styled("—", Style::default().fg(PREVIEW_MASKED)))
    } else if api_key.len() < 8 {
        Line::from(vec![
            Span::styled(api_key, Style::default().fg(ERROR_FG)),
            Span::styled(
                format!("  ({} / 8 chars)", api_key.len()),
                Style::default().fg(ERROR_FG).add_modifier(Modifier::DIM),
            ),
        ])
    } else {
        let visible = &api_key[..4.min(api_key.len())];
        let dots = "••••••••";
        Line::from(vec![
            Span::styled(visible.to_string(), Style::default().fg(PREVIEW_VALUE)),
            Span::styled(dots, Style::default().fg(PREVIEW_MASKED)),
        ])
    };

    // Protocol list.
    let proto_line: Line = if protocols.is_empty() {
        Line::from(Span::styled("—", Style::default().fg(PREVIEW_MASKED)))
    } else {
        Line::from(
            protocols
                .iter()
                .enumerate()
                .flat_map(|(i, p)| {
                    let sep = if i == 0 {
                        vec![]
                    } else {
                        vec![Span::styled("  ", Style::default())]
                    };
                    let mut spans = sep;
                    spans.push(Span::styled(
                        p.to_string(),
                        Style::default()
                            .fg(PREVIEW_VALUE)
                            .add_modifier(Modifier::BOLD),
                    ));
                    spans
                })
                .collect::<Vec<_>>(),
        )
    };

    // Status line.
    let status_line: Line = if app.submitted {
        Line::from(vec![
            Span::styled("✓  ", Style::default().fg(PREVIEW_OK)),
            Span::styled(
                "Saved",
                Style::default().fg(PREVIEW_OK).add_modifier(Modifier::BOLD),
            ),
        ])
    } else if app.is_complete() {
        Line::from(vec![
            Span::styled("●  ", Style::default().fg(PREVIEW_OK)),
            Span::styled("Ready to submit", Style::default().fg(PREVIEW_OK)),
        ])
    } else {
        Line::from(vec![
            Span::styled("◌  ", Style::default().fg(PREVIEW_WARN)),
            Span::styled("Incomplete", Style::default().fg(PREVIEW_WARN)),
        ])
    };

    // Horizontal rule.
    let rule = "─".repeat(area.width as usize);
    let rule_style = Style::default().fg(PREVIEW_DIM);

    // Row layout.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // 0  heading
            Constraint::Length(1), // 1  rule
            Constraint::Length(1), // 2  blank
            Constraint::Length(1), // 3  label: URL
            Constraint::Length(1), // 4  value: url
            Constraint::Length(1), // 5  blank
            Constraint::Length(1), // 6  label: Protocols
            Constraint::Length(1), // 7  value: protocols
            Constraint::Length(1), // 8  blank
            Constraint::Length(1), // 9  label: Port
            Constraint::Length(1), // 10 value: port
            Constraint::Length(1), // 11 blank
            Constraint::Length(1), // 12 label: API Key
            Constraint::Length(1), // 13 value: key
            Constraint::Min(0),
        ])
        .split(area);

    let lbl = |s: &'static str| -> Paragraph<'static> {
        Paragraph::new(Span::styled(s, Style::default().fg(PREVIEW_LABEL)))
    };
    let val = |s: String| -> Paragraph<'static> {
        Paragraph::new(Span::styled(s, Style::default().fg(PREVIEW_VALUE)))
    };

    // 0 — heading with status on the right
    // Status for heading.
    let status_spans: Vec<Span> = if app.submitted {
        vec![
            Span::styled("✓", Style::default().fg(PREVIEW_OK)),
            Span::raw(" "),
            Span::styled("Saved", Style::default().fg(PREVIEW_OK)),
        ]
    } else if app.is_complete() {
        vec![
            Span::styled("●", Style::default().fg(PREVIEW_OK)),
            Span::raw(" "),
            Span::styled("Ready", Style::default().fg(PREVIEW_OK)),
        ]
    } else {
        vec![
            Span::styled("◌", Style::default().fg(PREVIEW_WARN)),
            Span::raw(" "),
            Span::styled("Incomplete", Style::default().fg(PREVIEW_WARN)),
        ]
    };

    let heading_spans = vec![
        Span::styled(
            "Connection Preview",
            Style::default()
                .fg(PREVIEW_HEADING)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
    ]
    .into_iter()
    .chain(status_spans)
    .collect::<Vec<_>>();
    Paragraph::new(Line::from(heading_spans)).render(rows[0], buf);

    // 1 — rule
    Paragraph::new(Span::styled(rule.clone(), rule_style)).render(rows[1], buf);

    // 3–4 — URL
    lbl("URL").render(rows[3], buf);
    if url.is_empty() {
        Paragraph::new(Span::styled("—", Style::default().fg(PREVIEW_MASKED))).render(rows[4], buf);
    } else {
        Paragraph::new(Span::styled(url, Style::default().fg(PREVIEW_URL))).render(rows[4], buf);
    }

    // 6–7 — Protocols
    lbl("Protocols").render(rows[6], buf);
    Paragraph::new(proto_line).render(rows[7], buf);

    // 9–10 — Port
    lbl("Port").render(rows[9], buf);
    val(if port.is_empty() {
        "—".into()
    } else {
        port.into()
    })
    .render(rows[10], buf);

    // 12–13 — API Key
    lbl("API Key").render(rows[12], buf);
    Paragraph::new(key_line).render(rows[13], buf);
}

// ── render ────────────────────────────────────────────────────────────────────

fn render(app: &App, frame: &mut ratatui::Frame) {
    let area = frame.area();

    // Full-screen background.
    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);

    // Two-column layout: no borders, outer padding 2, inner (centre) padding 1.
    //
    // left_padding  — left:2  right:1  top:2  bottom:2
    // right_padding — left:1  right:2  top:2  bottom:2
    //
    // The content_left / content_right styles fill the full border-inner area
    // (including the padding cells), so the background colour covers uniformly.
    let (left_inner, right_inner) = TwoColumnLayout::new()
        .borders(HalfBorders::none())
        .header_left("Connection Preview")
        .header_right("Connection Form")
        .left_padding(HalfPadding::new().left(3).right(3).top(2).bottom(2))
        .right_padding(HalfPadding::new().left(3).right(3).top(2).bottom(2))
        .style(
            TwoColumnStyle::new()
                // padding_style fills the padding band, content fills the inner rect —
                // both set to the same bg so the entire panel appears uniformly coloured.
                .padding_style_left(Style::default().bg(LEFT_BG))
                .content_left(Style::default().bg(LEFT_BG))
                .padding_style_right(Style::default().bg(RIGHT_BG))
                .content_right(Style::default().bg(RIGHT_BG)),
        )
        .render_layout(area, frame.buffer_mut());

    // Left panel: live preview.
    render_preview(left_inner, frame.buffer_mut(), app);

    // Right panel: header + form.
    let right_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(right_inner);

    let header_style = Style::default()
        .fg(PREVIEW_HEADING)
        .bg(RIGHT_BG)
        .add_modifier(Modifier::BOLD);

    let rule_style = Style::default().fg(PREVIEW_DIM);

    Paragraph::new(Line::from(Span::styled("Connection Form", header_style)))
        .render(right_rows[0], frame.buffer_mut());

    let rule = "─".repeat(right_rows[1].width as usize);
    Paragraph::new(Span::styled(rule, rule_style)).render(right_rows[1], frame.buffer_mut());

    let form = make_form();
    form.render_with_state(right_rows[2], frame.buffer_mut(), &app.form_state);

    if let Some(pos) = make_form().cursor_position(right_rows[2], &app.form_state) {
        frame.set_cursor_position(pos);
    }
}

// ── main loop ─────────────────────────────────────────────────────────────────

fn run(mut terminal: DefaultTerminal) -> std::io::Result<()> {
    execute!(std::io::stdout(), SetCursorStyle::BlinkingBar)?;

    let mut app = App::new();

    loop {
        terminal.draw(|f| render(&app, f))?;

        let event = crossterm::event::read()?;

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
                        app.submitted = true;
                    }
                }
                FormEvent::FocusChanged { .. } => {
                    // Reset submitted banner when user continues editing.
                    app.submitted = false;
                }
                FormEvent::FieldEvent { index, event } => {
                    app.submitted = false;
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
