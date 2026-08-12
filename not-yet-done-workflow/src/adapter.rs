//! The `workflow` [`ContentAdapter`] — a standalone adapter (like calendar,
//! not a decorator) that surfaces local workflow definitions and their runs
//! through the ordinary node protocol, so every front-end reaches them with no
//! bespoke wiring.
//!
//! # Node hierarchy
//!
//! ```text
//! root → workflow → run → step
//! ```
//!
//! * **root** (`workflow:root`) — lists one `workflow:workflow` per `.md`
//!   definition in the repo. Carries the `create` action.
//! * **workflow** (`workflow:workflow`, id `wf:<name>`) — a single definition
//!   file. Its [`Content`] is the raw markdown; it lists its `workflow:run`
//!   history and carries `edit` / `rename` / `delete` / `run`.
//! * **run** (`workflow:run`, id `run:<run_id>`) — one execution instance from
//!   the SQLite store; lists its `workflow:step` protocol and carries `delete`.
//! * **step** (`workflow:step`, id `step:<run_id>:<seq>`) — one *visit* of a
//!   definition step; a read-only leaf whose [`Content`] is the captured output.
//!
//! # Execution — dynamic append-per-visit (Phase 6b)
//!
//! Starting a `run` seeds only the definition's **entry** step as a `pending`
//! visit. Each `advance` (run action) carries out the current frontier step per
//! its mode — an `auto` step runs its command via [`crate::exec`], an `ai` step
//! its instruction through the configured runner, a manual step is marked done —
//! then consults that step's [`Route`](crate::model::Route)s against the outcome
//! to decide what comes next and **appends** the successor step(s) as fresh
//! `pending` visits. Because a successor is a new row (`seq` = visit order, not
//! document order), the same definition step can be visited repeatedly, so loops
//! and branches fall out naturally; a [`MAX_VISITS`] guard turns a runaway loop
//! into a routed failure. A step with no matching route falls through to the next
//! step in document order (the plain-checklist case), or ends the run.
//!
//! `skip` settles the frontier as `skipped` (taking the success routes); `reset`
//! clears the protocol back to a single fresh entry visit. The run's aggregate
//! status is set by the routing driver (`running` while work is queued,
//! `done`/`failed` at a terminal) rather than a pure per-step aggregate.
//!
//! The visited steps are frozen in the protocol, but the *plan ahead* is read
//! from the definition file at each `advance` — editing the `.md` mid-run
//! redirects only the not-yet-taken path, never rewrites history.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;

use not_yet_done_content::*;

use crate::config::WorkflowConfig;
use crate::model::{RouteCondition, RouteTarget, Step, StepMode, Trigger, WorkflowDef};
use crate::repo::WorkflowRepo;
use crate::store::{run_status, step_status, NewStep, RunRow, RunStore, StepRow};

/// Id of the adapter root, addressable via [`ContentAdapter::get_by_id`].
const ROOT_ID: &str = "root";
/// Node-id prefix for a workflow definition (`wf:<name>`).
const WF_PREFIX: &str = "wf:";
/// Node-id prefix for a run (`run:<run_id>`).
const RUN_PREFIX: &str = "run:";
/// Node-id prefix for a run's step entry (`step:<run_id>:<seq>`).
const STEP_PREFIX: &str = "step:";

/// Create a new workflow definition (root action).
const CREATE_ACTION: &str = "create";
/// Edit a workflow's markdown (workflow action).
const EDIT_ACTION: &str = "edit";
/// Rename a workflow (workflow action).
const RENAME_ACTION: &str = "rename";
/// Delete a workflow definition or a run.
const DELETE_ACTION: &str = "delete";
/// Start a new run of a workflow (workflow action) — the Phase 3 bridge.
const RUN_ACTION: &str = "run";
/// Advance a run: carry out its frontier step, then route to the next (run action).
const ADVANCE_ACTION: &str = "advance";
/// Skip the run's frontier step (taking its success routes).
const SKIP_ACTION: &str = "skip";
/// Reset a run back to a single fresh entry visit.
const RESET_ACTION: &str = "reset";
/// Render a workflow definition as a Mermaid flowchart (workflow action).
const DIAGRAM_ACTION: &str = "diagram";

/// How many times a single definition step may be visited within one run before
/// the loop-cycle guard routes the run to failure — a backstop against a
/// self-referential route that never terminates.
const MAX_VISITS: u64 = 1000;

fn io_err(e: std::io::Error) -> ContentError {
    ContentError::Other(Box::new(e))
}

fn field(key: &str, label: &str, value: impl Into<String>) -> MetadataField {
    MetadataField {
        key: key.into(),
        value: value.into(),
        display_label: label.into(),
        editable: false,
        allowed_values: None,
    }
}

fn list_result(items: Vec<NodeSummary>) -> ListResult {
    ListResult {
        items,
        applied_sort: Vec::new(),
        page: None,
        batch_download_available: false,
        downloaded: Vec::new(),
    }
}

// -- Node types -------------------------------------------------------------

fn root_type() -> NodeType {
    NodeType {
        type_id: "workflow:root".into(),
        mime_type: String::new(),
        syntax: None,
        file_extension: String::new(),
        display_name: "Workflows".into(),
    }
}

fn workflow_type() -> NodeType {
    NodeType {
        type_id: "workflow:workflow".into(),
        mime_type: "text/markdown".into(),
        syntax: Some("markdown".into()),
        file_extension: ".md".into(),
        display_name: "Workflow".into(),
    }
}

fn run_type() -> NodeType {
    NodeType {
        type_id: "workflow:run".into(),
        mime_type: String::new(),
        syntax: None,
        file_extension: String::new(),
        display_name: "Run".into(),
    }
}

fn step_type() -> NodeType {
    NodeType {
        type_id: "workflow:step".into(),
        mime_type: "text/plain".into(),
        syntax: None,
        file_extension: ".log".into(),
        display_name: "Step".into(),
    }
}

// -- Action sets ------------------------------------------------------------

fn root_actions() -> Vec<NodeAction> {
    vec![NodeAction::new(
        CREATE_ACTION,
        "new workflow",
        InputSpec::Form {
            fields: vec![FormFieldSpec::text("name", "Workflow name")],
        },
    )]
}

fn workflow_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new(RUN_ACTION, "run workflow", InputSpec::None),
        NodeAction::new(EDIT_ACTION, "edit workflow", InputSpec::Editor),
        NodeAction::new(DIAGRAM_ACTION, "diagram (mermaid)", InputSpec::None),
        NodeAction::new(
            RENAME_ACTION,
            "rename workflow",
            InputSpec::Form {
                fields: vec![FormFieldSpec::text("name", "New name")],
            },
        ),
        NodeAction::new(DELETE_ACTION, "delete workflow", InputSpec::None),
    ]
}

fn run_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new(ADVANCE_ACTION, "advance run", InputSpec::None),
        NodeAction::new(SKIP_ACTION, "skip step", InputSpec::None),
        NodeAction::new(RESET_ACTION, "reset run", InputSpec::None),
        NodeAction::new(DELETE_ACTION, "delete run", InputSpec::None),
    ]
}

/// A step is a read-only record of one visit — the run node's `advance` / `skip`
/// drive the frontier, so the step leaf itself carries no actions.
fn step_actions() -> Vec<NodeAction> {
    Vec::new()
}

