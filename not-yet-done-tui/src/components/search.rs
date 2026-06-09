//! Reusable text-search component for views.
//!
//! Manages the search query, cursor, match indices, and navigation.
//! Each view that supports `/`-search embeds a `SearchComponent`.

use crate::views::{SearchKeyResult, SearchState};

/// Text search over a table, driven by `/` key.
pub struct SearchComponent {
    active: bool,
    query: String,
    cursor: usize,
    matches: Vec<usize>,
    current: usize,
}

impl SearchComponent {
    pub fn new() -> Self {
        Self {
            active: false,
            query: String::new(),
            cursor: 0,
            matches: Vec::new(),
            current: 0,
        }
    }

    pub fn active(&self) -> bool {
        self.active
    }

    pub fn open(&mut self) {
        self.active = true;
        self.cursor = self.query.chars().count();
    }

    pub fn close(&mut self) {
        self.active = false;
    }

    pub fn clear(&mut self) {
        self.query.clear();
        self.cursor = 0;
        self.matches.clear();
        self.current = 0;
    }

    /// Return a snapshot for UI synchronisation.
    pub fn state(&self) -> SearchState {
        SearchState {
            active: self.active,
            query: self.query.clone(),
            cursor: self.cursor,
            match_count: self.matches.len(),
            current: self.current,
        }
    }

    /// Handle a key press while search is active.
    ///
    /// The caller is responsible for calling [`update_matches`] and jumping
    /// to the first match after receiving [`SearchKeyResult::QueryChanged`].
    pub fn handle_key(&mut self, key: &str) -> SearchKeyResult {
        if key == "enter" {
            self.close();
            return SearchKeyResult::Accepted;
        }
        if key == "esc" {
            if self.query.is_empty() {
                self.close();
                return SearchKeyResult::Cancelled;
            } else {
                self.clear();
                return SearchKeyResult::Cancelled;
            }
        }
        match key {
            "backspace" => {
                if self.cursor > 0 {
                    let byte_pos = self.query.char_indices()
                        .nth(self.cursor - 1)
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.query.remove(byte_pos);
                    self.cursor -= 1;
                    return SearchKeyResult::QueryChanged;
                }
                SearchKeyResult::Handled
            }
            "left" => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                SearchKeyResult::Handled
            }
            "right" => {
                let max = self.query.chars().count();
                if self.cursor < max {
                    self.cursor += 1;
                }
                SearchKeyResult::Handled
            }
            ch if ch.chars().count() == 1 && !ch.chars().next().unwrap().is_control() => {
                let byte_pos = self.query.char_indices()
                    .nth(self.cursor)
                    .map(|(i, _)| i)
                    .unwrap_or(self.query.len());
                self.query.insert(byte_pos, ch.chars().next().unwrap());
                self.cursor += 1;
                SearchKeyResult::QueryChanged
            }
            _ => SearchKeyResult::Handled,
        }
    }

    /// Recompute matches from the given `(row_index, text)` pairs.
    ///
    /// Each view calls this with its own description data after receiving
    /// [`SearchKeyResult::QueryChanged`].
    pub fn update_matches(&mut self, descriptions: &[(usize, &str)]) {
        self.matches.clear();
        self.current = 0;
        if self.query.is_empty() {
            return;
        }
        let q = self.query.to_lowercase();
        for &(row_idx, text) in descriptions {
            if text.to_lowercase().contains(&q) {
                self.matches.push(row_idx);
            }
        }
    }

    /// Jump to the next or previous match. Returns the target row index.
    pub fn jump(&mut self, direction: isize) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        let len = self.matches.len() as isize;
        let new_idx = (self.current as isize + direction).rem_euclid(len) as usize;
        self.current = new_idx;
        Some(self.matches[new_idx])
    }

    /// Row index of the first match, if any. Resets `current` to 0.
    pub fn first_match(&mut self) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        self.current = 0;
        Some(self.matches[0])
    }

    pub fn matches(&self) -> &[usize] {
        &self.matches
    }
}
