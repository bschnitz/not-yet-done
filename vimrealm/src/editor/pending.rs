//! Input collected but not yet complete.
//!
//! Vim's normal mode is `["x]{count}{operator}{count}{motion}`, which means a
//! key press can be *incomplete*: `"`, `2`, `d`, `3` all leave the editor
//! waiting. This struct is that wait, and every completed command clears it.

use crate::operator::Operator;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Pending {
    /// Digits typed before an operator.
    pub count: Option<usize>,
    pub operator: Option<Operator>,
    /// Digits typed after an operator (`d2w`).
    pub op_count: Option<usize>,
    /// `g` was typed and we are waiting for the second key of `gg`.
    pub g: bool,
    /// `"` was typed and we are waiting for the register name.
    pub awaiting_register: bool,
    /// The register named by `"x`, if any.
    pub register: Option<char>,
    /// `i` or `a` was typed after an operator: a text object is coming, and
    /// `true` means the inner variant (`iw` rather than `aw`).
    pub textobject_inner: Option<bool>,
}

impl Pending {
    pub fn is_empty(&self) -> bool {
        *self == Pending::default()
    }

    /// The effective count — vim multiplies the two (`2d3w` deletes 6 words).
    pub fn count(&self) -> usize {
        self.count.unwrap_or(1) * self.op_count.unwrap_or(1)
    }

    pub fn explicit(&self) -> bool {
        self.count.is_some() || self.op_count.is_some()
    }

    /// Push a digit onto whichever count is being typed right now.
    pub fn push_digit(&mut self, d: usize) {
        let slot = if self.operator.is_some() {
            &mut self.op_count
        } else {
            &mut self.count
        };
        *slot = Some(slot.unwrap_or(0) * 10 + d);
    }

    /// Is a count currently being typed? Decides whether `0` is a digit or the
    /// `LineStart` motion.
    pub fn typing_count(&self) -> bool {
        if self.operator.is_some() {
            self.op_count.is_some()
        } else {
            self.count.is_some()
        }
    }

    /// What to show in the bottom-right corner while input is incomplete.
    pub fn label(&self) -> String {
        let mut out = String::new();
        if self.awaiting_register {
            out.push('"');
        }
        if let Some(name) = self.register {
            out.push('"');
            out.push(name);
        }
        if let Some(c) = self.count {
            out.push_str(&c.to_string());
        }
        if let Some(op) = self.operator {
            out.push(match op {
                Operator::Delete => 'd',
                Operator::Change => 'c',
                Operator::Yank => 'y',
            });
        }
        if let Some(c) = self.op_count {
            out.push_str(&c.to_string());
        }
        if let Some(inner) = self.textobject_inner {
            out.push(if inner { 'i' } else { 'a' });
        }
        if self.g {
            out.push('g');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_counts_multiply_and_default_to_one() {
        let mut p = Pending::default();
        assert_eq!(p.count(), 1);
        assert!(!p.explicit());
        p.push_digit(2);
        p.operator = Some(Operator::Delete);
        p.push_digit(3);
        assert_eq!(p.count(), 6);
        assert!(p.explicit());
    }

    #[test]
    fn digits_land_on_the_count_that_is_being_typed() {
        let mut p = Pending::default();
        p.push_digit(1);
        p.push_digit(2);
        assert_eq!(p.count, Some(12));
        p.operator = Some(Operator::Yank);
        p.push_digit(4);
        assert_eq!(p.count, Some(12), "the first count is finished");
        assert_eq!(p.op_count, Some(4));
    }

    #[test]
    fn the_label_shows_the_whole_typed_prefix() {
        let mut p = Pending::default();
        p.register = Some('a');
        p.push_digit(2);
        p.operator = Some(Operator::Delete);
        p.push_digit(3);
        p.textobject_inner = Some(true);
        assert_eq!(p.label(), "\"a2d3i");
    }
}
