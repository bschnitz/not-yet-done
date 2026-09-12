//! Decorator scaffolding: forward a whole [`ContentAdapter`] / [`Node`] to a
//! wrapped inner one, overriding only what a decorator actually changes.
//!
//! # Why this exists
//!
//! Every content-layer feature that applies to *all* adapters — anonymization,
//! custom columns, view-scripts, action aliases — is a decorator: it wraps the
//! real adapter and intercepts a handful of the trait's ~50 methods. Written by
//! hand, each wrapper must forward the other forty-odd methods itself, and a
//! method added to the trait later is not a compile error for a wrapper that
//! forgets it: the wrapper silently falls back to the trait's default, so the
//! feature the inner adapter implements (reminders, credential prompts,
//! per-query status) is dead behind that wrapper. That is exactly what had
//! happened to the anonymizing decorator (7 methods) before this module.
//!
//! # How it works
//!
//! [`AdapterDecorator`] mirrors every [`ContentAdapter`] method as a default
//! method that forwards to [`AdapterDecorator::inner`]. A blanket
//! `impl<T: AdapterDecorator> ContentAdapter for T` then makes any such type a
//! full adapter. A decorator implements `AdapterDecorator` — one required
//! method, `inner()` — and overrides only the methods it cares about; every
//! other method reaches the inner adapter untouched. [`NodeDecorator`] does
//! the same for [`Node`].
//!
//! The one place that must stay complete is this file: when a method is added
//! to `ContentAdapter` or `Node`, add its forwarder here **and** its arm in
//! the blanket impl. A test at the bottom compares the method lists in
//! `lib.rs` against the two blanket impls, so forgetting either fails the
//! build's test run instead of a feature in production.

use crate::*;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// A [`ContentAdapter`] that wraps another one. Implement [`Self::inner`] and
/// override whatever the decorator changes; everything else forwards.
///
/// Inside an override, call the inner adapter through `self.inner()` — never
/// through `self`, which would recurse into the blanket impl.
#[async_trait]
pub trait AdapterDecorator: Send + Sync {
    /// The wrapped adapter every non-overridden call is forwarded to.
    fn inner(&self) -> &dyn ContentAdapter;

