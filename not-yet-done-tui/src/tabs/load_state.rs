//! Async load state shared by data-backed tabs (e.g. Trackings).
//!
//! Tracks where a tab's background data fetch stands so the view can
//! render a spinner / error / empty-vs-loaded distinction without the
//! App layer having to thread that status through every call.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadState {
    Idle,
    Loading,
    Loaded,
    Error(String),
}
