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
