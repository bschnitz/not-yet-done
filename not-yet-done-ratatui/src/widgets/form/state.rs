use crate::widgets::multi_choice::state::{MultiChoiceEvent, MultiChoiceState};
use crate::widgets::text_input::state::{TextInputEvent, TextInputState};

// ── Per-field state ───────────────────────────────────────────────────────────

/// Owned state for a single form field.
#[derive(Debug, Clone)]
pub enum FormFieldState {
    TextInput(TextInputState),
    MultiChoice(MultiChoiceState),
}

impl FormFieldState {
    pub fn as_text_input(&self) -> Option<&TextInputState> {
        match self {
            Self::TextInput(s) => Some(s),
            Self::MultiChoice(_) => None,
        }
    }

    pub fn as_text_input_mut(&mut self) -> Option<&mut TextInputState> {
        match self {
            Self::TextInput(s) => Some(s),
            Self::MultiChoice(_) => None,
        }
    }

    pub fn as_multi_choice(&self) -> Option<&MultiChoiceState> {
        match self {
            Self::MultiChoice(s) => Some(s),
            Self::TextInput(_) => None,
        }
    }

    pub fn as_multi_choice_mut(&mut self) -> Option<&mut MultiChoiceState> {
        match self {
            Self::MultiChoice(s) => Some(s),
            Self::TextInput(_) => None,
        }
    }
}

// ── Events ────────────────────────────────────────────────────────────────────

/// Event emitted by an individual field, tagged with its index.
#[derive(Debug, Clone)]
pub enum FieldEvent {
    TextInput(TextInputEvent),
    MultiChoice(MultiChoiceEvent),
}

/// High-level event returned by [`Form::handle_event`].
#[derive(Debug, Clone)]
pub enum FormEvent {
    /// Tab / Shift+Tab moved focus; contains previous and new field indices.
    FocusChanged { from: usize, to: usize },
    /// A widget-level event occurred in the given field.
    FieldEvent { index: usize, event: FieldEvent },
    /// Enter was pressed while a `TextInput` field was active.
    Submit,
    /// The event was not consumed by the form.
    Ignored,
}

// ── Form state ────────────────────────────────────────────────────────────────

/// Mutable runtime state for the entire form.
#[derive(Debug, Clone)]
pub struct FormState {
    /// Index of the currently focused field.
    pub active: usize,
    /// Per-field states; must match the number of `FormField` entries in `Form`.
    pub fields: Vec<FormFieldState>,
}

impl FormState {
    /// Creates a new `FormState` from a list of per-field states.
    ///
    /// If the first field is a `MultiChoice` it is automatically opened.
    pub fn new(fields: Vec<FormFieldState>) -> Self {
        let mut s = Self { active: 0, fields };
        s.open_active_mc();
        s
    }

    /// Returns a reference to the state of field `index`, or `None` if out of range.
    pub fn field(&self, index: usize) -> Option<&FormFieldState> {
        self.fields.get(index)
    }

    /// Returns a mutable reference to the state of field `index`, or `None`.
    pub fn field_mut(&mut self, index: usize) -> Option<&mut FormFieldState> {
        self.fields.get_mut(index)
    }

    /// Moves focus to the next field (wraps around).
    pub fn focus_next(&mut self) {
        self.close_active_mc();
        self.active = (self.active + 1) % self.fields.len().max(1);
        self.open_active_mc();
    }

    /// Moves focus to the previous field (wraps around).
    pub fn focus_prev(&mut self) {
        self.close_active_mc();
        let len = self.fields.len();
        self.active = if self.active == 0 {
            len.saturating_sub(1)
        } else {
            self.active - 1
        };
        self.open_active_mc();
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    fn open_active_mc(&mut self) {
        if let Some(FormFieldState::MultiChoice(mc)) = self.fields.get_mut(self.active) {
            mc.open();
        }
    }

    fn close_active_mc(&mut self) {
        if let Some(FormFieldState::MultiChoice(mc)) = self.fields.get_mut(self.active) {
            mc.close();
        }
    }
}
