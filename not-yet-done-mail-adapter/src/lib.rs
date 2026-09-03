//! Mail (IMAP) ContentAdapter.
//!
//! The first adapter whose one instance holds **many** connections with
//! per-connection credentials: mail is a single workflow spread over several
//! mailboxes, so a tab per account would be the wrong shape. The calendar
//! adapter aggregates several sources the same way; here each account keeps
//! its own subtree (and, in the shipped view, its own subtab) instead of
//! being merged into one list.
//!
//! - [`config`] — the YAML shape: instance-wide defaults plus an `accounts:`
//!   list, each account with host, transport security and its own `auth:`.
//! - [`auth`] — the mechanisms this adapter can speak (`password`, `xoauth2`),
//!   published to `nyd config auth mail` and validated per account.
//! - [`credentials`] — resolving an account's `auth:` block, and the lane
//!   that keeps two accounts from asking the user at the same time.
//! - [`error`] — what went wrong, and whether the session survived it.
//! - [`ids`] — how a node id names its account, folder, message and part.
//! - [`model`] — the protocol-free shapes the IMAP layer hands upwards.
//! - [`imap`] — connecting, logging in, and the per-account connection actor.
//! - [`adapter`] — the `ContentAdapter`/`Node` impls over all of it.

pub mod adapter;
pub mod auth;
pub mod config;
pub(crate) mod credentials;
pub(crate) mod error;
pub(crate) mod ids;
pub(crate) mod imap;
pub(crate) mod model;

pub use config::MailConfig;
