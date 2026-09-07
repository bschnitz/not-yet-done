//! Action events: a decorator that reports every action a user (or a
//! front-end on their behalf) invokes on an instance, as the bus event
//! [`HOOK_ACTION_INVOKED`].
//!
//! # Why
//!
//! Some automation wants to know *what the user is working on* rather than
//! what changed: opening a ticket's editor, previewing it, jumping to it in
//! the browser. The adapters know nothing of that intent, and the front-ends
//! must not grow a side channel for it. So the host wraps every instance in
//! this decorator, which watches the one action surface both front-ends
//! already go through and publishes one event per successful invocation. The
//! host's hook runner then fires whatever the instance's `hooks:` block binds
//! to `action_invoked` — the same machinery as `tracking_started`.
//!
//! # What is reported
//!
//! One event per successful call to [`Node::invoke_action`] (phase
//! `invoke`), [`Node::prepare`] (`prepare` — the editor is about to open),
//! [`Node::execute`] and [`ContentAdapter::execute_addressed`] (`execute`).
//! An invocation that merely asks for confirmation ([`ActionDispatch::Confirm`])
//! or reports an error is not an invocation yet and stays silent; the
//! confirmed re-invocation reports. The payload is
//!
//! ```json
//! { "action": "edit", "phase": "prepare",
//!   "node_id": "…", "node_type": "jira:issue", "label": "…" }
//! ```
//!
//! `label` is absent on the addressed path, which never loads the node.
//! Scripts a front-end runs on a node are not adapter actions; the front-end
//! reports them itself through [`publish_action_invoked`] with the action
//! spelled `script:<file name>` and phase `script`, so one binding covers both.
//!
//! The decorator sits *inside* the aliasing one, so `action` is always the
//! resolved action id, never an alias name — a `when: { action: … }` filter
//! in a hook binding can rely on the id the view YAML declares.

use crate::decorate::{AdapterDecorator, NodeDecorator};
use crate::*;
use async_trait::async_trait;
use std::sync::Arc;

/// The hook name / bus topic this decorator publishes.
pub const HOOK_ACTION_INVOKED: &str = "action_invoked";

/// The `phase` values an [`HOOK_ACTION_INVOKED`] payload carries.
pub mod phase {
    pub const INVOKE: &str = "invoke";
    pub const PREPARE: &str = "prepare";
    pub const EXECUTE: &str = "execute";
    pub const SCRIPT: &str = "script";
}

/// Publish one [`HOOK_ACTION_INVOKED`] event for `instance`. The decorator
/// uses it for adapter actions; a front-end uses it for the scripts it runs
/// on a node (phase [`phase::SCRIPT`], action `script:<file name>`).
pub fn publish_action_invoked(
    bus: &dyn HostEventBus,
    instance: &str,
    action: &str,
    phase: &str,
    node_id: &str,
    node_type: &str,
    label: Option<&str>,
) {
    let mut payload = serde_json::json!({
        "action": action,
        "phase": phase,
        "node_id": node_id,
        "node_type": node_type,
    });
    if let Some(label) = label {
        payload["label"] = serde_json::Value::String(label.to_string());
    }
    publish_event(bus, BusEvent::new(HOOK_ACTION_INVOKED, instance, payload));
}

/// Wrap `inner` so every successful action invocation on `instance` is
/// published to `bus`. Apply before the aliasing decorator (see the module
/// docs for why).
pub fn action_event_adapter(
    inner: Box<dyn ContentAdapter>,
    instance: &str,
    bus: Arc<dyn HostEventBus>,
) -> Box<dyn ContentAdapter> {
    Box::new(ActionEventAdapter {
        inner: Arc::from(inner),
        emitter: Arc::new(Emitter {
            bus,
            instance: instance.to_string(),
        }),
    })
}

/// The bus and the instance id every event is stamped with.
struct Emitter {
    bus: Arc<dyn HostEventBus>,
    instance: String,
}

impl Emitter {
    fn publish(&self, action: &str, phase: &str, node: &dyn Node) {
        publish_action_invoked(
            &*self.bus,
            &self.instance,
            action,
            phase,
            node.id(),
            &node.node_type().type_id,
            Some(node.label()),
        );
    }
}