    fn adapter_type(&self) -> &str {
        self.inner().adapter_type()
    }
    fn instance_id(&self) -> &str {
        self.inner().instance_id()
    }
    fn instance_data_dir(&self) -> PathBuf {
        self.inner().instance_data_dir()
    }
    async fn root(&self) -> Result<Box<dyn Node>> {
        self.inner().root().await
    }
    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        self.inner().get_by_id(id).await
    }
    fn childs<'a>(&'a self, node: &'a dyn Node) -> Vec<crate::children::Child<'a>> {
        self.inner().childs(node)
    }
    async fn eager_subtree(
        &self,
        node: &dyn Node,
        params: &ListParams,
        depth: u32,
    ) -> Option<Result<Subtree>> {
        self.inner().eager_subtree(node, params, depth).await
    }
    async fn download_asset(&self, url: &str) -> Result<Vec<u8>> {
        self.inner().download_asset(url).await
    }
    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        self.inner().actions_for_type(node_type)
    }
    async fn execute_addressed(
        &self,
        node_type: &NodeType,
        id: &str,
        action_id: &str,
        input: ActionInput,
    ) -> Result<ActionOutcome> {
        self.inner()
            .execute_addressed(node_type, id, action_id, input)
            .await
    }
    fn collection_actions(&self, node_type: &NodeType) -> Vec<NodeAction> {
        self.inner().collection_actions(node_type)
    }
    async fn collection_prepare(
        &self,
        node_type: &NodeType,
        action_id: &str,
    ) -> Result<EditorPrep> {
        self.inner().collection_prepare(node_type, action_id).await
    }
    async fn execute_collection(
        &self,
        node_type: &NodeType,
        action_id: &str,
        input: ActionInput,
    ) -> Result<ActionOutcome> {
        self.inner()
            .execute_collection(node_type, action_id, input)
            .await
    }
    fn child_process_env(&self, node: &NodeRef) -> HashMap<String, String> {
        self.inner().child_process_env(node)
    }
    async fn augment_editor_buffer(&self, node: &NodeRef, buffer: String) -> String {
        self.inner().augment_editor_buffer(node, buffer).await
    }
    fn strip_editor_hints(&self, text: &str) -> String {
        self.inner().strip_editor_hints(text)
    }
    fn capabilities(&self) -> AdapterCapabilities {
        self.inner().capabilities()
    }
    fn has_active_tracking(&self) -> bool {
        self.inner().has_active_tracking()
    }
    async fn list_values(&self, source: &str) -> Result<Vec<ValueOption>> {
        self.inner().list_values(source).await
    }
    fn subscribe_status(&self) -> tokio::sync::watch::Receiver<AdapterStatus> {
        self.inner().subscribe_status()
    }
    fn subscribe_status_for(
        &self,
        query: Option<&str>,
    ) -> tokio::sync::watch::Receiver<AdapterStatus> {
        self.inner().subscribe_status_for(query)
    }
    fn subscribe_invalidations(&self) -> tokio::sync::broadcast::Receiver<Invalidation> {
        self.inner().subscribe_invalidations()
    }
    async fn refresh(&self) -> Result<()> {
        self.inner().refresh().await
    }
    fn subscribe_reminders(&self) -> tokio::sync::broadcast::Receiver<Reminder> {
        self.inner().subscribe_reminders()
    }
    fn take_prompt_requests(&self) -> Option<tokio::sync::mpsc::Receiver<PromptRequest>> {
        self.inner().take_prompt_requests()
    }
    async fn live_rows(&self) -> Vec<NodeSummary> {
        self.inner().live_rows().await
    }
    async fn bucket_for_now(&self, group_by: &GroupSpec) -> Option<String> {
        self.inner().bucket_for_now(group_by).await
    }
    async fn live_group_rows(&self, group_by: &GroupSpec, query: Option<&str>) -> Vec<NodeSummary> {
        self.inner().live_group_rows(group_by, query).await
    }
    async fn revalidate(&self) {
        self.inner().revalidate().await
    }
    async fn submit_credentials(&self, fields: HashMap<String, String>) -> Result<()> {
        self.inner().submit_credentials(fields).await
    }
    async fn cancel_credentials(&self) -> Result<()> {
        self.inner().cancel_credentials().await
    }
    async fn try_refresh_session(&self) -> Result<()> {
        self.inner().try_refresh_session().await
    }
    async fn invalidate_session(&self) -> Result<()> {
        self.inner().invalidate_session().await
    }
    async fn invalidate_credentials(&self) -> Result<()> {
        self.inner().invalidate_credentials().await
    }
    async fn load_view_sort(&self, scope: &str) -> Result<Vec<SortKey>> {
        self.inner().load_view_sort(scope).await
    }
    async fn save_view_sort(&self, scope: &str, sort: &[SortKey]) -> Result<()> {
        self.inner().save_view_sort(scope, sort).await
    }
    fn query_variables(&self, query: &str) -> Vec<QueryVariable> {
        self.inner().query_variables(query)
    }
    fn render_query(&self, query: &str, vars: &HashMap<String, String>) -> String {
        self.inner().render_query(query, vars)
    }
    async fn execute_custom_query(
        &self,
        query: &str,
        context: &CustomQueryContext,
    ) -> Result<CustomQueryResult> {
        self.inner().execute_custom_query(query, context).await
    }
    fn custom_query_context(&self, node_id: &str) -> CustomQueryContext {
        self.inner().custom_query_context(node_id)
    }
    fn saved_query_store(&self) -> Option<&dyn SavedQueryStore> {
        self.inner().saved_query_store()
    }
    fn extended_query_store(&self) -> Option<Box<dyn ExtendedQueryStore>> {
        self.inner().extended_query_store()
    }
    fn query_body_suffix(&self) -> &str {
        self.inner().query_body_suffix()
    }
    fn query_language(&self) -> &str {
        self.inner().query_language()
    }
    fn script_store(&self) -> Option<&dyn ScriptStore> {
        self.inner().script_store()
    }
    async fn locate_node_path(&self, node_id: &str) -> Result<Option<Vec<String>>> {
        self.inner().locate_node_path(node_id).await
    }
    fn hooks(&self) -> Vec<&str> {
        self.inner().hooks()
    }
    fn anonymizer(&self) -> Arc<dyn anonymize::Anonymizer> {
        self.inner().anonymizer()
    }
    async fn describe_columns(&self, node_type: &str) -> Vec<ColumnSchema> {
        self.inner().describe_columns(node_type).await
    }
}

