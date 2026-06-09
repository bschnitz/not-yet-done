/// Events emitted by a [`Table`] component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableEvent {
    /// The cursor moved to a new row index.
    CursorChanged(usize),
    /// The user confirmed (Enter) on a row.
    Confirmed(usize),
    /// The user cancelled (Esc).
    Cancelled,
}
