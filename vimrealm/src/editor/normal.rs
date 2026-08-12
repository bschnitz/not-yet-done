//! Normal mode: vim's `["x]{count}{operator}{count}{motion}` grammar.

use tuirealm::event::{Key, KeyEvent};

use super::VimEditor;
use super::pending::Pending;
use crate::buffer::Position;
use crate::keymap::{InsertAt, NormalCommand};
use crate::mode::Mode;
use crate::motion::{self, Motion};
use crate::operator::{self, Operator};
use crate::register::RegisterSink;
use crate::state::VimEvent;
use crate::textobject::{self, TextObject};

impl VimEditor {
    pub(super) fn on_normal(&mut self, key: KeyEvent) -> Option<VimEvent> {
        // Register names, digits and `g` are grammar, not bindings — they are
        // handled before the keymap so a host cannot accidentally shadow the
        // prefixes that every operator depends on.
        if let Some(event) = self.grammar_prefix(key) {
            return event.into_inner();
        }

        let Some(cmd) = self.keymap.normal(key) else {
            return None;
        };
        match cmd {
            NormalCommand::Motion(m) => self.run_motion(m),
            NormalCommand::Operator(op) => {
                match self.pending.operator {
                    // `dd`, `cc`, `yy` — the doubled operator is linewise.
                    Some(pending) if pending == op => {
                        let count = self.pending.count();
                        let name = self.pending.register;
                        let insert = operator::apply_current_lines(
                            &mut self.buffer,
                            &mut RegisterSink::new(&mut self.registers, name),
                            op,
                            count,
                        );
                        self.finish_operator(op, insert)
                    }
                    _ => {
                        self.pending.operator = Some(op);
                        None
                    }
                }
            }
            NormalCommand::Insert(at) => {
                self.pending = Pending::default();
                self.enter_insert(at);
                // `o`/`O` add a line, so the text already changed.
                matches!(at, InsertAt::OpenBelow | InsertAt::OpenAbove).then_some(VimEvent::Changed)
            }
            NormalCommand::DeleteChar => {
                let count = self.pending.count();
                let name = self.pending.register;
                operator::delete_chars(
                    &mut self.buffer,
                    &mut RegisterSink::new(&mut self.registers, name),
                    count,
                );
                self.pending = Pending::default();
                Some(VimEvent::Changed)
            }
            NormalCommand::DeleteToLineEnd => {
                let name = self.pending.register;
                operator::apply_to_line_end(
                    &mut self.buffer,
                    &mut RegisterSink::new(&mut self.registers, name),
                    Operator::Delete,
                );
                self.pending = Pending::default();
                Some(VimEvent::Changed)
            }
            NormalCommand::ChangeToLineEnd => {
                let name = self.pending.register;
                operator::apply_to_line_end(
                    &mut self.buffer,
                    &mut RegisterSink::new(&mut self.registers, name),
                    Operator::Change,
                );
                self.pending = Pending::default();
                self.mode = Mode::Insert;
                Some(VimEvent::Changed)
            }
            NormalCommand::Paste { after } => {
                let count = self.pending.count();
                let name = self.pending.register;
                self.pending = Pending::default();
                if self.registers.get(name).is_empty() {
                    self.message = Some("Nothing in register".into());
                    return None;
                }
                operator::paste(&mut self.buffer, self.registers.get(name), after, count);
                Some(VimEvent::Changed)
            }
            NormalCommand::Undo => {
                self.pending = Pending::default();
                if self.buffer.undo() {
                    Some(VimEvent::Changed)
                } else {
                    self.message = Some("Already at oldest change".into());
                    None
                }
            }
            NormalCommand::Redo => {
                self.pending = Pending::default();
                if self.buffer.redo() {
                    Some(VimEvent::Changed)
                } else {
                    self.message = Some("Already at newest change".into());
                    None
                }
            }
            NormalCommand::CommandLine => {
                self.pending = Pending::default();
                self.open_command_line(':');
                None
            }
            NormalCommand::Repeat => {
                let count = self.pending.count();
                self.pending = Pending::default();
                self.repeat_change(count)
            }
            NormalCommand::Search { forward } => {
                self.pending = Pending::default();
                self.open_command_line(if forward { '/' } else { '?' });
                None
            }
            NormalCommand::SearchNext { reverse } => {
                let count = self.pending.count();
                self.pending = Pending::default();
                self.repeat_search(reverse, count);
                None
            }
            NormalCommand::Visual { line } => {
                self.enter_visual(line);
                None
            }
            NormalCommand::Escape => {
                self.pending = Pending::default();
                None
            }
        }
    }