/// Every [`AdapterDecorator`] is a [`ContentAdapter`]: each method routes to
/// the decorator's (possibly overridden) forwarder. Fully qualified calls, so
/// the two same-named methods never resolve to each other by accident.
#[async_trait]
impl<T: AdapterDecorator> ContentAdapter for T {
    fn adapter_type(&self) -> &str {
        <T as AdapterDecorator>::adapter_type(self)
    }
    fn instance_id(&self) -> &str {
        <T as AdapterDecorator>::instance_id(self)
    }
    fn instance_data_dir(&self) -> PathBuf {
        <T as AdapterDecorator>::instance_data_dir(self)
    }
    async fn root(&self) -> Result<Box<dyn Node>> {
        <T as AdapterDecorator>::root(self).await
    }
    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        <T as AdapterDecorator>::get_by_id(self, id).await
    }
    fn childs<'a>(&'a self, node: &'a dyn Node) -> Vec<crate::children::Child<'a>> {
        <T as AdapterDecorator>::childs(self, node)
    }
    async fn eager_subtree(
        &self,
        node: &dyn Node,
        params: &ListParams,
        depth: u32,
    ) -> Option<Result<Subtree>> {
        <T as AdapterDecorator>::eager_subtree(self, node, params, depth).await
    }
    async fn download_asset(&self, url: &str) -> Result<Vec<u8>> {
        <T as AdapterDecorator>::download_asset(self, url).await
    }
    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        <T as AdapterDecorator>::actions_for_type(self, node_type)
    }
    async fn execute_addressed(
        &self,
        node_type: &NodeType,
        id: &str,
        action_id: &str,
        input: ActionInput,
    ) -> Result<ActionOutcome> {
        <T as AdapterDecorator>::execute_addressed(self, node_type, id, action_id, input).await
    }
    fn collection_actions(&self, node_type: &NodeType) -> Vec<NodeAction> {
        <T as AdapterDecorator>::collection_actions(self, node_type)
    }
    async fn collection_prepare(
        &self,
        node_type: &NodeType,
        action_id: &str,
    ) -> Result<EditorPrep> {
        <T as AdapterDecorator>::collection_prepare(self, node_type, action_id).await
    }
    async fn execute_collection(
        &self,
        node_type: &NodeType,
        action_id: &str,
        input: ActionInput,
    ) -> Result<ActionOutcome> {
        <T as AdapterDecorator>::execute_collection(self, node_type, action_id, input).await
    }
    fn child_process_env(&self, node: &NodeRef) -> HashMap<String, String> {
        <T as AdapterDecorator>::child_process_env(self, node)
    }
    async fn augment_editor_buffer(&self, node: &NodeRef, buffer: String) -> String {
        <T as AdapterDecorator>::augment_editor_buffer(self, node, buffer).await
    }
    fn strip_editor_hints(&self, text: &str) -> String {
        <T as AdapterDecorator>::strip_editor_hints(self, text)
    }
    fn capabilities(&self) -> AdapterCapabilities {
        <T as AdapterDecorator>::capabilities(self)
    }
    fn has_active_tracking(&self) -> bool {
        <T as AdapterDecorator>::has_active_tracking(self)
    }
    async fn list_values(&self, source: &str) -> Result<Vec<ValueOption>> {
        <T as AdapterDecorator>::list_values(self, source).await
    }
    fn subscribe_status(&self) -> tokio::sync::watch::Receiver<AdapterStatus> {
        <T as AdapterDecorator>::subscribe_status(self)
    }
    fn subscribe_status_for(
        &self,
        query: Option<&str>,
    ) -> tokio::sync::watch::Receiver<AdapterStatus> {
        <T as AdapterDecorator>::subscribe_status_for(self, query)
    }
    fn subscribe_invalidations(&self) -> tokio::sync::broadcast::Receiver<Invalidation> {
        <T as AdapterDecorator>::subscribe_invalidations(self)
    }
    async fn refresh(&self) -> Result<()> {
        <T as AdapterDecorator>::refresh(self).await
    }
    fn subscribe_reminders(&self) -> tokio::sync::broadcast::Receiver<Reminder> {
        <T as AdapterDecorator>::subscribe_reminders(self)
    }
    fn take_prompt_requests(&self) -> Option<tokio::sync::mpsc::Receiver<PromptRequest>> {
        <T as AdapterDecorator>::take_prompt_requests(self)
    }
    async fn live_rows(&self) -> Vec<NodeSummary> {
        <T as AdapterDecorator>::live_rows(self).await
    }
    async fn bucket_for_now(&self, group_by: &GroupSpec) -> Option<String> {
        <T as AdapterDecorator>::bucket_for_now(self, group_by).await
    }
    async fn live_group_rows(&self, group_by: &GroupSpec, query: Option<&str>) -> Vec<NodeSummary> {
        <T as AdapterDecorator>::live_group_rows(self, group_by, query).await
    }
    async fn revalidate(&self) {
        <T as AdapterDecorator>::revalidate(self).await
    }
    async fn submit_credentials(&self, fields: HashMap<String, String>) -> Result<()> {
        <T as AdapterDecorator>::submit_credentials(self, fields).await
    }
    async fn cancel_credentials(&self) -> Result<()> {
        <T as AdapterDecorator>::cancel_credentials(self).await
    }
    async fn try_refresh_session(&self) -> Result<()> {
        <T as AdapterDecorator>::try_refresh_session(self).await
    }
    async fn invalidate_session(&self) -> Result<()> {
        <T as AdapterDecorator>::invalidate_session(self).await
    }
    async fn invalidate_credentials(&self) -> Result<()> {
        <T as AdapterDecorator>::invalidate_credentials(self).await
    }
    async fn load_view_sort(&self, scope: &str) -> Result<Vec<SortKey>> {
        <T as AdapterDecorator>::load_view_sort(self, scope).await
    }
    async fn save_view_sort(&self, scope: &str, sort: &[SortKey]) -> Result<()> {
        <T as AdapterDecorator>::save_view_sort(self, scope, sort).await
    }
    fn query_variables(&self, query: &str) -> Vec<QueryVariable> {
        <T as AdapterDecorator>::query_variables(self, query)
    }
    fn render_query(&self, query: &str, vars: &HashMap<String, String>) -> String {
        <T as AdapterDecorator>::render_query(self, query, vars)
    }
    async fn execute_custom_query(
        &self,
        query: &str,
        context: &CustomQueryContext,
    ) -> Result<CustomQueryResult> {
        <T as AdapterDecorator>::execute_custom_query(self, query, context).await
    }
    fn custom_query_context(&self, node_id: &str) -> CustomQueryContext {
        <T as AdapterDecorator>::custom_query_context(self, node_id)
    }
    fn saved_query_store(&self) -> Option<&dyn SavedQueryStore> {
        <T as AdapterDecorator>::saved_query_store(self)
    }
    fn extended_query_store(&self) -> Option<Box<dyn ExtendedQueryStore>> {
        <T as AdapterDecorator>::extended_query_store(self)
    }
    fn query_body_suffix(&self) -> &str {
        <T as AdapterDecorator>::query_body_suffix(self)
    }
    fn query_language(&self) -> &str {
        <T as AdapterDecorator>::query_language(self)
    }
    fn script_store(&self) -> Option<&dyn ScriptStore> {
        <T as AdapterDecorator>::script_store(self)
    }
    async fn locate_node_path(&self, node_id: &str) -> Result<Option<Vec<String>>> {
        <T as AdapterDecorator>::locate_node_path(self, node_id).await
    }
    fn hooks(&self) -> Vec<&str> {
        <T as AdapterDecorator>::hooks(self)
    }
    fn anonymizer(&self) -> Arc<dyn anonymize::Anonymizer> {
        <T as AdapterDecorator>::anonymizer(self)
    }
    async fn describe_columns(&self, node_type: &str) -> Vec<ColumnSchema> {
        <T as AdapterDecorator>::describe_columns(self, node_type).await
    }
}

