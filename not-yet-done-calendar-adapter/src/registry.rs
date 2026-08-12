//! Compile-time backend registry.
//!
//! Every calendar protocol lives in its own crate pulled in as an optional,
//! feature-gated dependency. This function offers exactly the backends the
//! current build enabled — a build without `microsoft` simply doesn't know
//! that backend type, and the adapter rejects a connection asking for it.
//!
//! Adding a backend is: new crate depending only on `calendar-core`, an
//! optional dep + feature here, and one `#[cfg]`-gated push below. Nothing
//! else in the adapter changes.

use not_yet_done_calendar_core::CalendarBackendFactory;

/// All backend factories compiled into this build.
pub(crate) fn backend_factories() -> Vec<Box<dyn CalendarBackendFactory>> {
    let mut factories: Vec<Box<dyn CalendarBackendFactory>> = Vec::new();

    #[cfg(feature = "microsoft")]
    factories.push(Box::new(
        not_yet_done_calendar_msgraph::MsGraphBackendFactory::new(),
    ));

    #[cfg(feature = "office365-web")]
    factories.push(Box::new(
        not_yet_done_calendar_office365_web::Office365WebBackendFactory::new(),
    ));

    #[cfg(feature = "caldav")]
    factories.push(Box::new(
        not_yet_done_calendar_caldav::CalDavBackendFactory::new(),
    ));

    factories
}
