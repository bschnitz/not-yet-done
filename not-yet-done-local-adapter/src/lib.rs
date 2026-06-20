//! In-process adapter wiring for the host application's own data
//! (Tasks, Trackings).
//!
//! Every other adapter (Jira, Taiga, Postgres, …) talks to a *remote*
//! backend, so its factory is constructed from an opaque YAML config
//! string and nothing else. The local adapters are different: they wrap
//! the very [`TaskService`]/[`TrackingRepository`] the host App already
//! holds. Those handles cannot be reconstructed from a config string —
//! they must be *threaded in* from the App.
//!
//! [`CoreHandle`] is that thread. The App builds one once and captures it
//! in the adapter-factory closure (see `build_adapter_factories` in the
//! TUI crate). [`LocalAdapterFactory`] holds a clone of the handle and
//! hands it to each adapter it produces.
//!
//! The [`TaskAdapter`] (plan phase A1) lives in [`mod@task`] and grows out
//! of this wiring; the `TrackingAdapter` (A2) joins it here later. Both
//! share the [`CoreHandle`] and the domain-event → invalidation bridge
//! defined below.

use std::sync::Arc;

use not_yet_done_content::Invalidation;
use not_yet_done_task_core::events::{DomainEvent, DomainEventReceiver, DomainEventSender};
use not_yet_done_task_core::repository::TrackingRepository;
use not_yet_done_task_core::service::TaskService;
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

/// Live, in-process handles into the host application's core services.
///
/// Cloneable because every field is an `Arc` (or becomes one) — cloning
/// the handle shares the underlying services rather than duplicating
/// them. The App builds one handle and the factory closure captures a
/// clone so config reloads can rebuild the factory set without rebuilding
/// the services.
///
/// Grows over the adapterization phases: any further repositories the
/// local adapters need (e.g. a saved-query repository for the `task`
/// scope in A1) are added here as fields. We deliberately do *not* store
/// the raw `DatabaseConnection`: `TaskService`/`TrackingRepository`
/// already encapsulate it, and re-exposing the ORM connection would leak
/// a dependency the trait abstractions exist to hide.
///
/// The `events` sender is the [domain-event bus](not_yet_done_task_core::events):
/// the host wires it once, the local adapters bridge the events they care
/// about into their own invalidation stream (see [`spawn_event_bridge`]),
/// and — once A1/A2 land — the services emit onto it so a tracking toggle
/// in one tab repaints the others without any tab knowing about the rest.
#[derive(Clone)]
pub struct CoreHandle {
    pub task_service: Arc<dyn TaskService>,
    pub tracking_repo: Arc<dyn TrackingRepository>,
    pub events: DomainEventSender,
    /// Tracking policy mirrored from the host's `tracking.allow_parallel`
    /// config. When `false` (the default) starting tracking on a task via
    /// the editor's `tracking: true` field first stops every other active
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
        events: DomainEventSender,
        allow_parallel_tracking: bool,
    ) -> Self {
        Self {
            task_service,
            tracking_repo,
            events,
            allow_parallel_tracking,
        }
    }
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
pub fn spawn_event_bridge(mut events: DomainEventReceiver, inv_tx: broadcast::Sender<Invalidation>) {
    tokio::spawn(async move {
        use broadcast::error::RecvError;
        loop {
            match events.recv().await {
                Ok(ev) => {
                    if let Some(inv) = domain_event_to_invalidation(&ev) {
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

    use not_yet_done_task_core::events::new_bus;
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
        let bus = new_bus(8);
        let (inv_tx, mut inv_rx) = broadcast::channel(8);
        spawn_event_bridge(bus.subscribe(), inv_tx);
        // Give the spawned bridge a moment to reach its `recv().await`.
        tokio::task::yield_now().await;
        bus.send(DomainEvent::TrackingTick).unwrap();
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
