//! Render path for [`ContentSlot::Broken`] tabs.
//!
//! When a YAML view-file fails to parse or validate, the tab still
//! occupies a slot in `App::content_views` so the user sees a labeled
//! tab and can read the error in-app. This module owns the panel UI:
//! a centered block with the file path, the conflict list, and a hint
//! row naming the ways out.
//!
//! # Why the panel scrolls and copies
//!
//! A validator that finds one problem finds a dozen: one duplicate key in a
//! shared YAML anchor is reported once per subtab that uses the anchor. The
//! list is then longer than the terminal, and the part that names the *cause*
//! is as likely to be off the bottom as on screen. So the panel scrolls, `y`
//! puts the whole list on the clipboard, and — because the panel registers
//! itself with the mouse map — a drag picks a piece of it out by hand.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::app::{App, ContentSlot};

/// The panel's own keys, as the hint row spells them out. The notification
/// centre is not among them: it is a rebindable global, so its key is read
/// from the live bindings rather than written down here.
const HINTS: &[(&str, &str)] = &[("↑/↓", "scroll"), ("y", "copy all")];

/// Every problem of a broken slot as one block of plain text, for the
/// clipboard and for the notification log. `None` when the slot is not
/// broken.
pub fn error_text(app: &App, slot_idx: usize) -> Option<String> {
    let ContentSlot::Broken { path, errors, .. } = app.content_views.get(slot_idx)? else {
        return None;
    };
    Some(format_errors(path, errors))
}

/// The clipboard form of a broken file's problems: the path, the count, and
/// one bullet per problem, exactly as the panel reads on screen.
fn format_errors(path: &std::path::Path, errors: &[String]) -> String {
    let mut out = format!("{}\n\n{} problem(s):\n", path.display(), errors.len());
    for err in errors {
        out.push_str("  • ");
        out.push_str(err);
        out.push('\n');
    }
    out
}

/// Draw the panel and report the furthest the body can scroll.
///
/// That number is the caller's business, not the panel's: only the render
/// pass knows how wide the text ended up wrapping and how tall the body was,
/// and the key handler needs both to stop scrolling at the last line.
pub fn render(frame: &mut Frame, area: Rect, app: &App, slot_idx: usize) -> usize {
    let Some(ContentSlot::Broken { name, path, errors }) = app.content_views.get(slot_idx) else {
        return 0;
    };

    let theme = &app.shared_theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.error()))
        .title(Span::styled(
            format!(" Configuration error in {name} "),
            Style::default()
                .fg(theme.error())
                .add_modifier(Modifier::BOLD),
        ));

    // Center-ish: leave a 2-line breathing margin on top.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Fill(1)])
        .split(area);
    let panel = chunks[1];
    let inner = block.inner(panel);
    frame.render_widget(block, panel);

    // The hint row is chrome, not content: it keeps its place at the bottom
    // while the list scrolls past it.
    let (body, hint_row) = if inner.height >= 3 {
        (
            Rect {
                height: inner.height - 2,
                ..inner
            },
            Some(Rect {
                y: inner.y + inner.height - 1,
                height: 1,
                ..inner
            }),
        )
    } else {
        (inner, None)
    };

    let width = body.width.max(1) as usize;
    let lines = body_lines(theme, path, errors, width);
    let total = lines.len();
    let offset = app
        .config_error_scroll
        .min(total.saturating_sub(body.height as usize));

    frame.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), body);

    if let Some(row) = hint_row {
        // The centre's key is whatever the user bound it to — `f10` out of
        // the box, but a hint that names a key the user has rebound is worse
        // than no hint at all.
        let centre = app
            .keybindings
            .global
            .get(&crate::config::keybindings::GlobalAction::ShowNotifications)
            .map(|b| b.display_label());
        frame.render_widget(
            Paragraph::new(hint_line(
                theme,
                total,
                offset,
                body.height,
                centre.as_deref(),
            )),
            row,
        );
    }
    total.saturating_sub(body.height as usize)
}

