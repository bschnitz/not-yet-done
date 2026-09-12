//! Action aliases: a per-instance name for an adapter action plus a fixed set
//! of arguments, usable wherever the action itself is.
//!
//! # Why
//!
//! Arguments hang on a *binding* (`args:` on a key), so an action that should
//! always run with the same arguments has to repeat them on every key that
//! fires it — five times for `toggle-tracking`, across two view files, and a
//! forgotten one is not an error but a key that quietly runs the bare action.
//! Putting the arguments into the adapter's own config would fix that, but it
//! would also change the bare action for every caller, making the choice
//! global when it is not: the user wants `toggle-tracking` unchanged *and* a
//! grouped variant next to it.
//!
//! An alias is that variant. Declared once on the adapter instance:
//!
//! ```yaml
//! adapter:
//!   type: tasks
//!   aliases:
//!     my-toggle-tracking:
//!       action: toggle-tracking
//!       args: { group_paths: ["^/Work", "^/Other/Group"] }
//!       label: track (grouped)   # optional, defaults to the action's label
//! ```
//!
//! `my-toggle-tracking` then behaves like a real action of the adapter: it
//! is listed by [`ContentAdapter::actions_for_type`] on every level that
//! offers `toggle-tracking`, so key bindings, hooks, the CLI's `do`, `help`
//! and the keymap dump all see it without knowing about aliases. Levels that
//! lack the target lack the alias too — an alias is exactly as local as the
//! action it stands for.
//!
//! # Semantics
//!
//! - **Arguments are defaults.** The alias's `args:` are laid under whatever
//!   the invocation supplies; a binding may still pass `args:` to refine an
//!   alias. In the listing, the alias's values appear as the parameters'
//!   defaults, so a frontend resolving arguments against the declared
//!   parameters fills them in by itself.
//! - **Validated against the target.** The merged set goes through
//!   [`resolve_args`] with the target's parameters: an alias naming an
//!   argument the action does not declare, or of the wrong type, is refused
//!   at invocation with a message that names the alias.
//! - **A real action wins.** An alias whose name a level already uses as an
//!   action id is not listed there and never rewrites that call.
//! - **No chains.** An alias points at an adapter action, never at another
//!   alias — one lookup, one meaning.
//! - **Argument-less paths refuse.** `execute_addressed`, `collection_prepare`
//!   and `execute_collection` carry no arguments; an alias *with* arguments is
//!   refused there rather than run stripped of what makes it the alias.
//!
//! # Where it sits
//!
//! Aliases are per instance, so — unlike the factory-level decorators in
//! `host::factories()` — the wrapping happens where the instance block is
//! known: `host::decorate_instance`, called by every frontend that builds an
//! adapter from a view file. [`AliasingAdapter`] is the outermost decorator.

use crate::decorate::{AdapterDecorator, NodeDecorator};
use crate::*;
use async_trait::async_trait;
use std::sync::Arc;

/// One `aliases:` entry as written in the instance block. The alias's own
/// name is the mapping key.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct AliasSpec {
    /// The adapter action the alias stands for.
    #[serde(default)]
    pub action: String,
    /// Arguments laid under the invocation's own — see the module docs.
    #[serde(default)]
    pub args: ActionArgs,
    /// Label shown for the alias (action bar, which-key, `help`). Defaults
    /// to the target action's label.
    #[serde(default)]
    pub label: Option<String>,
}

/// A validated alias.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionAlias {
    pub id: String,
    pub action: String,
    pub args: ActionArgs,
    pub label: Option<String>,
}

/// The aliases of one adapter instance, validated once at construction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AliasTable {
    aliases: Vec<ActionAlias>,
}

