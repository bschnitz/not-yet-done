//! SQLite `ContentAdapter`.
//!
//! Browses local SQLite database files as a tree:
//!   root → database (one per file) → "Tables" → table → rows.
//!
//! There is no server here, so almost nothing the Postgres adapter needs
//! for connectivity applies: no transport, no credentials, no catalogue
//! server to ask which databases exist. What takes their place is
//! [`config::SqliteConfig::sources`] — an arbitrarily long list of glob
//! patterns whose matches become the root children (see [`sources`]).
//!
//! What *is* shared with Postgres lives in `not-yet-done-sql-core`:
//! identifier quoting, the SQL text sniffers, the on-disk script layout
//! and the whole `ScriptStore` implementation. Nothing in this crate
//! duplicates it.

pub mod adapter;
pub mod client;
pub mod config;
pub mod script_store;
pub mod sources;

pub use adapter::{SqliteAdapter, SqliteAdapterFactory};