// -- Row summaries ----------------------------------------------------------

fn run_summary(row: &RunRow) -> NodeSummary {
    NodeSummary {
        id: format!("{RUN_PREFIX}{}", row.id),
        label: if row.title.is_empty() {
            row.id.clone()
        } else {
            row.title.clone()
        },
        node_type: run_type(),
        metadata: Metadata {
            fields: vec![
                field("status", "Status", row.status.clone()),
                field("created", "Created", row.created_at.clone()),
                field("updated", "Updated", row.updated_at.clone()),
            ],
        },
        has_children: None,
    }
}

fn step_summary(row: &StepRow) -> NodeSummary {
    NodeSummary {
        id: format!("{STEP_PREFIX}{}:{}", row.run_id, row.seq),
        label: row.title.clone(),
        node_type: step_type(),
        metadata: Metadata {
            fields: vec![
                field("seq", "#", row.seq.to_string()),
                field("mode", "Mode", row.mode.clone()),
                field("status", "Status", row.status.clone()),
            ],
        },
        has_children: Some(false),
    }
}

/// The seed markdown for a freshly-created workflow.
fn new_template(name: &str) -> String {
    format!(
        "---\ntitle: {name}\nmode: manual\n---\n\nDescribe what this workflow does.\n\n## First step\n\nWhat to do in this step.\n"
    )
}

// ---------------------------------------------------------------------------
// Shared node context
// ---------------------------------------------------------------------------

/// The handles every node needs to resolve siblings/children and reach storage.
/// Cheap to clone (the repo is a path wrapper, the store an `Arc`), so each node
/// carries its own copy rather than reaching back into the adapter.
#[derive(Clone)]
pub(crate) struct Ctx {
    repo: WorkflowRepo,
    store: Arc<RunStore>,
    /// `workflow/<instance_id>` — isolates this instance's runs in a shared DB.
    scope: String,
    /// Workflow-level default mode when a file omits `mode:`. Reserved for the
    /// run/execution phases (Phase 3+); a run today resolves each step against
    /// the definition's own parsed mode.
    #[allow(dead_code)]
    default_mode: StepMode,
    /// Default run-logging when a file omits `log_runs:`. Reserved for Phase 3+
    /// retention; runs are always recorded today.
    #[allow(dead_code)]
    log_runs: bool,
    /// The configured AI runner command (`workflow.ai_command`). An `ai` step is
    /// carried out by running this with the step's instruction as the prompt;
    /// unset (or empty) degrades an `ai` step to a manual mark-done.
    ai_command: Option<String>,
}

impl Ctx {
    fn workflow_node(&self, name: String) -> WorkflowNode {
        WorkflowNode::new(self.clone(), name)
    }

    fn run_node(&self, row: RunRow) -> RunNode {
        RunNode::new(self.clone(), row)
    }

    fn step_node(&self, row: StepRow) -> StepNode {
        StepNode::new(row)
    }

    /// A list-row for a workflow definition, loading its parsed shape best-effort
    /// (a file that fails to parse still lists, under its file name).
    fn workflow_summary(&self, name: &str) -> NodeSummary {
        let (title, mode, steps) = match self.repo.load(name) {
            Ok(def) => (
                if def.title.is_empty() {
                    name.to_string()
                } else {
                    def.title
                },
                def.mode.as_str().to_string(),
                def.steps.len(),
            ),
            Err(_) => (name.to_string(), String::new(), 0),
        };
        NodeSummary {
            id: format!("{WF_PREFIX}{name}"),
            label: title,
            node_type: workflow_type(),
            metadata: Metadata {
                fields: vec![
                    field("mode", "Mode", mode),
                    field("steps", "Steps", steps.to_string()),
                ],
            },
            has_children: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Adapter
// ---------------------------------------------------------------------------

pub struct WorkflowAdapter {
    instance_id: String,
    ctx: Ctx,
    /// Whether the background trigger scheduler may run for this instance.
    triggers_enabled: bool,
}

impl WorkflowAdapter {
    /// Assemble the adapter from its parsed config and an opened run store (the
    /// factory owns opening the store).
    pub fn new(instance_id: String, cfg: WorkflowConfig, store: Arc<RunStore>) -> Self {
        let repo = cfg
            .storage_path
            .filter(|s| !s.trim().is_empty())
            .map(WorkflowRepo::new)
            .unwrap_or_default();
        let default_mode = cfg
            .mode
            .as_deref()
            .and_then(StepMode::parse)
            .unwrap_or(StepMode::Manual);
        let log_runs = cfg.log_runs.unwrap_or(true);
        let triggers_enabled = cfg.triggers_enabled.unwrap_or(true);
        let ai_command = cfg.ai_command.filter(|s| !s.trim().is_empty());
        let scope = format!("workflow/{instance_id}");
        Self {
            instance_id,
            triggers_enabled,
            ctx: Ctx {
                repo,
                store,
                scope,
                default_mode,
                log_runs,
                ai_command,
            },
        }
    }

    /// Start the background trigger scheduler (Phase 6c) for this instance,
    /// unless disabled by config or there are no triggers to watch. The factory
    /// calls this once, after construction, with the host event bus. A no-op
    /// when there is no Tokio runtime (e.g. a synchronous test harness).
    pub fn spawn_triggers(&self, event_bus: Arc<dyn HostEventBus>) {
        if !self.triggers_enabled {
            return;
        }
        crate::scheduler::spawn(self.ctx.clone(), event_bus);
    }

    fn build_root(&self) -> WorkflowRoot {
        WorkflowRoot::new(self.ctx.clone())
    }
}

/// Every declared trigger across this instance's definitions, paired with the
/// workflow name it belongs to. Read fresh from disk so edits take effect on the
/// next scheduler pass. A definition that fails to list/parse contributes none.
pub(crate) fn collect_triggers(ctx: &Ctx) -> Vec<(String, Trigger)> {
    let Ok(entries) = ctx.repo.list() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries {
        if let Ok(def) = ctx.repo.load(&entry.name) {
            for trigger in def.triggers {
                out.push((entry.name.clone(), trigger));
            }
        }
    }
    out
}

/// Start a run of `workflow_name` and drive it as far as it can go without a
/// person (Phase 6c triggers): each `auto`/`ai` frontier step is carried out and
/// routed on; the driver stops when the run terminates or the frontier is a
/// `manual` step waiting for a human. Returns the new run id (empty if the store
/// is inert).
pub(crate) async fn trigger_run(ctx: &Ctx, workflow_name: &str) -> Result<String> {
    let def = ctx.repo.load(workflow_name).map_err(io_err)?;
    let steps: Vec<NewStep> = def
        .steps
        .first()
        .map(|s| new_step_from(s, def.mode))
        .into_iter()
        .collect();
    let title = if def.title.is_empty() {
        workflow_name.to_string()
    } else {
        def.title.clone()
    };
    let now = Utc::now().to_rfc3339();
    let run_id = ctx
        .store
        .create_run(&ctx.scope, workflow_name, &title, &steps, &now)
        .await?;
    if run_id.is_empty() {
        return Ok(run_id);
    }
    if let Some(run) = ctx.store.get_run(&run_id).await? {
        drive_run(ctx, &run).await?;
    }
    Ok(run_id)
}

/// Advance a run automatically until it blocks on a `manual` step or reaches a
/// terminal (no pending step left). Only [`advance_frontier`]'s own visit guard
/// bounds a routed loop; each iteration re-reads the frozen frontier from the
/// store, so this shares the exact semantics of the interactive `advance`.
async fn drive_run(ctx: &Ctx, run: &RunRow) -> Result<()> {
    while let Some(frontier) = ctx.store.next_pending_step(&run.id).await? {
        // A manual step waits for a human — hand the run back to the user here.
        if StepMode::parse(&frontier.mode).unwrap_or(StepMode::Manual) == StepMode::Manual {
            break;
        }
        let now = Utc::now().to_rfc3339();
        let settled = execute_step(ctx, &frontier).await;
        advance_frontier(ctx, run, &frontier, &settled, &now).await?;
    }
    Ok(())
}

#[async_trait]
impl ContentAdapter for WorkflowAdapter {
    fn adapter_type(&self) -> &str {
        "workflow"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            supports_create: true,
            supports_delete: true,
            ..AdapterCapabilities::default()
        }
    }

    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        match node_type.type_id.as_str() {
            "workflow:root" => root_actions(),
            "workflow:workflow" => workflow_actions(),
            "workflow:run" => run_actions(),
            "workflow:step" => step_actions(),
            _ => Vec::new(),
        }
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        Ok(Box::new(self.build_root()))
    }

    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        if id == ROOT_ID {
            return Ok(Box::new(self.build_root()));
        }
        if let Some(name) = id.strip_prefix(WF_PREFIX) {
            return Ok(Box::new(self.ctx.workflow_node(name.to_string())));
        }
        if let Some(run_id) = id.strip_prefix(RUN_PREFIX) {
            let row = self
                .ctx
                .store
                .get_run(run_id)
                .await?
                .ok_or_else(|| ContentError::NotFound(id.to_string()))?;
            return Ok(Box::new(self.ctx.run_node(row)));
        }
        if let Some(rest) = id.strip_prefix(STEP_PREFIX) {
            let step = resolve_step(&self.ctx, rest)
                .await?
                .ok_or_else(|| ContentError::NotFound(id.to_string()))?;
            return Ok(Box::new(self.ctx.step_node(step)));
        }
        Err(ContentError::NotFound(id.to_string()))
    }

