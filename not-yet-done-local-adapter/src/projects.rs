//! In-process **Projects** adapter (plan phase D3b-3).
//!
//! Exposes the task domain's projects as a flat list of content rows with
//! create / edit / delete actions, so the same generic frontends that drive
//! every other adapter (the TUI's content tabs, the CLI's `ls`/`do`) can
//! manage projects without any project-specific glue:
//!
//! ```text
//! nyd projects ls
//! nyd projects do create            --field name="Acme" --field description="..."
//! nyd projects do edit   <id>       --field name="Acme GmbH"
//! nyd projects do delete <id>       --field cascade=true
//! ```
//!
//! Unlike the Tasks/Trackings adapters this one is **stateless**: projects are
//! few and cheap to list, so every read calls
//! [`ProjectService::list_projects`] afresh rather than maintaining an eager
//! snapshot. There is therefore no in-memory cache to invalidate — a mutation
//! publishes [`DomainEvent::ProjectChanged`] on the shared bus and the generic
//! [`crate::spawn_event_bridge`] turns it into an [`Invalidation::All`] that
//! makes the pane re-list. Because the bus channel is the DSN, a project
//! rename / cascade-delete also nudges any Tasks/Trackings tabs on the same
//! database (their bridges map `ProjectChanged` to a full resync, since a
//! cascade delete soft-deletes that project's tasks).
//!
//! Actions are `InputSpec::Form` actions dispatched through [`Node::execute`]
//! (the same path the Trackings adapter's `split`/`move` use): `create` lives
//! on the synthetic root, `edit`/`delete` on each project row. `edit` prefills
//! the form with the project's current values via [`Node::form_prep`].

use std::collections::HashMap;

use async_trait::async_trait;
use not_yet_done_content::{
    ActionInput, ActionOutcome, AdapterCapabilities, ColumnSchema, ContentAdapter, ContentError,
    FormFieldSpec, HostContext, InputSpec, Invalidation, ListParams, ListResult, Metadata,
    MetadataField, Node, NodeAction, NodeSummary, NodeType, Result, TypedAdapterFactory,
    apply_sort,
};
use not_yet_done_task_core::entity::project;
use not_yet_done_task_core::error::AppError;
use not_yet_done_task_core::events::DomainEvent;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::CoreHandle;
use crate::form::{form_flag, form_opt, form_required};

/// Stable id of the synthetic list-root node.
const ROOT_ID: &str = "project:root";

// ---------------------------------------------------------------------------
// Node types
// ---------------------------------------------------------------------------

/// The synthetic list root the adapter exposes from [`ContentAdapter::root`].
fn project_root_type() -> NodeType {
    NodeType {
        type_id: "project:root".to_string(),
        mime_type: "text/plain".to_string(),
        syntax: None,
        file_extension: ".txt".to_string(),
        display_name: "Projects".to_string(),
    }
}

