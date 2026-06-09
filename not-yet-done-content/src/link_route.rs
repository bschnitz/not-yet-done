//! Top-down routing protocol for [`crate::NodeRef`] open-by-path.
//!
//! Each routable layer (App → tab → adapter → node) implements
//! [`LinkRoute`]. When the user follows a link, every layer peels its
//! own head segment off the path and forwards the tail to the layer
//! below, which alone knows how to interpret it. No layer needs to
//! understand the inner structure of any layer below itself.
//!
//! Open is therefore symmetric to the "mark current node" direction:
//! every layer contributes one segment going up, and consumes one
//! segment going down.

use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LinkRouteError {
    /// Head segment did not match any route at this layer.
    #[error("unknown route segment: {0}")]
    UnknownRoute(String),

    /// Path resolved structurally but the target no longer exists.
    /// Surfaces in the UI as a "stale link, delete?" confirmation.
    #[error("stale link: {0}")]
    Stale(String),

    /// Layer recognised the head but cannot open it (e.g. Postgres
    /// rows in v1 — their IDs are not stable across refreshes).
    #[error("not supported: {0}")]
    NotSupported(String),

    /// Catch-all for layer-specific failures (network, IO, …).
    #[error("{0}")]
    Other(String),
}

/// One layer of the link-open dispatch chain.
///
/// `tail` is everything after this layer's own head segment. `None`
/// means "open this layer's root" — useful when a `NodeRef` ends at
/// the layer's boundary.
#[async_trait]
pub trait LinkRoute {
    async fn open_ref(&mut self, tail: Option<&str>) -> Result<(), LinkRouteError>;
}