    fn childs<'a>(&'a self, node: &'a dyn Node) -> Vec<Child<'a>> {
        match node.node_type().type_id.as_str() {
            "workflow:root" => vec![Child {
                node_type: workflow_type(),
                columns: Vec::new(),
                list: Box::new(move |_params| {
                    Box::pin(async move {
                        let entries = self.ctx.repo.list().map_err(io_err)?;
                        let items = entries
                            .iter()
                            .map(|e| self.ctx.workflow_summary(&e.name))
                            .collect();
                        Ok(list_result(items))
                    })
                }),
            }],
            "workflow:workflow" => {
                let name = strip(node.id(), WF_PREFIX);
                vec![Child {
                    node_type: run_type(),
                    columns: Vec::new(),
                    list: Box::new(move |_params| {
                        Box::pin(async move {
                            let runs = self.ctx.store.list_runs(&self.ctx.scope, &name).await?;
                            let items = runs.iter().map(run_summary).collect();
                            Ok(list_result(items))
                        })
                    }),
                }]
            }
            "workflow:run" => {
                let run_id = strip(node.id(), RUN_PREFIX);
                vec![Child {
                    node_type: step_type(),
                    columns: Vec::new(),
                    list: Box::new(move |_params| {
                        Box::pin(async move {
                            let steps = self.ctx.store.list_steps(&run_id).await?;
                            let items = steps.iter().map(step_summary).collect();
                            Ok(list_result(items))
                        })
                    }),
                }]
            }
            _ => Vec::new(),
        }
    }
}

/// Strip a node-id prefix, falling back to the whole id when it is absent.
fn strip(id: &str, prefix: &str) -> String {
    id.strip_prefix(prefix).unwrap_or(id).to_string()
}

/// Resolve a `step:` id remainder (`<run_id>:<seq>`) to its protocol row.
async fn resolve_step(ctx: &Ctx, rest: &str) -> Result<Option<StepRow>> {
    let Some((run_id, seq)) = rest.rsplit_once(':') else {
        return Ok(None);
    };
    let Ok(seq) = seq.parse::<i32>() else {
        return Ok(None);
    };
    let step = ctx
        .store
        .list_steps(run_id)
        .await?
        .into_iter()
        .find(|s| s.seq == seq);
    Ok(step)
}

// ---------------------------------------------------------------------------
// Root node
// ---------------------------------------------------------------------------

struct WorkflowRoot {
    ctx: Ctx,
    node_type: NodeType,
    metadata: Metadata,
}

impl WorkflowRoot {
    fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            node_type: root_type(),
            metadata: Metadata::default(),
        }
    }
}

#[async_trait]
impl Node for WorkflowRoot {
    fn id(&self) -> &str {
        ROOT_ID
    }
    fn label(&self) -> &str {
        "Workflows"
    }
    fn node_type(&self) -> &NodeType {
        &self.node_type
    }
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        if let Some(name) = id.strip_prefix(WF_PREFIX) {
            return Ok(Box::new(self.ctx.workflow_node(name.to_string())));
        }
        Err(ContentError::NotFound(id.to_string()))
    }

    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match action_id {
            CREATE_ACTION => {
                let fields = form_fields(input)?;
                let name = required(&fields, "name")?;
                self.ctx
                    .repo
                    .create(&name, &new_template(&name))
                    .map_err(io_err)?;
                Ok(ActionOutcome::Navigate {
                    node_id: format!("{WF_PREFIX}{name}"),
                    node_type: workflow_type(),
                    message: Some(format!("Created workflow {name}")),
                })
            }
            other => Err(ContentError::NotSupported(format!(
                "workflow root has no action '{other}'"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Workflow node (one definition file)
// ---------------------------------------------------------------------------

struct WorkflowNode {
    ctx: Ctx,
    name: String,
    id: String,
    node_type: NodeType,
    metadata: Metadata,
}

impl WorkflowNode {
    fn new(ctx: Ctx, name: String) -> Self {
        let id = format!("{WF_PREFIX}{name}");
        Self {
            ctx,
            name,
            id,
            node_type: workflow_type(),
            metadata: Metadata::default(),
        }
    }
}

#[async_trait]
impl Node for WorkflowNode {
    fn id(&self) -> &str {
        &self.id
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

    fn content(&self) -> Option<&dyn Content> {
        Some(self)
    }
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        if let Some(run_id) = id.strip_prefix(RUN_PREFIX) {
            let row = self
                .ctx
                .store
                .get_run(run_id)
                .await?
                .ok_or_else(|| ContentError::NotFound(id.to_string()))?;
            return Ok(Box::new(self.ctx.run_node(row)));
        }
        Err(ContentError::NotFound(id.to_string()))
    }

    async fn invoke_action(&self, name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
        match name {
            DELETE_ACTION => Ok(ActionDispatch::DeleteSelf {
                confirm: Some(format!("Delete workflow '{}'? (y/n)", self.name)),
            }),
            _ => Ok(ActionDispatch::Noop),
        }
    }

    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        if action_id == EDIT_ACTION {
            let template = self.ctx.repo.read(&self.name).map_err(io_err)?;
            return Ok(EditorPrep {
                template,
                version: String::new(),
                suffix: ".md".into(),
                file_path: None,
            });
        }
        Err(ContentError::NotSupported(format!(
            "workflow node has no editor for '{action_id}'"
        )))
    }

    async fn form_prep(
        &self,
        action_id: &str,
    ) -> Result<std::collections::HashMap<String, String>> {
        let mut map = std::collections::HashMap::new();
        if action_id == RENAME_ACTION {
            map.insert("name".to_string(), self.name.clone());
        }
        Ok(map)
    }

    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match (action_id, input) {
            (EDIT_ACTION, ActionInput::Edited { text, .. }) => {
                self.ctx.repo.write(&self.name, &text).map_err(io_err)?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Saved workflow {}", self.name)),
                })
            }
            (RENAME_ACTION, input) => {
                let fields = form_fields(input)?;
                let new_name = required(&fields, "name")?;
                self.ctx
                    .repo
                    .rename(&self.name, &new_name)
                    .map_err(io_err)?;
                Ok(ActionOutcome::Navigate {
                    node_id: format!("{WF_PREFIX}{new_name}"),
                    node_type: workflow_type(),
                    message: Some(format!("Renamed {} → {new_name}", self.name)),
                })
            }
            (DELETE_ACTION, _) => {
                self.ctx.repo.delete(&self.name).map_err(io_err)?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Deleted workflow {}", self.name)),
                })
            }
            (RUN_ACTION, _) => self.start_run().await,
            (DIAGRAM_ACTION, _) => {
                let def = self.ctx.repo.load(&self.name).map_err(io_err)?;
                Ok(ActionOutcome::Done {
                    message: Some(crate::mermaid::render(&def)),
                })
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "workflow node has no action '{other}'"
            ))),
        }
    }
}

