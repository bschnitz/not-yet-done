//! SeaORM entities of the calendar adapter's persistent event store (see
//! [`crate::store`]). Schema-synced on connect, scoped to this module, so the
//! store may share a database with other tables.

pub mod calendar_event;
pub mod calendar_sync;
