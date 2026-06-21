//! In-process adapter wiring for the host application's own data
//! (Tasks, Trackings).
//!
//! Every adapter — local or remote — is constructed from an opaque YAML
//! config string plus the host-provided [`HostContext`]. The local
//! Tasks/Trackings adapters are *self-contained* (Phase C4): their factory
//! reads a `database:` DSN from the config and opens its own task-domain
//! services via [`not_yet_done_task_core::bootstrap::open`], rather than
//! having concrete services threaded in from the App.
//!
//! [`CoreHandle`] bundles those freshly-opened services together with the
//! host event bus for one adapter instance. [`TaskAdapterFactory`] /
//! [`TrackingAdapterFactory`] each build a handle in `create` and hand it to
//! the adapter they produce.
//!
//! Cross-tab coordination goes through the host-owned [`HostEventBus`]
//! (injected via [`HostContext`]), **keyed by the DSN**: every local adapter
//! opened on the same database shares that channel, so a tracking toggle in
//! one tab repaints the others; adapters on a different database use a
//! different channel and stay silent. The bus carries opaque
//! [`HostEvent`](not_yet_done_content::HostEvent) payloads; the local
//! adapters privately agree these are [`DomainEvent`]s and downcast them in
//! their bridges (see [`spawn_event_bridge`] and the per-adapter bridges).
//!
//! The [`TaskAdapter`] (plan phase A1) lives in [`mod@task`]; the
//! [`TrackingAdapter`] (A2) lives in [`mod@tracking`]. Both share the
//! [`CoreHandle`] and the domain-event → invalidation bridge defined below.

use std::sync::Arc;

use not_yet_done_content::{HostEvent, HostEventBus, Invalidation};
use not_yet_done_task_core::events::DomainEvent;
use not_yet_done_task_core::repository::TrackingRepository;
use not_yet_done_task_core::service::TagService;
use not_yet_done_task_core::service::TaskService;
use not_yet_done_task_core::service::TrackingService;
use tokio::sync::broadcast;

pub mod editor_templates;
pub mod notes;
pub mod task;
pub mod tracking;
// Subtree-restructure tree editor (serialize → parse → diff → apply). Moved
// here from the TUI crate: `apply_changes` drives `task_service` + the local
// `notes::move_notes`, so it belongs with the task-domain operations. Both the
// TaskAdapter and the transitional native `RestructureSession` consume it.
pub mod tree_edit;
pub use task::{TaskAdapter, TaskAdapterFactory};
pub use tracking::{TrackingAdapter, TrackingAdapterFactory};

/// The task-domain services for one local adapter instance, bundled with
/// the host event bus they coordinate over.
///
/// Cloneable because every field is an `Arc` (or `String`) — cloning the
/// handle shares the underlying services and bus rather than duplicating
/// them. The factory builds one handle per adapter in `create` (opening the
/// DSN via [`not_yet_done_task_core::bootstrap::open`]) and the adapter and
/// its background bridges clone it.
///
/// We deliberately do *not* store the raw `DatabaseConnection`:
/// `TaskService`/`TrackingRepository` already encapsulate it, and
/// re-exposing the ORM connection would leak a dependency the trait
/// abstractions exist to hide.
///
/// `bus` + `channel` are the cross-adapter coordination path: the host owns
/// the [`HostEventBus`], `channel` is the DSN, and [`publish`](Self::publish)
/// emits a [`DomainEvent`] as an opaque payload on that channel. The local
/// adapters [`subscribe`](Self::subscribe) and bridge the events they care
/// about into their own invalidation stream, so a tracking toggle in one tab
/// repaints the others without any tab knowing about the rest.
#[derive(Clone)]
pub struct CoreHandle {
    pub task_service: Arc<dyn TaskService>,
    pub tracking_repo: Arc<dyn TrackingRepository>,
    /// High-level tracking operations the repo doesn't encapsulate (split/move
    /// with gravity, overlap + future guards) — backs the Trackings adapter's
    /// `split`/`move` entry actions.
    pub tracking_service: Arc<dyn TrackingService>,
    /// Tag listing/management service — backs the task adapter's
    /// `list_values("tags")` (the `option_menu` source) and tag mutations.
    pub tag_service: Arc<dyn TagService>,
    /// Host-owned cross-adapter event bus (see the struct docs).
    bus: Arc<dyn HostEventBus>,
    /// Bus channel key for this handle — the database DSN. Local adapters on
    /// the same database share it; adapters elsewhere use a different channel.
    channel: String,
    /// Tracking policy mirrored from the adapter's `allow_parallel` config.
    /// When `false` (the default) starting tracking on a task via the
    /// editor's `tracking: true` field first stops every other active
    /// tracking, so at most one task tracks at a time; when `true` parallel
    /// trackings are allowed. The local adapters need it because the
    /// `tracking:` toggle in the task edit-buffer (A1b) goes through the
    /// adapter, not the host's native session.
    pub allow_parallel_tracking: bool,
}

