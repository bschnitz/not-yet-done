/// Events emitted by a selectable [`super::LeaderList`] in response to input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaderListEvent {
    /// The cursor moved to a new entry (carries the new index).
    CursorChanged(usize),
    /// The visible window scrolled without a cursor (non-selectable scrollable
    /// mode); carries the new top-entry index.
    Scrolled(usize),
    /// An entry was confirmed with the confirm key (carries its index).
    Selected(usize),
    /// The list was cancelled (e.g. `Esc`).
    Cancelled,
}
