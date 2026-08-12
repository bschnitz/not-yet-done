//! The command line — a one-line editor shared by `:` and, later, `/`.
//!
//! The prompt character decides what pressing Enter means, which is why the
//! mode is one [`Mode::Command`] rather than one mode per prompt: the editing
//! (typing, backspace, Esc) is identical, only the verb differs.

use tuirealm::event::{Key, KeyEvent};

use super::VimEditor;
use crate::mode::Mode;
use crate::state::VimEvent;

impl VimEditor {
    /// Open the command line with `prompt` as its leading character.
    pub(super) fn open_command_line(&mut self, prompt: char) {
        self.mode = Mode::Command;
        self.prompt = prompt;
        self.command.clear();
    }

    pub(super) fn on_command(&mut self, key: KeyEvent) -> Option<VimEvent> {
        match key.code {
            Key::Esc => {
                self.mode = Mode::Normal;
                self.command.clear();
                None
            }
            Key::Enter => self.run_command_line(),
            Key::Backspace => {
                if self.command.pop().is_none() {
                    // Backspacing away the prompt leaves command mode, as in vim.
                    self.mode = Mode::Normal;
                }
                None
            }
            Key::Char(c) => {
                self.command.push(c);
                None
            }
            _ => None,
        }
    }

    fn run_command_line(&mut self) -> Option<VimEvent> {
        let line = std::mem::take(&mut self.command);
        self.mode = Mode::Normal;
        match self.prompt {
            // A search pattern is taken literally — a trailing space is part of
            // what the user is looking for, so it must not be trimmed away.
            '/' => self.run_search(&line, true),
            '?' => self.run_search(&line, false),
            _ => self.run_ex_command(line.trim()),
        }
    }

    /// Run what was typed after `:`. The crate deliberately knows no files —
    /// every write turns into an event for the host.
    fn run_ex_command(&mut self, cmd: &str) -> Option<VimEvent> {
        match cmd {
            "" => None,
            "w" => {
                self.buffer.mark_clean();
                Some(VimEvent::Save)
            }
            "wq" | "x" | "wq!" | "x!" => {
                self.buffer.mark_clean();
                Some(VimEvent::SaveAndClose)
            }
            "q" if self.buffer.is_dirty() => {
                self.message = Some("E37: No write since last change (add ! to override)".into());
                None
            }
            "q" | "q!" => Some(VimEvent::Cancel),
            other => {
                self.message = Some(format!("E492: Not an editor command: {other}"));
                None
            }
        }
    }
}