impl CoreHandle {
    pub fn new(
        task_service: Arc<dyn TaskService>,
        tracking_repo: Arc<dyn TrackingRepository>,
        tracking_service: Arc<dyn TrackingService>,
        tag_service: Arc<dyn TagService>,
        bus: Arc<dyn HostEventBus>,
        channel: String,
        allow_parallel_tracking: bool,
    ) -> Self {
        Self {
            task_service,
            tracking_repo,
            tracking_service,
            tag_service,
            bus,
            channel,
            allow_parallel_tracking,
        }
    }

    /// Publish a [`DomainEvent`] on the host bus for this handle's channel.
    /// Wrapped as an opaque [`HostEvent`]; peers on the same channel downcast
    /// it back (see [`as_domain_event`]).
    pub fn publish(&self, ev: DomainEvent) {
        self.bus.publish(&self.channel, Arc::new(ev));
    }

    /// Subscribe to this handle's bus channel. Yields opaque [`HostEvent`]s
    /// the bridges downcast back to [`DomainEvent`].
    pub fn subscribe(&self) -> broadcast::Receiver<HostEvent> {
        self.bus.subscribe(&self.channel)
    }
}

/// Downcast an opaque [`HostEvent`] back to the [`DomainEvent`] the local
/// adapters agree to exchange on their shared (DSN-keyed) channel. Returns
/// `None` for a payload of any other type — keeps the bridges total even if
/// a future peer ever shares a channel with a different payload.
pub(crate) fn as_domain_event(payload: &HostEvent) -> Option<DomainEvent> {
    payload.downcast_ref::<DomainEvent>().cloned()
}

/// Config the local Tasks/Trackings adapter factories accept (the tab's
/// `config_inline` / `config:` YAML). Both fields are optional so the
/// historic `config_inline: "{}"` keeps working.
#[derive(Debug, Default, serde::Deserialize)]
pub struct LocalAdapterConfig {
    /// SeaORM DSN of the database backing this adapter, e.g.
    /// `sqlite:///home/me/.local/share/not_yet_done/tasks.db?mode=rwc`.
    ///
    /// *Why it exists:* the local adapters are self-contained (Phase C4) —
    /// each opens its own database rather than borrowing the App's. Omit it
    /// to use the per-host default ([`default_task_dsn`]); set it to point a
    /// Tasks and a Trackings tab at the **same** file (they then coordinate
    /// live via the DSN-keyed bus), or at separate files to isolate them.
    #[serde(default)]
    pub database: Option<String>,
    /// Whether starting a tracking leaves other running trackings alone.
    ///
    /// *Why it exists:* mirrors the host's old `tracking.allow_parallel`. With
    /// `false` (the default) starting a tracking first stops every other
    /// active one — at most one task tracks at a time, matching the native
    /// behaviour; `true` permits concurrent trackings.
    #[serde(default)]
    pub allow_parallel: Option<bool>,
}

/// The per-host default task DSN: `<data-local>/not_yet_done/tasks.db`
/// (e.g. `~/.local/share/not_yet_done/tasks.db`) as a `mode=rwc` SQLite DSN,
/// so a fresh file is created on first open. Falls back to the temp dir when
/// no data-local dir is known. Creates the parent directory if missing.
pub fn default_task_dsn() -> String {
    let dir = dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("not_yet_done");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("tasks.db");
    format!("sqlite://{}?mode=rwc", path.display())
}

