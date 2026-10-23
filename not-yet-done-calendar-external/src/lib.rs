//! Calendar backend `external`: a calendar served by another program.
//!
//! nyd starts the program named in the connection's `command`, hands it a
//! `start` line (its opaque `config:` block and the secrets nyd resolved for
//! it) and from then on talks JSON lines with it — one call per line, one
//! reply per call, plus whatever the program says unasked: load progress, a
//! sentence for a person to act on, a form to fill in. The protocol is in
//! [`protocol`]; ADR 0014 says why calendars plug in this way.
//!
//! The program is started on first use and kept for the backend's lifetime.
//! Connections naming the same `command` and `share` key talk to one process,
//! so two calendars of one account need one sign-in.

mod config;
mod process;
pub mod protocol;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use tokio::sync::{broadcast, mpsc, watch};

use not_yet_done_calendar_core::{
    CalEvent, CalendarBackend, CalendarBackendFactory, CalendarError, CalendarRef, EventDraft,
    EventPatch, LoadProgress, TimeRange,
};
use not_yet_done_content::PromptRequest;
use not_yet_done_content::auth::{CredentialPrompts, CredentialResolver};

use config::ExternalConfig;
use process::{Process, PromptSink, StartSpec};
use protocol::{ErrorKind, ReplyError, ToBackend};

/// Which process a connection talks to: its `command` and `share` key.
type ProcessKey = (String, String);
type Slot = Arc<tokio::sync::Mutex<Weak<Process>>>;

/// Every running backend process, held weakly: the backends hold the strong
/// handles, so a process lives exactly as long as a connection uses it.
/// A slot's lock is held while its process starts, so two connections sharing
/// one never start it twice.
static PROCESSES: LazyLock<Mutex<HashMap<ProcessKey, Slot>>> = LazyLock::new(Mutex::default);

/// One `external` connection.
pub struct ExternalBackend {
    connection_id: String,
    label: String,
    key: ProcessKey,
    command: String,
    backend_config: serde_json::Value,
    secret_resolvers: BTreeMap<String, Box<dyn CredentialResolver>>,
    call_timeout: Duration,
    /// The process this connection uses, once started.
    process: tokio::sync::Mutex<Option<Arc<Process>>>,
    /// The backend's progress and attention, re-sent from whichever process
    /// it uses — created up front, so the adapter can subscribe before the
    /// first call starts anything.
    ready_tx: broadcast::Sender<LoadProgress>,
    attention_tx: watch::Sender<Option<String>>,
    /// Where the backend's forms and the secrets' own questions go.
    prompt_tx: mpsc::Sender<PromptRequest>,
    prompt_rx: Mutex<Option<mpsc::Receiver<PromptRequest>>>,
    credential_prompts: CredentialPrompts,
    /// Every attention and form the backend put to a person.
    asked: Arc<AtomicU64>,
    empty_is_authoritative: AtomicBool,
}

impl ExternalBackend {
    /// The running process, started (or found running for another connection
    /// sharing it) on first use and started again after it ended.
    async fn process(&self) -> Result<Arc<Process>, CalendarError> {
        let mut mine = self.process.lock().await;
        if let Some(process) = mine.as_ref().filter(|p| p.is_alive()) {
            return Ok(Arc::clone(process));
        }
        let slot = Arc::clone(
            PROCESSES
                .lock()
                .unwrap()
                .entry(self.key.clone())
                .or_default(),
        );
        let mut running = slot.lock().await;
        let process = match running.upgrade().filter(|p| p.is_alive()) {
            Some(process) => process,
            None => {
                let process = Process::start(
                    StartSpec {
                        name: self.key.1.clone(),
                        command: self.command.clone(),
                        connection: self.key.1.clone(),
                        config: self.backend_config.clone(),
                        secrets: self.resolve_secrets().await?,
                    },
                    Some(PromptSink {
                        source: self.label.clone(),
                        tx: self.prompt_tx.clone(),
                    }),
                    Arc::clone(&self.asked),
                )
                .await?;
                *running = Arc::downgrade(&process);
                process
            }
        };
        self.wire(&process);
        *mine = Some(Arc::clone(&process));
        Ok(process)
    }