/// A [`Node`] that wraps another one — the node-side twin of
/// [`AdapterDecorator`]. Implement the two accessors and override what the
/// decorator changes.
#[async_trait]
pub trait NodeDecorator: Send + Sync {
    /// The wrapped node every non-overridden call is forwarded to.
    fn inner(&self) -> &dyn Node;
    /// Mutable access to the wrapped node, for `hydrate` and `execute`.
    fn inner_mut(&mut self) -> &mut dyn Node;

    fn id(&self) -> &str {
        self.inner().id()
    }
    fn label(&self) -> &str {
        self.inner().label()
    }
    fn node_type(&self) -> &NodeType {
        self.inner().node_type()
    }
    fn metadata(&self) -> &Metadata {
        self.inner().metadata()
    }
    async fn hydrate(&mut self) {
        self.inner_mut().hydrate().await
    }
    fn row_summary(&self) -> NodeSummary {
        self.inner().row_summary()
    }
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        self.inner().get_child(id).await
    }
    fn content(&self) -> Option<&dyn Content> {
        self.inner().content()
    }
    async fn invoke_action(&self, name: &str, ctx: &ActionContext) -> Result<ActionDispatch> {
        self.inner().invoke_action(name, ctx).await
    }
    async fn prepare(&self, action_id: &str, args: &ActionArgs) -> Result<EditorPrep> {
        self.inner().prepare(action_id, args).await
    }
    async fn picker_options(&self, action_id: &str) -> Result<Vec<ActionOption>> {
        self.inner().picker_options(action_id).await
    }
    async fn form_prep(&self, action_id: &str) -> Result<HashMap<String, String>> {
        self.inner().form_prep(action_id).await
    }
    async fn execute(
        &mut self,
        action_id: &str,
        input: ActionInput,
        args: &ActionArgs,
    ) -> Result<ActionOutcome> {
        self.inner_mut().execute(action_id, input, args).await
    }
}