impl WorkflowNode {
    /// Seed a fresh run with only the definition's entry step as a `pending`
    /// visit; successors are appended as the run is advanced (Phase 6b).
    async fn start_run(&self) -> Result<ActionOutcome> {
        let def = self.ctx.repo.load(&self.name).map_err(io_err)?;
        let steps: Vec<NewStep> = def
            .steps
            .first()
            .map(|s| new_step_from(s, def.mode))
            .into_iter()
            .collect();
        let title = if def.title.is_empty() {
            self.name.clone()
        } else {
            def.title.clone()
        };
        let now = Utc::now().to_rfc3339();
        let run_id = self
            .ctx
            .store
            .create_run(&self.ctx.scope, &self.name, &title, &steps, &now)
            .await?;
        if run_id.is_empty() {
            return Ok(ActionOutcome::Done {
                message: Some(format!(
                    "Run store unavailable — nothing recorded for {}",
                    self.name
                )),
            });
        }
        Ok(ActionOutcome::Navigate {
            node_id: format!("{RUN_PREFIX}{run_id}"),
            node_type: run_type(),
            message: Some(format!("Started run {run_id}")),
        })
    }
}

#[async_trait]
impl Content for WorkflowNode {
    fn node_type(&self) -> &NodeType {
        &self.node_type
    }
    fn version(&self) -> Option<&str> {
        None
    }
    async fn read(&self) -> Result<Vec<u8>> {
        Ok(self.ctx.repo.read(&self.name).map_err(io_err)?.into_bytes())
    }
}

// ---------------------------------------------------------------------------
// Run node
// ---------------------------------------------------------------------------

struct RunNode {
    ctx: Ctx,
    row: RunRow,
    id: String,
    node_type: NodeType,
    metadata: Metadata,
}

impl RunNode {
    fn new(ctx: Ctx, row: RunRow) -> Self {
        let id = format!("{RUN_PREFIX}{}", row.id);
        let metadata = Metadata {
            fields: vec![
                field("status", "Status", row.status.clone()),
                field("created", "Created", row.created_at.clone()),
                field("updated", "Updated", row.updated_at.clone()),
            ],
        };
        Self {
            ctx,
            id,
            node_type: run_type(),
            metadata,
            row,
        }
    }
}

#[async_trait]
impl Node for RunNode {
    fn id(&self) -> &str {
        &self.id
    }
    fn label(&self) -> &str {
        if self.row.title.is_empty() {
            &self.row.id
        } else {
            &self.row.title
        }
    }
    fn node_type(&self) -> &NodeType {
        &self.node_type
    }
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        if let Some(rest) = id.strip_prefix(STEP_PREFIX) {
            let step = resolve_step(&self.ctx, rest)
                .await?
                .ok_or_else(|| ContentError::NotFound(id.to_string()))?;
            return Ok(Box::new(self.ctx.step_node(step)));
        }
        Err(ContentError::NotFound(id.to_string()))
    }

    async fn invoke_action(&self, name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
        match name {
            DELETE_ACTION => Ok(ActionDispatch::DeleteSelf {
                confirm: Some(format!("Delete run '{}'? (y/n)", self.row.id)),
            }),
            _ => Ok(ActionDispatch::Noop),
        }
    }

    async fn execute(&mut self, action_id: &str, _input: ActionInput) -> Result<ActionOutcome> {
        let now = Utc::now().to_rfc3339();
        match action_id {
            ADVANCE_ACTION => match self.ctx.store.next_pending_step(&self.row.id).await? {
                Some(frontier) => {
                    let settled = execute_step(&self.ctx, &frontier).await;
                    let msg =
                        advance_frontier(&self.ctx, &self.row, &frontier, &settled, &now).await?;
                    Ok(ActionOutcome::Done { message: Some(msg) })
                }
                None => Ok(ActionOutcome::NoChanges),
            },
            SKIP_ACTION => match self.ctx.store.next_pending_step(&self.row.id).await? {
                Some(frontier) => {
                    let settled = Settled::skipped();
                    let msg =
                        advance_frontier(&self.ctx, &self.row, &frontier, &settled, &now).await?;
                    Ok(ActionOutcome::Done { message: Some(msg) })
                }
                None => Ok(ActionOutcome::NoChanges),
            },
            RESET_ACTION => {
                let def = self.ctx.repo.load(&self.row.workflow).map_err(io_err)?;
                self.ctx.store.clear_steps(&self.row.id).await?;
                if let Some(entry) = def.steps.first() {
                    self.ctx
                        .store
                        .append_step(&self.row.id, &new_step_from(entry, def.mode))
                        .await?;
                }
                self.ctx
                    .store
                    .set_run_status(&self.row.id, run_status::PENDING, &now)
                    .await?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Reset run {} → pending", self.row.id)),
                })
            }
            DELETE_ACTION => {
                self.ctx.store.delete_run(&self.row.id).await?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Deleted run {}", self.row.id)),
                })
            }
            other => Err(ContentError::NotSupported(format!(
                "run node has no action '{other}'"
            ))),
        }
    }
}

/// Map a definition [`Step`] to a fresh run-step seed, resolving its mode
/// against the workflow default.
fn new_step_from(step: &Step, default_mode: StepMode) -> NewStep {
    NewStep {
        step_id: step.id.clone(),
        title: step.title.clone(),
        mode: step.resolved_mode(default_mode).as_str().to_string(),
        command: step.command.clone().unwrap_or_default(),
        description: step.description.clone(),
    }
}

