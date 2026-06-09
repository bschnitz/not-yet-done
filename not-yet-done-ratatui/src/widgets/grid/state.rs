/// Events emitted by [`Grid`](super::Grid) to the tuirealm application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GridEvent {
    /// The focused cell changed. `row` and `col` are the new focus position.
    FocusChanged { row: usize, col: usize },
}