/// The node type for a single project row. `project:item` is what a
/// `views/projects.yaml` binds its columns to.
fn project_item_type() -> NodeType {
    NodeType {
        type_id: "project:item".to_string(),
        mime_type: "text/plain".to_string(),
        syntax: None,
        file_extension: ".txt".to_string(),
        display_name: "Project".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn to_content_err(e: AppError) -> ContentError {
    ContentError::Other(Box::new(e))
}

/// A read-only metadata field. Projects carry no inline-editable fields (edits
/// go through the `edit` Form action).
fn field(key: &str, value: String, label: &str) -> MetadataField {
    MetadataField {
        key: key.to_string(),
        value,
        display_label: label.to_string(),
        editable: false,
        allowed_values: None,
    }
}

/// Column-backing metadata for a project row.
fn item_metadata(p: &project::Model) -> Metadata {
    Metadata {
        fields: vec![
            field("name", p.name.clone(), "Name"),
            field(
                "description",
                p.description.clone().unwrap_or_default(),
                "Description",
            ),
            field("created", p.created_at.to_rfc3339(), "Created"),
            field("id", p.id.to_string(), "ID"),
        ],
    }
}

/// Build a project's [`NodeSummary`]. Projects are leaves
/// (`has_children: Some(false)`) — the list is flat.
fn item_summary(p: &project::Model) -> NodeSummary {
    NodeSummary {
        id: p.id.to_string(),
        label: p.name.clone(),
        node_type: project_item_type(),
        metadata: item_metadata(p),
        has_children: Some(false),
    }
}

/// Columns a project list can be sorted on (the `S` item-sort). The adapter
/// applies the sort itself in [`ProjectRootNode::list`] via the generic
/// [`apply_sort`].
fn project_columns() -> Vec<ColumnSchema> {
    [("name", "Name", "text"), ("created", "Created", "datetime")]
        .into_iter()
        .map(|(key, label, value_type)| ColumnSchema::new(key, label).typed(value_type))
        .collect()
}

/// List every project as a sorted flat list of `project:item` rows — the one
/// implementation both the legacy [`ProjectRootNode::list`] and the
/// [`ContentAdapter::childs`] fetch closure run. Projects have no query
/// language, so the pane's query is ignored; `params.sort` is applied via the
/// generic [`apply_sort`].
async fn list_projects(handle: &CoreHandle, params: &ListParams) -> Result<ListResult> {
    let projects = handle
        .project_service
        .list_projects()
        .await
        .map_err(to_content_err)?;
    let mut items: Vec<NodeSummary> = projects.iter().map(item_summary).collect();
    let applied = apply_sort(&mut items, &params.sort, &project_columns());
    Ok(ListResult {
        items,
        applied_sort: applied,
        page: None,
        batch_download_available: false,
        downloaded: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// The root exposes `create` — a list-wide operation (no target row), so it
/// lives on the root rather than an item.
fn project_root_actions() -> Vec<NodeAction> {
    vec![NodeAction::new(
        "create",
        "New project",
        create_input_spec(),
    )]
}

/// Actions a single project row exposes: `edit` (rename / re-describe) and
/// `delete` (with an optional cascade onto the project's tasks).
fn project_item_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("edit", "Edit", edit_input_spec()),
        NodeAction::new("delete", "Delete", delete_input_spec()),
    ]
}

/// Form for `create`: a required name and an optional description.
fn create_input_spec() -> InputSpec {
    InputSpec::Form {
        fields: vec![
            FormFieldSpec::text("name", "Project name"),
            FormFieldSpec::text("description", "Description (optional)").optional(),
        ],
    }
}

/// Form for `edit`: name + description, both optional — an empty field leaves
/// the current value unchanged (mirrors `ProjectRepository::update`'s
/// `None`-means-unchanged semantics). [`ProjectItemNode::form_prep`] prefills
/// both with the project's current values.
fn edit_input_spec() -> InputSpec {
    InputSpec::Form {
        fields: vec![
            FormFieldSpec::text("name", "Project name").optional(),
            FormFieldSpec::text("description", "Description").optional(),
        ],
    }
}

/// Form for `delete`: a single cascade toggle. Off (the default) deletes only
/// the project row; on also soft-deletes that project's tasks
/// ([`ProjectService::delete_project`]'s `cascade`). The explicit toggle is
/// the confirmation — a project delete is destructive, so the choice is made
/// in the form rather than via a separate yes/no prompt.
fn delete_input_spec() -> InputSpec {
    InputSpec::Form {
        fields: vec![FormFieldSpec::toggle(
            "cascade",
            "Also delete this project's tasks",
        )],
    }
}

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

/// `execute("create")` — add a project from the form's `name` (+ optional
/// `description`). Emits [`DomainEvent::ProjectChanged`] so the list (and any
/// Tasks/Trackings tab on the same DB) refreshes, then navigates to the new
/// row.
async fn execute_create(
    handle: &CoreHandle,
    values: &HashMap<String, String>,
) -> Result<ActionOutcome> {
    let name = form_required(values, "name")?.to_string();
    let description = form_opt(values, "description");
    let created = handle
        .project_service
        .add_project(name, description)
        .await
        .map_err(to_content_err)?;
    handle.publish(DomainEvent::ProjectChanged { id: created.id });
    Ok(ActionOutcome::Navigate {
        node_id: created.id.to_string(),
        node_type: project_item_type(),
        message: None,
    })
}

/// `execute("edit")` — update name and/or description. An empty form field
/// leaves the current value as-is (`None` → unchanged in the repo).
async fn execute_edit(
    handle: &CoreHandle,
    id: Uuid,
    values: &HashMap<String, String>,
) -> Result<ActionOutcome> {
    let name = form_opt(values, "name");
    let description = form_opt(values, "description");
    if name.is_none() && description.is_none() {
        return Ok(ActionOutcome::NoChanges);
    }
    handle
        .project_service
        .edit_project(id, name, description)
        .await
        .map_err(to_content_err)?;
    handle.publish(DomainEvent::ProjectChanged { id });
    Ok(ActionOutcome::Done {
        message: Some("Project updated".to_string()),
    })
}

/// `execute("delete")` — delete the project, cascading onto its tasks when the
/// `cascade` toggle is on.
async fn execute_delete(
    handle: &CoreHandle,
    id: Uuid,
    values: &HashMap<String, String>,
) -> Result<ActionOutcome> {
    let cascade = form_flag(values, "cascade");
    handle
        .project_service
        .delete_project(id, cascade)
        .await
        .map_err(to_content_err)?;
    handle.publish(DomainEvent::ProjectChanged { id });
    Ok(ActionOutcome::Done {
        message: Some(if cascade {
            "Project and its tasks deleted".to_string()
        } else {
            "Project deleted".to_string()
        }),
    })
}

// ---------------------------------------------------------------------------
// Nodes
// ---------------------------------------------------------------------------

/// The synthetic list root: lists every project and hosts the `create` action.
struct ProjectRootNode {
    handle: CoreHandle,
    node_type: NodeType,
    metadata: Metadata,
}

#[async_trait]
impl Node for ProjectRootNode {
    fn id(&self) -> &str {
        ROOT_ID
    }
    fn label(&self) -> &str {
        "Projects"
    }
    fn node_type(&self) -> &NodeType {
        &self.node_type
    }
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        ProjectItemNode::fetch(&self.handle, id).await
    }
    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("create", ActionInput::Form(values)) => execute_create(&self.handle, &values).await,
            (other, _) => Err(ContentError::NotSupported(format!(
                "action `{other}` not supported on the projects root"
            ))),
        }
    }
}

