//! Events the editor reports to its host.

/// What a key press meant to the world outside the widget.
///
/// The crate knows nothing about files: `:w` does not write anything, it emits
/// [`VimEvent::Save`] and lets the host decide what "saving" is — a file, a
/// chat message, a ticket body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimEvent {
    /// The buffer text changed.
    Changed,
    /// `:w` — persist the current text, keep editing.
    Save,
    /// `:wq` / `:x` — persist and close.
    SaveAndClose,
    /// `:q` on a clean buffer, or `:q!` — close, discarding changes.
    Cancel,
}