/// The adapter decorator: wraps the nodes it hands out and reports the
/// addressed execution path, which bypasses the nodes.
pub struct ActionEventAdapter {
    inner: Arc<dyn ContentAdapter>,
    emitter: Arc<Emitter>,
}

impl ActionEventAdapter {
    fn wrap(&self, node: Box<dyn Node>) -> Box<dyn Node> {
        Box::new(ActionEventNode {
            inner: node,
            emitter: Arc::clone(&self.emitter),
        })
    }
}

#[async_trait]
impl AdapterDecorator for ActionEventAdapter {
    fn inner(&self) -> &dyn ContentAdapter {
        &*self.inner
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        Ok(self.wrap(self.inner.root().await?))
    }
    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        Ok(self.wrap(self.inner.get_by_id(id).await?))
    }

    async fn execute_addressed(
        &self,
        node_type: &NodeType,
        id: &str,
        action_id: &str,
        input: ActionInput,
    ) -> Result<ActionOutcome> {
        let outcome = self
            .inner
            .execute_addressed(node_type, id, action_id, input)
            .await?;
        publish_action_invoked(
            &*self.emitter.bus,
            &self.emitter.instance,
            action_id,
            phase::EXECUTE,
            id,
            &node_type.type_id,
            None,
        );
        Ok(outcome)
    }

    fn hooks(&self) -> Vec<&str> {
        let mut hooks = self.inner.hooks();
        if !hooks.contains(&HOOK_ACTION_INVOKED) {
            hooks.push(HOOK_ACTION_INVOKED);
        }
        hooks
    }
}

/// The node decorator: reports the three node-level invocation paths.
pub struct ActionEventNode {
    inner: Box<dyn Node>,
    emitter: Arc<Emitter>,
}

#[async_trait]
impl NodeDecorator for ActionEventNode {
    fn inner(&self) -> &dyn Node {
        &*self.inner
    }
    fn inner_mut(&mut self) -> &mut dyn Node {
        &mut *self.inner
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        Ok(Box::new(ActionEventNode {
            inner: self.inner.get_child(id).await?,
            emitter: Arc::clone(&self.emitter),
        }))
    }

