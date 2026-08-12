//! Reusable command-line component for views.
//!
//! Manages the `:` command-line input state. Command execution remains
//! with the App since it needs process spawning and modal messages.

use crate::views::CmdlineKeyResult;
use crate::views::CmdlineState;

/// Command-line input component, driven by `:` key.
pub struct CmdlineComponent {
    active: bool,
    query: String,
    cursor: usize,
}

impl CmdlineComponent {
    pub fn new() -> Self {
        Self {
            active: false,
            query: String::new(),
            cursor: 0,
        }
    }

    pub fn active(&self) -> bool {
        self.active
    }

    pub fn open(&mut self) {
        self.active = true;
        self.query.clear();
        self.cursor = 0;
    }

    /// Open the cmdline with `prefill` already typed and the cursor at
    /// the end. Used by adapter-action flows that want the user to
    /// finish a partially-typed command (e.g. `:db-script new `
    /// → user types just the script name and presses Enter).
    pub fn open_with(&mut self, prefill: &str) {
        self.active = true;
        self.query = prefill.to_string();
        self.cursor = self.query.chars().count();
    }

    pub fn close(&mut self) {
        self.active = false;
    }

    /// Return a snapshot for UI synchronisation.
    pub fn state(&self) -> CmdlineState {
        CmdlineState {
            active: self.active,
            query: self.query.clone(),
            cursor: self.cursor,
        }
    }

    /// Handle a key press while the command line is active.
    pub fn handle_key(&mut self, key: &str) -> CmdlineKeyResult {
        match key {
            "enter" => {
                let cmd = self.query.clone();
                self.close();
                if cmd.trim().is_empty() {
                    CmdlineKeyResult::Closed
                } else {
                    CmdlineKeyResult::Execute(cmd)
                }
            }
            "esc" => {
                self.close();
                CmdlineKeyResult::Closed
            }
            "backspace" => {
                if self.cursor > 0 {
                    let byte_pos = self
                        .query
                        .char_indices()
                        .nth(self.cursor - 1)
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.query.remove(byte_pos);
                    self.cursor -= 1;
                }
                CmdlineKeyResult::Handled
            }
            "left" => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                CmdlineKeyResult::Handled
            }
            "right" => {
                let max = self.query.chars().count();
                if self.cursor < max {
                    self.cursor += 1;
                }
                CmdlineKeyResult::Handled
            }
            ch if ch.chars().count() == 1 && !ch.chars().next().unwrap().is_control() => {
                let byte_pos = self
                    .query
                    .char_indices()
                    .nth(self.cursor)
                    .map(|(i, _)| i)
                    .unwrap_or(self.query.len());
                self.query.insert(byte_pos, ch.chars().next().unwrap());
                self.cursor += 1;
                CmdlineKeyResult::Handled
            }
            _ => CmdlineKeyResult::Handled,
        }
    }
}