/// A single project row. A leaf: it has no children. Carries the project's
/// current name/description so `edit` can prefill its form.
struct ProjectItemNode {
    id_str: String,
    name: String,
    description: Option<String>,
    node_type: NodeType,
    metadata: Metadata,
    handle: CoreHandle,
}

impl ProjectItemNode {
    /// Rebuild a project node from its uuid by listing projects and finding the
    /// match. `ProjectService` exposes no by-id lookup, but the list is small,
    /// so a list-and-find is cheap and keeps the adapter free of the repo.
    async fn fetch(handle: &CoreHandle, id: &str) -> Result<Box<dyn Node>> {
        let uuid = Uuid::parse_str(id).map_err(|_| ContentError::NotFound(id.to_string()))?;
        let project = handle
            .project_service
            .list_projects()
            .await
            .map_err(to_content_err)?
            .into_iter()
            .find(|p| p.id == uuid)
            .ok_or_else(|| ContentError::NotFound(id.to_string()))?;
        Ok(Box::new(ProjectItemNode {
            id_str: id.to_string(),
            name: project.name.clone(),
            description: project.description.clone(),
            node_type: project_item_type(),
            metadata: item_metadata(&project),
            handle: handle.clone(),
        }))
    }

    fn project_id(&self) -> Result<Uuid> {
        Uuid::parse_str(&self.id_str).map_err(|_| ContentError::NotFound(self.id_str.clone()))
    }
}

