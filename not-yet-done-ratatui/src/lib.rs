pub mod utils;
pub mod widgets;

pub use utils::open_editor::{open_editor, EditorError};

// --- shared primitives ---
pub use widgets::common::hex_color;

// --- text_input ---
pub use widgets::text_input::{
    TextInput, TextInputEvent, TextInputKeymap,
    TextInputStyle, TextInputStyleType, ATTR_ERROR,
};

// --- multi_choice (pending tui-realm migration) ---
pub use widgets::multi_choice::{
    MultiChoice, MultiChoiceEvent, MultiChoiceKeymap, MultiChoiceState, MultiChoiceStyle,
    MultiChoiceStyleType,
};

// --- form (pending tui-realm migration) ---
pub use widgets::form::{
    FieldEvent, Form, FormEvent, FormField, FormFieldState, FormFieldWidget, FormKeymap,
    FormState, FormStyle, FormWidgetStyle,
};

// --- two_column (pending tui-realm migration) ---
pub use widgets::two_column::{
    BorderStyleType, ColumnWidth, HalfBorders, HalfPadding, TwoColumnLayout, TwoColumnStyle,
};
