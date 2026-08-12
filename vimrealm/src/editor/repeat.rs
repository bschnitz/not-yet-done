//! `.` — do the last change again.
//!
//! Vim remembers the last change *semantically*: the operator, its count, the
//! text that was typed. This remembers the **keys** instead and feeds them back
//! through the state machine. It is far less code, and it repeats things a
//! semantic record would each need a case for — `ciwfoo<Esc>`, `vlld`, `2dw`,
//! `p` — because they are already expressed as keys.
//!
//! What it costs: a repeat re-runs the grammar, so it depends on the cursor
//! rather than on a remembered span, and a count in front of `.` repeats the
//! replay instead of replacing the recorded count. Both differ from vim only in
//! cases where the recorded command itself carried a count.

use tuirealm::event::KeyEvent;

use super::VimEditor;
use crate::keymap::NormalCommand;
use crate::mode::Mode;
use crate::state::VimEvent;

impl VimEditor {
    /// Collect `key` as part of the command currently being typed.
    pub(super) fn record(&mut self, key: KeyEvent) {
        if self.replaying || self.is_history_key(key) {
            return;
        }
        self.recording.push(key);
    }

    /// Decide, after the key has been handled, whether the recorded run is a
    /// finished change, a dead end, or still growing.
    ///
    /// `before` is the mode the key was handled in, `event` what it produced.
    pub(super) fn settle_recording(&mut self, before: Mode, event: Option<VimEvent>) {
        if self.replaying || self.recording.is_empty() {
            return;
        }
        // An insert session is part of the change that opened it, so nothing is
        // finished until the buffer is out of insert mode again.
        if self.mode == Mode::Insert {
            return;
        }
        let changed = event == Some(VimEvent::Changed);
        if changed || before == Mode::Insert {
            self.last_change = std::mem::take(&mut self.recording);
            return;
        }
        // A motion, a cancelled operator, an ex command: nothing to repeat. Keep
        // collecting only while something is still half-typed or selected.
        if self.mode == Mode::Normal && !self.is_pending() {
            self.recording.clear();
        }
    }

    /// `u`, `Ctrl+R` and `.` operate *on* the history and must stay out of it —
    /// otherwise `.` would repeat itself.
    fn is_history_key(&self, key: KeyEvent) -> bool {
        self.mode == Mode::Normal
            && matches!(
                self.keymap.normal(key),
                Some(NormalCommand::Undo | NormalCommand::Redo | NormalCommand::Repeat)
            )
    }

    /// Replay the last change `count` times.
    pub(super) fn repeat_change(&mut self, count: usize) -> Option<VimEvent> {
        if self.last_change.is_empty() {
            self.message = Some("Nothing to repeat".into());
            return None;
        }
        let keys = self.last_change.clone();
        self.replaying = true;
        let mut event = None;
        for _ in 0..count.max(1) {
            for key in &keys {
                if let Some(produced) = self.on_key(*key) {
                    event = Some(produced);
                }
            }
        }
        self.replaying = false;
        // A replay that ends mid-insert would leave the user in insert mode with
        // no way back that they asked for; vim's `.` always lands in normal mode.
        if self.mode == Mode::Insert {
            self.leave_insert();
        }
        event
    }
}