impl AliasTable {
    /// Validate the declared aliases. Refuses an empty name, an empty or
    /// self-referencing `action:`, and an alias pointing at another alias.
    pub fn new(
        specs: impl IntoIterator<Item = (String, AliasSpec)>,
    ) -> std::result::Result<Self, String> {
        let mut aliases: Vec<ActionAlias> = Vec::new();
        for (id, spec) in specs {
            let id = id.trim().to_string();
            if id.is_empty() {
                return Err("an alias name must not be empty".into());
            }
            let action = spec.action.trim().to_string();
            if action.is_empty() {
                return Err(format!(
                    "alias '{id}': `action:` must name the adapter action it stands for"
                ));
            }
            if action == id {
                return Err(format!("alias '{id}' points at itself"));
            }
            if aliases.iter().any(|a| a.id == id) {
                return Err(format!("alias '{id}' is declared twice"));
            }
            aliases.push(ActionAlias {
                id,
                action,
                args: spec.args,
                label: spec.label,
            });
        }
        if let Some(chained) = aliases
            .iter()
            .find(|a| aliases.iter().any(|b| b.id == a.action))
        {
            return Err(format!(
                "alias '{}' points at alias '{}'; name the adapter action instead",
                chained.id, chained.action
            ));
        }
        Ok(Self { aliases })
    }

    pub fn is_empty(&self) -> bool {
        self.aliases.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &ActionAlias> {
        self.aliases.iter()
    }

    /// The alias called `id`, if any.
    pub fn get(&self, id: &str) -> Option<&ActionAlias> {
        self.aliases.iter().find(|a| a.id == id)
    }

    /// Append to a level's action list the aliases whose target it offers,
    /// each a copy of the target under the alias's name and label, with the
    /// alias's arguments as the parameters' defaults. A name the level already
    /// uses for a real action is left alone.
    pub fn expand(&self, mut actions: Vec<NodeAction>) -> Vec<NodeAction> {
        let mut added = Vec::new();
        for alias in &self.aliases {
            if actions.iter().any(|a| a.id == alias.id) {
                continue;
            }
            let Some(target) = actions.iter().find(|a| a.id == alias.action) else {
                continue;
            };
            let mut action = target.clone();
            action.id = alias.id.clone();
            if let Some(label) = &alias.label {
                action.label = label.clone();
            }
            for param in action.params.iter_mut() {
                if let Some(value) = alias.args.get(&param.key) {
                    param.default = Some(value.clone());
                }
            }
            added.push(action);
        }
        actions.extend(added);
        actions
    }

    /// What invoking `name` on a level offering `actions` means: `None` when
    /// it is a real action (or no alias at all — the inner adapter answers),
    /// otherwise the alias and the target action it resolves to.
    fn resolve(
        &self,
        actions: &[NodeAction],
        name: &str,
    ) -> Result<Option<(&ActionAlias, NodeAction)>> {
        if actions.iter().any(|a| a.id == name) {
            return Ok(None);
        }
        let Some(alias) = self.get(name) else {
            return Ok(None);
        };
        let Some(target) = actions.iter().find(|a| a.id == alias.action) else {
            return Err(ContentError::NotSupported(format!(
                "alias '{name}' stands for '{}', which this level does not offer",
                alias.action
            )));
        };
        Ok(Some((alias, target.clone())))
    }
}

/// The alias's arguments under the invocation's, validated against the
/// target's declared parameters.
fn merged_args(
    alias: &ActionAlias,
    target: &NodeAction,
    supplied: &ActionArgs,
) -> Result<ActionArgs> {
    let mut args = alias.args.clone();
    args.merge(supplied);
    resolve_args(&target.params, &args).map_err(|problems| {
        ContentError::Other(
            format!("alias '{}': {}", alias.id, describe_problems(&problems)).into(),
        )
    })
}

/// An alias with arguments cannot travel a path that carries none.
fn refuse_args(alias: &ActionAlias, path: &str) -> Result<()> {
    if alias.args.is_empty() {
        Ok(())
    } else {
        Err(ContentError::NotSupported(format!(
            "alias '{}' carries arguments, which {path} cannot deliver; invoke it on the node",
            alias.id
        )))
    }
}

/// Wrap `inner` so that `table`'s aliases are listed and resolved on every
/// level. A table without aliases returns `inner` untouched.
pub fn aliasing_adapter(
    inner: Box<dyn ContentAdapter>,
    table: AliasTable,
) -> Box<dyn ContentAdapter> {
    if table.is_empty() {
        return inner;
    }
    Box::new(AliasingAdapter {
        inner: Arc::from(inner),
        table: Arc::new(table),
    })
}

/// The adapter decorator: extends the action lists and rewrites the
/// adapter-level invocation paths. Nodes it hands out are [`AliasingNode`]s.
pub struct AliasingAdapter {
    inner: Arc<dyn ContentAdapter>,
    table: Arc<AliasTable>,
}

impl AliasingAdapter {
    fn wrap(&self, node: Box<dyn Node>) -> Box<dyn Node> {
        Box::new(AliasingNode {
            inner: node,
            adapter: Arc::clone(&self.inner),
            table: Arc::clone(&self.table),
        })
    }
}

#[async_trait]
impl AdapterDecorator for AliasingAdapter {
    fn inner(&self) -> &dyn ContentAdapter {
        &*self.inner
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        Ok(self.wrap(self.inner.root().await?))
    }
    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        Ok(self.wrap(self.inner.get_by_id(id).await?))
    }

    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        self.table.expand(self.inner.actions_for_type(node_type))
    }
    fn collection_actions(&self, node_type: &NodeType) -> Vec<NodeAction> {
        self.table.expand(self.inner.collection_actions(node_type))
    }

    async fn execute_addressed(
        &self,
        node_type: &NodeType,
        id: &str,
        action_id: &str,
        input: ActionInput,
    ) -> Result<ActionOutcome> {
        let actions = self.inner.actions_for_type(node_type);
        let action_id = match self.table.resolve(&actions, action_id)? {
            None => action_id,
            Some((alias, _)) => {
                refuse_args(alias, "an addressed execution")?;
                alias.action.as_str()
            }
        };
        self.inner
            .execute_addressed(node_type, id, action_id, input)
            .await
    }
    async fn collection_prepare(
        &self,
        node_type: &NodeType,
        action_id: &str,
    ) -> Result<EditorPrep> {
        let actions = self.inner.collection_actions(node_type);
        let action_id = match self.table.resolve(&actions, action_id)? {
            None => action_id,
            Some((alias, _)) => {
                refuse_args(alias, "a collection action")?;
                alias.action.as_str()
            }
        };
        self.inner.collection_prepare(node_type, action_id).await
    }
    async fn execute_collection(
        &self,
        node_type: &NodeType,
        action_id: &str,
        input: ActionInput,
    ) -> Result<ActionOutcome> {
        let actions = self.inner.collection_actions(node_type);
        let action_id = match self.table.resolve(&actions, action_id)? {
            None => action_id,
            Some((alias, _)) => {
                refuse_args(alias, "a collection action")?;
                alias.action.as_str()
            }
        };
        self.inner
            .execute_collection(node_type, action_id, input)
            .await
    }
}

