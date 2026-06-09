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
    AdapterCapabilities, AdapterFactory, ContentAdapter, Metadata, Node, NodeType, Result,
};
use not_yet_done_core::repository::TrackingRepository;
use not_yet_done_core::service::TaskService;

/// Live, in-process handles into the host application's core services.
///
/// Cloneable because every field is an `Arc` (or becomes one) — cloning
/// the handle shares the underlying services rather than duplicating
/// them. The App builds one handle and the factory closure captures a
/// clone so config reloads can rebuild the factory set without rebuilding
/// the services.
///
/// Grows over the adapterization phases: the **domain-event-bus sender**
/// (plan phase E4a, mechanism M1) and any further repositories the local
/// adapters need (e.g. a saved-query repository for the `task` scope in
/// A1) are added here as fields. We deliberately do *not* store the raw
/// `DatabaseConnection`: `TaskService`/`TrackingRepository` already
/// encapsulate it, and re-exposing the ORM connection would leak a
/// dependency the trait abstractions exist to hide.
#[derive(Clone)]
pub struct CoreHandle {
    pub task_service: Arc<dyn TaskService>,
    pub tracking_repo: Arc<dyn TrackingRepository>,
}

impl CoreHandle {
    pub fn new(
        task_service: Arc<dyn TaskService>,
        tracking_repo: Arc<dyn TrackingRepository>,
    ) -> Self {
        Self {
            task_service,
            tracking_repo,
        }
    }
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
        Ok(Box::new(LocalAdapter {
            instance_id: instance_id.to_string(),
            handle: self.handle.clone(),
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
}
