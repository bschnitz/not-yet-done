//! Layout & behaviour options for a [`super::Form`], separate from its colours
//! ([`super::FormStyle`]).
//!
//! These are the facets a front-end exposes as configuration: how many columns
//! the form is laid out in, which field lands in which column, whether the
//! focused field shows a filled bar, and how a select field is rendered
//! (inline radio list vs. dropdown). All default to the historical
//! single-column / dropdown / no-bar behaviour, so a form built with
//! [`FormOptions::default`] looks exactly as it always has.

/// How a `Select` field is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectStyle {
    /// Every option shown inline as a radio row with a `▍` gutter stripe
    /// (the "event_form" look). The option cursor moves with up/down.
    Inline,
    /// Compact dropdown that expands only while focused (the classic look).
    #[default]
    Dropdown,
}

/// Layout & behaviour for a whole form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormOptions {
    /// Number of side-by-side columns (1 or 2). Values >2 are clamped to 2.
    pub columns: u8,
    /// Per-field column index (`0` or `1`), parallel to the form's field list.
    /// When empty, the driver derives an assignment (balance by height for two
    /// columns; all in column 0 for one).
    pub column_of: Vec<usize>,
    /// Whether the focused field shows a filled background bar. Requires the
    /// palette to carry a `field_bg` colour, else it is a no-op.
    pub field_bar: bool,
    /// How select fields are rendered.
    pub select_style: SelectStyle,
    /// Verb on the panel look's submit bar. `None` → "Save". A form that does
    /// not save anything ("Log in", "Apply") says so here.
    pub submit_label: Option<String>,
}

impl Default for FormOptions {
    fn default() -> Self {
        Self {
            columns: 1,
            column_of: Vec::new(),
            field_bar: false,
            select_style: SelectStyle::Dropdown,
            submit_label: None,
        }
    }
}

impl FormOptions {
    /// Effective column count, clamped to the supported 1..=2 range.
    pub fn column_count(&self) -> u8 {
        self.columns.clamp(1, 2)
    }
}
