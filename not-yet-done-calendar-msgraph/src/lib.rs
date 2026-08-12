//! Microsoft Graph calendar backend.
//!
//! Reads events from a Microsoft 365 mailbox (the same calendar the Teams and
//! Outlook clients show) through the Graph REST API. Auth is a bearer token
//! resolved on demand from the config's `token` credential provider — with
//! `type: command` and `az account get-access-token --resource
//! https://graph.microsoft.com` this needs nothing more than a logged-in Azure
//! CLI. A full in-process OAuth2 device-code flow can be added later as a
//! second `token` mechanism without touching this seam.
//!
//! Read + write: it lists events (the mailbox's **default** calendar via
//! `/me/calendarView`), enumerates the mailbox's calendars (`/me/calendars`),
//! and creates events (`POST /me/[calendars/{id}/]events`). Writing needs the
//! delegated `Calendars.ReadWrite` scope; without it Graph answers 403, which
//! the client surfaces as an actionable scope hint. Fanning the *listing* out
//! to every calendar of the account is still a later refinement.

mod client;
mod config;

use async_trait::async_trait;
use not_yet_done_calendar_core::{
    CalEvent, CalendarBackend, CalendarBackendFactory, CalendarError, CalendarRef, EventDraft,
    TimeRange,
};

use client::MsGraphClient;
use config::MsGraphConfig;

/// A single Microsoft Graph connection (one mailbox / account).
pub struct MsGraphBackend {
    connection_id: String,
    label: String,
    client: MsGraphClient,
}

#[async_trait]
impl CalendarBackend for MsGraphBackend {
    fn connection_id(&self) -> &str {
        &self.connection_id
    }

    fn connection_label(&self) -> &str {
        &self.label
    }

    async fn list_events(&self, range: &TimeRange) -> Result<Vec<CalEvent>, CalendarError> {
        self.client.calendar_view(range, &self.label).await
    }

    async fn list_calendars(&self) -> Result<Vec<CalendarRef>, CalendarError> {
        self.client.list_calendars().await
    }

    async fn create_event(
        &self,
        calendar_id: Option<&str>,
        draft: &EventDraft,
    ) -> Result<CalEvent, CalendarError> {
        self.client
            .create_event(calendar_id, draft, &self.label)
            .await
    }
}

/// Registers as backend type `microsoft`.
#[derive(Default)]
pub struct MsGraphBackendFactory;

impl MsGraphBackendFactory {
    pub fn new() -> Self {
        Self
    }
}

impl CalendarBackendFactory for MsGraphBackendFactory {
    fn backend_type(&self) -> &str {
        "microsoft"
    }

    fn create(
        &self,
        connection_id: &str,
        config: &str,
        _ctx: &not_yet_done_content::HostContext,
    ) -> Result<Box<dyn CalendarBackend>, CalendarError> {
        let cfg: MsGraphConfig = serde_yaml::from_str(config)
            .map_err(|e| CalendarError::Config(format!("invalid microsoft backend config: {e}")))?;

        let label = cfg
            .name
            .clone()
            .unwrap_or_else(|| connection_id.to_string());
        let client = MsGraphClient::from_config(cfg)?;

        Ok(Box::new(MsGraphBackend {
            connection_id: connection_id.to_string(),
            label,
            client,
        }))
    }
}
