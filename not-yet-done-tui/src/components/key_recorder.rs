//! Shared key-chord recorder — the "press the keys you want" state machine.
//!
//! One recorder drives every place the user assigns a shortcut: the shortcut
//! menu's Ctrl-N / Ctrl-U, the query menu's and script menu's in-popup
//! shortcut editor, and the App's capture overlay after a new query is saved.
//! Because they all feed the same [`KeyRecorder`], every one of them records
//! *sequences* — `f f`, `ctrl+k l`, `space` — not just a single key.
//!
//! Steps accumulate until Return; Backspace drops the last step and Esc (or
//! Return with nothing recorded) cancels. Return is therefore never itself a
//! recordable step — a deliberate trade: it is the one key the recorder needs
//! for itself.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::ui::theme::Theme;

/// Key hints shown while a recording is in progress. Every host renders the
/// same line, so the recorder looks identical wherever it is embedded.
pub const RECORDING_HINTS: &[(&str, &str)] =
    &[("\u{21b5}", "save"), ("\u{232b}", "del"), ("Esc", "cancel")];

/// What a key press did to the recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecorderStep {
    /// Key consumed, recording continues.
    Recording,
    /// Recording aborted (Esc, or Return with no steps) — nothing to bind.
    Cancelled,
    /// Recording finished; the payload is the surface form of the sequence,
    /// steps joined by spaces (`"ctrl+k l"`).
    Saved(String),
}

/// In-progress key recording: the steps captured so far plus the record mode.
#[derive(Debug, Clone, Default)]
pub struct KeyRecorder {
    steps: Vec<String>,
    /// Replace the target's existing bindings (Ctrl-U) rather than adding an
    /// alternative (Ctrl-N). Hosts that only ever hold a single chord (the
    /// DB-stored shortcuts) always record in overwrite mode.
    overwrite: bool,
}

impl KeyRecorder {
    pub fn new(overwrite: bool) -> Self {
        Self {
            steps: Vec::new(),
            overwrite,
        }
    }

    pub fn overwrite(&self) -> bool {
        self.overwrite
    }

    /// The sequence recorded so far, in its YAML surface form.
    pub fn surface(&self) -> String {
        surface_form(&self.steps)
    }

    /// Feed one key string. See the module docs for the Return / Backspace /
    /// Esc contract.
    pub fn feed(&mut self, key: &str) -> RecorderStep {
        match key {
            "esc" => RecorderStep::Cancelled,
            "backspace" => {
                self.steps.pop();
                RecorderStep::Recording
            }
            "enter" => {
                if self.steps.is_empty() {
                    RecorderStep::Cancelled
                } else {
                    RecorderStep::Saved(self.surface())
                }
            }
            other => {
                self.steps.push(other.to_string());
                RecorderStep::Recording
            }
        }
    }
}

/// Render one recorded step in its YAML surface form (a literal space becomes
/// the word `space` so the step is legible and re-parseable).
pub fn step_to_surface(step: &str) -> String {
    if step == " " {
        "space".to_string()
    } else if let Some(mods) = step.strip_suffix("+ ") {
        format!("{mods}+space")
    } else {
        step.to_string()
    }
}

/// The full surface form of a recorded sequence: steps joined by spaces.
pub fn surface_form(steps: &[String]) -> String {
    steps
        .iter()
        .map(|s| step_to_surface(s))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The live `● rec …▏` heading shown while recording. `subject` names what is
/// being bound (`"'my query'"`), or is empty when the host's own heading
/// already says so.
pub fn recording_heading(rec: &KeyRecorder, subject: &str, t: &Theme) -> Line<'static> {
    let accent = Style::default()
        .fg(t.form_accent())
        .add_modifier(Modifier::BOLD);
    let label = if rec.overwrite() {
        "\u{25cf} rec (replace) "
    } else {
        "\u{25cf} rec "
    };
    let mut spans = vec![Span::styled(label, accent)];
    if !subject.is_empty() {
        spans.push(Span::styled(
            format!("{subject} "),
            Style::default().fg(t.form_hint()),
        ));
    }
    spans.push(Span::styled(
        format!("{}\u{258f}", rec.surface()),
        Style::default()
            .fg(t.form_text())
            .add_modifier(Modifier::BOLD),
    ));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_surface_forms() {
        assert_eq!(step_to_surface("a"), "a");
        assert_eq!(step_to_surface("ctrl+k"), "ctrl+k");
        assert_eq!(step_to_surface(" "), "space");
        assert_eq!(step_to_surface("ctrl+ "), "ctrl+space");
        assert_eq!(
            surface_form(&["ctrl+k".into(), "l".into()]),
            "ctrl+k l".to_string()
        );
    }

    #[test]
    fn records_a_sequence_and_saves_it() {
        let mut r = KeyRecorder::new(false);
        assert_eq!(r.feed("ctrl+k"), RecorderStep::Recording);
        assert_eq!(r.feed("l"), RecorderStep::Recording);
        assert_eq!(r.feed("enter"), RecorderStep::Saved("ctrl+k l".into()));
    }

    #[test]
    fn backspace_drops_the_last_step() {
        let mut r = KeyRecorder::new(false);
        r.feed("f");
        r.feed("x");
        r.feed("backspace");
        r.feed("f");
        assert_eq!(r.feed("enter"), RecorderStep::Saved("f f".into()));
    }

    #[test]
    fn esc_and_empty_return_both_cancel() {
        let mut r = KeyRecorder::new(false);
        r.feed("g");
        assert_eq!(r.feed("esc"), RecorderStep::Cancelled);

        let mut empty = KeyRecorder::new(false);
        assert_eq!(empty.feed("enter"), RecorderStep::Cancelled);
    }

    #[test]
    fn a_literal_space_records_as_the_word_space() {
        let mut r = KeyRecorder::new(true);
        r.feed(" ");
        assert_eq!(r.feed("enter"), RecorderStep::Saved("space".into()));
        assert!(r.overwrite());
    }
}