/// The result of carrying out one step, before it is recorded: its terminal
/// status, captured output, and an optional human note (e.g. why an `ai` step
/// degraded to a plain mark-done).
struct Settled {
    status: &'static str,
    output: String,
    note: String,
    /// The process exit code, or `None` for a step that ran no command
    /// (manual/skipped) or was signalled. Feeds expression route guards.
    exit: Option<i32>,
    /// Captured stdout / stderr, kept separate from `output` for route guards.
    stdout: String,
    stderr: String,
}

impl Settled {
    /// The frontier was skipped rather than executed — treated as success for
    /// routing (a `skipped` step takes the success/`else` path).
    fn skipped() -> Self {
        Self {
            status: step_status::SKIPPED,
            output: String::new(),
            note: String::new(),
            exit: None,
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    /// The guard variables this outcome exposes to expression routes.
    fn guard_vars(&self) -> crate::guard::GuardVars {
        crate::guard::GuardVars {
            exit: self.exit,
            success: self.status == step_status::DONE || self.status == step_status::SKIPPED,
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
        }
    }
}

/// Carry out one step per its recorded mode, **without** touching storage —
/// returns what happened so the caller can record it and route on the outcome.
///
/// * `auto` with a command → run it via [`crate::exec::run_command`].
/// * `ai` with a configured `ai_command` and an instruction → hand the
///   instruction to the AI runner via [`crate::exec::run_ai`].
/// * everything else (manual, an `auto` step that lost its command, an `ai` step
///   with no runner or no instruction) → `done`, mirroring a human ticking the
///   step off (with a note when an `ai` step lacked its prerequisites).
async fn execute_step(ctx: &Ctx, step: &StepRow) -> Settled {
    let mode = StepMode::parse(&step.mode).unwrap_or(StepMode::Manual);

    if mode == StepMode::Auto && !step.command.trim().is_empty() {
        let r = crate::exec::run_command(&step.command).await;
        return Settled {
            status: r.status,
            output: r.output,
            note: String::new(),
            exit: r.exit,
            stdout: r.stdout,
            stderr: r.stderr,
        };
    }

    if mode == StepMode::Ai {
        match ctx.ai_command.as_deref() {
            Some(cmd) if !step.description.trim().is_empty() => {
                let r = crate::exec::run_ai(
                    cmd,
                    crate::exec::AiContext {
                        run_id: &step.run_id,
                        step_id: &step.step_id,
                        title: &step.title,
                        prompt: &step.description,
                    },
                )
                .await;
                return Settled {
                    status: r.status,
                    output: r.output,
                    note: String::new(),
                    exit: r.exit,
                    stdout: r.stdout,
                    stderr: r.stderr,
                };
            }
            _ => {
                let note = if ctx.ai_command.is_none() {
                    " (no ai_command configured)"
                } else {
                    " (ai step has no instruction)"
                };
                return Settled {
                    status: step_status::DONE,
                    output: String::new(),
                    note: note.to_string(),
                    exit: None,
                    stdout: String::new(),
                    stderr: String::new(),
                };
            }
        }
    }

    Settled {
        status: step_status::DONE,
        output: String::new(),
        note: String::new(),
        exit: None,
        stdout: String::new(),
        stderr: String::new(),
    }
}

/// Record a settled frontier step, then evaluate its routes against the outcome
/// to append the successor visit(s) and set the run's resulting status. Returns
/// a human summary of what ran and where the run went next.
async fn advance_frontier(
    ctx: &Ctx,
    run: &RunRow,
    frontier: &StepRow,
    settled: &Settled,
    now: &str,
) -> Result<String> {
    // 1. Freeze this visit's outcome in the protocol.
    ctx.store
        .write_step_result(&run.id, frontier.seq, settled.status, &settled.output, now)
        .await?;

    // 2. Read the current plan and resolve this step's successors.
    let def = ctx.repo.load(&run.workflow).map_err(io_err)?;
    let vars = settled.guard_vars();
    let mut notes: Vec<String> = Vec::new();
    let targets = match def.step(&frontier.step_id) {
        Some(step) => resolve_targets(step, &vars, &def, &mut notes),
        None => {
            notes.push(format!(
                "step '{}' no longer in definition",
                frontier.step_id
            ));
            Vec::new()
        }
    };

    // 3. Apply the successors: append step visits, note terminals.
    let mut appended: Vec<String> = Vec::new();
    let mut reached_end = false;
    let mut reached_fail = false;
    for target in targets {
        match target {
            RouteTarget::End => reached_end = true,
            RouteTarget::Fail => reached_fail = true,
            RouteTarget::Step(id) => match def.step(&id) {
                Some(s) => {
                    if ctx.store.count_step_visits(&run.id, &s.id).await? >= MAX_VISITS {
                        reached_fail = true;
                        notes.push(format!("'{}' exceeded {MAX_VISITS} visits", s.id));
                    } else {
                        ctx.store
                            .append_step(&run.id, &new_step_from(s, def.mode))
                            .await?;
                        appended.push(s.id.clone());
                    }
                }
                None => notes.push(format!("route target '{id}' not found")),
            },
        }
    }

    // 4. Decide the run's status from the control flow (not a pure aggregate):
    //    a routed failure wins; otherwise any queued work keeps it running; a
    //    frontier that itself failed with nothing queued fails the run.
    let final_status = if reached_fail {
        run_status::FAILED
    } else if ctx.store.next_pending_step(&run.id).await?.is_some() {
        run_status::RUNNING
    } else if settled.status == step_status::FAILED {
        run_status::FAILED
    } else {
        run_status::DONE
    };
    ctx.store.set_run_status(&run.id, final_status, now).await?;

    // 5. Human summary.
    let mut msg = format!(
        "Ran step {} ({}) — run {final_status}",
        frontier.step_id, settled.status
    );
    if !appended.is_empty() {
        msg.push_str(&format!("; next: {}", appended.join(", ")));
    }
    if reached_end {
        msg.push_str("; reached end");
    }
    if reached_fail {
        msg.push_str("; routed to fail");
    }
    msg.push_str(&settled.note);
    for note in notes {
        msg.push_str(&format!("; {note}"));
    }
    Ok(msg)
}

/// Choose a step's successor targets: the first route whose guard matches the
/// outcome wins; if none match (or the step declares no routes), fall through to
/// the next step in document order, or [`RouteTarget::End`] when it is the last.
/// An expression guard is evaluated against `vars` (Phase 6b2); a malformed
/// guard is recorded as a note and treated as *not matched* rather than taken.
fn resolve_targets(
    step: &Step,
    vars: &crate::guard::GuardVars,
    def: &WorkflowDef,
    notes: &mut Vec<String>,
) -> Vec<RouteTarget> {
    for route in &step.routes {
        let matched = match &route.condition {
            RouteCondition::Else => true,
            RouteCondition::OnSuccess => vars.success,
            RouteCondition::OnFailure => !vars.success,
            RouteCondition::Expr(e) => match crate::guard::eval(e, vars) {
                Ok(m) => m,
                Err(err) => {
                    notes.push(format!("guard '{e}': {err} (skipped)"));
                    false
                }
            },
        };
        if matched {
            return route.targets.clone();
        }
    }
    match next_in_doc_order(def, &step.id) {
        Some(next) => vec![RouteTarget::Step(next.id.clone())],
        None => vec![RouteTarget::End],
    }
}

/// The step following `current_id` in document order, if any.
fn next_in_doc_order<'a>(def: &'a WorkflowDef, current_id: &str) -> Option<&'a Step> {
    let idx = def.steps.iter().position(|s| s.id == current_id)?;
    def.steps.get(idx + 1)
}

