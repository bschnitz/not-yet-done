//! The `calendar` content adapter.
//!
//! One adapter, many connections: it aggregates events from every configured
//! calendar connection into a single flat, time-sorted, groupable event list.
//! Connections may mix protocols and include several instances of the same
//! protocol — each is a [`CalendarBackend`](not_yet_done_calendar_core::CalendarBackend)
//! from a feature-gated backend crate (see [`registry`]). Read-only for now.
//!
//! Live update is decoupled from the frontend: a background poll task detects
//! externally-made changes and pushes
//! [`Invalidation::All`](not_yet_done_content::Invalidation::All) over the
//! content layer's broadcast channel, which the generic frontend watcher
//! already knows how to act on.

mod adapter;
pub mod config;
mod factory;
mod query;
mod registry;

pub use factory::CalendarAdapterFactory;
