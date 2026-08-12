//! Key → command tables.
//!
//! The other widgets in this workspace expose their bindings as one struct
//! field per key. With roughly thirty vim bindings that would be unwieldy, so
//! the map here is a `HashMap<KeyEvent, Command>` instead: callers override a
//! single binding with [`Keymap::bind`] and drop one with [`Keymap::unbind`],
//! without the struct growing a field per key.
//!
//! Two things are deliberately *not* in the table, because they are grammar
//! rather than bindings and the state machine in [`crate::editor`] owns them:
//! digits (the `{count}` prefix) and the `g` prefix of `gg`.

use std::collections::HashMap;

use tuirealm::event::{Key, KeyEvent, KeyModifiers};

use crate::motion::Motion;
use crate::operator::Operator;

/// Where `i a I A o O` put the cursor before switching to insert mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertAt {
    /// `i`
    Cursor,
    /// `a`
    After,
    /// `I`
    LineFirstNonBlank,
    /// `A`
    LineEnd,
    /// `o`
    OpenBelow,
    /// `O`
    OpenAbove,
}

/// What a key does in normal mode. Operator-pending is a state, not a command:
/// [`NormalCommand::Operator`] arms it and the next motion resolves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalCommand {
    Motion(Motion),
    Operator(Operator),
    Insert(InsertAt),
    /// `x`
    DeleteChar,
    /// `D`
    DeleteToLineEnd,
    /// `C`
    ChangeToLineEnd,
    /// `p` / `P`
    Paste {
        after: bool,
    },
    /// `v` / `V` — start a selection. Once it runs, the keys that act *on* the
    /// selection are the editor's own, not this table's.
    Visual {
        line: bool,
    },
    /// `u`
    Undo,
    /// `.` — replay the last change.
    Repeat,
    /// `Ctrl+R`
    Redo,
    /// `:` — start an ex command.
    CommandLine,
    /// `/` and `?` — start a search.
    Search {
        forward: bool,
    },
    /// `n` / `N` — jump to the next match of the last pattern.
    SearchNext {
        reverse: bool,
    },
    /// `Esc` — cancel a pending count/operator.
    Escape,
}

/// What a key does in insert mode. Anything not in the table and printable is
/// inserted literally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertCommand {
    /// Back to normal mode; the cursor steps left like vim's.
    Escape,
    Newline,
    Backspace,
    Delete,
    /// Arrow keys still work in insert mode, as in vim.
    Motion(Motion),
}

#[derive(Debug, Clone)]
pub struct Keymap {
    normal: HashMap<KeyEvent, NormalCommand>,
    insert: HashMap<KeyEvent, InsertCommand>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::vim()
    }
}

impl Keymap {
    /// An empty map — for hosts that want to spell out every binding.
    pub fn empty() -> Self {
        Self {
            normal: HashMap::new(),
            insert: HashMap::new(),
        }
    }

    /// The default vim bindings.
    pub fn vim() -> Self {
        use InsertAt as At;
        use Motion as M;
        use NormalCommand as N;

        let mut map = Self::empty();
        for (c, cmd) in [
            ('h', N::Motion(M::Left)),
            ('j', N::Motion(M::Down)),
            ('k', N::Motion(M::Up)),
            ('l', N::Motion(M::Right)),
            ('w', N::Motion(M::WordForward)),
            ('b', N::Motion(M::WordBackward)),
            ('e', N::Motion(M::WordEnd)),
            ('0', N::Motion(M::LineStart)),
            ('^', N::Motion(M::LineFirstNonBlank)),
            ('$', N::Motion(M::LineEnd)),
            ('G', N::Motion(M::FileEnd)),
            ('d', N::Operator(Operator::Delete)),
            ('c', N::Operator(Operator::Change)),
            ('y', N::Operator(Operator::Yank)),
            ('i', N::Insert(At::Cursor)),
            ('a', N::Insert(At::After)),
            ('I', N::Insert(At::LineFirstNonBlank)),
            ('A', N::Insert(At::LineEnd)),
            ('o', N::Insert(At::OpenBelow)),
            ('O', N::Insert(At::OpenAbove)),
            ('x', N::DeleteChar),
            ('D', N::DeleteToLineEnd),
            ('C', N::ChangeToLineEnd),
            ('p', N::Paste { after: true }),
            ('P', N::Paste { after: false }),
            ('u', N::Undo),
            ('.', N::Repeat),
            ('v', N::Visual { line: false }),
            ('V', N::Visual { line: true }),
            (':', N::CommandLine),
            ('/', N::Search { forward: true }),
            ('?', N::Search { forward: false }),
            ('n', N::SearchNext { reverse: false }),
            ('N', N::SearchNext { reverse: true }),
        ] {
            map.bind(Key::Char(c).into(), cmd);
        }
        // Arrow keys mirror hjkl; they are the escape hatch for anyone who has
        // not internalised the home row yet.
        map.bind(Key::Left.into(), N::Motion(M::Left));
        map.bind(Key::Down.into(), N::Motion(M::Down));
        map.bind(Key::Up.into(), N::Motion(M::Up));
        map.bind(Key::Right.into(), N::Motion(M::Right));
        map.bind(Key::Home.into(), N::Motion(M::LineStart));
        map.bind(Key::End.into(), N::Motion(M::LineEnd));
        map.bind(Key::Esc.into(), N::Escape);
        map.bind(
            KeyEvent::new(Key::Char('r'), KeyModifiers::CONTROL),
            N::Redo,
        );

        use InsertCommand as I;
        map.bind_insert(Key::Esc.into(), I::Escape);
        map.bind_insert(Key::Enter.into(), I::Newline);
        map.bind_insert(Key::Backspace.into(), I::Backspace);
        map.bind_insert(Key::Delete.into(), I::Delete);
        map.bind_insert(Key::Left.into(), I::Motion(M::Left));
        map.bind_insert(Key::Down.into(), I::Motion(M::Down));
        map.bind_insert(Key::Up.into(), I::Motion(M::Up));
        map.bind_insert(Key::Right.into(), I::Motion(M::Right));
        map.bind_insert(Key::Home.into(), I::Motion(M::LineStart));
        map.bind_insert(Key::End.into(), I::Motion(M::LineEnd));
        map
    }