// ---------------------------------------------------------------------------
// Step node (protocol entry leaf)
// ---------------------------------------------------------------------------

struct StepNode {
    row: StepRow,
    id: String,
    node_type: NodeType,
    metadata: Metadata,
}

impl StepNode {
    fn new(row: StepRow) -> Self {
        let id = format!("{STEP_PREFIX}{}:{}", row.run_id, row.seq);
        let metadata = Metadata {
            fields: vec![
                field("seq", "#", row.seq.to_string()),
                field("mode", "Mode", row.mode.clone()),
                field("status", "Status", row.status.clone()),
                field("started", "Started", row.started_at.clone()),
                field("finished", "Finished", row.finished_at.clone()),
            ],
        };
        Self {
            row,
            id,
            node_type: step_type(),
            metadata,
        }
    }
}

#[async_trait]
impl Node for StepNode {
    fn id(&self) -> &str {
        &self.id
    }
    fn label(&self) -> &str {
        &self.row.title
    }
    fn node_type(&self) -> &NodeType {
        &self.node_type
    }
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn content(&self) -> Option<&dyn Content> {
        Some(self)
    }
    async fn execute(&mut self, action_id: &str, _input: ActionInput) -> Result<ActionOutcome> {
        Err(ContentError::NotSupported(format!(
            "step node is read-only — drive the run instead of '{action_id}'"
        )))
    }
}

#[async_trait]
impl Content for StepNode {
    fn node_type(&self) -> &NodeType {
        &self.node_type
    }
    fn version(&self) -> Option<&str> {
        None
    }
    async fn read(&self) -> Result<Vec<u8>> {
        // The execution log once the step has run; until then, the step's
        // instruction, so a pending step still shows what it is meant to do.
        let body = if self.row.output.is_empty() {
            &self.row.description
        } else {
            &self.row.output
        };
        Ok(body.clone().into_bytes())
    }
}

// ---------------------------------------------------------------------------
// Input helpers
// ---------------------------------------------------------------------------

fn form_fields(input: ActionInput) -> Result<std::collections::HashMap<String, String>> {
    match input {
        ActionInput::Form(map) => Ok(map),
        _ => Err(ContentError::Other(
            "workflow action expects form input".into(),
        )),
    }
}

