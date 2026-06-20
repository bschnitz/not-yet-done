//! PostgreSQL `ContentAdapter`.
//!
//! Lists the live database catalogue of a target Postgres server. Read
//! only for now — actions on individual databases (list tables, run
//! queries) are deferred to a future phase.
//!
//! Connectivity goes through `not-yet-done-transport` so the adapter
//! can be configured for either a direct `host:port` connection or an
//! in-process SSH tunnel through a bastion host. The adapter does not
//! see the difference: `transport::connect()` always returns a
//! `host:port` it can hand to `tokio_postgres`.

pub mod adapter;
pub mod client;
pub mod config;
pub mod query;
pub mod script_completions;
pub mod script_store;

pub use adapter::{PostgresAdapter, PostgresAdapterFactory};
pub use script_store::PostgresScriptStore;