/// Parse a local adapter's `config`, open its task database, and bundle the
/// resolved services with the host bus into a [`CoreHandle`].
///
/// Synchronous despite opening an async database connection: it borrows the
/// running multi-thread Tokio runtime via `block_in_place` + `block_on`, the
/// same async-from-sync idiom the editor/tag-menu setup uses, so the
/// `AdapterFactory::create` contract can stay synchronous.
///
/// The handle's bus channel is the resolved DSN: two local-adapter tabs on
/// the same database share the channel (a mutation in one repaints the
/// other), tabs on different databases stay isolated.
pub fn open_core_handle(
    config: &str,
    ctx: &not_yet_done_content::HostContext,
) -> not_yet_done_content::Result<CoreHandle> {
    use not_yet_done_content::ContentError;

    let cfg: LocalAdapterConfig = serde_yaml::from_str(config)
        .map_err(|e| ContentError::Other(format!("Invalid local-adapter config: {e}").into()))?;
    let dsn = cfg.database.unwrap_or_else(default_task_dsn);

    let domain = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(not_yet_done_task_core::bootstrap::open(&dsn))
    })
    .map_err(|e| {
        ContentError::Other(format!("Failed to open task database ({dsn}): {e}").into())
    })?;

    Ok(CoreHandle::new(
        domain.task_service,
        domain.tracking_repo,
        domain.tracking_service,
        domain.tag_service,
        ctx.event_bus.clone(),
        dsn,
        cfg.allow_parallel.unwrap_or(false),
    ))
}

/// Map a core [`DomainEvent`] to the [`Invalidation`] a local adapter
/// should publish for it, or `None` if the adapter ignores that event.
///
/// This skeleton forwards every event; the real adapters (A1/A2) narrow
/// it — the TaskAdapter ignores `TrackingTick`, the TrackingAdapter maps
/// `TrackingTick` to the repaint that makes live durations tick:
///
/// - structural events (`TaskChanged`, `TrackingStarted/Stopped`) →
///   refetch ([`Invalidation::Node`]/[`Invalidation::All`]);
/// - the `TrackingTick` heartbeat → redraw only ([`Invalidation::Repaint`]),
///   never a refetch.
pub fn domain_event_to_invalidation(ev: &DomainEvent) -> Option<Invalidation> {
    Some(match ev {
        DomainEvent::TaskChanged { id } => Invalidation::Node { id: id.to_string() },
        DomainEvent::TrackingStarted { .. }
        | DomainEvent::TrackingStopped { .. }
        | DomainEvent::TrackingChanged { .. } => Invalidation::All,
        DomainEvent::TrackingTick => Invalidation::Repaint,
    })
}

/// Publish a set of refreshed rows as in-place [`Invalidation::Row`]
/// patches (M9) instead of a coarse [`Invalidation::All`] reload.
///
/// The frontend's `patch_row` swaps each row's state by `id` across every
/// pane that currently shows it — no refetch, no tree rebuild, no
/// selection/scroll change. A row whose `id` is not visible is silently
/// ignored, so over-reporting is harmless.
///
/// Any local adapter that can cheaply recompute the rows touched by a
/// structural change should prefer this over `All` so a deep,
/// fully-expanded tree stays responsive: a start/stop toggle patches the
/// affected marker/duration in place rather than re-folding and
/// re-expanding the whole forest. The user presses `r` for a full
/// structural refresh (rows added/removed, ancestor aggregates).
pub fn publish_row_patches(
    inv_tx: &broadcast::Sender<Invalidation>,
    rows: impl IntoIterator<Item = not_yet_done_content::NodeSummary>,
) {
    for row in rows {
        let _ = inv_tx.send(Invalidation::Row(row));
    }
}