fn required(fields: &std::collections::HashMap<String, String>, key: &str) -> Result<String> {
    match fields.get(key) {
        Some(v) if !v.trim().is_empty() => Ok(v.trim().to_string()),
        _ => Err(ContentError::Other(
            format!("workflow action requires a non-empty `{key}`").into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::children;
    use std::collections::HashMap;

    fn params(nt: NodeType) -> ListParams {
        ListParams {
            node_type: nt,
            query: None,
            sort: Vec::new(),
            page: None,
            download: false,
            group_by: None,
        }
    }

    async fn adapter(db: &str) -> (tempfile::TempDir, WorkflowAdapter) {
        adapter_with(db, None).await
    }

    async fn adapter_with(
        db: &str,
        ai_command: Option<&str>,
    ) -> (tempfile::TempDir, WorkflowAdapter) {
        let t = tempfile::tempdir().unwrap();
        let url = format!("sqlite:file:{db}?mode=memory&cache=shared");
        let store = Arc::new(RunStore::new(Arc::new(
            crate::store::connect(&url).await.unwrap(),
        )));
        let cfg = WorkflowConfig {
            storage_path: Some(t.path().to_string_lossy().into_owned()),
            ai_command: ai_command.map(str::to_string),
            ..Default::default()
        };
        (t, WorkflowAdapter::new("wf".into(), cfg, store))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_list_run_and_delete_full_cycle() {
        let (_t, a) = adapter("wf_adapter_cycle").await;

        // Create a workflow via the root action.
        let mut root = a.get_by_id(ROOT_ID).await.unwrap();
        let out = root
            .execute(
                CREATE_ACTION,
                ActionInput::Form(HashMap::from([("name".into(), "release".into())])),
            )
            .await
            .unwrap();
        match out {
            ActionOutcome::Navigate { node_id, .. } => assert_eq!(node_id, "wf:release"),
            _ => panic!("expected Navigate"),
        }

        // The root lists the new workflow.
        let root = a.root().await.unwrap();
        let listed = children::list(&a, root.as_ref(), params(workflow_type()))
            .await
            .unwrap();
        assert_eq!(listed.items.len(), 1);
        assert_eq!(listed.items[0].id, "wf:release");

        // Start a run; it snapshots the template's single step.
        let mut wf = a.get_by_id("wf:release").await.unwrap();
        let run_id = match wf.execute(RUN_ACTION, ActionInput::None).await.unwrap() {
            ActionOutcome::Navigate { node_id, .. } => node_id,
            _ => panic!("expected Navigate"),
        };
        assert!(run_id.starts_with("run:release-"));

        // The workflow node lists that run.
        let wf = a.get_by_id("wf:release").await.unwrap();
        let runs = children::list(&a, wf.as_ref(), params(run_type()))
            .await
            .unwrap();
        assert_eq!(runs.items.len(), 1);

        // The run node lists its step protocol (one pending step).
        let run = a.get_by_id(&run_id).await.unwrap();
        let steps = children::list(&a, run.as_ref(), params(step_type()))
            .await
            .unwrap();
        assert_eq!(steps.items.len(), 1);
        assert_eq!(steps.items[0].metadata.fields[2].value, "pending");

        // Editing rewrites the markdown; content reads it back.
        let mut wf = a.get_by_id("wf:release").await.unwrap();
        wf.execute(
            EDIT_ACTION,
            ActionInput::Edited {
                text: "---\ntitle: Rel\n---\n## Only\nx\n".into(),
                original: String::new(),
                version: String::new(),
            },
        )
        .await
        .unwrap();
        let wf = a.get_by_id("wf:release").await.unwrap();
        let body = wf.content().unwrap().read_text().await.unwrap();
        assert!(body.contains("title: Rel"));

        // Delete the workflow → the root lists nothing.
        let mut wf = a.get_by_id("wf:release").await.unwrap();
        wf.execute(DELETE_ACTION, ActionInput::None).await.unwrap();
        let root = a.root().await.unwrap();
        let listed = children::list(&a, root.as_ref(), params(workflow_type()))
            .await
            .unwrap();
        assert!(listed.items.is_empty());
    }

    /// Drive a linear (routeless) run through the node protocol: it starts with
    /// only the entry step, `advance` appends the next in document order, and
    /// `reset` returns it to a single fresh entry visit.
    #[tokio::test(flavor = "multi_thread")]
    async fn manual_run_advances_through_nodes() {
        let (_t, a) = adapter("wf_adapter_manual").await;

        // Create a two-step workflow.
        let mut root = a.get_by_id(ROOT_ID).await.unwrap();
        root.execute(
            CREATE_ACTION,
            ActionInput::Form(HashMap::from([("name".into(), "flow".into())])),
        )
        .await
        .unwrap();
        let mut wf = a.get_by_id("wf:flow").await.unwrap();
        wf.execute(
            EDIT_ACTION,
            ActionInput::Edited {
                text: "---\ntitle: Flow\n---\n## One\na\n## Two\nb\n".into(),
                original: String::new(),
                version: String::new(),
            },
        )
        .await
        .unwrap();

        // Start a run — only the entry step is seeded.
        let mut wf = a.get_by_id("wf:flow").await.unwrap();
        let run_id = match wf.execute(RUN_ACTION, ActionInput::None).await.unwrap() {
            ActionOutcome::Navigate { node_id, .. } => node_id,
            _ => panic!("expected Navigate"),
        };
        let run = a.get_by_id(&run_id).await.unwrap();
        let steps = children::list(&a, run.as_ref(), params(step_type()))
            .await
            .unwrap();
        assert_eq!(steps.items.len(), 1);

        // Advance completes "one" and appends "two"; the run is now running.
        let mut run = a.get_by_id(&run_id).await.unwrap();
        match run
            .execute(ADVANCE_ACTION, ActionInput::None)
            .await
            .unwrap()
        {
            ActionOutcome::Done { .. } => {}
            _ => panic!("expected Done"),
        }
        let run_node = a.get_by_id(&run_id).await.unwrap();
        assert_eq!(run_node.metadata().fields[0].value, "running");
        let run = a.get_by_id(&run_id).await.unwrap();
        let steps = children::list(&a, run.as_ref(), params(step_type()))
            .await
            .unwrap();
        assert_eq!(steps.items.len(), 2);

        // Advance completes "two"; it is the last step, so the run is done.
        let mut run = a.get_by_id(&run_id).await.unwrap();
        run.execute(ADVANCE_ACTION, ActionInput::None)
            .await
            .unwrap();
        let run_node = a.get_by_id(&run_id).await.unwrap();
        assert_eq!(run_node.metadata().fields[0].value, "done");

        // Advancing past the end is a no-op.
        let mut run = a.get_by_id(&run_id).await.unwrap();
        match run
            .execute(ADVANCE_ACTION, ActionInput::None)
            .await
            .unwrap()
        {
            ActionOutcome::NoChanges => {}
            _ => panic!("expected NoChanges past the end"),
        }

        // Reset takes the run back to a single pending entry visit.
        let mut run = a.get_by_id(&run_id).await.unwrap();
        run.execute(RESET_ACTION, ActionInput::None).await.unwrap();
        let run_node = a.get_by_id(&run_id).await.unwrap();
        assert_eq!(run_node.metadata().fields[0].value, "pending");
        let run = a.get_by_id(&run_id).await.unwrap();
        let steps = children::list(&a, run.as_ref(), params(step_type()))
            .await
            .unwrap();
        assert_eq!(steps.items.len(), 1);
    }

    /// A workflow that spins up a fresh run, edits in the given markdown, and
    /// returns the run node id — the shared preamble of the routing tests.
    async fn started_run(a: &WorkflowAdapter, name: &str, md: &str) -> String {
        let mut root = a.get_by_id(ROOT_ID).await.unwrap();
        root.execute(
            CREATE_ACTION,
            ActionInput::Form(HashMap::from([("name".into(), name.to_string())])),
        )
        .await
        .unwrap();
        let mut wf = a.get_by_id(&format!("wf:{name}")).await.unwrap();
        wf.execute(
            EDIT_ACTION,
            ActionInput::Edited {
                text: md.to_string(),
                original: String::new(),
                version: String::new(),
            },
        )
        .await
        .unwrap();
        let mut wf = a.get_by_id(&format!("wf:{name}")).await.unwrap();
        match wf.execute(RUN_ACTION, ActionInput::None).await.unwrap() {
            ActionOutcome::Navigate { node_id, .. } => node_id,
            _ => panic!("expected Navigate"),
        }
    }

    /// A successful step with only an `on_failure` route falls through to the
    /// next step in document order; the last step's `else: end` finishes the run.
    #[tokio::test(flavor = "multi_thread")]
    async fn routing_success_falls_through_then_ends() {
        let (_t, a) = adapter("wf_adapter_route_ok").await;
        let md = "---\ntitle: Gate\nmode: auto\n---\n\
## Build\n\n```command\nexit 0\n```\n\n```yaml routing\non_failure: fail\n```\n\
## Test\n\n```command\necho ok\n```\n\n```yaml routing\nelse: end\n```\n";
        let run_id = started_run(&a, "gate", md).await;

        // Build succeeds → no failure route → linear fall-through appends Test.
        let mut run = a.get_by_id(&run_id).await.unwrap();
        run.execute(ADVANCE_ACTION, ActionInput::None)
            .await
            .unwrap();
        let run_node = a.get_by_id(&run_id).await.unwrap();
        assert_eq!(run_node.metadata().fields[0].value, "running");
        let run = a.get_by_id(&run_id).await.unwrap();
        let steps = children::list(&a, run.as_ref(), params(step_type()))
            .await
            .unwrap();
        assert_eq!(steps.items.len(), 2);

        // Test's else route ends the run.
        let mut run = a.get_by_id(&run_id).await.unwrap();
        run.execute(ADVANCE_ACTION, ActionInput::None)
            .await
            .unwrap();
        let run_node = a.get_by_id(&run_id).await.unwrap();
        assert_eq!(run_node.metadata().fields[0].value, "done");
    }

    /// A failing step's `on_failure: fail` route fails the run without queueing
    /// the fall-through successor.
    #[tokio::test(flavor = "multi_thread")]
    async fn routing_failure_routes_to_fail() {
        let (_t, a) = adapter("wf_adapter_route_fail").await;
        let md = "---\ntitle: Gate\nmode: auto\n---\n\
## Build\n\n```command\nexit 1\n```\n\n```yaml routing\non_failure: fail\n```\n\
## Test\n\n```command\necho ok\n```\n\n```yaml routing\nelse: end\n```\n";
        let run_id = started_run(&a, "gate", md).await;

        let mut run = a.get_by_id(&run_id).await.unwrap();
        match run
            .execute(ADVANCE_ACTION, ActionInput::None)
            .await
            .unwrap()
        {
            ActionOutcome::Done { message } => assert!(message.unwrap().contains("routed to fail")),
            _ => panic!("expected Done"),
        }
        let run_node = a.get_by_id(&run_id).await.unwrap();
        assert_eq!(run_node.metadata().fields[0].value, "failed");
        // Only the failed Build visit exists — Test was never queued.
        let run = a.get_by_id(&run_id).await.unwrap();
        let steps = children::list(&a, run.as_ref(), params(step_type()))
            .await
            .unwrap();
        assert_eq!(steps.items.len(), 1);
        assert_eq!(steps.items[0].metadata.fields[2].value, "failed");
    }

    /// A self-referential `else` route appends a fresh visit each advance,
    /// proving loops grow the protocol rather than collapsing onto one row.
    #[tokio::test(flavor = "multi_thread")]
    async fn routing_loop_back_appends_repeated_visits() {
        let (_t, a) = adapter("wf_adapter_route_loop").await;
        let md = "---\ntitle: Loopy\n---\n## Loop\n\ndo it\n\n```yaml routing\nelse: loop\n```\n";
        let run_id = started_run(&a, "loopy", md).await;

        // Two advances → three visits of "loop" (row0 done, row1 done, row2 pending).
        let mut run = a.get_by_id(&run_id).await.unwrap();
        run.execute(ADVANCE_ACTION, ActionInput::None)
            .await
            .unwrap();
        run.execute(ADVANCE_ACTION, ActionInput::None)
            .await
            .unwrap();
        let run_node = a.get_by_id(&run_id).await.unwrap();
        assert_eq!(run_node.metadata().fields[0].value, "running");
        let run = a.get_by_id(&run_id).await.unwrap();
        let steps = children::list(&a, run.as_ref(), params(step_type()))
            .await
            .unwrap();
        assert_eq!(steps.items.len(), 3);
    }

    /// An expression guard (`exit == 0`) is evaluated against the step outcome:
    /// a zero exit takes the guarded branch instead of the linear fall-through.
    #[tokio::test(flavor = "multi_thread")]
    async fn routing_expr_guard_selects_branch() {
        let (_t, a) = adapter("wf_adapter_route_expr").await;
        let md = "---\ntitle: Gate\nmode: auto\n---\n\
## Build\n\n```command\nexit 0\n```\n\n```yaml routing\nexit == 0: deploy\nelse: fail\n```\n\
## Skip\n\ndo nothing\n\n## Deploy\n\n```command\necho shipped\n```\n\n```yaml routing\nelse: end\n```\n";
        let run_id = started_run(&a, "gate", md).await;

        // Build exits 0 → `exit == 0` matches → Deploy is queued (Skip is jumped).
        let mut run = a.get_by_id(&run_id).await.unwrap();
        run.execute(ADVANCE_ACTION, ActionInput::None)
            .await
            .unwrap();
        let run = a.get_by_id(&run_id).await.unwrap();
        let steps = children::list(&a, run.as_ref(), params(step_type()))
            .await
            .unwrap();
        assert_eq!(steps.items.len(), 2);
        assert_eq!(steps.items[1].label, "Deploy");

        // Deploy's else route finishes the run.
        let mut run = a.get_by_id(&run_id).await.unwrap();
        run.execute(ADVANCE_ACTION, ActionInput::None)
            .await
            .unwrap();
        let run_node = a.get_by_id(&run_id).await.unwrap();
        assert_eq!(run_node.metadata().fields[0].value, "done");
    }

    /// Advancing an `auto` step runs its command and captures the output.
    #[tokio::test(flavor = "multi_thread")]
    async fn auto_step_runs_command() {
        let (_t, a) = adapter("wf_adapter_auto").await;

        let mut root = a.get_by_id(ROOT_ID).await.unwrap();
        root.execute(
            CREATE_ACTION,
            ActionInput::Form(HashMap::from([("name".into(), "auto".into())])),
        )
        .await
        .unwrap();
        let mut wf = a.get_by_id("wf:auto").await.unwrap();
        wf.execute(
            EDIT_ACTION,
            ActionInput::Edited {
                text:
                    "---\ntitle: Auto\nmode: auto\n---\n## Greet\n\n```command\necho phase4\n```\n"
                        .into(),
                original: String::new(),
                version: String::new(),
            },
        )
        .await
        .unwrap();

        let mut wf = a.get_by_id("wf:auto").await.unwrap();
        let run_id = match wf.execute(RUN_ACTION, ActionInput::None).await.unwrap() {
            ActionOutcome::Navigate { node_id, .. } => node_id,
            _ => panic!("expected Navigate"),
        };

        // Advancing runs the command; the run completes.
        let mut run = a.get_by_id(&run_id).await.unwrap();
        run.execute(ADVANCE_ACTION, ActionInput::None)
            .await
            .unwrap();
        let run_node = a.get_by_id(&run_id).await.unwrap();
        assert_eq!(run_node.metadata().fields[0].value, "done");

        // The step recorded status done and captured stdout.
        let step_id = run_id.replacen("run:", "step:", 1) + ":0";
        let step = a.get_by_id(&step_id).await.unwrap();
        let body = step.content().unwrap().read_text().await.unwrap();
        assert!(body.contains("phase4"));
        // Status field on the step metadata is index 2.
        assert_eq!(step.metadata().fields[2].value, "done");
    }

    /// An `ai` step runs the configured `ai_command`, handed the step's
    /// instruction as the prompt on stdin.
    #[tokio::test(flavor = "multi_thread")]
    async fn ai_step_runs_configured_runner() {
        // The runner echoes its prompt back so we can assert it was delivered.
        let (_t, a) = adapter_with("wf_adapter_ai", Some("cat")).await;

        let mut root = a.get_by_id(ROOT_ID).await.unwrap();
        root.execute(
            CREATE_ACTION,
            ActionInput::Form(HashMap::from([("name".into(), "ai".into())])),
        )
        .await
        .unwrap();
        let mut wf = a.get_by_id("wf:ai").await.unwrap();
        wf.execute(
            EDIT_ACTION,
            ActionInput::Edited {
                text: "---\ntitle: AI\nmode: ai\n---\n## Think\n\nSummarise the release notes.\n"
                    .into(),
                original: String::new(),
                version: String::new(),
            },
        )
        .await
        .unwrap();

        let mut wf = a.get_by_id("wf:ai").await.unwrap();
        let run_id = match wf.execute(RUN_ACTION, ActionInput::None).await.unwrap() {
            ActionOutcome::Navigate { node_id, .. } => node_id,
            _ => panic!("expected Navigate"),
        };

        let mut run = a.get_by_id(&run_id).await.unwrap();
        run.execute(ADVANCE_ACTION, ActionInput::None)
            .await
            .unwrap();

        let run_node = a.get_by_id(&run_id).await.unwrap();
        assert_eq!(run_node.metadata().fields[0].value, "done");

        // The step captured the prompt the runner echoed back.
        let step_id = run_id.replacen("run:", "step:", 1) + ":0";
        let step = a.get_by_id(&step_id).await.unwrap();
        let body = step.content().unwrap().read_text().await.unwrap();
        assert!(body.contains("Summarise the release notes."));
    }

    /// Without a configured runner an `ai` step degrades to a manual mark-done.
    #[tokio::test(flavor = "multi_thread")]
    async fn ai_step_without_runner_marks_done() {
        let (_t, a) = adapter("wf_adapter_ai_norunner").await;

        let mut root = a.get_by_id(ROOT_ID).await.unwrap();
        root.execute(
            CREATE_ACTION,
            ActionInput::Form(HashMap::from([("name".into(), "ai".into())])),
        )
        .await
        .unwrap();
        let mut wf = a.get_by_id("wf:ai").await.unwrap();
        wf.execute(
            EDIT_ACTION,
            ActionInput::Edited {
                text: "---\ntitle: AI\nmode: ai\n---\n## Think\n\nDo a thing.\n".into(),
                original: String::new(),
                version: String::new(),
            },
        )
        .await
        .unwrap();

        let mut wf = a.get_by_id("wf:ai").await.unwrap();
        let run_id = match wf.execute(RUN_ACTION, ActionInput::None).await.unwrap() {
            ActionOutcome::Navigate { node_id, .. } => node_id,
            _ => panic!("expected Navigate"),
        };
        let mut run = a.get_by_id(&run_id).await.unwrap();
        match run
            .execute(ADVANCE_ACTION, ActionInput::None)
            .await
            .unwrap()
        {
            ActionOutcome::Done { message } => {
                assert!(message.unwrap().contains("no ai_command configured"))
            }
            _ => panic!("expected Done"),
        }
        let run_node = a.get_by_id(&run_id).await.unwrap();
        assert_eq!(run_node.metadata().fields[0].value, "done");
    }
}
