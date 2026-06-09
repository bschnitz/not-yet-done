//! Filter form state: field definitions, status filter, and text editing state.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterField {
    CreatedAfter,
    CreatedBefore,
    Description,
    Status,
    Priority,
    ShowDeleted,
}

impl Default for FilterField {
    fn default() -> Self {
        FilterField::CreatedAfter
    }
}

impl FilterField {
    pub const ALL: &'static [FilterField] = &[
        FilterField::CreatedAfter,
        FilterField::CreatedBefore,
        FilterField::Description,
        FilterField::Status,
        FilterField::Priority,
        FilterField::ShowDeleted,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            FilterField::CreatedAfter => "Created after",
            FilterField::CreatedBefore => "Created before",
            FilterField::Description => "Description",
            FilterField::Status => "Status",
            FilterField::Priority => "Priority ≥",
            FilterField::ShowDeleted => "Include deleted",
        }
    }

    pub fn next(&self) -> FilterField {
        let idx = Self::ALL.iter().position(|f| f == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    pub fn prev(&self) -> FilterField {
        let idx = Self::ALL.iter().position(|f| f == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, Default)]
pub struct StatusFilter {
    pub todo: bool,
    pub in_progress: bool,
    pub done: bool,
    pub cancelled: bool,
}

impl StatusFilter {
    pub fn is_empty(&self) -> bool {
        !self.todo && !self.in_progress && !self.done && !self.cancelled
    }
}

#[derive(Debug, Clone, Default)]
pub struct FilterState {
    pub created_after_raw: String,
    pub created_before_raw: String,
    pub description_like: String,
    pub status: StatusFilter,
    pub priority_min_raw: String,
    pub show_deleted: bool,

    pub created_after_err: Option<String>,
    pub created_before_err: Option<String>,
    pub priority_err: Option<String>,

    pub focused_field: FilterField,
    pub cursor_pos: usize,
    pub status_cursor: usize,
}

impl FilterState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn focused_text_mut(&mut self) -> Option<&mut String> {
        match self.focused_field {
            FilterField::CreatedAfter => Some(&mut self.created_after_raw),
            FilterField::CreatedBefore => Some(&mut self.created_before_raw),
            FilterField::Description => Some(&mut self.description_like),
            FilterField::Priority => Some(&mut self.priority_min_raw),
            FilterField::Status | FilterField::ShowDeleted => None,
        }
    }

    pub fn insert_char(&mut self, c: char) {
        let pos = self.cursor_pos;
        if let Some(s) = self.focused_text_mut() {
            let byte_pos = s.char_indices().nth(pos).map(|(i, _)| i).unwrap_or(s.len());
            s.insert(byte_pos, c);
            self.cursor_pos += 1;
        }
    }

    pub fn backspace(&mut self) {
        if self.cursor_pos == 0 { return; }
        let pos = self.cursor_pos;
        if let Some(s) = self.focused_text_mut() {
            if s.is_empty() { return; }
            let byte_pos = s.char_indices().nth(pos - 1).map(|(i, _)| i).unwrap_or(0);
            s.remove(byte_pos);
            self.cursor_pos -= 1;
        }
    }

    pub fn cursor_left(&mut self) {
        if self.cursor_pos > 0 { self.cursor_pos -= 1; }
    }

    pub fn cursor_right(&mut self) {
        let len = self.focused_text_len();
        if self.cursor_pos < len { self.cursor_pos += 1; }
    }

    fn focused_text_len(&self) -> usize {
        match self.focused_field {
            FilterField::CreatedAfter => self.created_after_raw.chars().count(),
            FilterField::CreatedBefore => self.created_before_raw.chars().count(),
            FilterField::Description => self.description_like.chars().count(),
            FilterField::Priority => self.priority_min_raw.chars().count(),
            FilterField::Status | FilterField::ShowDeleted => 0,
        }
    }

    pub fn focus_next(&mut self) {
        self.focused_field = self.focused_field.next();
        self.clamp_cursor();
    }

    pub fn focus_prev(&mut self) {
        self.focused_field = self.focused_field.prev();
        self.clamp_cursor();
    }

    fn clamp_cursor(&mut self) {
        let max = self.focused_text_len();
        if self.cursor_pos > max { self.cursor_pos = max; }
    }

    pub fn toggle_status_cursor(&mut self) {
        match self.status_cursor {
            0 => self.status.todo = !self.status.todo,
            1 => self.status.in_progress = !self.status.in_progress,
            2 => self.status.done = !self.status.done,
            3 => self.status.cancelled = !self.status.cancelled,
            _ => {}
        }
    }

    pub fn status_cursor_next(&mut self) {
        self.status_cursor = (self.status_cursor + 1) % 4;
    }

    pub fn status_cursor_prev(&mut self) {
        self.status_cursor = (self.status_cursor + 3) % 4;
    }

    pub fn toggle_show_deleted(&mut self) {
        self.show_deleted = !self.show_deleted;
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_backspace() {
        let mut f = FilterState::new();
        f.focused_field = FilterField::Description;
        f.insert_char('a');
        f.insert_char('b');
        assert_eq!(f.description_like, "ab");
        assert_eq!(f.cursor_pos, 2);
        f.backspace();
        assert_eq!(f.description_like, "a");
        assert_eq!(f.cursor_pos, 1);
    }

    #[test]
    fn cursor_movement() {
        let mut f = FilterState::new();
        f.focused_field = FilterField::Description;
        f.insert_char('x');
        f.insert_char('y');
        f.cursor_left();
        assert_eq!(f.cursor_pos, 1);
        f.cursor_left();
        assert_eq!(f.cursor_pos, 0);
        f.cursor_left(); // at start, stays
        assert_eq!(f.cursor_pos, 0);
        f.cursor_right();
        assert_eq!(f.cursor_pos, 1);
    }

    #[test]
    fn focus_next_cycles() {
        let mut f = FilterState::new();
        assert_eq!(f.focused_field, FilterField::CreatedAfter);
        f.focus_next();
        assert_eq!(f.focused_field, FilterField::CreatedBefore);
        for _ in 0..5 {
            f.focus_next();
        }
        assert_eq!(f.focused_field, FilterField::CreatedAfter); // cycled back
    }

    #[test]
    fn toggle_status() {
        let mut f = FilterState::new();
        assert!(!f.status.todo);
        f.status_cursor = 0;
        f.toggle_status_cursor();
        assert!(f.status.todo);
        f.toggle_status_cursor();
        assert!(!f.status.todo);
    }

    #[test]
    fn reset_clears_everything() {
        let mut f = FilterState::new();
        f.focused_field = FilterField::Priority;
        f.description_like = "test".into();
        f.cursor_pos = 3;
        f.reset();
        assert_eq!(f.focused_field, FilterField::CreatedAfter);
        assert_eq!(f.description_like, "");
        assert_eq!(f.cursor_pos, 0);
    }

    #[test]
    fn backspace_at_start_is_noop() {
        let mut f = FilterState::new();
        f.focused_field = FilterField::Description;
        f.backspace(); // cursor at 0, should not panic
        assert_eq!(f.cursor_pos, 0);
    }

    #[test]
    fn insert_on_status_field_is_noop() {
        let mut f = FilterState::new();
        f.focused_field = FilterField::Status;
        assert!(f.focused_text_mut().is_none());
    }
}
