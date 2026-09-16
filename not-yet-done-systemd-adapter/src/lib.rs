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
//! * [`deps`] — the dependency graph as two levels: what a unit needs, as a
//!   tree, and what it is ordered against, as one flat hop.
//! * [`security`] — what `systemd-analyze security` says about a unit, as a
//!   level, plus the curated table of directives that answer one row of it.
//!   The one place in this crate that keeps knowledge rather than reading it.
//! * [`journal`] — a unit's log lines as rows, read from `journalctl
//!   --output=json`, and the one key that opens the pager instead.
//! * [`live`] — the manager's own signals, coalesced into the row and level
//!   invalidations the frontend acts on. What makes a level watchable rather
//!   than a snapshot.
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
pub mod chain;
pub mod config;
pub mod control;
pub mod create;
pub mod deps;
pub mod edit;
pub mod factory;
pub mod failed;
pub mod journal;
pub mod live;
pub mod model;
pub mod preset;
pub mod protect;
pub mod query;
pub mod security;
pub mod shadow;

pub use adapter::SystemdAdapter;
pub use config::{Manager, SystemdConfig};
pub use factory::SystemdAdapterFactory;

/// Node-id prefix for a loaded `.service` unit (`service:<name>`).
pub const SERVICE_PREFIX: &str = "service:";
/// Node-id prefix for a loaded `.timer` unit (`timer:<name>`).
pub const TIMER_PREFIX: &str = "timer:";
/// Node-id prefix for a unit file on disk (`unitfile:<name>`).
pub const UNIT_FILE_PREFIX: &str = "unitfile:";
/// Node-id prefix for one node of a unit's dependency tree
/// (`dep:<unit>><unit>>…`).
///
/// The id is the whole path the row was reached by, not just its unit — the
/// same unit occupies many positions in one tree, and the frontend addresses a
/// row by its id. See [`deps`] for the measurement behind that.
pub const DEP_PREFIX: &str = "dep:";
/// Node-id prefix for one unit another unit is ordered against
/// (`order:<unit>><unit>`). Two elements, always: the ordering level does not
/// recurse.
pub const ORDER_PREFIX: &str = "order:";
/// Node-id prefix for one property of one unit (`property:<unit>:<Name>`).
///
/// Two segments because a property is only meaningful with the unit it belongs
/// to, and unit names contain dots but never colons — so the first colon after
/// the prefix splits the pair unambiguously.
pub const PROPERTY_PREFIX: &str = "property:";
/// Node-id prefix for one journal entry of one unit (`log:<unit>:<cursor>`).
///
/// Two segments for the same reason as [`PROPERTY_PREFIX`], and split the same
/// way — from the right. A journal cursor never contains a colon; a unit name
/// can (`dbus-:1.19-org.a11y.atspi.Registry@0.service`), so the *last* colon is
/// the one that separates the pair.
pub const LOG_PREFIX: &str = "log:";
/// Node-id prefix for one security check of one unit
/// (`security:<unit>:<json_field>`).
///
/// Two segments, split from the right like [`LOG_PREFIX`]: systemd's
/// `json_field` never holds a colon, a unit name may.
pub const SECURITY_PREFIX: &str = "security:";
/// Node-id prefix for one unit on the critical chain of another
/// (`chain:<root>><unit>`).
///
/// The unit the chain was opened on travels with the row because the same unit
/// sits on many chains, and a row is addressed by its id. Split on `>` like a
/// dependency id: a unit name may hold a colon, never a `>`.
pub const CHAIN_PREFIX: &str = "chain:";

/// Node-id prefix for a unit on the Failed level (`failed:<unit>`).
///
/// Its own prefix rather than `service:` because the level's population is
/// every kind of unit, and a mount addressed as a service would be looked up
/// on a level it has no row on. The suffix is the whole unit name, so the
/// control verbs — which act on a name — need nothing of their own here.
pub const FAILED_PREFIX: &str = "failed:";
