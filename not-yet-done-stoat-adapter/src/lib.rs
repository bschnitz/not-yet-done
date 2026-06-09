//! Stoat (Revolt-fork chat) ContentAdapter.
//!
//! The first **streaming** adapter: chat bootstrap and live updates are
//! push-only over a WebSocket, which breaks the pull/request-response
//! model the other adapters use. Responsibilities are split across:
//!
//! - [`client`] — stateless REST (login, discovery, later message
//!   history) carrying the `X-Session-Token`.
//! - [`gateway`] — the single background task that owns the WebSocket
//!   (`Authenticate → Ready → events`, heartbeat, reconnect) and the
//!   in-memory [`gateway::StoatState`] tree source of truth.
//! - [`adapter`] — the `ContentAdapter` impl tying auth, the gateway,
//!   and a unified status channel together.
//!
//! Persistence is intentionally minimal: only the session token and
//! per-view sort state hit SQLite — chat state stays in memory (see
//! [`db`]).

pub mod adapter;
pub mod auth_session_store;
pub mod client;
pub mod db;
pub mod entity;
pub mod gateway;

pub use adapter::{StoatAdapter, StoatAdapterFactory};
pub use auth_session_store::SqlAuthSessionStore;
pub use client::{MeData, RootInfo, StoatClient, StoatSession, perform_login};
