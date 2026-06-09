/// Events emitted by a [`MultiChoice`] component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiChoiceEvent {
    /// The selection changed; the new selected indices are provided.
    SelectionChanged(Vec<usize>),
    /// The highlighted item changed; the new cursor index is provided.
    HighlightChanged(usize),
    /// The item order changed (only when ordering is enabled).
    /// Contains the new order as a vec of original indices.
    OrderChanged(Vec<usize>),
    /// The dropdown was closed by the user (e.g. via the close key binding).
    Closed,
}