/// The scrollable part: the file, the count, and one wrapped bullet per
/// problem.
fn body_lines<'a>(
    theme: &crate::ui::theme::Theme,
    path: &std::path::Path,
    errors: &'a [String],
    width: usize,
) -> Vec<Line<'a>> {
    let med = Style::default().fg(theme.text_med());
    let high = Style::default().fg(theme.text_high());
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("File: ", med),
        Span::styled(path.display().to_string(), high),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("{} problem(s):", errors.len()),
        high.add_modifier(Modifier::BOLD),
    )));
    // Wrapped here rather than by `Wrap`, because a scroll offset counts
    // laid-out lines: the widget's own wrapping happens after the offset is
    // applied and would make every notch jump by a random amount.
    for err in errors {
        // The bullet sits in the two cells the continuation lines indent to,
        // so a wrapped problem reads as one block.
        for (i, chunk) in crate::components::notification_center::wrap(err, width.saturating_sub(4))
            .into_iter()
            .enumerate()
        {
            let lead = if i == 0 { "  • " } else { "    " };
            lines.push(Line::from(vec![
                Span::styled(lead, Style::default().fg(theme.error())),
                Span::styled(chunk, high),
            ]));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Fix the YAML and restart.", med)));
    lines
}

/// The fixed bottom row: what is off-screen on the left, the keys on the
/// right.
fn hint_line(
    theme: &crate::ui::theme::Theme,
    total: usize,
    offset: usize,
    height: u16,
    centre: Option<&str>,
) -> Line<'static> {
    let med = Style::default().fg(theme.text_med());
    let mut spans = Vec::new();
    let hidden = total.saturating_sub(offset + height as usize);
    if hidden > 0 {
        spans.push(Span::styled(
            format!("{hidden} more below   "),
            Style::default().fg(theme.error()),
        ));
    } else if offset > 0 {
        spans.push(Span::styled(format!("{offset} above   "), med));
    }
    let mut hints: Vec<(String, &str)> =
        HINTS.iter().map(|(k, w)| (format!("[{k}]"), *w)).collect();
    if let Some(centre) = centre {
        hints.push((centre.to_string(), "notification centre"));
    }
    for (i, (key, what)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", med));
        }
        spans.push(Span::styled(
            key.clone(),
            Style::default()
                .fg(theme.text_high())
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {what}"), med));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::Theme;

    fn theme() -> Theme {
        Theme::new(crate::config::ThemeConfig::default())
    }

    fn errors() -> Vec<String> {
        vec![
            "views.Berlin.children.messages.attachments: key [\"o s\"] is claimed by both \
             global.shortcut_overview and the level's own open action"
                .to_string(),
            "short one".to_string(),
        ]
    }

    /// What `y` puts on the clipboard has to be pasteable as it stands: the
    /// file it came from, then every problem, one per line.
    #[test]
    fn the_copied_text_names_the_file_and_every_problem() {
        let text = format_errors(std::path::Path::new("/tmp/views/mail.yaml"), &errors());
        assert!(text.starts_with("/tmp/views/mail.yaml"), "got: {text}");
        assert!(text.contains("2 problem(s):"));
        assert_eq!(text.lines().filter(|l| l.contains('•')).count(), 2);
        assert!(text.contains("short one"));
    }

    /// A problem wider than the panel keeps its wording — it wraps, and only
    /// the first of its lines carries the bullet, so the list stays countable.
    #[test]
    fn a_long_problem_wraps_under_one_bullet() {
        let errs = errors();
        let lines = body_lines(
            &theme(),
            std::path::Path::new("/tmp/views/mail.yaml"),
            &errs,
            40,
        );
        let rendered: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let bullets = rendered.iter().filter(|l| l.contains('•')).count();
        assert_eq!(
            bullets, 2,
            "one bullet per problem, not per line: {rendered:#?}"
        );
        // The long one really did need more than one line.
        assert!(rendered.len() > 2 + 2 + 2, "nothing wrapped: {rendered:#?}");
        // No line is wider than the panel.
        assert!(
            rendered.iter().all(|l| l.chars().count() <= 40),
            "a line overflowed: {rendered:#?}"
        );
    }

    /// The hint row is the only thing that says the list continues past the
    /// bottom edge — without it a truncated panel looks complete.
    #[test]
    fn the_hint_row_counts_what_is_off_screen() {
        let t = theme();
        let below: String = hint_line(&t, 40, 0, 10, Some("[z l]"))
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(below.starts_with("30 more below"), "got: {below}");

        let done: String = hint_line(&t, 12, 2, 10, Some("[z l]"))
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(!done.contains("below"), "got: {done}");
        assert!(done.contains("[y] copy all"), "got: {done}");
    }

    /// The centre is a rebindable global. A hint that says `f10` on a config
    /// that moved it to `z l` sends the reader to a key that does nothing —
    /// which is exactly what the panel is there to prevent.
    #[test]
    fn the_hint_row_names_the_bound_key_not_the_default() {
        let t = theme();
        let rebound: String = hint_line(&t, 12, 0, 20, Some("[z l]"))
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(
            rebound.contains("[z l] notification centre"),
            "got: {rebound}"
        );
        assert!(!rebound.contains("f10"), "got: {rebound}");

        // Unbound: the panel says nothing rather than pointing nowhere.
        let unbound: String = hint_line(&t, 12, 0, 20, None)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(!unbound.contains("notification centre"), "got: {unbound}");
        assert!(unbound.contains("[y] copy all"), "got: {unbound}");
    }
}