    /// Re-send `process`'s progress and attention as this backend's own,
    /// until the process ends.
    fn wire(&self, process: &Process) {
        self.empty_is_authoritative
            .store(process.empty_is_authoritative(), Ordering::Relaxed);
        let mut progress = process.subscribe_progress();
        let ready_tx = self.ready_tx.clone();
        tokio::spawn(async move {
            loop {
                match progress.recv().await {
                    Ok(p) => {
                        let _ = ready_tx.send(p);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        let mut attention = process.subscribe_attention();
        let attention_tx = self.attention_tx.clone();
        tokio::spawn(async move {
            loop {
                let said = attention.borrow_and_update().clone();
                attention_tx.send_replace(said);
                if attention.changed().await.is_err() {
                    break;
                }
            }
            attention_tx.send_replace(None);
        });
    }

    async fn resolve_secrets(&self) -> Result<BTreeMap<String, String>, CalendarError> {
        let mut secrets = BTreeMap::new();
        for (name, resolver) in &self.secret_resolvers {
            let value = resolver
                .resolve()
                .await
                .map_err(|e| CalendarError::Auth(format!("resolve secret {name}: {e}")))?;
            secrets.insert(name.clone(), value);
        }
        Ok(secrets)
    }

    /// Make one call and read its reply as `T`.
    async fn call<T: DeserializeOwned>(
        &self,
        what: &str,
        make: impl FnOnce(u64) -> ToBackend,
    ) -> Result<T, ReplyError> {
        let process = self.process().await.map_err(|e| ReplyError::from(&e))?;
        let value = process.call(what, make, self.call_timeout).await?;
        serde_json::from_value(value).map_err(|e| ReplyError {
            kind: ErrorKind::Other,
            message: format!(
                "calendar backend `{}` answered `{what}` with something else: {e}",
                self.key.1
            ),
        })
    }

    /// The event as this connection shows it: labelled with the connection
    /// when the backend named no calendar.
    fn labelled(&self, mut event: CalEvent) -> CalEvent {
        if event.calendar.is_empty() {
            event.calendar = self.label.clone();
        }
        event
    }
}

#[async_trait]
impl CalendarBackend for ExternalBackend {
    fn connection_id(&self) -> &str {
        &self.connection_id
    }

    fn connection_label(&self) -> &str {
        &self.label
    }

    async fn list_events(&self, range: &TimeRange) -> Result<Vec<CalEvent>, CalendarError> {
        let events: Vec<CalEvent> = self
            .call("list_events", |id| ToBackend::ListEvents {
                id,
                range: range.clone(),
            })
            .await
            .map_err(ReplyError::into_calendar_error)?;
        Ok(events.into_iter().map(|e| self.labelled(e)).collect())
    }

    /// A backend that cannot list its calendars offers no write targets.
    async fn list_calendars(&self) -> Result<Vec<CalendarRef>, CalendarError> {
        match self
            .call("list_calendars", |id| ToBackend::ListCalendars { id })
            .await
        {
            Ok(calendars) => Ok(calendars),
            Err(e) if e.kind == ErrorKind::Unsupported => Ok(Vec::new()),
            Err(e) => Err(e.into_calendar_error()),
        }
    }

    async fn create_event(
        &self,
        calendar_id: Option<&str>,
        draft: &EventDraft,
    ) -> Result<CalEvent, CalendarError> {
        let event = self
            .call("create_event", |id| ToBackend::CreateEvent {
                id,
                calendar_id: calendar_id.map(str::to_string),
                draft: draft.clone(),
            })
            .await
            .map_err(ReplyError::into_calendar_error)?;
        Ok(self.labelled(event))
    }

    async fn delete_event(&self, uid: &str) -> Result<(), CalendarError> {
        self.call::<serde_json::Value>("delete_event", |id| ToBackend::DeleteEvent {
            id,
            uid: uid.to_string(),
        })
        .await
        .map(drop)
        .map_err(ReplyError::into_calendar_error)
    }

    async fn update_event(&self, uid: &str, patch: &EventPatch) -> Result<(), CalendarError> {
        self.call::<serde_json::Value>("update_event", |id| ToBackend::UpdateEvent {
            id,
            uid: uid.to_string(),
            patch: patch.clone(),
        })
        .await
        .map(drop)
        .map_err(ReplyError::into_calendar_error)
    }

    /// Only a running backend has a window; this never starts one to show it.
    async fn toggle_window(&self) -> Result<bool, CalendarError> {
        let running = self
            .process
            .lock()
            .await
            .as_ref()
            .is_some_and(|p| p.is_alive());
        if !running {
            return Err(CalendarError::Other(format!(
                "calendar backend `{}` is not running",
                self.key.1
            )));
        }
        match self
            .call("toggle_window", |id| ToBackend::ToggleWindow { id })
            .await
        {
            Ok(shown) => Ok(shown),
            Err(e) if e.kind == ErrorKind::Unsupported => Err(CalendarError::Other(
                "this calendar connection has no window".into(),
            )),
            Err(e) => Err(e.into_calendar_error()),
        }
    }

    fn subscribe_ready(&self) -> Option<broadcast::Receiver<LoadProgress>> {
        Some(self.ready_tx.subscribe())
    }

    fn subscribe_attention(&self) -> Option<watch::Receiver<Option<String>>> {
        Some(self.attention_tx.subscribe())
    }

    fn take_prompt_requests(&self) -> Option<mpsc::Receiver<PromptRequest>> {
        self.prompt_rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    fn sign_in_prompts(&self) -> u64 {
        self.asked.load(Ordering::Relaxed) + self.credential_prompts.asked()
    }

    fn empty_is_authoritative(&self) -> bool {
        self.empty_is_authoritative.load(Ordering::Relaxed)
    }
}

/// Registers as backend type `external`.
#[derive(Default)]
pub struct ExternalBackendFactory;

impl ExternalBackendFactory {
    pub fn new() -> Self {
        Self
    }
}

impl CalendarBackendFactory for ExternalBackendFactory {
    fn backend_type(&self) -> &str {
        "external"
    }

    fn create(
        &self,
        connection_id: &str,
        config: &str,
        _ctx: &not_yet_done_content::HostContext,
    ) -> Result<Box<dyn CalendarBackend>, CalendarError> {
        let cfg: ExternalConfig = serde_yaml::from_str(config)
            .map_err(|e| CalendarError::Config(format!("invalid external backend config: {e}")))?;
        let label = cfg
            .name
            .clone()
            .unwrap_or_else(|| connection_id.to_string());
        let (prompt_tx, prompt_rx) = mpsc::channel(8);
        let credential_prompts = CredentialPrompts::new(label.clone(), prompt_tx.clone());
        let secret_resolvers = cfg
            .build_secret_resolvers(&credential_prompts)
            .map_err(|e| CalendarError::Config(format!("external backend secrets: {e}")))?;
        let backend_config = cfg.backend_config().map_err(CalendarError::Config)?;
        let share = cfg
            .share
            .clone()
            .unwrap_or_else(|| connection_id.to_string());
        let (ready_tx, _) = broadcast::channel(16);
        let (attention_tx, _) = watch::channel(None);
        Ok(Box::new(ExternalBackend {
            connection_id: connection_id.to_string(),
            label,
            key: (cfg.command.clone(), share),
            command: cfg.command,
            backend_config,
            secret_resolvers,
            call_timeout: Duration::from_secs(cfg.call_timeout_secs),
            process: tokio::sync::Mutex::new(None),
            ready_tx,
            attention_tx,
            prompt_tx,
            prompt_rx: Mutex::new(Some(prompt_rx)),
            credential_prompts,
            asked: Arc::new(AtomicU64::new(0)),
            empty_is_authoritative: AtomicBool::new(false),
        }))
    }
}

#[cfg(test)]
mod tests;
