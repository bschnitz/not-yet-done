//! Render path for [`ContentSlot::Broken`] tabs.
//!
//! When a YAML view-file fails to parse or validate, the tab still
//! occupies a slot in `App::content_views` so the user sees a labeled
//! tab and can read the error in-app. This module owns the panel UI:
//! a centered block with the file path, the conflict list, and a
//! "Fix the YAML and restart" hint.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::app::{App, ContentSlot};

pub fn render(frame: &mut Frame, area: Rect, app: &App, slot_idx: usize) {
    let Some(ContentSlot::Broken { name, path, errors }) = app.content_views.get(slot_idx) else {
        return;
    };

    let theme = &app.shared_theme;
    let title = format!(" Configuration error in {} ", name);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.error()))
        .title(Span::styled(
            title,
            Style::default()
                .fg(theme.error())
                .add_modifier(Modifier::BOLD),
        ));

    // Center-ish: leave a 2-line breathing margin on top.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Fill(1)])
        .split(area);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("File: ", Style::default().fg(theme.text_med())),
        Span::styled(
            path.display().to_string(),
            Style::default().fg(theme.text_high()),
        ),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("{} problem(s):", errors.len()),
        Style::default()
            .fg(theme.text_high())
            .add_modifier(Modifier::BOLD),
    )));
    for err in errors {
        lines.push(Line::from(vec![
            Span::styled("  • ", Style::default().fg(theme.error())),
            Span::styled(err.clone(), Style::default().fg(theme.text_high())),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Fix the YAML and restart.",
        Style::default().fg(theme.text_med()),
    )));

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(block);
    frame.render_widget(paragraph, chunks[1]);
}
