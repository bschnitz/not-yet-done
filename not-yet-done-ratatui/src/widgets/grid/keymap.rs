use tuirealm::event::KeyEvent;

/// Keyboard navigation bindings for [`Grid`](super::Grid).
///
/// All fields are `Option<KeyEvent>` — `None` means the action is unbound.
/// By default every field is `None`; configure explicitly:
///
/// ```rust
/// use tuirealm::event::{Key, KeyEvent, KeyModifiers};
/// use not_yet_done_ratatui::widgets::grid::GridKeymap;
///
/// let keymap = GridKeymap {
///     next_cell: Some(KeyEvent { code: Key::Tab,     modifiers: KeyModifiers::NONE }),
///     prev_cell: Some(KeyEvent { code: Key::BackTab, modifiers: KeyModifiers::SHIFT }),
///     ..GridKeymap::default()
/// };
/// ```
#[derive(Debug, Clone, Default)]
pub struct GridKeymap {
    /// Move one cell to the right in the current row (wraps to the first cell).
    pub next_in_row: Option<KeyEvent>,
    /// Move one cell to the left in the current row (wraps to the last cell).
    pub prev_in_row: Option<KeyEvent>,
    /// Move one cell down in the current column (wraps to the first cell).
    pub next_in_col: Option<KeyEvent>,
    /// Move one cell up in the current column (wraps to the last cell).
    pub prev_in_col: Option<KeyEvent>,
    /// Next cell in natural (zig-zag) order, cycling back to the first.
    pub next_cell: Option<KeyEvent>,
    /// Previous cell in natural (zig-zag) order, cycling back to the last.
    pub prev_cell: Option<KeyEvent>,
}
