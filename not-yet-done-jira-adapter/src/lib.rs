//! Jira ContentAdapter — REST client + cache + adapter implementation.
//!
//! Self-contained crate: owns its own SQLite/Postgres connection (configured
//! per view via the YAML adapter config) and depends only on the
//! `not-yet-done-content` trait crate.

pub mod adapter;
pub mod auth_session_store;
pub mod cache_store;
pub mod client;
pub mod db;
pub mod entity;

pub use adapter::{JiraAdapter, JiraAdapterFactory};
pub use auth_session_store::SqlAuthSessionStore;
pub use client::{
    JiraAttachment, JiraClient, JiraComment, JiraIssueDetail, JiraSession, JiraTicket,
    JiraTransition, JiraUser,
};
