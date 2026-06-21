//! Task/tracking/project/tag domain extracted out of `not-yet-done-core`
//! (Phase C2 of the DB-split). This crate owns the *domain*: entities,
//! repositories, services, the domain-event bus, and the SeaORM-bound
//! filter binding (`FilterBuilder` + tree-operator resolution). The
//! host-agnostic filter *language* lives one layer down in
//! `not-yet-done-filter`.
//!
//! `not-yet-done-core` keeps the app-shell concerns (link / saved_query /
//! settings / query_shortcut / backup) and, during the C2 transition,
//! re-exports everything below under the historic `not_yet_done_core::…`
//! paths so consumers don't churn. C3 removes that bridge and re-points
//! consumers here directly.

pub mod backup;
pub mod bootstrap;
pub mod entity;
pub mod error;
pub mod events;
pub mod filter;
pub mod local_context;
pub mod module;
pub mod repository;
pub mod service;
pub mod task_path;
