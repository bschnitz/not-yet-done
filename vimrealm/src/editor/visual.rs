//! Visual mode: pick a span first, then say what happens to it.
//!
//! Normal mode is `{operator}{motion}` — the span is described before it
//! exists. Visual mode turns that around: the selection is already on screen,
//! so an operator applies to it immediately and there is nothing pending.
//!
//! The keys that act on a selection (`d c y o v V`) are handled here rather
//! than through [`crate::keymap`], because they are only meaningful while a
//! selection exists and would otherwise need a second, near-empty table. The
//! keys that *move* the far end are looked up in the keymap, so a host that
//! rebinds `j` gets that binding in visual mode too.

use tuirealm::event::{Key, KeyEvent};

use super::VimEditor;
use super::normal::Handled;
use super::pending::Pending;
use crate::buffer::Position;
use crate::keymap::NormalCommand;
use crate::mode::Mode;
use crate::motion::{self, Motion};
use crate::operator::{self, Operator};
use crate::register::RegisterSink;
use crate::state::VimEvent;

impl VimEditor {
    /// Start a selection anchored at the cursor.
    pub(super) fn enter_visual(&mut self, line: bool) {
        self.pending = Pending::default();
        self.visual_anchor = self.buffer.cursor();
        self.mode = match line {
            true => Mode::VisualLine,
            false => Mode::Visual,
        };
    }

    pub(super) fn on_visual(&mut self, key: KeyEvent) -> Option<VimEvent> {
        // `"x`, counts, `g` and `i`/`a` mean the same as in normal mode.
        if let Some(event) = self.grammar_prefix(key) {
            return event.into_inner();
        }
        if let Key::Char(c) = key.code {
            if let Some(event) = self.visual_verb(c) {
                return event.into_inner();
            }
        }
        // Everything else has to be a motion; other normal-mode commands would
        // need a selection to be meaningful and are simply ignored.
        match self.keymap.normal(key) {
            Some(NormalCommand::Motion(m)) => {
                self.extend(m);
                None
            }
            Some(NormalCommand::Escape) => {
                self.leave_visual();
                None
            }
            Some(NormalCommand::Visual { line }) => {
                // `v` in charwise visual leaves it, as in vim; `V` switches.
                match (self.mode, line) {
                    (Mode::Visual, false) | (Mode::VisualLine, true) => self.leave_visual(),
                    _ => self.enter_visual_keeping_anchor(line),
                }
                None
            }
            _ => {
                self.pending = Pending::default();
                None
            }
        }
    }

    /// The verbs that only exist while something is selected.
    fn visual_verb(&mut self, c: char) -> Option<Handled> {
        let event = match c {
            'd' | 'x' => self.apply_visual(Operator::Delete),
            'c' | 's' => self.apply_visual(Operator::Change),
            'y' => self.apply_visual(Operator::Yank),
            // `o` jumps to the other end, so a selection started in the wrong
            // direction can be fixed without starting over.
            'o' => {
                let cursor = self.buffer.cursor();
                self.buffer.set_cursor(self.visual_anchor);
                self.visual_anchor = cursor;
                None
            }
            _ => return None,
        };
        Some(event.into())
    }

    /// Move the cursor end of the selection; the anchor stays put.
    fn extend(&mut self, m: Motion) {
        let count = self.pending.count();
        let explicit = self.pending.explicit();
        self.pending = Pending::default();
        let target = motion::resolve(&self.buffer, m, count, explicit);
        self.buffer.set_cursor(target);
    }

    /// Run `op` over the selection and go back to normal mode.
    fn apply_visual(&mut self, op: Operator) -> Option<VimEvent> {
        let (from, to) = self.selection()?;
        let name = self.pending.register;
        let linewise = self.mode == Mode::VisualLine;
        // Back to normal *before* the operator, so `finish_operator` can put us
        // into insert mode for `c` without visual mode leaking through.
        self.mode = Mode::Normal;
        let sink = &mut RegisterSink::new(&mut self.registers, name);
        let insert = match linewise {
            true => operator::apply_lines(&mut self.buffer, sink, op, from.line, to.line),
            false => operator::apply_inclusive(&mut self.buffer, sink, op, from, to),
        };
        self.finish_operator(op, insert)
    }

    /// Select `[from, to)` — the range a text object resolved to.
    pub(super) fn select_range(&mut self, from: Position, to: Position) {
        self.pending = Pending::default();
        // The object's end is exclusive, the selection's is not.
        let last = self.buffer.prev_pos(to).unwrap_or(from);
        self.visual_anchor = from;
        self.buffer.set_cursor(last);
        // `Vi(` narrows to the block itself, so it becomes charwise — keeping
        // it linewise would select the lines the block merely touches.
        self.mode = Mode::Visual;
    }

    fn leave_visual(&mut self) {
        self.pending = Pending::default();
        self.mode = Mode::Normal;
    }

    /// Switch between charwise and linewise without losing the selection.
    fn enter_visual_keeping_anchor(&mut self, line: bool) {
        self.pending = Pending::default();
        self.mode = match line {
            true => Mode::VisualLine,
            false => Mode::Visual,
        };
    }
}
