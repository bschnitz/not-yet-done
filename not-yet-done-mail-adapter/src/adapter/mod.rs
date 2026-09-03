//! `MailAdapter` — the `ContentAdapter` implementation.
//!
//! One instance, many accounts (see `docs/plan-mail-adapter.md` §4.2): the
//! root lists the configured accounts, each account its folder tree, each
//! folder its messages. Every id below the root names its account first, so
//! a call routes to a connection by reading its id (see [`crate::ids`]).

/// Id of the root node — the instance itself, above any account.
pub(crate) const ROOT_ID: &str = "root";
