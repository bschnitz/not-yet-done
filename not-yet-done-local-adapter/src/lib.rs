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
//! This crate currently ships a no-op [`LocalAdapter`] whose sole job is
//! to prove the wiring compiles and that a registered factory can reach
//! live core services. The real `TaskAdapter`/`TrackingAdapter` (plan
//! phases A1/A2) grow out of this skeleton.

use std::sync::Arc;

use async_trait::async_trait;
use not_yet_done_content::{
    AdapterCapabilities, AdapterFactory, ContentAdapter, Invalidation, Metadata, Node, NodeType,
    Result,
};
use not_yet_done_core::events::{DomainEvent, DomainEventReceiver, DomainEventSender};
use not_yet_done_core::repository::TrackingRepository;
use not_yet_done_core::service::TaskService;
use tokio::sync::broadcast;

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
/// The `events` sender is the [domain-event bus](not_yet_done_core::events):
/// the host wires it once, the local adapters bridge the events they care
/// about into their own invalidation stream (see [`spawn_event_bridge`]),
/// and — once A1/A2 land — the services emit onto it so a tracking toggle
/// in one tab repaints the others without any tab knowing about the rest.
#[derive(Clone)]
pub struct CoreHandle {
    pub task_service: Arc<dyn TaskService>,
    pub tracking_repo: Arc<dyn TrackingRepository>,
    pub events: DomainEventSender,
}

impl CoreHandle {
    pub fn new(
        task_service: Arc<dyn TaskService>,
        tracking_repo: Arc<dyn TrackingRepository>,
        events: DomainEventSender,
    ) -> Self {
        Self {
            task_service,
            tracking_repo,
            events,
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
        DomainEvent::TrackingStarted { .. } | DomainEvent::TrackingStopped { .. } => {
            Invalidation::All
        }
        DomainEvent::TrackingTick => Invalidation::Repaint,
    })
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

/// Builds [`LocalAdapter`] instances bound to a captured [`CoreHandle`].
///
/// Unlike the remote-adapter factories, `create`'s `config` string is
/// (for now) unused — the local adapter's backing store is the captured
/// handle, not anything described in YAML. The arg is kept to satisfy the
/// [`AdapterFactory`] contract and to leave room for future per-instance
/// view options.
pub struct LocalAdapterFactory {
    handle: CoreHandle,
}

impl LocalAdapterFactory {
    pub fn new(handle: CoreHandle) -> Self {
        Self { handle }
    }
}

impl AdapterFactory for LocalAdapterFactory {
    fn adapter_type(&self) -> &str {
        "local"
    }

    fn create(&self, instance_id: &str, _config: &str) -> Result<Box<dyn ContentAdapter>> {
        // The adapter owns an invalidation broadcast; the bridge task
        // republishes domain events onto it so views bound to this
        // adapter refetch/repaint without polling.
        let (inv_tx, _) = broadcast::channel(64);
        spawn_event_bridge(self.handle.events.subscribe(), inv_tx.clone());
        Ok(Box::new(LocalAdapter {
            instance_id: instance_id.to_string(),
            handle: self.handle.clone(),
            inv_tx,
        }))
    }
}

/// No-op skeleton adapter. Proves the in-process wiring: it holds a live
/// [`CoreHandle`] and is reachable through the standard factory registry.
/// All content methods are placeholders until phases A1/A2 implement the
/// real Task/Tracking trees.
pub struct LocalAdapter {
    instance_id: String,
    #[allow(dead_code)] // wired now, consumed in A1/A2.
    handle: CoreHandle,
    /// Owns the adapter's invalidation broadcast. The bridge task holds a
    /// clone of the sender; keeping the sender here means every
    /// `subscribe_invalidations()` receiver stays open for the adapter's
    /// lifetime.
    inv_tx: broadcast::Sender<Invalidation>,
}

#[async_trait]
impl ContentAdapter for LocalAdapter {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn adapter_type(&self) -> &str {
        "local"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::default()
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        Ok(Box::new(LocalRootNode {
            node_type: local_root_type(),
            metadata: Metadata::default(),
        }))
    }

    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        Err(not_yet_done_content::ContentError::NotFound(id.to_string()))
    }

    fn subscribe_invalidations(&self) -> broadcast::Receiver<Invalidation> {
        self.inv_tx.subscribe()
    }
}

fn local_root_type() -> NodeType {
    NodeType {
        type_id: "local:root".to_string(),
        mime_type: "text/plain".to_string(),
        syntax: None,
        file_extension: ".txt".to_string(),
        display_name: "Local".to_string(),
    }
}

/// Minimal root node for the skeleton adapter.
struct LocalRootNode {
    node_type: NodeType,
    metadata: Metadata,
}

#[async_trait]
impl Node for LocalRootNode {
    fn id(&self) -> &str {
        "local:root"
    }

    fn label(&self) -> &str {
        "Local"
    }

    fn node_type(&self) -> &NodeType {
        &self.node_type
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use not_yet_done_core::events::new_bus;
    use uuid::Uuid;

    #[test]
    fn root_node_surface_is_stable() {
        // The CoreHandle needs live core services (a DB), so the
        // wiring is exercised end-to-end once A1/A2 consume it. Here we
        // pin the no-op root node's identity, which the renderer relies
        // on and which needs no handle.
        let node = LocalRootNode {
            node_type: local_root_type(),
            metadata: Metadata::default(),
        };
        assert_eq!(node.id(), "local:root");
        assert_eq!(node.label(), "Local");
        assert_eq!(node.node_type().type_id, "local:root");
    }

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
}
