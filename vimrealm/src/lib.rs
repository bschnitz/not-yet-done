//! `vimrealm` — a modal, vim-like multi-line text editor widget for
//! [`ratatui`], ready to mount as a [`tuirealm`] component.
//!
//! The crate is layered so each piece stays testable on its own:
//!
//! - [`buffer`] — text storage, cursor arithmetic and snapshot undo. The only
//!   module that knows how the text is stored.
//! - [`mode`] — Normal / Insert / Command.
//! - [`motion`] — vim motions and their exclusive/inclusive/linewise kind.
//! - [`register`] — the unnamed and named registers, and vim's naming rules.
//! - [`operator`] — `d`/`c`/`y` over a motion.
//! - [`keymap`] — key → command tables, overridable per binding.
//! - [`editor`] — [`VimEditor`]: modes, pending input, the key state machine.
//! - [`state`] — what the editor reports back to its host.
//! - [`textobject`] — `iw`, `a"`, `i(` and friends.
//! - [`search`] — the substring search behind `/`, `?`, `n` and `N`.
//! - [`render`] — soft wrap, viewport, gutter, status line.
//! - [`component`] — the tuirealm `Component` / `AppComponent` impls.

pub mod buffer;
pub mod component;
pub mod editor;
pub mod keymap;
pub mod mode;
pub mod motion;
pub mod operator;
pub mod register;
pub mod render;
pub mod search;
pub mod state;
pub mod style;
pub mod textobject;

pub use buffer::{Buffer, Position};
pub use editor::VimEditor;
pub use keymap::{InsertAt, InsertCommand, Keymap, NormalCommand};
pub use mode::Mode;
pub use motion::{Motion, MotionKind};
pub use operator::Operator;
pub use register::{Register, RegisterSink, Registers};
pub use state::VimEvent;
pub use style::{VimStyle, VimStyleType};
pub use textobject::TextObject;
