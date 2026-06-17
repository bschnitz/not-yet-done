//! Generic edit session for any `InputSpec::Editor` action a `Node` exposes.
//!
//! Replaces the per-action sessions (`JiraIssueEditSession`,
//! `ContentChildCreateSession`): the session is now a thin shim over the
//! node's own `prepare`/`execute` pair, parameterized by `action_id`.

use std::sync::Arc;

use async_trait::async_trait;
use not_yet_done_content::{
    ActionInput, ActionOutcome, ContentAdapter, ContentError, InputSpec,
};

use crate::views::content_view::PaneId;

use super::{CommitOutcome, EditSession, FollowUp, SessionScope};

/// Optional context for actions whose `ActionOutcome::Navigate` should
/// reload a drill-down view (e.g. add-comment).
pub struct NavContext {
    pub view_index: usize,
    pub parent_node_id: String,
    pub child_node_type: String,
}

/// Identifies the originating pane so the editor's `Done` outcome can
/// reload the same content pane that opened the editor.
#[derive(Clone, Copy)]
pub struct ReloadTarget {
    pub view_index: usize,
    pub pane_id: PaneId,
}

pub struct NodeActionEditSession {
    adapter: Arc<dyn ContentAdapter>,
    node_id: String,
    action_id: String,
    template: String,
    version: String,
    suffix: String,
    label: String,
    nav: Option<NavContext>,
    reload: Option<ReloadTarget>,
    /// Editor profile (from the view config's action `editor:` field) the
    /// App resolves against `editors:` when spawning. `None` → `default`.
    editor_profile: Option<String>,
    /// When true, each editor save (`:w`) applies the action instead of
    /// only the final close. The first save runs the configured action;
    /// if that action creates a new node (`ActionOutcome::Navigate`), the
    /// session retargets that node's editor action so later saves edit the
    /// just-created node in place. See [`EditSession::live_apply`].
    commit_on_save: bool,
    /// Text of the last buffer successfully applied (sent or edited).
    /// Initialised to the opening template so an unchanged first save is a
    /// no-op; updated on every successful apply so repeated saves with no
    /// change — and the final close — never re-send. Only consulted while
    /// [`Self::commit_on_save`] is set.
    last_applied: Option<String>,
}

impl NodeActionEditSession {
    /// Fetch the node, render its prepare template, and return a ready
    /// session. Errors during fetch/prepare surface here so the App can
    /// notify and bail before opening `$EDITOR`.
    pub async fn new(
        adapter: Arc<dyn ContentAdapter>,
        node_id: String,
        action_id: String,
        label: String,
        nav: Option<NavContext>,
        reload: Option<ReloadTarget>,
        editor_profile: Option<String>,
        commit_on_save: bool,
    ) -> Result<Self, ContentError> {
        let node = adapter.get_by_id(&node_id).await?;
        if !node.actions().iter().any(|a| a.id == action_id) {
            return Err(ContentError::NotSupported(format!(
                "action `{action_id}` not available on this node"
            )));
        }
        let prep = node.prepare(&action_id).await?;
        Ok(Self {
            adapter,
            node_id,
            action_id,
            last_applied: Some(prep.template.clone()),
            template: prep.template,
            version: prep.version,
            suffix: prep.suffix,
            label,
            nav,
            reload,
            editor_profile,
            commit_on_save,
        })
    }
}

#[async_trait]
impl EditSession for NodeActionEditSession {
    fn template(&self) -> &str {
        &self.template
    }

    fn suffix(&self) -> &str {
        &self.suffix
    }

    fn editor_profile(&self) -> Option<&str> {
        self.editor_profile.as_deref()
    }

    fn scope(&self) -> SessionScope {
        SessionScope::Content
    }

    fn label(&self) -> &str {
        &self.label
    }

