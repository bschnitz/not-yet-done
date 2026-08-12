//! Global overlay for adapter-initiated, mid-operation prompts
//! ([`PromptRequest`]) — the input-capturing counterpart to the fire-and-forget
//! reminder/invalidation streams.
//!
//! Where a NodeAction popup is *user*-initiated, this is *adapter*-initiated: a
//! long-running async operation (chiefly an interactive browser sign-in raising
//! an MFA challenge) discovers it needs the user to provide — or merely
//! acknowledge — something before it can continue, and pushes a request up. The
//! overlay is deliberately **global and tab-agnostic**: the raising tab is only
//! context (shown as the `source` label), never a reason to switch views.
//!
//! Input collection **reuses the Action vocabulary** ([`InputSpec`] /
//! [`ActionInput`]) so there is no parallel input machinery: an
//! [`InputSpec::Form`] embeds the very same [`ContentFormPopup`] the form
//! actions use; [`InputSpec::None`] is a bare acknowledge (the number-match MFA
//! case, where showing the `detail` number is the whole point). The answer is
//! delivered on the request's one-shot `respond` channel, unblocking the
//! operation; dismissing it sends [`PromptAnswer::Cancelled`] so the operation
//! unwinds instead of hanging.

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use tokio::sync::oneshot;

use not_yet_done_content::{ActionInput, InputSpec, PromptAnswer, PromptRequest};

use crate::components::content_form_popup::{ContentFormEvent, ContentFormPopup};
use crate::ui::theme::Theme;

/// Outcome of feeding a key to the overlay.
pub enum PromptKeyOutcome {
    /// The prompt was answered or dismissed — the overlay should close.
    Closed,
    /// Key consumed; the overlay stays open.
    Consumed,
}

/// How the overlay collects the answer, chosen from the request's [`InputSpec`].
enum PromptBody {
    /// Bare acknowledge ([`InputSpec::None`]): Enter confirms, Esc dismisses.
    Acknowledge,
    /// Structured form ([`InputSpec::Form`]): embeds the shared form popup.
    Form(ContentFormPopup),
    /// An input shape the overlay does not render inline yet. Kept explicit so
    /// such a prompt is cancelled *cleanly* (see [`AdapterPromptPopup::take_unsupported`])
    /// rather than silently dropped or left hanging.
    Unsupported(&'static str),
}

/// A single adapter prompt rendered as a centered, modal overlay.
pub struct AdapterPromptPopup {
    /// Label of the raising instance/connection (e.g. the account name) — pure
    /// context, shown as the box title.
    source: String,
    /// The instruction text shown to the user (adapter-supplied, typically
    /// user-configurable per callback).
    prompt: String,
    /// Read-only detail rendered prominently above the input — e.g. the MFA
    /// number to match. Display only.
    detail: Option<String>,
    body: PromptBody,
    /// One-shot back to the raising operation; consumed the moment we answer or
    /// cancel, so a subsequent key can never double-send.
    respond: Option<oneshot::Sender<PromptAnswer>>,
}

impl AdapterPromptPopup {
    /// Build the overlay from a request, moving its one-shot responder in. The
    /// [`Theme`] is baked into the form widget's styles at construction.
    pub fn new(request: PromptRequest, theme: &Theme) -> Self {
        let PromptRequest {
            source,
            prompt,
            detail,
            input,
            respond,
        } = request;
        let body = match input {
            InputSpec::None => PromptBody::Acknowledge,
            InputSpec::Form { fields } => PromptBody::Form(ContentFormPopup::new(
                source.clone(),
                fields,
                &HashMap::new(),
                theme,
                &not_yet_done_ratatui::FormOptions::default(),
            )),
            InputSpec::Editor => PromptBody::Unsupported("editor"),
            InputSpec::Picker => PromptBody::Unsupported("picker"),
            InputSpec::FilePicker { .. } => PromptBody::Unsupported("file picker"),
            // A described-column form needs a node/adapter to resolve its
            // schema; the ambient prompt channel has neither.
            InputSpec::ColumnForm => PromptBody::Unsupported("column form"),
        };
        Self {
            source,
            prompt,
            detail,
            body,
            respond: Some(respond),
        }
    }

    /// If the request carried an input shape the overlay can't render inline,
    /// cancel it right away and return its label for a diagnostic — the caller
    /// must then *not* open the overlay. Returns `None` for a renderable prompt.
    pub fn take_unsupported(&mut self) -> Option<&'static str> {
        if let PromptBody::Unsupported(what) = self.body {
            self.send(PromptAnswer::Cancelled);
            Some(what)
        } else {
            None
        }
    }

    /// Feed a normalized key string (same vocabulary as the other popups).
    pub fn handle_key(&mut self, key: &str) -> PromptKeyOutcome {
        match &mut self.body {
            PromptBody::Acknowledge => match key {
                "enter" => {
                    self.send(PromptAnswer::Provided(ActionInput::None));
                    PromptKeyOutcome::Closed
                }
                "esc" => {
                    self.send(PromptAnswer::Cancelled);
                    PromptKeyOutcome::Closed
                }
                _ => PromptKeyOutcome::Consumed,
            },
            PromptBody::Form(form) => match form.handle_key(key) {
                ContentFormEvent::Submitted(values) => {
                    self.send(PromptAnswer::Provided(ActionInput::Form(values)));
                    PromptKeyOutcome::Closed
                }
                ContentFormEvent::Cancelled => {
                    self.send(PromptAnswer::Cancelled);
                    PromptKeyOutcome::Closed
                }
                ContentFormEvent::Consumed => PromptKeyOutcome::Consumed,
            },
            // Should never be shown (the caller cancels via `take_unsupported`),
            // but stay defensive: any key closes it, cancelled.
            PromptBody::Unsupported(_) => {
                self.send(PromptAnswer::Cancelled);
                PromptKeyOutcome::Closed
            }
        }
    }