    /// The prefixes the state machine owns: `"x`, `{count}`, `g` and the
    /// `i`/`a` of a text object. Returns [`Handled`] when the key belonged to
    /// one of them, so the caller knows to stop before the keymap.
    pub(super) fn grammar_prefix(&mut self, key: KeyEvent) -> Option<Handled> {
        if self.pending.awaiting_register {
            self.pending.awaiting_register = false;
            match key.code {
                Key::Char(c) => self.pending.register = Some(c),
                // `"` followed by anything else is not a register name; vim
                // drops the whole half-typed command.
                _ => self.pending = Pending::default(),
            }
            return Some(Handled(None));
        }
        let Key::Char(c) = key.code else {
            // Only a character can name a text object, so `ci<Left>` is not a
            // command — drop it rather than leaving `ci` armed.
            if self.pending.textobject_inner.is_some() {
                self.pending = Pending::default();
                return Some(Handled(None));
            }
            return None;
        };
        if let Some(inner) = self.pending.textobject_inner {
            return Some(Handled(self.run_textobject(inner, c)));
        }
        if c == '"' {
            self.pending.awaiting_register = true;
            return Some(Handled(None));
        }
        if let Some(d) = c.to_digit(10) {
            if d != 0 || self.pending.typing_count() {
                self.pending.push_digit(d as usize);
                return Some(Handled(None));
            }
        }
        // `i`/`a` only introduce a text object while an operator waits for its
        // span, or while a selection is being made — bare `i` and `a` in normal
        // mode are the insert commands.
        if self.pending.operator.is_some() || self.mode.is_visual() {
            if let Some(inner) = match c {
                'i' => Some(true),
                'a' => Some(false),
                _ => None,
            } {
                self.pending.textobject_inner = Some(inner);
                return Some(Handled(None));
            }
        }
        if self.pending.g {
            self.pending.g = false;
            return Some(Handled(match c {
                'g' => self.run_motion(Motion::FileStart),
                // Unknown `g` sequence: vim beeps, we just drop it.
                _ => {
                    self.pending = Pending::default();
                    None
                }
            }));
        }
        if c == 'g' {
            self.pending.g = true;
            return Some(Handled(None));
        }
        None
    }

    /// Complete `{operator}{i|a}{object}`. A text object always resolves to a
    /// charwise range, so `ciw` needs no path of its own beside `cw`.
    ///
    /// Anything the object cannot find — an unknown key, an unclosed bracket,
    /// an empty pair — drops the command without a change, which is vim's beep.
    fn run_textobject(&mut self, inner: bool, key: char) -> Option<VimEvent> {
        let count = self.pending.count();
        let name = self.pending.register;
        let span = TextObject::from_char(key)
            .and_then(|obj| textobject::resolve(&self.buffer, obj, inner, count));
        let Some((from, to)) = span else {
            self.pending = Pending::default();
            return None;
        };
        if self.mode.is_visual() {
            // `viw` selects the object instead of operating on it.
            self.select_range(from, to);
            return None;
        }
        let Some(op) = self.pending.operator else {
            self.pending = Pending::default();
            return None;
        };
        operator::apply_range(
            &mut self.buffer,
            &mut RegisterSink::new(&mut self.registers, name),
            op,
            from,
            to,
        );
        self.finish_operator(op, op == Operator::Change)
    }

    /// A motion either moves the cursor or completes a pending operator.
    pub(super) fn run_motion(&mut self, m: Motion) -> Option<VimEvent> {
        let count = self.pending.count();
        let explicit = self.pending.explicit();
        match self.pending.operator {
            Some(op) => {
                let name = self.pending.register;
                let insert = operator::apply(
                    &mut self.buffer,
                    &mut RegisterSink::new(&mut self.registers, name),
                    op,
                    m,
                    count,
                    explicit,
                );
                self.finish_operator(op, insert)
            }
            None => {
                self.pending = Pending::default();
                let target = motion::resolve(&self.buffer, m, count, explicit);
                self.buffer.set_cursor(target);
                None
            }
        }
    }

    pub(super) fn finish_operator(&mut self, op: Operator, enter_insert: bool) -> Option<VimEvent> {
        self.pending = Pending::default();
        if enter_insert {
            self.mode = Mode::Insert;
        }
        // `y` leaves the text alone, so the host has nothing to react to.
        (op != Operator::Yank).then_some(VimEvent::Changed)
    }

    /// Enter insert mode, taking the single undo snapshot for the whole session
    /// — that is what makes `u` undo a typed run instead of one character.
    fn enter_insert(&mut self, at: InsertAt) {
        self.buffer.snapshot();
        let cursor = self.buffer.cursor();
        let pos = match at {
            InsertAt::Cursor => cursor,
            InsertAt::After => self
                .buffer
                .next_pos(cursor)
                .filter(|p| p.line == cursor.line)
                .unwrap_or(Position::new(cursor.line, self.buffer.end_col(cursor.line))),
            InsertAt::LineFirstNonBlank => {
                Position::new(cursor.line, self.buffer.first_non_blank(cursor.line))
            }
            InsertAt::LineEnd => Position::new(cursor.line, self.buffer.end_col(cursor.line)),
            InsertAt::OpenBelow => {
                let at = self
                    .buffer
                    .insert_lines(cursor.line, vec![String::new()], true);
                Position::new(at, 0)
            }
            InsertAt::OpenAbove => {
                let at = self
                    .buffer
                    .insert_lines(cursor.line, vec![String::new()], false);
                Position::new(at, 0)
            }
        };
        self.buffer.set_cursor_insert(pos);
        self.mode = Mode::Insert;
    }
}

/// "This key was mine" plus whatever event it produced — used by every layer
/// that gets a look at a key before the keymap. A plain
/// `Option<Option<VimEvent>>` would read the same but say nothing.
pub(super) struct Handled(Option<VimEvent>);

impl Handled {
    pub(super) fn into_inner(self) -> Option<VimEvent> {
        self.0
    }
}

impl From<Option<VimEvent>> for Handled {
    fn from(event: Option<VimEvent>) -> Self {
        Self(event)
    }
}
