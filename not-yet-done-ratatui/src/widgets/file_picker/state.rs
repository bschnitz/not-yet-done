use std::path::PathBuf;

/// Events emitted by a [`FilePicker`](super::FilePicker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilePickerEvent {
    /// Internal focus moved between Directory / Glob / Files / Selected.
    FocusChanged,
    /// User confirmed (Ctrl+Enter); carries the absolute paths of all
    /// items currently in the Selected list.
    Confirmed(Vec<PathBuf>),
    /// User cancelled (Esc).
    Cancelled,
}
