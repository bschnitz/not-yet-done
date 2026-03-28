pub mod widgets;
pub mod utils;

pub use utils::open_editor::{open_editor, EditorError};

// --- gemeinsame Primitives ---
pub use widgets::common::{hex_color, KeyBinding};

// --- text_input ---
pub use widgets::text_input::{
    TextInput, TextInputEvent, TextInputKeymap, TextInputState, TextInputStyle, TextInputStyleType,
};

// --- multi_choice ---
pub use widgets::multi_choice::{
    MultiChoice, MultiChoiceEvent, MultiChoiceKeymap, MultiChoiceState, MultiChoiceStyle,
    MultiChoiceStyleType,
};