/// The node decorator: rewrites the node-level invocation paths. Needs the
/// inner adapter to learn which actions its level really offers.
pub struct AliasingNode {
    inner: Box<dyn Node>,
    adapter: Arc<dyn ContentAdapter>,
    table: Arc<AliasTable>,
}

impl AliasingNode {
    /// The alias `name` resolves to on this node's level, with the arguments
    /// it should run with — or `None` for a real action.
    fn resolve(&self, name: &str, supplied: &ActionArgs) -> Result<Option<(String, ActionArgs)>> {
        let actions = self.adapter.actions_for_type(self.inner.node_type());
        match self.table.resolve(&actions, name)? {
            None => Ok(None),
            Some((alias, target)) => {
                let args = merged_args(alias, &target, supplied)?;
                Ok(Some((alias.action.clone(), args)))
            }
        }
    }

    /// Name rewrite only, for the paths that carry no arguments of their
    /// own but precede an `execute` that does.
    fn resolve_name(&self, name: &str) -> Result<String> {
        let actions = self.adapter.actions_for_type(self.inner.node_type());
        Ok(match self.table.resolve(&actions, name)? {
            None => name.to_string(),
            Some((alias, _)) => alias.action.clone(),
        })
    }
}

#[async_trait]
impl NodeDecorator for AliasingNode {
    fn inner(&self) -> &dyn Node {
        &*self.inner
    }
    fn inner_mut(&mut self) -> &mut dyn Node {
        &mut *self.inner
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        Ok(Box::new(AliasingNode {
            inner: self.inner.get_child(id).await?,
            adapter: Arc::clone(&self.adapter),
            table: Arc::clone(&self.table),
        }))
    }