#[async_trait]
impl Node for ProjectItemNode {
    fn id(&self) -> &str {
        &self.id_str
    }
    fn label(&self) -> &str {
        &self.name
    }
    fn node_type(&self) -> &NodeType {
        &self.node_type
    }
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    async fn form_prep(&self, action_id: &str) -> Result<HashMap<String, String>> {
        let mut prefill = HashMap::new();
        if action_id == "edit" {
            prefill.insert("name".to_string(), self.name.clone());
            prefill.insert(
                "description".to_string(),
                self.description.clone().unwrap_or_default(),
            );
        }
        Ok(prefill)
    }
    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("edit", ActionInput::Form(values)) => {
                execute_edit(&self.handle, self.project_id()?, &values).await
            }
            ("delete", ActionInput::Form(values)) => {
                execute_delete(&self.handle, self.project_id()?, &values).await
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "action `{other}` not supported on a project"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Adapter + factory
// ---------------------------------------------------------------------------

/// Builds self-contained [`ProjectAdapter`] instances. Stateless: each
/// `create` opens its own database from the tab's `config`
/// (see [`crate::open_core_handle`] / [`crate::LocalAdapterConfig`]) and wires
/// the resulting [`CoreHandle`] to the host bus from [`HostContext`].
#[derive(Default)]
pub struct ProjectAdapterFactory;

impl ProjectAdapterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl TypedAdapterFactory for ProjectAdapterFactory {
    type Config = crate::LocalAdapterConfig;

    fn adapter_type(&self) -> &str {
        "projects"
    }

    fn build(
        &self,
        instance_id: &str,
        cfg: crate::LocalAdapterConfig,
        ctx: &HostContext,
    ) -> Result<Box<dyn ContentAdapter>> {
        let handle = crate::open_core_handle(cfg, ctx)?;
        Ok(Box::new(ProjectAdapter::new(instance_id, handle)))
    }
}

/// In-process adapter presenting the task domain's projects as content rows.
pub struct ProjectAdapter {
    instance_id: String,
    handle: CoreHandle,
    inv_tx: broadcast::Sender<Invalidation>,
}

impl ProjectAdapter {
    /// Build an adapter over an already-opened [`CoreHandle`]: set up the
    /// invalidation broadcast and spawn the domain-event → invalidation
    /// bridge (which maps [`DomainEvent::ProjectChanged`] to
    /// [`Invalidation::All`]). The factory uses this after
    /// [`crate::open_core_handle`]; tests use it over a handle built on their
    /// own in-memory database.
    pub(crate) fn new(instance_id: &str, handle: CoreHandle) -> Self {
        let (inv_tx, _) = broadcast::channel(64);
        crate::spawn_event_bridge(handle.subscribe(), inv_tx.clone());
        Self {
            instance_id: instance_id.to_string(),
            handle,
            inv_tx,
        }
    }
}

#[async_trait]
impl ContentAdapter for ProjectAdapter {
    fn adapter_type(&self) -> &str {
        "projects"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Anonymize project names (`label` / `name`) with a dedicated invented
    /// company-name pool, and the free-text `description` with the standard
    /// fallback. See [`crate::anonymize`].
    fn anonymizer(&self) -> std::sync::Arc<dyn not_yet_done_content::Anonymizer> {
        std::sync::Arc::new(crate::anonymize::LocalAnonymizer::project())
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            // `create` is a root Form action; `edit`/`delete` are item Form
            // actions. Delete is a Form (cascade toggle) rather than the
            // generic confirm flow, so `supports_delete` stays false.
            supports_create: true,
            ..AdapterCapabilities::default()
        }
    }

    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        match node_type.type_id.as_str() {
            "project:root" => project_root_actions(),
            "project:item" => project_item_actions(),
            _ => Vec::new(),
        }
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        Ok(Box::new(ProjectRootNode {
            handle: self.handle.clone(),
            node_type: project_root_type(),
            metadata: Metadata::default(),
        }))
    }

    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        if id == ROOT_ID {
            return Ok(Box::new(ProjectRootNode {
                handle: self.handle.clone(),
                node_type: project_root_type(),
                metadata: Metadata::default(),
            }));
        }
        ProjectItemNode::fetch(&self.handle, id).await
    }

