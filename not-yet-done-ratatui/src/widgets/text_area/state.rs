/// Events emitted by a [`TextArea`](super::TextArea) component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextAreaEvent {
    /// The text changed; the whole newline-joined value is included.
    Changed(String),
    /// The cursor moved without changing the text: `(row, byte offset)`.
    CursorMoved(usize, usize),
}