    pub fn bind(&mut self, key: KeyEvent, cmd: NormalCommand) -> &mut Self {
        self.normal.insert(normalize(key), cmd);
        self
    }

    pub fn bind_insert(&mut self, key: KeyEvent, cmd: InsertCommand) -> &mut Self {
        self.insert.insert(normalize(key), cmd);
        self
    }

    pub fn unbind(&mut self, key: KeyEvent) -> &mut Self {
        self.normal.remove(&normalize(key));
        self
    }

    pub fn unbind_insert(&mut self, key: KeyEvent) -> &mut Self {
        self.insert.remove(&normalize(key));
        self
    }

    pub fn normal(&self, key: KeyEvent) -> Option<NormalCommand> {
        self.normal.get(&normalize(key)).copied()
    }

    pub fn insert(&self, key: KeyEvent) -> Option<InsertCommand> {
        self.insert.get(&normalize(key)).copied()
    }
}

/// Drop the shift flag on character keys: terminals report `D` as
/// `Char('D') + SHIFT`, and a table keyed on the character alone would then
/// never match. The character itself already carries the case.
fn normalize(key: KeyEvent) -> KeyEvent {
    match key.code {
        Key::Char(_) => KeyEvent::new(key.code, key.modifiers - KeyModifiers::SHIFT),
        _ => key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_map_covers_the_core_bindings() {
        let map = Keymap::vim();
        assert_eq!(
            map.normal(Key::Char('w').into()),
            Some(NormalCommand::Motion(Motion::WordForward))
        );
        assert_eq!(
            map.normal(Key::Char('d').into()),
            Some(NormalCommand::Operator(Operator::Delete))
        );
        assert_eq!(
            map.normal(KeyEvent::new(Key::Char('r'), KeyModifiers::CONTROL)),
            Some(NormalCommand::Redo)
        );
        assert_eq!(map.insert(Key::Esc.into()), Some(InsertCommand::Escape));
    }

    #[test]
    fn a_shifted_character_key_still_matches() {
        let map = Keymap::vim();
        let shifted = KeyEvent::new(Key::Char('D'), KeyModifiers::SHIFT);
        assert_eq!(map.normal(shifted), Some(NormalCommand::DeleteToLineEnd));
    }

    #[test]
    fn bindings_can_be_overridden_and_removed() {
        let mut map = Keymap::vim();
        map.bind(Key::Char('x').into(), NormalCommand::Undo);
        assert_eq!(map.normal(Key::Char('x').into()), Some(NormalCommand::Undo));
        map.unbind(Key::Char('x').into());
        assert_eq!(map.normal(Key::Char('x').into()), None);
    }

    #[test]
    fn digits_are_grammar_and_stay_out_of_the_table() {
        let map = Keymap::vim();
        assert_eq!(map.normal(Key::Char('5').into()), None);
        assert_eq!(
            map.normal(Key::Char('0').into()),
            Some(NormalCommand::Motion(Motion::LineStart)),
            "0 is a motion; only a pending count turns it into a digit"
        );
        assert_eq!(map.normal(Key::Char('g').into()), None, "g is a prefix");
    }
}
