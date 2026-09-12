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

pub mod bus;
pub mod config;
pub mod model;
pub mod query;

pub use config::{Manager, SystemdConfig};

/// Node-id prefix for a loaded `.service` unit (`service:<name>`).
pub const SERVICE_PREFIX: &str = "service:";
/// Node-id prefix for a loaded `.timer` unit (`timer:<name>`).
pub const TIMER_PREFIX: &str = "timer:";
/// Node-id prefix for a unit file on disk (`unitfile:<name>`).
pub const UNIT_FILE_PREFIX: &str = "unitfile:";
