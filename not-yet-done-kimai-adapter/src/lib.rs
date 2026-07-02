//! Kimai time-tracking adapter.
//!
//! Read-only `ContentAdapter` over the Kimai REST API (`/api/timesheets`
//! plus the `projects`/`activities` lookup endpoints). The primary view is
//! a flat timesheet list with project / activity / duration columns; the
//! frontend groups it engine-side (day/week/month) exactly like the local
//! trackings view.

mod adapter;
mod client;

pub use adapter::KimaiAdapterFactory;
