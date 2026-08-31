//! CalDAV calendar backend.
//!
//! Reads events from any CalDAV server (RFC 4791) — mail.de, Nextcloud, Baïkal,
//! Fastmail, iCloud, Radicale, … — over HTTP Basic auth. From the configured
//! entry URL it discovers the account's calendar collections and issues a
//! `calendar-query` `REPORT` per window, asking the server to **expand**
//! recurring events into concrete instances so this crate never has to
//! interpret `RRULE`/`VTIMEZONE` (see [`client`]).
//!
//! Read + write: it lists events, enumerates its calendars, and creates events
//! by `PUT`ting a fresh single-`VEVENT` resource into the chosen collection
//! (see [`client::CalDavClient::create_event`]). Depends only on
//! `calendar-core` (the backend seam) and `content` (the credential resolver) —
//! never on the adapter crate or the TUI, so it stays an independent,
//! feature-gated unit like the Microsoft Graph backend.

mod client;
mod config;
mod ical;

use std::sync::Mutex;

use async_trait::async_trait;
use not_yet_done_calendar_core::{
    CalEvent, CalendarBackend, CalendarBackendFactory, CalendarError, CalendarRef, EventDraft,
    TimeRange,
};
use not_yet_done_content::PromptRequest;
use not_yet_done_content::auth::CredentialPrompts;
use tokio::sync::mpsc;

use client::CalDavClient;
use config::CalDavConfig;

/// A single CalDAV connection (one account, possibly several calendars).
pub struct CalDavBackend {
    connection_id: String,
    label: String,
    client: CalDavClient,
    /// The end of the credential prompt stream the adapter takes once,
    /// present only when a provider of this connection can actually ask
    /// something (see [`CredentialProvider::can_prompt`]). Taken out on
    /// the first call, hence the `Option`.
    prompts: Mutex<Option<mpsc::Receiver<PromptRequest>>>,
}

#[async_trait]
impl CalendarBackend for CalDavBackend {
    fn connection_id(&self) -> &str {
        &self.connection_id
    }

    fn connection_label(&self) -> &str {
        &self.label
    }

    async fn list_events(&self, range: &TimeRange) -> Result<Vec<CalEvent>, CalendarError> {
        self.client.list_events(range, &self.label).await
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

    /// The stream a `script` credential provider asks through — the
    /// password store's passphrase, mid-`list_events`. `None` for a
    /// connection whose providers never ask (a literal, a plain
    /// `command`), so the adapter can tell "nothing to service" from
    /// "someone might".
    fn take_prompt_requests(&self) -> Option<mpsc::Receiver<PromptRequest>> {
        self.prompts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
}

/// Registers as backend type `caldav`.
#[derive(Default)]
pub struct CalDavBackendFactory;

impl CalDavBackendFactory {
    pub fn new() -> Self {
        Self
    }
}

impl CalendarBackendFactory for CalDavBackendFactory {
    fn backend_type(&self) -> &str {
        "caldav"
    }

    fn create(
        &self,
        connection_id: &str,
        config: &str,
        _ctx: &not_yet_done_content::HostContext,
    ) -> Result<Box<dyn CalendarBackend>, CalendarError> {
        let cfg: CalDavConfig = serde_yaml::from_str(config)
            .map_err(|e| CalendarError::Config(format!("invalid caldav backend config: {e}")))?;
        let label = cfg
            .name
            .clone()
            .unwrap_or_else(|| connection_id.to_string());
        // Only a provider that can raise a dialog earns a stream; without
        // one the adapter offers the frontend nothing to service.
        let interactive = cfg.username.can_prompt() || cfg.password.can_prompt();
        let (prompts, rx) = if interactive {
            let (tx, rx) = mpsc::channel::<PromptRequest>(8);
            (Some(CredentialPrompts::new(label.clone(), tx)), Some(rx))
        } else {
            (None, None)
        };
        let client = CalDavClient::from_config(&cfg, prompts.as_ref())?;
        Ok(Box::new(CalDavBackend {
            connection_id: connection_id.to_string(),
            label,
            client,
            prompts: Mutex::new(rx),
        }))
    }
}
