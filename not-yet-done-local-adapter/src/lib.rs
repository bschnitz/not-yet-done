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
use not_yet_done_task_core::service::ProjectService;
use not_yet_done_task_core::service::TagService;
use not_yet_done_task_core::service::TaskService;
use not_yet_done_task_core::service::TrackingService;
use tokio::sync::broadcast;

// Domain anonymizer for Tasks/Trackings/Projects: maps the sensitive name
// columns to stable invented pseudo-names (consistent across the three),
// passes structural columns through. Returned from `anonymizer()`.
mod anonymize;
// Human-friendly datetime / offset parsing for the trackings adapter's
// `split`/`move` Form actions (mirrors the CLI's datetime.rs / offset.rs).
mod datetime;
// Shared `InputSpec::Form` field-map readers for the local adapters'
// `execute` paths (trackings split/move, projects create/edit/delete).
mod form;
pub mod editor_templates;
pub mod notes;
pub mod projects;
pub mod task;
pub mod tracking;
// Subtree-restructure tree editor (serialize → parse → diff → apply). Moved
// here from the TUI crate: `apply_changes` drives `task_service` + the local
// `notes::move_notes`, so it belongs with the task-domain operations. Both the
// TaskAdapter and the transitional native `RestructureSession` consume it.
pub mod tree_edit;
pub use projects::{ProjectAdapter, ProjectAdapterFactory};
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
    /// Project listing + CRUD — backs the Projects adapter's `list` and its
    /// `create`/`edit`/`delete` actions.
    pub project_service: Arc<dyn ProjectService>,
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
    /// Directory the `backup` action writes timestamped copies of this
    /// handle's database into. Defaults to
    /// [`not_yet_done_task_core::backup::default_backup_dir`]; overridable via
    /// the adapter config's `backup.directory`.
    backup_dir: std::path::PathBuf,
    /// How many of *this database's* backups to keep (older ones are pruned).
    /// Defaults to 10; overridable via the adapter config's `backup.max_count`.
    backup_max_count: usize,
}

impl CoreHandle {
    pub fn new(
        task_service: Arc<dyn TaskService>,
        tracking_repo: Arc<dyn TrackingRepository>,
        tracking_service: Arc<dyn TrackingService>,
        tag_service: Arc<dyn TagService>,
        project_service: Arc<dyn ProjectService>,
        bus: Arc<dyn HostEventBus>,
        channel: String,
        allow_parallel_tracking: bool,
    ) -> Self {
        Self {
            task_service,
            tracking_repo,
            tracking_service,
            tag_service,
            project_service,
            bus,
            channel,
            allow_parallel_tracking,
            backup_dir: not_yet_done_task_core::backup::default_backup_dir(),
            backup_max_count: 10,
        }
    }

    /// Override the backup destination and retention for the `backup` action.
    /// Builder-style so the many `CoreHandle::new` call sites (tests included)
    /// keep the sensible defaults without change.
    pub fn with_backup(mut self, dir: std::path::PathBuf, max_count: usize) -> Self {
        self.backup_dir = dir;
        self.backup_max_count = max_count;
        self
    }

    /// The DSN of the database this handle backs (also its bus channel key).
    pub fn dsn(&self) -> &str {
        &self.channel
    }

    /// The configured backup directory and retention count.
    pub fn backup_settings(&self) -> (&std::path::Path, usize) {
        (&self.backup_dir, self.backup_max_count)
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

/// Shared `backup` container action for the local Tasks and Trackings
/// adapters: write a timestamped copy of the handle's database into the
/// configured backup directory (default `<data-local>/not_yet_done/backups`)
/// and prune that database's own backups to the retention count. Returns a
/// [`Notify`](not_yet_done_content::ActionDispatch::Notify) carrying the path,
/// or an [`Error`](not_yet_done_content::ActionDispatch::Error).
///
/// Both tabs back up the *same* `tasks.db`, so either tab's `backup` does the
/// identical thing. The blocking file copy runs on the blocking pool so it
/// never stalls the async runtime.
pub(crate) async fn invoke_backup(handle: &CoreHandle) -> not_yet_done_content::ActionDispatch {
    use not_yet_done_content::ActionDispatch;
    let dsn = handle.dsn().to_string();
    let (dir, max_count) = handle.backup_settings();
    let dir = dir.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        not_yet_done_task_core::backup::create_backup_at(&dsn, &dir, max_count)
    })
    .await;
    match result {
        Ok(Ok(path)) => ActionDispatch::Notify {
            message: format!("Backup written: {path}"),
        },
        Ok(Err(e)) => ActionDispatch::Error(format!("Backup failed: {e}")),
        Err(e) => ActionDispatch::Error(format!("Backup task panicked: {e}")),
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
    /// Where the `backup` action writes timestamped copies of this adapter's
    /// database, and how many to retain.
    ///
    /// *Why it exists:* the `backup` action (a container action on the Tasks
    /// and Trackings tabs) makes an on-the-spot copy of `tasks.db`. Omit the
    /// block to use the per-host default directory
    /// (`<data-local>/not_yet_done/backups`) and keep the last 10 — the same
    /// location the legacy daily backup used, so all backups stay together.
    /// Set `directory` to redirect them (e.g. onto a synced volume) or
    /// `max_count` to keep more/fewer. Retention is per-database: pruning this
    /// adapter's backups never touches another database's copies in the same
    /// directory.
    #[serde(default)]
    pub backup: Option<BackupSettings>,
}

/// Optional `backup:` config block for a local adapter (see
/// [`LocalAdapterConfig::backup`]).
#[derive(Debug, Default, serde::Deserialize)]
pub struct BackupSettings {
    /// Backup directory. Default: `<data-local>/not_yet_done/backups`.
    #[serde(default)]
    pub directory: Option<std::path::PathBuf>,
    /// Number of this database's backups to retain. Default: 10.
    #[serde(default)]
    pub max_count: Option<usize>,
}

/// The per-host default task DSN: `<data-local>/not_yet_done/tasks.db`.
///
/// Re-exported from the core ([`not_yet_done_task_core::bootstrap::default_task_dsn`])
/// so the adapter and the standalone `nyd-t` CLI resolve the *same* file when no
/// explicit `database:` DSN is set. Kept as a re-export (rather than moving call
/// sites) so this crate's public surface is unchanged.
pub use not_yet_done_task_core::bootstrap::default_task_dsn;

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

    let backup = cfg.backup.unwrap_or_default();
    let backup_dir = backup
        .directory
        .unwrap_or_else(not_yet_done_task_core::backup::default_backup_dir);
    let backup_max_count = backup.max_count.unwrap_or(10);

    Ok(CoreHandle::new(
        domain.task_service,
        domain.tracking_repo,
        domain.tracking_service,
        domain.tag_service,
        domain.project_service,
        ctx.event_bus.clone(),
        dsn,
        cfg.allow_parallel.unwrap_or(false),
    )
    .with_backup(backup_dir, backup_max_count))
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
        | DomainEvent::TrackingChanged { .. }
        | DomainEvent::ProjectChanged { .. } => Invalidation::All,
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
