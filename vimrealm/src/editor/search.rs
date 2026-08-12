//! The search *commands* — what `/`, `?`, `n` and `N` do to the editor.
//!
//! The searching itself is in [`crate::search`]; what lives here is the state
//! vim keeps between searches (the last pattern and its direction) and the
//! messages it prints.

use super::VimEditor;
use crate::search;
use crate::state::VimEvent;

/// Vim's wording, so anyone who has hit this in vim recognises it.
const NO_PATTERN: &str = "E35: No previous regular expression";

impl VimEditor {
    /// Run what was typed after `/` or `?`. An empty pattern repeats the last
    /// one, which is how `//` works in vim.
    pub(super) fn run_search(&mut self, pattern: &str, forward: bool) -> Option<VimEvent> {
        let pattern = match pattern.is_empty() {
            true => self.last_search.clone(),
            false => Some(pattern.to_string()),
        };
        let Some(pattern) = pattern else {
            self.message = Some(NO_PATTERN.into());
            return None;
        };
        self.last_search = Some(pattern.clone());
        self.search_forward = forward;
        self.jump_to_match(&pattern, forward, 1);
        None
    }

    /// `n` (`reverse == false`) and `N`, which is the same search the other way.
    pub(super) fn repeat_search(&mut self, reverse: bool, count: usize) {
        let Some(pattern) = self.last_search.clone() else {
            self.message = Some(NO_PATTERN.into());
            return;
        };
        let forward = self.search_forward != reverse;
        self.jump_to_match(&pattern, forward, count);
    }

    /// Move to the `count`-th match. A search is never a change, so nothing is
    /// reported to the host — the only outcome is the cursor and a message.
    fn jump_to_match(&mut self, pattern: &str, forward: bool, count: usize) {
        let mut wrapped = false;
        for _ in 0..count.max(1) {
            let Some(hit) = search::find(&self.buffer, pattern, self.buffer.cursor(), forward)
            else {
                self.message = Some(format!("E486: Pattern not found: {pattern}"));
                return;
            };
            self.buffer.set_cursor(hit.pos);
            wrapped |= hit.wrapped;
        }
        if wrapped {
            self.message = Some(
                match forward {
                    true => "search hit BOTTOM, continuing at TOP",
                    false => "search hit TOP, continuing at BOTTOM",
                }
                .into(),
            );
        }
    }
}