    async fn invoke_action(&self, name: &str, ctx: &ActionContext) -> Result<ActionDispatch> {
        let dispatch = self.inner.invoke_action(name, ctx).await?;
        if !matches!(
            dispatch,
            ActionDispatch::Confirm { .. } | ActionDispatch::Error(_)
        ) {
            self.emitter.publish(name, phase::INVOKE, &*self.inner);
        }
        Ok(dispatch)
    }
    async fn prepare(&self, action_id: &str, args: &ActionArgs) -> Result<EditorPrep> {
        let prep = self.inner.prepare(action_id, args).await?;
        self.emitter
            .publish(action_id, phase::PREPARE, &*self.inner);
        Ok(prep)
    }
    async fn execute(
        &mut self,
        action_id: &str,
        input: ActionInput,
        args: &ActionArgs,
    ) -> Result<ActionOutcome> {
        let outcome = self.inner.execute(action_id, input, args).await?;
        self.emitter
            .publish(action_id, phase::EXECUTE, &*self.inner);
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{MockAdapterBuilder, MockNodeData, issue_type};

    /// An adapter whose nodes answer every action: `ask` wants confirmation,
    /// `broken` fails, anything else succeeds.
    struct Answering {
        mock: crate::mock::MockAdapter,
    }

    struct AnsweringNode {
        inner: Box<dyn Node>,
    }

    #[async_trait]
    impl NodeDecorator for AnsweringNode {
        fn inner(&self) -> &dyn Node {
            &*self.inner
        }
        fn inner_mut(&mut self) -> &mut dyn Node {
            &mut *self.inner
        }
        async fn invoke_action(&self, name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
            Ok(match name {
                "ask" => ActionDispatch::Confirm {
                    prompt: "sure?".into(),
                },
                "broken" => ActionDispatch::Error("nope".into()),
                _ => ActionDispatch::Noop,
            })
        }
        async fn prepare(&self, action_id: &str, _args: &ActionArgs) -> Result<EditorPrep> {
            if action_id == "broken" {
                return Err(ContentError::NotSupported("nope".into()));
            }
            Ok(EditorPrep::default())
        }
        async fn execute(
            &mut self,
            _action_id: &str,
            _input: ActionInput,
            _args: &ActionArgs,
        ) -> Result<ActionOutcome> {
            Ok(ActionOutcome::Done { message: None })
        }
    }

    #[async_trait]
    impl AdapterDecorator for Answering {
        fn inner(&self) -> &dyn ContentAdapter {
            &self.mock
        }
        async fn root(&self) -> Result<Box<dyn Node>> {
            Ok(Box::new(AnsweringNode {
                inner: self.mock.root().await?,
            }))
        }
        async fn execute_addressed(
            &self,
            _node_type: &NodeType,
            _id: &str,
            _action_id: &str,
            _input: ActionInput,
        ) -> Result<ActionOutcome> {
            Ok(ActionOutcome::Done { message: None })
        }
    }

    fn subject() -> (
        Box<dyn ContentAdapter>,
        tokio::sync::broadcast::Receiver<HostEvent>,
    ) {
        let mock = MockAdapterBuilder::new("mock")
            .node(MockNodeData::new("root", "Root").node_type(issue_type()))
            .build();
        let bus: Arc<dyn HostEventBus> = Arc::new(InMemoryHostBus::new(16));
        let rx = subscribe_events(&*bus);
        let adapter = action_event_adapter(Box::new(Answering { mock }), "inst", bus);
        (adapter, rx)
    }

    fn drain(rx: &mut tokio::sync::broadcast::Receiver<HostEvent>) -> Vec<BusEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let Some(ev) = BusEvent::from_host_event(&ev) {
                out.push(ev);
            }
        }
        out
    }

    #[tokio::test]
    async fn node_paths_report_action_phase_and_node() {
        let (adapter, mut rx) = subject();
        let mut node = adapter.root().await.unwrap();
        node.invoke_action("open", &ActionContext::default())
            .await
            .unwrap();
        node.prepare("edit", &ActionArgs::default()).await.unwrap();
        node.execute("edit", ActionInput::None, &ActionArgs::default())
            .await
            .unwrap();

        let events = drain(&mut rx);
        let seen: Vec<(String, String)> = events
            .iter()
            .map(|e| {
                (
                    e.payload["action"].as_str().unwrap().to_string(),
                    e.payload["phase"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                ("open".to_string(), "invoke".to_string()),
                ("edit".to_string(), "prepare".to_string()),
                ("edit".to_string(), "execute".to_string()),
            ]
        );
        for e in &events {
            assert_eq!(e.topic, HOOK_ACTION_INVOKED);
            assert_eq!(e.source, "inst");
            assert_eq!(e.payload["node_id"], "root");
            assert_eq!(e.payload["label"], "Root");
            assert_eq!(e.payload["node_type"], issue_type().type_id);
        }
    }

    #[tokio::test]
    async fn confirmation_requests_errors_and_failures_stay_silent() {
        let (adapter, mut rx) = subject();
        let node = adapter.root().await.unwrap();
        node.invoke_action("ask", &ActionContext::default())
            .await
            .unwrap();
        node.invoke_action("broken", &ActionContext::default())
            .await
            .unwrap();
        assert!(node.prepare("broken", &ActionArgs::default()).await.is_err());
        assert!(drain(&mut rx).is_empty());
    }

    #[tokio::test]
    async fn addressed_execution_reports_without_a_label() {
        let (adapter, mut rx) = subject();
        adapter
            .execute_addressed(&issue_type(), "n7", "toggle", ActionInput::None)
            .await
            .unwrap();
        let events = drain(&mut rx);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload["action"], "toggle");
        assert_eq!(events[0].payload["phase"], "execute");
        assert_eq!(events[0].payload["node_id"], "n7");
        assert!(events[0].payload.get("label").is_none());
    }

    #[test]
    fn the_hook_is_declared_once() {
        let mock = MockAdapterBuilder::new("mock").build();
        let bus: Arc<dyn HostEventBus> = Arc::new(InMemoryHostBus::new(1));
        let adapter = action_event_adapter(Box::new(mock), "inst", bus);
        let hooks = adapter.hooks();
        assert_eq!(
            hooks.iter().filter(|h| **h == HOOK_ACTION_INVOKED).count(),
            1
        );
    }
}