/// Spawn a background task that bridges the core domain-event bus into an
/// adapter's own invalidation broadcast: each [`DomainEvent`] is mapped
/// via [`domain_event_to_invalidation`] and republished on `inv_tx`.
///
/// The task lives as long as the bus has senders (the [`CoreHandle`]
/// keeps one) and `inv_tx` has not been dropped. Send failures on
/// `inv_tx` mean "no view is subscribed right now" and are ignored — the
/// adapter may outlive every view and gain new subscribers later. On
/// `Lagged` we resync conservatively with [`Invalidation::All`] so a
/// momentarily-slow bridge never silently drops a structural change.
pub fn spawn_event_bridge(
    mut events: broadcast::Receiver<HostEvent>,
    inv_tx: broadcast::Sender<Invalidation>,
) {
    tokio::spawn(async move {
        use broadcast::error::RecvError;
        loop {
            match events.recv().await {
                Ok(payload) => {
                    if let Some(inv) = as_domain_event(&payload)
                        .as_ref()
                        .and_then(domain_event_to_invalidation)
                    {
                        let _ = inv_tx.send(inv);
                    }
                }
                Err(RecvError::Lagged(_)) => {
                    let _ = inv_tx.send(Invalidation::All);
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    use not_yet_done_content::InMemoryHostBus;
    use uuid::Uuid;

    #[test]
    fn tick_maps_to_repaint_not_refetch() {
        assert_eq!(
            domain_event_to_invalidation(&DomainEvent::TrackingTick),
            Some(Invalidation::Repaint)
        );
    }

    #[test]
    fn task_change_maps_to_node_refetch() {
        let id = Uuid::nil();
        assert_eq!(
            domain_event_to_invalidation(&DomainEvent::TaskChanged { id }),
            Some(Invalidation::Node {
                id: id.to_string()
            })
        );
    }

    #[test]
    fn tracking_lifecycle_maps_to_full_refetch() {
        let task_id = Uuid::nil();
        let tracking_id = Uuid::nil();
        assert_eq!(
            domain_event_to_invalidation(&DomainEvent::TrackingStarted {
                task_id,
                tracking_id
            }),
            Some(Invalidation::All)
        );
        assert_eq!(
            domain_event_to_invalidation(&DomainEvent::TrackingStopped {
                task_id,
                tracking_id
            }),
            Some(Invalidation::All)
        );
    }

    #[tokio::test]
    async fn bridge_forwards_domain_event_as_invalidation() {
        let bus = InMemoryHostBus::default();
        let (inv_tx, mut inv_rx) = broadcast::channel(8);
        spawn_event_bridge(bus.subscribe("c"), inv_tx);
        // Give the spawned bridge a moment to reach its `recv().await`.
        tokio::task::yield_now().await;
        bus.publish("c", Arc::new(DomainEvent::TrackingTick));
        assert_eq!(inv_rx.recv().await.unwrap(), Invalidation::Repaint);
    }

    fn summary(id: &str) -> not_yet_done_content::NodeSummary {
        not_yet_done_content::NodeSummary {
            id: id.to_string(),
            label: id.to_string(),
            node_type: not_yet_done_content::NodeType {
                type_id: "t".into(),
                mime_type: "text/plain".into(),
                syntax: None,
                file_extension: ".txt".into(),
                display_name: "T".into(),
            },
            metadata: not_yet_done_content::Metadata::default(),
            has_children: Some(false),
        }
    }

    #[test]
    fn publish_row_patches_emits_one_row_invalidation_per_row_in_order() {
        let (inv_tx, mut inv_rx) = broadcast::channel(8);
        publish_row_patches(&inv_tx, [summary("a"), summary("b")]);
        assert_eq!(
            inv_rx.try_recv().unwrap(),
            Invalidation::Row(summary("a"))
        );
        assert_eq!(
            inv_rx.try_recv().unwrap(),
            Invalidation::Row(summary("b"))
        );
        // Exactly two — no stray coarse `All`.
        assert!(inv_rx.try_recv().is_err());
    }

    #[test]
    fn publish_row_patches_empty_emits_nothing() {
        let (inv_tx, mut inv_rx) = broadcast::channel(8);
        publish_row_patches(&inv_tx, std::iter::empty());
        assert!(inv_rx.try_recv().is_err());
    }
}
