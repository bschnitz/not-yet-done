//! Core domain-event bus.
//!
//! A process-wide `tokio::sync::broadcast` channel over which the core
//! services announce *what changed* in the domain — without knowing who
//! listens. It is the backbone of the "no staleness" requirement: an
//! adapter (or the Waybar module, or any other consumer) subscribes and
//! reacts, but the emitter never depends on the consumer.
//!
//! ## Why a bus and not direct calls
//!
//! Toggling a tracking from the Tasks tab must update the Trackings tab,
//! the task's own running-marker, and the Waybar indicator. Wiring those
//! as direct method calls would couple every tab to every other tab. With
//! the bus, each consumer subscribes independently; the writer just emits
//! one [`DomainEvent`] and is done. Adapters bridge the events they care
//! about into their own `subscribe_invalidations()` stream (see
//! `not-yet-done-local-adapter`).
//!
//! ## Event granularity
//!
//! - Structural changes (a row created/deleted, a tracking started or
//!   stopped) carry the affected id(s); a consumer maps them to a
//!   refetch.
//! - [`DomainEvent::TrackingTick`] is the heartbeat emitted ~1 Hz while a
//!   tracking runs. It carries no id and must **not** trigger a refetch —
//!   consumers map it to a lightweight repaint so live durations tick
//!   without hammering the database.

use tokio::sync::broadcast;
use uuid::Uuid;

/// A domain change worth announcing. `Clone` because `broadcast` hands
/// each subscriber its own copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DomainEvent {
    /// A task's own fields changed (created, edited, reparented, deleted,
    /// restored). Consumers refetch the affected subtree/list.
    TaskChanged { id: Uuid },
    /// A tracking was started for a task. Structural: a new tracking row
    /// exists and the task is now "running".
    TrackingStarted { task_id: Uuid, tracking_id: Uuid },
    /// A running tracking was stopped. Structural: the tracking row gained
    /// an end time and the task is no longer "running".
    TrackingStopped { task_id: Uuid, tracking_id: Uuid },
    /// A tracking row changed without a start/stop transition — soft-deleted
    /// or restored. Structural: the tracking list (and a task's active marker,
    /// if the affected row was running) may change, so consumers refetch.
    /// Distinct from `TrackingStarted`/`Stopped`, which mark live transitions.
    TrackingChanged { tracking_id: Uuid },
    /// Heartbeat while at least one tracking runs (~1 Hz). Carries no id;
    /// consumers map it to a **repaint only**, never a refetch — it exists
    /// so live "elapsed" cells tick.
    TrackingTick,
}

/// Sender half of the bus. Cloneable; lives as long as any holder keeps a
/// clone, so subscribers never see a spurious "closed" until the last
/// sender drops.
pub type DomainEventSender = broadcast::Sender<DomainEvent>;

/// Receiver half. Each consumer holds its own.
pub type DomainEventReceiver = broadcast::Receiver<DomainEvent>;

/// Create a fresh domain-event bus, returning the sender. Subscribers
/// call [`DomainEventSender::subscribe`]. `capacity` bounds how many
/// unread events a slow subscriber may buffer before it observes
/// `Lagged`; consumers must treat `Lagged` as "I missed some — resync
/// conservatively" rather than as a hard error.
pub fn new_bus(capacity: usize) -> DomainEventSender {
    broadcast::channel(capacity).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn delivers_events_to_subscribers() {
        let bus = new_bus(8);
        let mut rx = bus.subscribe();
        bus.send(DomainEvent::TrackingTick).unwrap();
        assert_eq!(rx.recv().await.unwrap(), DomainEvent::TrackingTick);
    }

    #[tokio::test]
    async fn each_subscriber_gets_its_own_copy() {
        let bus = new_bus(8);
        let mut a = bus.subscribe();
        let mut b = bus.subscribe();
        let id = Uuid::nil();
        bus.send(DomainEvent::TaskChanged { id }).unwrap();
        assert_eq!(a.recv().await.unwrap(), DomainEvent::TaskChanged { id });
        assert_eq!(b.recv().await.unwrap(), DomainEvent::TaskChanged { id });
    }
}