    /// Single source of truth about a project node's children. The root lists
    /// `project:item` rows via the shared [`list_projects`] free fn; a project
    /// row is a leaf. The fetch closure reads only from adapter state
    /// (`self.handle`), never the concrete node.
    fn childs<'a>(&'a self, node: &'a dyn Node) -> Vec<not_yet_done_content::Child<'a>> {
        use not_yet_done_content::Child;
        match node.node_type().type_id.as_str() {
            "project:root" => vec![Child {
                node_type: project_item_type(),
                columns: project_columns(),
                list: Box::new(move |params| {
                    Box::pin(async move { list_projects(&self.handle, &params).await })
                }),
            }],
            _ => Vec::new(),
        }
    }

    fn subscribe_invalidations(&self) -> broadcast::Receiver<Invalidation> {
        self.inv_tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use not_yet_done_content::InMemoryHostBus;
    use not_yet_done_task_core::module::TaskDomainModule;
    use not_yet_done_task_core::repository::{
        ProjectRepositoryImpl, ProjectRepositoryImplParameters, TagRepositoryImpl,
        TagRepositoryImplParameters, TaskRepositoryImpl, TaskRepositoryImplParameters,
        TrackingRepositoryImpl, TrackingRepositoryImplParameters,
    };
    use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Schema};
    use shaku::HasComponent;
    use std::sync::Arc;

    // -- pure (no DB) action-advertisement tests ----------------------------

    #[test]
    fn root_exposes_create_item_exposes_edit_delete() {
        let root: Vec<String> = project_root_actions().into_iter().map(|a| a.id).collect();
        assert_eq!(root, vec!["create"]);
        let item: Vec<String> = project_item_actions().into_iter().map(|a| a.id).collect();
        assert_eq!(item, vec!["edit", "delete"]);
    }

    #[test]
    fn actions_advertise_their_form_fields() {
        let create_fields = match create_input_spec() {
            InputSpec::Form { fields } => fields.into_iter().map(|f| f.key).collect::<Vec<_>>(),
            _ => panic!("create must be a Form"),
        };
        assert_eq!(create_fields, vec!["name", "description"]);

        let delete_fields = match delete_input_spec() {
            InputSpec::Form { fields } => fields.into_iter().map(|f| f.key).collect::<Vec<_>>(),
            _ => panic!("delete must be a Form"),
        };
        assert_eq!(delete_fields, vec!["cascade"]);
    }

    // -- DB-backed lifecycle tests ------------------------------------------

    /// Build a `projects` adapter over a fresh in-memory database with the
    /// task-domain schema synced, returning the adapter and the connection.
    async fn setup() -> (ProjectAdapter, DatabaseConnection) {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory sqlite");
        let schema = Schema::new(DbBackend::Sqlite);
        use not_yet_done_task_core::entity::{
            global_tag, project, project_tag, task, task_global_tag, task_project_tag, tracking,
        };
        for stmt in [
            schema.create_table_from_entity(project::Entity),
            schema.create_table_from_entity(task::Entity),
            schema.create_table_from_entity(tracking::Entity),
            schema.create_table_from_entity(global_tag::Entity),
            schema.create_table_from_entity(project_tag::Entity),
            schema.create_table_from_entity(task_global_tag::Entity),
            schema.create_table_from_entity(task_project_tag::Entity),
        ] {
            db.execute(&stmt).await.expect("schema creation");
        }
        let module = TaskDomainModule::builder()
            .with_component_parameters::<TaskRepositoryImpl>(TaskRepositoryImplParameters {
                db: Some(db.clone()),
            })
            .with_component_parameters::<ProjectRepositoryImpl>(ProjectRepositoryImplParameters {
                db: Some(db.clone()),
            })
            .with_component_parameters::<TagRepositoryImpl>(TagRepositoryImplParameters {
                db: Some(db.clone()),
            })
            .with_component_parameters::<TrackingRepositoryImpl>(TrackingRepositoryImplParameters {
                db: Some(db.clone()),
            })
            .build();
        let handle = CoreHandle::new(
            module.resolve(),
            module.resolve(),
            module.resolve(),
            module.resolve(),
            module.resolve(),
            Arc::new(InMemoryHostBus::default()),
            "test".to_string(),
            false,
        );
        (ProjectAdapter::new("test", handle), db)
    }

