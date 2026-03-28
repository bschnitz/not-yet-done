use crate::widgets::multi_choice::MultiChoice;
use crate::widgets::text_input::TextInput;

// ── Widget variant ────────────────────────────────────────────────────────────

/// Wrapper enum around the concrete widget types supported by the form.
#[derive(Debug, Clone)]
pub enum FormFieldWidget<'a> {
    TextInput(TextInput<'a>),
    MultiChoice(MultiChoice<'a>),
}

// ── Form field descriptor ─────────────────────────────────────────────────────

/// Configuration for a single field in a `Form`.
///
/// `height` is the number of terminal rows the field occupies in the layout.
/// When a `MultiChoice` is expanded (open) it will render *below* this height,
/// overlapping adjacent fields — that overflow is intentional.
#[derive(Debug, Clone)]
pub struct FormField<'a> {
    pub widget: FormFieldWidget<'a>,
    /// Layout height in terminal rows (closed / normal state).
    pub height: u16,
}

impl<'a> FormField<'a> {
    /// Creates a `TextInput` field.
    ///
    /// Default layout height is **3** rows (title + input + error).
    pub fn text_input(widget: TextInput<'a>) -> Self {
        Self {
            widget: FormFieldWidget::TextInput(widget),
            height: 3,
        }
    }

    /// Creates a `MultiChoice` field.
    ///
    /// Default layout height is **2** rows (title + summary when collapsed).
    /// When expanded the widget overlaps rows below; the layout slot stays fixed.
    pub fn multi_choice(widget: MultiChoice<'a>) -> Self {
        Self {
            widget: FormFieldWidget::MultiChoice(widget),
            height: 2,
        }
    }

    /// Overrides the layout height for this field.
    pub fn with_height(mut self, h: u16) -> Self {
        self.height = h;
        self
    }
}