    async fn invoke_action(&self, name: &str, ctx: &ActionContext) -> Result<ActionDispatch> {
        match self.resolve(name, &ctx.args)? {
            None => self.inner.invoke_action(name, ctx).await,
            Some((target, args)) => {
                let ctx = ActionContext {
                    args,
                    ..ctx.clone()
                };
                self.inner.invoke_action(&target, &ctx).await
            }
        }
    }
    async fn prepare(&self, action_id: &str, args: &ActionArgs) -> Result<EditorPrep> {
        match self.resolve(action_id, args)? {
            None => self.inner.prepare(action_id, args).await,
            Some((target, args)) => self.inner.prepare(&target, &args).await,
        }
    }
    async fn picker_options(&self, action_id: &str) -> Result<Vec<ActionOption>> {
        let target = self.resolve_name(action_id)?;
        self.inner.picker_options(&target).await
    }
    async fn form_prep(
        &self,
        action_id: &str,
    ) -> Result<std::collections::HashMap<String, String>> {
        let target = self.resolve_name(action_id)?;
        self.inner.form_prep(&target).await
    }
    async fn execute(
        &mut self,
        action_id: &str,
        input: ActionInput,
        args: &ActionArgs,
    ) -> Result<ActionOutcome> {
        match self.resolve(action_id, args)? {
            None => self.inner.execute(action_id, input, args).await,
            Some((target, args)) => self.inner.execute(&target, input, &args).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{MockAdapterBuilder, MockNodeData, issue_type};

    /// An adapter whose nodes report back what they were invoked with.
    struct Recorder {
        mock: crate::mock::MockAdapter,
    }

    struct RecordingNode {
        inner: Box<dyn Node>,
    }

    #[async_trait]
    impl NodeDecorator for RecordingNode {
        fn inner(&self) -> &dyn Node {
            &*self.inner
        }
        fn inner_mut(&mut self) -> &mut dyn Node {
            &mut *self.inner
        }
        async fn invoke_action(&self, name: &str, ctx: &ActionContext) -> Result<ActionDispatch> {
            Ok(ActionDispatch::Notify {
                message: format!("{name} {:?}", ctx.args),
            })
        }
        async fn prepare(&self, action_id: &str, args: &ActionArgs) -> Result<EditorPrep> {
            Ok(EditorPrep {
                template: format!("{action_id} {args:?}"),
                ..Default::default()
            })
        }
        async fn execute(
            &mut self,
            action_id: &str,
            _input: ActionInput,
            args: &ActionArgs,
        ) -> Result<ActionOutcome> {
            Ok(ActionOutcome::Done {
                message: Some(format!("{action_id} {args:?}")),
            })
        }
    }

    #[async_trait]
    impl AdapterDecorator for Recorder {
        fn inner(&self) -> &dyn ContentAdapter {
            &self.mock
        }
        async fn root(&self) -> Result<Box<dyn Node>> {
            Ok(Box::new(RecordingNode {
                inner: self.mock.root().await?,
            }))
        }
        async fn execute_addressed(
            &self,
            _node_type: &NodeType,
            _id: &str,
            action_id: &str,
            _input: ActionInput,
        ) -> Result<ActionOutcome> {
            Ok(ActionOutcome::Done {
                message: Some(action_id.to_string()),
            })
        }
    }

    fn actions() -> Vec<NodeAction> {
        vec![
            NodeAction::new("toggle-tracking", "track", InputSpec::None)
                .param(ParamSpec::list("group_paths", "path regexes"))
                .param(ParamSpec::int("limit", "rows").with_default(50i64)),
            NodeAction::new("plain", "plain", InputSpec::None),
        ]
    }

    fn table(entries: &[(&str, &str, ActionArgs)]) -> AliasTable {
        AliasTable::new(entries.iter().map(|(id, action, args)| {
            (
                id.to_string(),
                AliasSpec {
                    action: action.to_string(),
                    args: args.clone(),
                    label: None,
                },
            )
        }))
        .expect("valid table")
    }

    fn adapter(table: AliasTable) -> Box<dyn ContentAdapter> {
        let mock = MockAdapterBuilder::new("mock")
            .node(MockNodeData::new("root", "Root").node_type(issue_type()))
            .actions_for(issue_type().type_id, actions())
            .build();
        aliasing_adapter(Box::new(Recorder { mock }), table)
    }

    fn grouped() -> ActionArgs {
        ActionArgs::new().with("group_paths", ArgValue::List(vec!["^/Work".into()]))
    }

    #[test]
    fn listing_adds_the_alias_with_its_arguments_as_defaults() {
        let adapter = adapter(table(&[("my-toggle", "toggle-tracking", grouped())]));
        let listed = adapter.actions_for_type(&issue_type());
        let ids: Vec<&str> = listed.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["toggle-tracking", "plain", "my-toggle"]);
        let alias = listed.iter().find(|a| a.id == "my-toggle").unwrap();
        assert_eq!(alias.label, "track", "label falls back to the target's");
        let group = alias
            .params
            .iter()
            .find(|p| p.key == "group_paths")
            .unwrap();
        assert_eq!(group.default, Some(ArgValue::List(vec!["^/Work".into()])));
        let limit = alias.params.iter().find(|p| p.key == "limit").unwrap();
        assert_eq!(
            limit.default,
            Some(ArgValue::Int(50)),
            "untouched parameter keeps its default"
        );
    }

    #[test]
    fn listing_skips_an_alias_whose_target_the_level_lacks() {
        let adapter = adapter(table(&[("my-x", "absent", ActionArgs::new())]));
        let ids: Vec<String> = adapter
            .actions_for_type(&issue_type())
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(ids, ["toggle-tracking", "plain"]);
    }

    #[test]
    fn a_real_action_of_the_same_name_wins() {
        let adapter = adapter(table(&[("plain", "toggle-tracking", grouped())]));
        let listed = adapter.actions_for_type(&issue_type());
        assert_eq!(listed.iter().filter(|a| a.id == "plain").count(), 1);
        assert!(
            listed
                .iter()
                .find(|a| a.id == "plain")
                .unwrap()
                .params
                .is_empty()
        );
    }

    #[tokio::test]
    async fn invoking_the_alias_runs_the_target_with_the_merged_arguments() {
        let adapter = adapter(table(&[("my-toggle", "toggle-tracking", grouped())]));
        let root = adapter.root().await.unwrap();
        let ctx = ActionContext::default();
        let ActionDispatch::Notify { message } =
            root.invoke_action("my-toggle", &ctx).await.unwrap()
        else {
            panic!("expected the recorder's notify");
        };
        assert!(message.starts_with("toggle-tracking "), "{message}");
        assert!(message.contains("group_paths"), "{message}");
        assert!(
            message.contains("limit"),
            "declared defaults are filled: {message}"
        );
    }

    #[tokio::test]
    async fn the_invocation_overrides_the_alias() {
        let adapter = adapter(table(&[("my-toggle", "toggle-tracking", grouped())]));
        let root = adapter.root().await.unwrap();
        let ctx = ActionContext {
            args: ActionArgs::new().with("group_paths", ArgValue::List(vec!["^/Other".into()])),
            ..Default::default()
        };
        let ActionDispatch::Notify { message } =
            root.invoke_action("my-toggle", &ctx).await.unwrap()
        else {
            panic!("expected the recorder's notify");
        };
        assert!(
            message.contains("^/Other") && !message.contains("^/Work"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn a_real_action_passes_through_untouched() {
        let adapter = adapter(table(&[("my-toggle", "toggle-tracking", grouped())]));
        let root = adapter.root().await.unwrap();
        let ctx = ActionContext::default();
        let ActionDispatch::Notify { message } = root.invoke_action("plain", &ctx).await.unwrap()
        else {
            panic!("expected the recorder's notify");
        };
        assert_eq!(message, "plain ActionArgs([])");
    }

    #[tokio::test]
    async fn an_undeclared_alias_argument_is_refused_by_name() {
        let bad = ActionArgs::new().with("nope", ArgValue::Text("x".into()));
        let adapter = adapter(table(&[("my-toggle", "toggle-tracking", bad)]));
        let root = adapter.root().await.unwrap();
        let err = root
            .invoke_action("my-toggle", &ActionContext::default())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("alias 'my-toggle'") && err.contains("unknown argument 'nope'"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn prepare_and_execute_are_rewritten_too() {
        let adapter = adapter(table(&[("my-toggle", "toggle-tracking", grouped())]));
        let mut root = adapter.root().await.unwrap();
        let prep = root.prepare("my-toggle", &ActionArgs::new()).await.unwrap();
        assert!(prep.template.starts_with("toggle-tracking ") && prep.template.contains("^/Work"));
        let outcome = root
            .execute("my-toggle", ActionInput::None, &ActionArgs::new())
            .await;
        let Ok(ActionOutcome::Done {
            message: Some(message),
        }) = outcome
        else {
            panic!("expected done");
        };
        assert!(message.starts_with("toggle-tracking ") && message.contains("^/Work"));
    }

    #[tokio::test]
    async fn the_addressed_path_rewrites_an_argument_less_alias_and_refuses_one_with_arguments() {
        let adapter = adapter(table(&[
            ("bare", "plain", ActionArgs::new()),
            ("my-toggle", "toggle-tracking", grouped()),
        ]));
        let outcome = adapter
            .execute_addressed(&issue_type(), "root", "bare", ActionInput::None)
            .await;
        let Ok(ActionOutcome::Done { message }) = outcome else {
            panic!("expected done");
        };
        assert_eq!(message.as_deref(), Some("plain"));
        let Err(err) = adapter
            .execute_addressed(&issue_type(), "root", "my-toggle", ActionInput::None)
            .await
        else {
            panic!("an alias with arguments must be refused on the addressed path");
        };
        assert!(err.to_string().contains("carries arguments"), "{err}");
    }

    #[test]
    fn a_table_without_aliases_is_no_wrapper() {
        let mock = MockAdapterBuilder::new("mock").build();
        let adapter = aliasing_adapter(Box::new(mock), AliasTable::default());
        assert_eq!(adapter.adapter_type(), "mock");
    }

    #[test]
    fn construction_refuses_broken_declarations() {
        let spec = |action: &str| AliasSpec {
            action: action.into(),
            ..Default::default()
        };
        let err = |entries: Vec<(&str, AliasSpec)>| {
            AliasTable::new(entries.into_iter().map(|(k, v)| (k.to_string(), v))).unwrap_err()
        };
        assert!(err(vec![("", spec("x"))]).contains("must not be empty"));
        assert!(err(vec![("a", spec(""))]).contains("`action:`"));
        assert!(err(vec![("a", spec("a"))]).contains("itself"));
        assert!(err(vec![("a", spec("x")), ("b", spec("a"))]).contains("points at alias 'a'"));
    }

    #[test]
    fn the_spec_reads_from_yaml() {
        let yaml = r#"
my-toggle:
  action: toggle-tracking
  args: { group_paths: ["^/Work"] }
  label: grouped
bare:
  action: plain
"#;
        let specs: std::collections::BTreeMap<String, AliasSpec> =
            serde_yaml::from_str(yaml).unwrap();
        assert_eq!(specs["my-toggle"].action, "toggle-tracking");
        assert_eq!(specs["my-toggle"].label.as_deref(), Some("grouped"));
        assert_eq!(
            specs["my-toggle"].args.get("group_paths"),
            Some(&ArgValue::List(vec!["^/Work".into()]))
        );
        assert!(specs["bare"].args.is_empty());
    }
}
