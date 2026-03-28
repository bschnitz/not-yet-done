use crate::widgets::multi_choice::style::{MultiChoiceStyle, MultiChoiceStyleType};
use crate::widgets::text_input::style::{TextInputStyle, TextInputStyleType};
use ratatui::style::{Color, Style};

/// Style properties the form applies to a single widget slot (active or inactive).
///
/// Every field is `Option`: `None` means "inherit from the form default"; a `Some`
/// value on the *widget's own style* always wins over anything set here.
///
/// Mapping to widget style types:
///
/// | `FormWidgetStyle` field  | `TextInputStyleType` | `MultiChoiceStyleType`    |
/// |--------------------------|----------------------|---------------------------|
/// | `title`                  | `Title`              | `Title`                   |
/// | `body`                   | `Input`              | `Normal`                  |
/// | `error`                  | `Error`              | —                         |
/// | `mc_cursor`              | —                    | `Active`                  |
/// | `mc_selected`            | —                    | `Selected`                |
/// | `mc_selected_cursor`     | —                    | `SelectedActive`          |
/// | `mc_closing_line`        | —                    | `LastLine`                |
#[derive(Debug, Clone, Default)]
pub struct FormWidgetStyle {
    /// Colour of the `▍` prefix bar.
    pub prefix_color: Option<Color>,
    /// Title row style.
    pub title: Option<Style>,
    /// Input / body row style (TextInput input line; MultiChoice Normal row).
    pub body: Option<Style>,
    /// Error row style (TextInput only).
    pub error: Option<Style>,
    /// Placeholder text colour.
    pub placeholder: Option<Color>,
    /// MultiChoice row where the cursor rests but the item is not selected.
    pub mc_cursor: Option<Style>,
    /// MultiChoice row that is selected but the cursor is elsewhere.
    pub mc_selected: Option<Style>,
    /// MultiChoice row that is both selected and under the cursor.
    pub mc_selected_cursor: Option<Style>,
    /// Blank closing line rendered below the expanded MultiChoice list.
    pub mc_closing_line: Option<Style>,
}

impl FormWidgetStyle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn prefix_color(mut self, c: Color) -> Self {
        self.prefix_color = Some(c);
        self
    }

    pub fn title(mut self, s: Style) -> Self {
        self.title = Some(s);
        self
    }

    pub fn body(mut self, s: Style) -> Self {
        self.body = Some(s);
        self
    }

    pub fn error(mut self, s: Style) -> Self {
        self.error = Some(s);
        self
    }

    pub fn placeholder(mut self, c: Color) -> Self {
        self.placeholder = Some(c);
        self
    }

    pub fn mc_cursor(mut self, s: Style) -> Self {
        self.mc_cursor = Some(s);
        self
    }

    pub fn mc_selected(mut self, s: Style) -> Self {
        self.mc_selected = Some(s);
        self
    }

    pub fn mc_selected_cursor(mut self, s: Style) -> Self {
        self.mc_selected_cursor = Some(s);
        self
    }

    pub fn mc_closing_line(mut self, s: Style) -> Self {
        self.mc_closing_line = Some(s);
        self
    }
}

/// Top-level style configuration for the `Form` widget.
#[derive(Debug, Clone, Default)]
pub struct FormStyle {
    /// Background colour painted over the whole form area before widgets render.
    pub background: Option<Color>,
    /// Style applied to the *active* (focused) field — fills in any unset widget slots.
    pub active: FormWidgetStyle,
    /// Style applied to *inactive* (unfocused) fields — fills in any unset widget slots.
    pub inactive: FormWidgetStyle,
}

impl FormStyle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn background(mut self, c: Color) -> Self {
        self.background = Some(c);
        self
    }

    pub fn active(mut self, s: FormWidgetStyle) -> Self {
        self.active = s;
        self
    }

    pub fn inactive(mut self, s: FormWidgetStyle) -> Self {
        self.inactive = s;
        self
    }
}

// ── Merge helpers ─────────────────────────────────────────────────────────────
//
// Each helper fills unset (`None`) slots in the widget style from the form slot.
// Widget-level configuration always wins — we only write to `None` positions.

/// Merges `form_slot` into a cloned `TextInputStyle`.
pub(crate) fn merge_text_input(
    mut widget: TextInputStyle,
    form_slot: &FormWidgetStyle,
) -> TextInputStyle {
    if widget.prefix_color.is_none() {
        widget.prefix_color = form_slot.prefix_color;
    }
    if widget.style(TextInputStyleType::Title).is_none() {
        if let Some(s) = form_slot.title {
            widget = widget.set_style(TextInputStyleType::Title, s);
        }
    }
    if widget.style(TextInputStyleType::Input).is_none() {
        if let Some(s) = form_slot.body {
            widget = widget.set_style(TextInputStyleType::Input, s);
        }
    }
    if widget.style(TextInputStyleType::Error).is_none() {
        if let Some(s) = form_slot.error {
            widget = widget.set_style(TextInputStyleType::Error, s);
        }
    }
    if widget.placeholder_color.is_none() {
        widget.placeholder_color = form_slot.placeholder;
    }
    widget
}

/// Merges `form_slot` into a cloned `MultiChoiceStyle`.
pub(crate) fn merge_multi_choice(
    mut widget: MultiChoiceStyle,
    form_slot: &FormWidgetStyle,
) -> MultiChoiceStyle {
    if widget.prefix_color.is_none() {
        widget.prefix_color = form_slot.prefix_color;
    }
    macro_rules! merge_slot {
        ($mc_type:expr, $form_field:ident) => {
            if widget.style($mc_type).is_none() {
                if let Some(s) = form_slot.$form_field {
                    widget = widget.set_style($mc_type, s);
                }
            }
        };
    }
    merge_slot!(MultiChoiceStyleType::Title, title);
    merge_slot!(MultiChoiceStyleType::Normal, body);
    merge_slot!(MultiChoiceStyleType::Active, mc_cursor);
    merge_slot!(MultiChoiceStyleType::Selected, mc_selected);
    merge_slot!(MultiChoiceStyleType::SelectedActive, mc_selected_cursor);
    merge_slot!(MultiChoiceStyleType::LastLine, mc_closing_line);
    widget
}