/// Every [`NodeDecorator`] is a [`Node`] — see the adapter twin above.
#[async_trait]
impl<T: NodeDecorator> Node for T {
    fn id(&self) -> &str {
        <T as NodeDecorator>::id(self)
    }
    fn label(&self) -> &str {
        <T as NodeDecorator>::label(self)
    }
    fn node_type(&self) -> &NodeType {
        <T as NodeDecorator>::node_type(self)
    }
    fn metadata(&self) -> &Metadata {
        <T as NodeDecorator>::metadata(self)
    }
    async fn hydrate(&mut self) {
        <T as NodeDecorator>::hydrate(self).await
    }
    fn row_summary(&self) -> NodeSummary {
        <T as NodeDecorator>::row_summary(self)
    }
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        <T as NodeDecorator>::get_child(self, id).await
    }
    fn content(&self) -> Option<&dyn Content> {
        <T as NodeDecorator>::content(self)
    }
    async fn invoke_action(&self, name: &str, ctx: &ActionContext) -> Result<ActionDispatch> {
        <T as NodeDecorator>::invoke_action(self, name, ctx).await
    }
    async fn prepare(&self, action_id: &str, args: &ActionArgs) -> Result<EditorPrep> {
        <T as NodeDecorator>::prepare(self, action_id, args).await
    }
    async fn picker_options(&self, action_id: &str) -> Result<Vec<ActionOption>> {
        <T as NodeDecorator>::picker_options(self, action_id).await
    }
    async fn form_prep(&self, action_id: &str) -> Result<HashMap<String, String>> {
        <T as NodeDecorator>::form_prep(self, action_id).await
    }
    async fn execute(
        &mut self,
        action_id: &str,
        input: ActionInput,
        args: &ActionArgs,
    ) -> Result<ActionOutcome> {
        <T as NodeDecorator>::execute(self, action_id, input, args).await
    }
}

