//! App-level shortcut capture overlay — the host for a chord recording (and
//! for the conflict prompt it may raise) when no popup is open to hold them.
//!
//! Two paths end up here:
//!
//! * The editor saved a new query and the shortcut is assigned right after —
//!   there is no menu on screen, so the overlay records the chord itself.
//! * The query/script menus recorded their chord *in* their popup, which then
//!   closed; if that chord collides, the overlay carries the resulting
//!   [`ConflictPrompt`] so the user still gets the usual y/n question instead
//!   of a bare error.
//!
//! Both use the very same [`KeyRecorder`] and conflict prompt as the shortcut
//! menu, so a chord recorded here behaves exactly like one recorded there —
//! multi-key sequences included.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::components::key_conflict::{
    CONFLICT_HINTS, ConflictItem, ConflictPrompt, ConflictReply, conflict_heading, conflict_lines,
    conflict_width,
};
use crate::components::key_recorder::{
    KeyRecorder, RECORDING_HINTS, RecorderStep, recording_heading,
};
use crate::ui::panel_chrome::PanelChrome;
use crate::ui::theme::Theme;

/// The y/n wording of the capture overlay's conflict prompt. Phrased like the
/// shortcut menu's so the same question always reads the same way.
const CONFLICT_QUESTION: &str = "Remove and bind here? (y/n)";

/// What a key press did to the capture overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureMessage {
    /// Key consumed, the overlay stays up.
    Handled,
    /// Aborted (Esc / n) — the overlay should close, nothing was bound.
    Cancelled,
    /// A chord was recorded. The host now checks it for collisions and either
    /// binds it or calls [`ShortcutCapture::raise_conflicts`].
    Recorded(String),
    /// The pending chord was confirmed: free `drop`, then bind `chord`.
    Apply {
        chord: String,
        drop: Vec<ConflictItem>,
    },
}

/// A pending shortcut assignment: what it will bind (`target`, opaque to this
/// component), how to name it on screen, and the recorder or conflict prompt
/// currently on top.
#[derive(Debug, Clone)]
pub struct ShortcutCapture<T> {
    target: T,
    /// What the chord is being bound to, e.g. `"'my query'"`.
    subject: String,
    recorder: Option<KeyRecorder>,
    /// The pending prompt; its kind is the recorded chord awaiting the answer.
    conflict: Option<ConflictPrompt<String>>,
}

impl<T> ShortcutCapture<T> {
    /// Start with a live recording — the path with no popup on screen.
    pub fn recording(target: T, subject: impl Into<String>) -> Self {
        Self {
            target,
            subject: subject.into(),
            // A DB-stored shortcut holds exactly one chord, so recording here
            // always replaces.
            recorder: Some(KeyRecorder::new(true)),
            conflict: None,
        }
    }

    /// Start straight at the conflict prompt — the chord was already recorded
    /// elsewhere (in the query or script menu's popup) and turned out to
    /// collide.
    pub fn conflicting(
        target: T,
        subject: impl Into<String>,
        chord: String,
        items: Vec<ConflictItem>,
    ) -> Self {
        Self {
            target,
            subject: subject.into(),
            recorder: None,
            conflict: Some(ConflictPrompt::new(chord, items)),
        }
    }

    pub fn target(&self) -> &T {
        &self.target
    }

    /// Swap the finished recording for the prompt raised by its collisions.
    pub fn raise_conflicts(&mut self, chord: String, items: Vec<ConflictItem>) {
        self.recorder = None;
        self.conflict = Some(ConflictPrompt::new(chord, items));
    }

    pub fn handle_key(&mut self, key: &str) -> CaptureMessage {
        if let Some(c) = self.conflict.as_ref() {
            return match c.handle_key(key) {
                ConflictReply::Pending => CaptureMessage::Handled,
                ConflictReply::Dismiss => CaptureMessage::Cancelled,
                ConflictReply::Apply => CaptureMessage::Apply {
                    chord: c.kind.clone(),
                    drop: c.items.clone(),
                },
            };
        }
        match self.recorder.as_mut() {
            Some(rec) => match rec.feed(key) {
                RecorderStep::Recording => CaptureMessage::Handled,
                RecorderStep::Cancelled => CaptureMessage::Cancelled,
                RecorderStep::Saved(chord) => CaptureMessage::Recorded(chord),
            },
            // Neither recording nor prompting — nothing to answer; treat any
            // key as a dismissal rather than swallowing input forever.
            None => CaptureMessage::Cancelled,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, t: &Theme) {
        let (heading, hints, lines) = match (self.conflict.as_ref(), self.recorder.as_ref()) {
            (Some(c), _) => {
                let summary = format!("'{}'", c.kind);
                (
                    conflict_heading(t),
                    CONFLICT_HINTS.to_vec(),
                    conflict_lines(&summary, &c.items, CONFLICT_QUESTION, t),
                )
            }
            (None, Some(rec)) => (
                recording_heading(rec, &self.subject, t),
                RECORDING_HINTS.to_vec(),
                vec![Line::from(Span::styled(
                    "Press the keys of the shortcut, then \u{21b5}.",
                    Style::default().fg(t.form_hint()),
                ))],
            ),
            (None, None) => return,
        };
        let width = match self.conflict.as_ref() {
            Some(c) => conflict_width(&format!("'{}'", c.kind), &c.items, CONFLICT_QUESTION),
            None => lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.chars().count()).sum())
                .max()
                .unwrap_or(0),
        };
        let body = PanelChrome::new(heading)
            .hints(hints)
            .body(width, lines.len() as u16)
            .render(frame, area, t);
        if let Some(body) = body {
            frame.render_widget(Paragraph::new(lines), body);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::keybindings::GlobalAction;
    use crate::keymap::KeySource;

    fn item() -> ConflictItem {
        ConflictItem {
            source: KeySource::Global(GlobalAction::Quit),
            current: vec!["q".into()],
            drop: "q".into(),
            name: "Quit".into(),
            removable: true,
        }
    }

    #[test]
    fn records_a_multi_key_chord_and_reports_it() {
        let mut c = ShortcutCapture::recording("target", "'my query'");
        assert_eq!(c.handle_key("f"), CaptureMessage::Handled);
        assert_eq!(c.handle_key("f"), CaptureMessage::Handled);
        assert_eq!(
            c.handle_key("enter"),
            CaptureMessage::Recorded("f f".to_string())
        );
    }

    #[test]
    fn esc_cancels_the_recording() {
        let mut c = ShortcutCapture::recording((), "'x'");
        c.handle_key("g");
        assert_eq!(c.handle_key("esc"), CaptureMessage::Cancelled);
    }

    #[test]
    fn a_raised_conflict_applies_on_yes_and_dismisses_on_no() {
        let mut c = ShortcutCapture::recording((), "'x'");
        c.raise_conflicts("q".to_string(), vec![item()]);
        assert_eq!(c.handle_key("x"), CaptureMessage::Handled);
        assert_eq!(
            c.handle_key("y"),
            CaptureMessage::Apply {
                chord: "q".to_string(),
                drop: vec![item()],
            }
        );
        assert_eq!(c.handle_key("n"), CaptureMessage::Cancelled);
    }

    #[test]
    fn a_conflict_only_capture_never_records() {
        let mut c = ShortcutCapture::conflicting((), "'x'", "f f".to_string(), vec![item()]);
        assert_eq!(c.handle_key("a"), CaptureMessage::Handled);
        assert_eq!(c.handle_key("esc"), CaptureMessage::Cancelled);
    }
}
