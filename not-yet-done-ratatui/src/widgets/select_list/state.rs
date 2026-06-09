/// Events emitted by a [`SelectList`] component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectListEvent {
    /// Selection changed — carries the new set of selected indices.
    SelectionChanged(Vec<usize>),
    /// Cursor moved to a new item.
    CursorChanged(usize),
    /// The user confirmed the selection (e.g. Enter).
    Confirmed(Vec<usize>),
    /// The user cancelled (e.g. Esc).
    Cancelled,
}
