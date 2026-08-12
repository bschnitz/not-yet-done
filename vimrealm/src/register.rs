//! Registers — where deleted and yanked text goes.
//!
//! Vim's register file is a keyed store with three wrinkles worth encoding
//! rather than re-deriving at every call site: every yank or delete also lands
//! in the *unnamed* register, an uppercase name (`"A`) appends instead of
//! overwriting, and `"_` is a black hole that swallows text without disturbing
//! anything else.
//!
//! Operators write through a [`RegisterSink`], which bundles the store with the
//! register the user asked for. That keeps those three rules in one place and
//! keeps the operator functions from growing a parameter each.

use std::collections::BTreeMap;

/// The register that discards whatever is written to it.
pub const BLACK_HOLE: char = '_';

/// One register's content. `linewise` decides whether `p` opens a new line or
/// pastes after the cursor — the same distinction vim keeps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Register {
    pub text: String,
    pub linewise: bool,
}

/// What [`Registers::get`] hands back for an empty or black-hole register.
static EMPTY: Register = Register {
    text: String::new(),
    linewise: false,
};

impl Register {
    pub fn charwise(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            linewise: false,
        }
    }

    pub fn linewise(lines: &[String]) -> Self {
        Self {
            text: lines.join("\n"),
            linewise: true,
        }
    }

    /// Empty means "nothing to paste". A linewise register holding one empty
    /// line is *not* empty — pasting it inserts a blank line, as in vim.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && !self.linewise
    }

    /// Append for `"A`. Linewise content joins on a new line; mixing the two
    /// promotes the result to linewise, because half a line cannot be pasted
    /// linewise.
    fn append(&mut self, other: &Register) {
        if self.is_empty() {
            *self = other.clone();
            return;
        }
        if self.linewise || other.linewise {
            self.text.push('\n');
            self.linewise = true;
        }
        self.text.push_str(&other.text);
    }
}

/// The unnamed register plus the named ones (`"a`…`"z`).
#[derive(Debug, Clone, Default)]
pub struct Registers {
    unnamed: Register,
    named: BTreeMap<char, Register>,
}

impl Registers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read a register. `None` is the unnamed one; an uppercase name reads its
    /// lowercase counterpart, since the case only ever meant "append on write".
    pub fn get(&self, name: Option<char>) -> &Register {
        match name {
            None => &self.unnamed,
            Some(BLACK_HOLE) => &EMPTY,
            Some(c) => self.named.get(&c.to_ascii_lowercase()).unwrap_or(&EMPTY),
        }
    }

    /// Write a register, applying vim's rules: uppercase appends, the black
    /// hole discards, and anything written to a named register also becomes
    /// the unnamed one.
    pub fn set(&mut self, name: Option<char>, content: Register) {
        match name {
            Some(BLACK_HOLE) => {}
            Some(c) if c.is_ascii_uppercase() => {
                let entry = self.named.entry(c.to_ascii_lowercase()).or_default();
                entry.append(&content);
                self.unnamed = entry.clone();
            }
            Some(c) => {
                self.named.insert(c, content.clone());
                self.unnamed = content;
            }
            None => self.unnamed = content,
        }
    }

    /// The names in use, for a host that wants to show them.
    pub fn names(&self) -> impl Iterator<Item = char> + '_ {
        self.named.keys().copied()
    }
}

/// An operator's write target: the store plus the register the user named.
pub struct RegisterSink<'a> {
    regs: &'a mut Registers,
    name: Option<char>,
}

impl<'a> RegisterSink<'a> {
    pub fn new(regs: &'a mut Registers, name: Option<char>) -> Self {
        Self { regs, name }
    }

    /// The unnamed register alone — the sink for hosts and tests that do not
    /// care about names.
    pub fn unnamed(regs: &'a mut Registers) -> Self {
        Self::new(regs, None)
    }

    pub fn set_charwise(&mut self, text: String) {
        self.regs.set(self.name, Register::charwise(text));
    }

    pub fn set_linewise(&mut self, lines: &[String]) {
        self.regs.set(self.name, Register::linewise(lines));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_named_write_also_becomes_the_unnamed_register() {
        let mut regs = Registers::new();
        regs.set(Some('a'), Register::charwise("foo"));
        assert_eq!(regs.get(Some('a')).text, "foo");
        assert_eq!(
            regs.get(None).text,
            "foo",
            "vim mirrors the write into \"\""
        );
    }

    #[test]
    fn an_uppercase_name_appends() {
        let mut regs = Registers::new();
        regs.set(Some('a'), Register::charwise("foo"));
        regs.set(Some('A'), Register::charwise("bar"));
        assert_eq!(regs.get(Some('a')).text, "foobar");
        assert_eq!(
            regs.get(Some('A')).text,
            "foobar",
            "reading ignores the case"
        );
    }

    #[test]
    fn appending_linewise_content_starts_a_new_line() {
        let mut regs = Registers::new();
        regs.set(Some('a'), Register::linewise(&["one".to_string()]));
        regs.set(Some('A'), Register::linewise(&["two".to_string()]));
        let reg = regs.get(Some('a'));
        assert_eq!(reg.text, "one\ntwo");
        assert!(reg.linewise);
    }

    #[test]
    fn appending_a_charwise_run_to_a_linewise_one_stays_linewise() {
        let mut regs = Registers::new();
        regs.set(Some('a'), Register::linewise(&["one".to_string()]));
        regs.set(Some('A'), Register::charwise("two"));
        assert!(regs.get(Some('a')).linewise);
    }

    #[test]
    fn the_black_hole_swallows_without_touching_anything() {
        let mut regs = Registers::new();
        regs.set(None, Register::charwise("keep"));
        regs.set(Some(BLACK_HOLE), Register::charwise("gone"));
        assert_eq!(regs.get(None).text, "keep");
        assert!(regs.get(Some(BLACK_HOLE)).is_empty());
    }

    #[test]
    fn an_unwritten_register_reads_empty() {
        let regs = Registers::new();
        assert!(regs.get(Some('z')).is_empty());
        assert!(regs.get(None).is_empty());
    }

    #[test]
    fn a_linewise_empty_line_is_still_worth_pasting() {
        let reg = Register::linewise(&[String::new()]);
        assert!(!reg.is_empty(), "pasting it inserts a blank line");
    }

    #[test]
    fn the_sink_routes_writes_through_the_naming_rules() {
        let mut regs = Registers::new();
        RegisterSink::new(&mut regs, Some('q')).set_charwise("x".into());
        assert_eq!(regs.get(Some('q')).text, "x");
        RegisterSink::unnamed(&mut regs).set_linewise(&["l".to_string()]);
        assert!(regs.get(None).linewise);
        assert_eq!(regs.get(Some('q')).text, "x", "the named one is untouched");
        assert_eq!(regs.names().collect::<Vec<_>>(), vec!['q']);
    }
}
