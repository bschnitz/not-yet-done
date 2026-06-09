//! Confluence ContentAdapter (Server / Data-Center).
//!
//! CF-2a slice: DB + entities + auth-session store + scope-id helper +
//! `ConfluenceClient` stub with one `current_user()` health probe.
//! Auth-bridge wiring (CF-2b) and live-API node implementations
//! (CF-3..CF-15) come in subsequent phases. Plan:
//! `docs/plan-confluence-adapter.md`.

pub mod adapter;
pub mod auth_session_store;
pub mod cache_store;
pub mod client;
pub mod config;
pub mod db;
pub mod entity;

pub use adapter::{ConfluenceAdapter, ConfluenceAdapterFactory};
pub use auth_session_store::SqlAuthSessionStore;
pub use client::{ConfluenceClient, ConfluenceSession, ConfluenceUser};
