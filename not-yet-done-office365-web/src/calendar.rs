//! Calendar domain API — one of the typed "outward interfaces" a session
//! exposes. Obtained via [`SessionHandle::calendar`](crate::SessionHandle).

use std::sync::Arc;

use crate::dto::{MsCalEvent, MsTimeRange};
use crate::error::MsOfficeError;
use crate::session::SessionInner;

/// Read access to the account's calendar over the Office 365 web surface.
pub struct CalendarApi {
    inner: Arc<SessionInner>,
}

impl CalendarApi {
    pub(crate) fn new(inner: Arc<SessionInner>) -> Self {
        Self { inner }
    }

    /// All events overlapping `range`. Requires the session to be logged in;
    /// on an unauthenticated session this fails with
    /// [`MsOfficeError::LoginRequired`] (call
    /// [`SessionHandle::ensure_logged_in`](crate::SessionHandle::ensure_logged_in)
    /// first, or retry after the interactive login completes).
    pub async fn get_view(&self, range: MsTimeRange) -> Result<Vec<MsCalEvent>, MsOfficeError> {
        self.inner.get_calendar_view(range).await
    }
}