    async fn commit(&mut self, text: &str) -> CommitOutcome {
        // Live-apply / close after nothing changed since the last apply
        // (including repeated `:w`) must not re-send. `last_applied` starts
        // at the opening template, so an unchanged buffer is caught here.
        if self.commit_on_save && self.last_applied.as_deref() == Some(text) {
            return CommitOutcome::Cancelled { message: None };
        }

        let mut node = match self.adapter.get_by_id(&self.node_id).await {
            Ok(n) => n,
            Err(e) => {
                return CommitOutcome::Cancelled {
                    message: Some(format!("Failed to fetch {}: {e}", self.node_id)),
                };
            }
        };

        let input = ActionInput::Edited {
            text: text.to_string(),
            original: self.template.clone(),
            version: self.version.clone(),
        };

        match node.execute(&self.action_id, input).await {
            Ok(ActionOutcome::Done { message }) => {
                self.mark_applied(text);
                let msg = message.unwrap_or_else(|| format!("{} updated", self.node_id));
                self.done_with_row_patch(msg)
            }
            Ok(ActionOutcome::Reopen { content, new_version }) => {
                if let Some(v) = new_version {
                    self.version = v;
                }
                CommitOutcome::Reopen { content }
            }
            Ok(ActionOutcome::NoChanges) => {
                // Record the buffer so a follow-up identical save is a no-op
                // rather than another round-trip to the backend.
                self.mark_applied(text);
                CommitOutcome::Cancelled {
                    message: Some("No changes".into()),
                }
            }
            Ok(ActionOutcome::Navigate { node_id: new_id, node_type }) => {
                self.mark_applied(text);
                // commit_on_save: the action just created a node; retarget so
                // later saves edit it in place instead of creating again.
                if self.commit_on_save {
                    self.retarget_to_created(new_id.clone()).await;
                }
                // A create always carries both `nav` (parent + child type)
                // and `reload` (origin pane). Splice the new child in place
                // rather than full-reloading: the App decides tree-local
                // insert vs. drill-refresh per pane kind.
                match (self.nav.as_ref(), self.reload) {
                    (Some(ctx), Some(target)) => {
                        CommitOutcome::FollowUp(FollowUp::InsertContentChild {
                            view_index: ctx.view_index,
                            pane_id: target.pane_id,
                            parent_node_id: ctx.parent_node_id.clone(),
                            child_node_type: ctx.child_node_type.clone(),
                            message: format!(
                                "Created {} on {}",
                                node_type.display_name, ctx.parent_node_id
                            ),
                        })
                    }
                    _ => self.done_with_row_patch(format!("Created {new_id}")),
                }
            }
            Err(e) => CommitOutcome::Cancelled {
                message: Some(format!("Failed to execute {}: {e}", self.action_id)),
            },
        }
    }

    async fn live_apply(&mut self, text: &str) -> Option<FollowUp> {
        if !self.commit_on_save {
            return None;
        }
        // `commit` runs the action and updates internal state (mark_applied,
        // create→edit retarget). Map its outcome onto the live follow-up
        // channel; a mid-edit `Reopen` (validation error) can't reopen the
        // already-open editor, so it is swallowed until the final close.
        match self.commit(text).await {
            CommitOutcome::FollowUp(fu) => Some(fu),
            CommitOutcome::Done { .. }
            | CommitOutcome::Cancelled { .. }
            | CommitOutcome::Reopen { .. } => None,
        }
    }
}

impl NodeActionEditSession {
    /// Record `text` as the last successfully applied buffer and keep it as
    /// the `original` for the next edit's 3-way merge. Stops a follow-up
    /// identical save — or the final close — from re-running the action.
    fn mark_applied(&mut self, text: &str) {
        self.last_applied = Some(text.to_string());
        self.template = text.to_string();
    }

    /// After a `commit_on_save` create returned `new_id`, point this session
    /// at the new node's editor action so subsequent saves edit it in place.
    /// Best effort: if the node can't be fetched or exposes no
    /// `InputSpec::Editor` action, the session stays on the create action —
    /// the no-op guard (via [`Self::mark_applied`]) still prevents a second
    /// send for an unchanged buffer.
    async fn retarget_to_created(&mut self, new_id: String) {
        let Ok(node) = self.adapter.get_by_id(&new_id).await else {
            return;
        };
        let Some(edit_id) = node
            .actions()
            .iter()
            .find(|a| matches!(a.input, InputSpec::Editor))
            .map(|a| a.id.clone())
        else {
            return;
        };
        self.node_id = new_id;
        self.action_id = edit_id;
        if let Ok(prep) = node.prepare(&self.action_id).await {
            self.version = prep.version;
        }
    }

