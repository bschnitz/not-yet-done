//! Taiga ContentAdapter — REST client + persistent cache + adapter
//! implementation.
//!
//! Self-contained: each instance owns one Taiga connection (auth + JWT)
//! and a per-URL SQLite cache for the JWT and per-project metadata
//! (statuses / members / tags). Implements the generic `ContentAdapter`
//! interface from `not-yet-done-content`.

pub mod adapter;
pub mod auth_session_store;
pub mod cache_store;
pub mod client;
pub mod db;
pub mod entity;

pub use adapter::{TaigaAdapter, TaigaAdapterFactory};
pub use auth_session_store::SqlAuthSessionStore;
pub use client::{
    ItemType, QuerySpec, TaigaAttachment, TaigaClient, TaigaComment, TaigaSession,
    download_attachment, edit_comment, fetch_comments, list_attachments, parse_query_yaml,
    perform_login, run_queries, toggle_watch,
};