    fn form(pairs: &[(&str, &str)]) -> ActionInput {
        ActionInput::Form(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[tokio::test]
    async fn create_then_list_shows_the_project() {
        let (adapter, _db) = setup().await;
        let mut root = adapter.root().await.unwrap();
        let outcome = root
            .execute(
                "create",
                form(&[("name", "Acme"), ("description", "Widgets")]),
            )
            .await
            .unwrap();
        let new_id = match outcome {
            ActionOutcome::Navigate { node_id, .. } => node_id,
            _ => panic!("expected Navigate"),
        };

        let params = ListParams {
            node_type: project_item_type(),
            query: None,
            sort: Vec::new(),
            page: None,
            download: false,
            group_by: None,
        };
        let root = adapter.root().await.unwrap();
        let list = not_yet_done_content::children::list(&adapter, root.as_ref(), params)
            .await
            .unwrap();
        assert_eq!(list.items.len(), 1);
        assert_eq!(list.items[0].id, new_id);
        assert_eq!(list.items[0].label, "Acme");
    }

    #[tokio::test]
    async fn create_requires_a_name() {
        let (adapter, _db) = setup().await;
        let mut root = adapter.root().await.unwrap();
        let err = root
            .execute("create", form(&[("description", "no name")]))
            .await
            .err()
            .expect("missing name must error");
        assert!(format!("{err}").contains("name"), "got: {err}");
    }

    #[tokio::test]
    async fn edit_prefills_and_updates_name() {
        let (adapter, _db) = setup().await;
        let id = match adapter
            .root()
            .await
            .unwrap()
            .execute("create", form(&[("name", "Old")]))
            .await
            .unwrap()
        {
            ActionOutcome::Navigate { node_id, .. } => node_id,
            _ => panic!("expected Navigate"),
        };

        // form_prep seeds the current name.
        let node = adapter.get_by_id(&id).await.unwrap();
        let prep = node.form_prep("edit").await.unwrap();
        assert_eq!(prep.get("name").map(String::as_str), Some("Old"));

        let mut node = adapter.get_by_id(&id).await.unwrap();
        node.execute("edit", form(&[("name", "New")]))
            .await
            .unwrap();
        let reloaded = adapter.get_by_id(&id).await.unwrap();
        assert_eq!(reloaded.label(), "New");
    }

    #[tokio::test]
    async fn delete_removes_the_project() {
        let (adapter, _db) = setup().await;
        let id = match adapter
            .root()
            .await
            .unwrap()
            .execute("create", form(&[("name", "Doomed")]))
            .await
            .unwrap()
        {
            ActionOutcome::Navigate { node_id, .. } => node_id,
            _ => panic!("expected Navigate"),
        };

        let mut node = adapter.get_by_id(&id).await.unwrap();
        node.execute("delete", form(&[("cascade", "false")]))
            .await
            .unwrap();

        let params = ListParams {
            node_type: project_item_type(),
            query: None,
            sort: Vec::new(),
            page: None,
            download: false,
            group_by: None,
        };
        let root = adapter.root().await.unwrap();
        let list = not_yet_done_content::children::list(&adapter, root.as_ref(), params)
            .await
            .unwrap();
        assert!(list.items.is_empty(), "project should be gone");
    }
}