    /// Turn a successful `Done` (an in-place edit) into a single-row patch
    /// follow-up when the session knows its originating pane — the editor
    /// changed one node's content, so only that row needs refreshing (no
    /// full reload, which is reserved for external changes). Falls back to a
    /// plain `Done` for callers without a pane (tests, future non-pane
    /// invocations) so they still terminate cleanly.
    fn done_with_row_patch(&self, message: String) -> CommitOutcome {
        match self.reload {
            Some(target) => CommitOutcome::FollowUp(FollowUp::PatchContentRow {
                view_index: target.view_index,
                pane_id: target.pane_id,
                node_id: self.node_id.clone(),
                message,
            }),
            None => CommitOutcome::Done { message: Some(message) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::{
        AdapterCapabilities, EditorPrep, Metadata, Node, NodeAction, NodeType,
        Result as ContentResult,
    };
    use std::sync::Mutex;

    fn ntype(id: &str) -> NodeType {
        NodeType {
            type_id: id.into(),
            mime_type: "text/markdown".into(),
            syntax: Some("markdown".into()),
            file_extension: ".md".into(),
            display_name: id.into(),
        }
    }

    /// Shared backend log so a test can assert how many sends vs. edits the
    /// (re-fetched) nodes performed.
    #[derive(Default)]
    struct Backend {
        sends: Vec<String>,
        edits: Vec<String>,
    }

    /// `id == "channel"` → a create node (`send_message`, returns Navigate
    /// to "msg"); `id == "msg"` → an edit node (`edit_message`, returns
    /// Done). Both share one [`Backend`] so the test can count round-trips.
    struct MockAdapter {
        backend: Arc<Mutex<Backend>>,
        meta: Metadata,
    }

    struct MockNode {
        id: String,
        is_channel: bool,
        node_type: NodeType,
        meta: Metadata,
        backend: Arc<Mutex<Backend>>,
    }

    #[async_trait]
    impl ContentAdapter for MockAdapter {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn adapter_type(&self) -> &str {
            "mock"
        }
        fn instance_id(&self) -> &str {
            "mock"
        }
        fn capabilities(&self) -> AdapterCapabilities {
            AdapterCapabilities::default()
        }
        async fn root(&self) -> ContentResult<Box<dyn Node>> {
            self.get_by_id("channel").await
        }
        async fn get_by_id(&self, id: &str) -> ContentResult<Box<dyn Node>> {
            Ok(Box::new(MockNode {
                id: id.to_string(),
                is_channel: id == "channel",
                node_type: ntype(if id == "channel" { "mock:channel" } else { "mock:message" }),
                meta: self.meta.clone(),
                backend: Arc::clone(&self.backend),
            }))
        }
    }

    #[async_trait]
    impl Node for MockNode {
        fn id(&self) -> &str {
            &self.id
        }
        fn label(&self) -> &str {
            &self.id
        }
        fn node_type(&self) -> &NodeType {
            &self.node_type
        }
        fn metadata(&self) -> &Metadata {
            &self.meta
        }
        fn actions(&self) -> Vec<NodeAction> {
            if self.is_channel {
                vec![NodeAction::new("send_message", "send", InputSpec::Editor)]
            } else {
                vec![
                    NodeAction::new("edit_message", "edit", InputSpec::Editor),
                    NodeAction::new("delete_message", "delete", InputSpec::None),
                ]
            }
        }
        async fn prepare(&self, _action_id: &str) -> ContentResult<EditorPrep> {
            Ok(EditorPrep {
                template: String::new(),
                version: "v1".into(),
                suffix: ".md".into(),
            })
        }
        async fn execute(
            &mut self,
            action_id: &str,
            input: ActionInput,
        ) -> ContentResult<ActionOutcome> {
            let ActionInput::Edited { text, .. } = input else {
                unreachable!("mock only handles editor input");
            };
            match action_id {
                "send_message" => {
                    self.backend.lock().unwrap().sends.push(text);
                    Ok(ActionOutcome::Navigate {
                        node_id: "msg".into(),
                        node_type: ntype("mock:message"),
                    })
                }
                "edit_message" => {
                    self.backend.lock().unwrap().edits.push(text);
                    Ok(ActionOutcome::Done { message: None })
                }
                other => unreachable!("unexpected action {other}"),
            }
        }
    }

    async fn session(commit_on_save: bool) -> (NodeActionEditSession, Arc<Mutex<Backend>>) {
        let backend = Arc::new(Mutex::new(Backend::default()));
        let adapter = Arc::new(MockAdapter {
            backend: Arc::clone(&backend),
            meta: Metadata::default(),
        });
        let s = NodeActionEditSession::new(
            adapter,
            "channel".into(),
            "send_message".into(),
            "new".into(),
            None,
            None,
            None,
            commit_on_save,
        )
        .await
        .expect("session builds");
        (s, backend)
    }

    /// First `:w` sends; the session then retargets the created message, so a
    /// later changed `:w` edits in place. Unchanged saves (and the close) are
    /// no-ops, so nothing is ever sent twice.
    #[tokio::test]
    async fn first_save_sends_then_subsequent_saves_edit() {
        let (mut s, backend) = session(true).await;

        // Empty first save equals the template → no-op.
        assert!(s.live_apply("").await.is_none());
        assert_eq!(backend.lock().unwrap().sends.len(), 0);

        // First real save sends and retargets to the created message.
        s.live_apply("hello").await;
        assert_eq!(backend.lock().unwrap().sends, vec!["hello".to_string()]);
        assert_eq!(s.action_id, "edit_message");
        assert_eq!(s.node_id, "msg");

        // Identical save → no edit round-trip.
        assert!(s.live_apply("hello").await.is_none());
        assert_eq!(backend.lock().unwrap().edits.len(), 0);

        // Changed save → one edit.
        s.live_apply("hello world").await;
        assert_eq!(backend.lock().unwrap().edits, vec!["hello world".to_string()]);

        // Close with no change since the last apply → still one send, one edit.
        s.commit("hello world").await;
        let b = backend.lock().unwrap();
        assert_eq!(b.sends.len(), 1);
        assert_eq!(b.edits.len(), 1);
    }

    /// A create (`ActionOutcome::Navigate`) carrying both `nav` and `reload`
    /// asks the App to splice the new child in place (no full reload), with
    /// the parent id + child type + origin pane threaded through. This is the
    /// `add`/`A` path; the App then chooses tree-local insert vs. drill-refresh.
    #[tokio::test]
    async fn create_with_nav_and_reload_yields_insert_content_child() {
        let backend = Arc::new(Mutex::new(Backend::default()));
        let adapter = Arc::new(MockAdapter {
            backend: Arc::clone(&backend),
            meta: Metadata::default(),
        });
        let mut s = NodeActionEditSession::new(
            adapter,
            "channel".into(),
            "send_message".into(),
            "add".into(),
            Some(NavContext {
                view_index: 2,
                parent_node_id: "channel".into(),
                child_node_type: "mock:message".into(),
            }),
            Some(ReloadTarget { view_index: 2, pane_id: 7 }),
            None,
            false,
        )
        .await
        .expect("session builds");

        match s.commit("hello").await {
            CommitOutcome::FollowUp(FollowUp::InsertContentChild {
                view_index,
                pane_id,
                parent_node_id,
                child_node_type,
                ..
            }) => {
                assert_eq!(view_index, 2);
                assert_eq!(pane_id, 7);
                assert_eq!(parent_node_id, "channel");
                assert_eq!(child_node_type, "mock:message");
            }
            _ => panic!("expected InsertContentChild follow-up"),
        }
        assert_eq!(backend.lock().unwrap().sends, vec!["hello".to_string()]);
    }

    /// Without `commit_on_save`, intermediate saves stay no-ops (the legacy
    /// commit-on-close behaviour every other adapter relies on).
    #[tokio::test]
    async fn without_flag_live_apply_is_inert() {
        let (mut s, backend) = session(false).await;
        assert!(s.live_apply("hello").await.is_none());
        assert_eq!(backend.lock().unwrap().sends.len(), 0);
        // Closing still commits once.
        s.commit("hello").await;
        assert_eq!(backend.lock().unwrap().sends, vec!["hello".to_string()]);
    }
}
