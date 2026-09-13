//! The **systemd** content adapter — a systemd manager browsed as a content
//! tree: services, timers and unit files as tables, over D-Bus.
//!
//! See `docs/plan-systemd-adapter.md` for the phase plan and the decisions
//! behind the shape. This crate is phase 0: the read-only levels. Control,
//! editing, creating, the journal and live signals build on exactly these
//! types.
//!
//! # Layers
//!
//! * [`bus`] — the D-Bus transport. One lazily-opened connection per instance
//!   and the handful of calls the levels are built from. Knows nothing about
//!   nodes or columns.
//! * [`model`] — the rows and their column schemas, one type per unit kind.
//!   Every cell leaves canonical (bytes as bytes, instants as RFC 3339); the
//!   table engine formats.
//! * [`query`] — the `FilterExpr` query each level is filtered by. What a level
//!   shows is a query the user switches at runtime, never configuration.
//! * [`config`] — `manager:` and `timeout_secs:`, which is all that is left
//!   once filtering is a query.
//!
//! * [`control`] — the verbs (start, stop, enable, mask, …): one table that is
//!   at once the action list, the confirmation policy and the dispatcher.
//! * [`edit`] — writing a unit's configuration: which file a change lands in,
//!   the staging check that runs before it does, and the one question a write
//!   leaves behind.
//! * [`create`] — making a unit that does not exist yet: the timer/service
//!   pair from one form, and the empty file in the editor.
//! * [`calendar`] — what a person types, turned into an `OnCalendar=` systemd
//!   agrees with.
//! * [`protect`] — the units a disruptive verb refuses to touch, because a
//!   confirmation prompt is no guard against the muscle memory that pressed
//!   the key.
//!
//! On top of those sit [`adapter`] — the node protocol itself, three levels
//! under one root — and [`factory`], which lifts it into the host registry.

pub mod adapter;
pub mod bus;
pub mod calendar;
pub mod config;
pub mod control;
pub mod create;
pub mod edit;
pub mod factory;
pub mod model;
pub mod protect;
pub mod query;

pub use adapter::SystemdAdapter;
pub use config::{Manager, SystemdConfig};
pub use factory::SystemdAdapterFactory;

/// Node-id prefix for a loaded `.service` unit (`service:<name>`).
pub const SERVICE_PREFIX: &str = "service:";
/// Node-id prefix for a loaded `.timer` unit (`timer:<name>`).
pub const TIMER_PREFIX: &str = "timer:";
/// Node-id prefix for a unit file on disk (`unitfile:<name>`).
pub const UNIT_FILE_PREFIX: &str = "unitfile:";
/// Node-id prefix for one property of one unit (`property:<unit>:<Name>`).
///
/// Two segments because a property is only meaningful with the unit it belongs
/// to, and unit names contain dots but never colons — so the first colon after
/// the prefix splits the pair unambiguously.
pub const PROPERTY_PREFIX: &str = "property:";
