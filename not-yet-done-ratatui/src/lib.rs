pub mod widgets;

// --- gemeinsame Primitives ---
pub use widgets::common::{hex_color, KeyBinding, LineStyle};

// --- text_input ---
pub use widgets::text_input::{
    TextInput,
    TextInputEvent,
    TextInputKeymap,
    TextInputState,
    TextInputStyle,
};

// --- multi_choice ---
pub use widgets::multi_choice::{
    MultiChoice,
    MultiChoiceEvent,
    MultiChoiceKeymap,
    MultiChoiceState,
    MultiChoiceStyle,
};