#[cfg(test)]
mod completeness {
    //! The drift guard: every method the two core traits declare must have a
    //! forwarder in this file, or a decorator silently falls back to the trait
    //! default for it. Parsed from the source text, because the compiler
    //! cannot tell a missing arm from a deliberately inherited default.

    use std::collections::BTreeSet;

    const LIB: &str = include_str!("lib.rs");
    const SELF: &str = include_str!("decorate.rs");

    /// The `fn` names declared between the line starting with `start` and
    /// the next line that is exactly `}`.
    fn method_names(source: &str, start: &str) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        let mut inside = false;
        for line in source.lines() {
            if line.starts_with(start) {
                inside = true;
                continue;
            }
            if !inside {
                continue;
            }
            if line == "}" {
                break;
            }
            let trimmed = line.trim_start();
            if trimmed.starts_with("///") || trimmed.starts_with("//") {
                continue;
            }
            let Some(rest) = trimmed
                .strip_prefix("async fn ")
                .or_else(|| trimmed.strip_prefix("fn "))
            else {
                continue;
            };
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            names.insert(name);
        }
        assert!(inside, "block starting with {start:?} not found");
        names
    }

    #[test]
    fn adapter_decorator_forwards_every_content_adapter_method() {
        let declared = method_names(LIB, "pub trait ContentAdapter");
        let forwarded = method_names(SELF, "pub trait AdapterDecorator");
        let blanket = method_names(SELF, "impl<T: AdapterDecorator> ContentAdapter for T");
        let missing: Vec<_> = declared.difference(&forwarded).collect();
        assert!(
            missing.is_empty(),
            "AdapterDecorator lacks forwarders for {missing:?}"
        );
        let missing: Vec<_> = declared.difference(&blanket).collect();
        assert!(
            missing.is_empty(),
            "the ContentAdapter blanket impl lacks arms for {missing:?}"
        );
    }

    #[test]
    fn node_decorator_forwards_every_node_method() {
        let declared = method_names(LIB, "pub trait Node:");
        let forwarded = method_names(SELF, "pub trait NodeDecorator");
        let blanket = method_names(SELF, "impl<T: NodeDecorator> Node for T");
        let mut forwarded_only = forwarded.clone();
        forwarded_only.remove("inner");
        forwarded_only.remove("inner_mut");
        let missing: Vec<_> = declared.difference(&forwarded_only).collect();
        assert!(
            missing.is_empty(),
            "NodeDecorator lacks forwarders for {missing:?}"
        );
        let missing: Vec<_> = declared.difference(&blanket).collect();
        assert!(
            missing.is_empty(),
            "the Node blanket impl lacks arms for {missing:?}"
        );
    }
}