    /// Send the answer exactly once; a dropped receiver (op already gone) is
    /// harmless.
    fn send(&mut self, answer: PromptAnswer) {
        if let Some(tx) = self.respond.take() {
            let _ = tx.send(answer);
        }
    }

    /// Draw the overlay centered over `area`. Mutable because the form widget
    /// places the terminal cursor itself while rendering.
    pub fn render(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        match &mut self.body {
            // The form popup renders its own centered box (titled with `source`).
            PromptBody::Form(form) => form.render(frame, area),
            // Acknowledge (and the never-shown unsupported fallback): our own box
            // presenting `detail` prominently plus the instruction and hint.
            _ => self.render_message(frame, area, theme),
        }
    }

    fn render_message(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let accent = theme.primary();
        let mut lines: Vec<Line> = Vec::new();
        if let Some(detail) = &self.detail {
            lines.push(Line::from(Span::styled(
                detail.clone(),
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            self.prompt.clone(),
            Style::default().fg(theme.text_high()),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "enter — acknowledge    esc — dismiss",
            Style::default().fg(theme.text_dim()),
        )));

        let popup = centered_rect(area, lines.len() as u16);
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(accent))
            .title(Span::styled(
                format!(" {} ", self.source),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(theme.form_bg()));
        let paragraph = Paragraph::new(lines)
            .block(block)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, popup);
    }
}

/// A centered rect sized for `content_lines` rows plus borders + padding.
fn centered_rect(area: Rect, content_lines: u16) -> Rect {
    let popup_h = (content_lines + 2)
        .min(area.height.saturating_sub(2))
        .max(5);
    let popup_w = area.width.saturating_sub(8).min(70).max(40);
    let x = area.x + area.width.saturating_sub(popup_w) / 2;
    let y = area.y + area.height.saturating_sub(popup_h) / 2;
    Rect::new(x, y, popup_w, popup_h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::new(crate::config::ThemeConfig::default())
    }

    fn ack_request() -> (PromptRequest, oneshot::Receiver<PromptAnswer>) {
        let (tx, rx) = oneshot::channel();
        (
            PromptRequest {
                source: "Work".into(),
                prompt: "Approve the sign-in".into(),
                detail: Some("42".into()),
                input: InputSpec::None,
                respond: tx,
            },
            rx,
        )
    }

    #[test]
    fn acknowledge_enter_sends_none_and_closes() {
        let (req, mut rx) = ack_request();
        let mut popup = AdapterPromptPopup::new(req, &theme());
        assert!(matches!(
            popup.handle_key("enter"),
            PromptKeyOutcome::Closed
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(PromptAnswer::Provided(ActionInput::None))
        ));
    }

    #[test]
    fn acknowledge_esc_cancels() {
        let (req, mut rx) = ack_request();
        let mut popup = AdapterPromptPopup::new(req, &theme());
        assert!(matches!(popup.handle_key("esc"), PromptKeyOutcome::Closed));
        assert!(matches!(rx.try_recv(), Ok(PromptAnswer::Cancelled)));
    }

    #[test]
    fn acknowledge_ignores_other_keys() {
        let (req, _rx) = ack_request();
        let mut popup = AdapterPromptPopup::new(req, &theme());
        assert!(matches!(popup.handle_key("a"), PromptKeyOutcome::Consumed));
    }

    #[test]
    fn form_submit_delivers_values() {
        let (tx, mut rx) = oneshot::channel();
        let req = PromptRequest {
            source: "Work".into(),
            prompt: "Enter code".into(),
            detail: None,
            input: InputSpec::Form {
                fields: vec![not_yet_done_content::FormFieldSpec::text("code", "Code")],
            },
            respond: tx,
        };
        let mut popup = AdapterPromptPopup::new(req, &theme());
        for c in "123".chars() {
            popup.handle_key(&c.to_string());
        }
        assert!(matches!(
            popup.handle_key("enter"),
            PromptKeyOutcome::Closed
        ));
        match rx.try_recv() {
            Ok(PromptAnswer::Provided(ActionInput::Form(v))) => {
                assert_eq!(v.get("code").unwrap(), "123");
            }
            other => panic!("expected form values, got {:?}", matches!(other, Ok(_))),
        }
    }

    #[test]
    fn unsupported_input_cancels_without_opening() {
        let (tx, mut rx) = oneshot::channel();
        let req = PromptRequest {
            source: "Work".into(),
            prompt: "Pick".into(),
            detail: None,
            input: InputSpec::Picker,
            respond: tx,
        };
        let mut popup = AdapterPromptPopup::new(req, &theme());
        assert_eq!(popup.take_unsupported(), Some("picker"));
        assert!(matches!(rx.try_recv(), Ok(PromptAnswer::Cancelled)));
    }
}
