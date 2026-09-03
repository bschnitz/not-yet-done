//! The IMAP layer: one connection actor per account.
//!
//! IMAP is stateful and strictly one command at a time — a session has a
//! *selected* mailbox and no way to interleave two requests. The layer here
//! therefore owns the session inside a task and serialises access through a
//! channel, rather than handing a lock around.

pub(crate) mod conn;
pub(crate) mod login;
pub(crate) mod ops;
pub(crate) mod stream;
#[cfg(test)]
pub(crate) mod testserver;
