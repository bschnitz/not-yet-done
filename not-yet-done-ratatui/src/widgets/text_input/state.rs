/// Events emitted by a [`TextInput`] component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextInputEvent {
    /// The input value changed; the new value is included.
    Changed(String),
    /// The user confirmed the input (e.g., pressed Enter); the current value is included.
    Submitted(String),
}
