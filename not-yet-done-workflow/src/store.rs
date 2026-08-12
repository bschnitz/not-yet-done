//! [`RunStore`] — the SQLite store of workflow runs and their per-step protocol.
//!
//! Mirrors the custom-columns store: a connection is opened once and schema-
//! synced (scoped to this crate's entities, so a shared database is safe), and
//! the store holds an `Arc<DatabaseConnection>` whose query methods await plain
//! `Send` sea-orm futures. A `None` connection is the inert fallback — every
//! method no-ops or returns empty, so a store that failed to open degrades to
//! "no run history" rather than breaking adapter construction.
//!
//! Definitions stay on disk (the `.md` files, via [`crate::repo`]); only *runs*
//! and *what happened during them* live here.

use std::path::PathBuf;
use std::sync::Arc;

use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, Database, DatabaseConnection, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder,
};

use not_yet_done_content::{ContentError, Result};

use crate::entity::{run_step, workflow_run};

/// Run lifecycle states (stored verbatim in `workflow_run.status`).
pub mod run_status {
    pub const PENDING: &str = "pending";
    pub const RUNNING: &str = "running";
    pub const DONE: &str = "done";
    pub const FAILED: &str = "failed";
    pub const CANCELLED: &str = "cancelled";
}

/// Step-entry lifecycle states (stored verbatim in `run_step.status`).
pub mod step_status {
    pub const PENDING: &str = "pending";
    pub const RUNNING: &str = "running";
    pub const DONE: &str = "done";
    pub const SKIPPED: &str = "skipped";
    pub const FAILED: &str = "failed";
}

/// A run row, as handed to the adapter for listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRow {
    pub id: String,
    pub workflow: String,
    pub title: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<workflow_run::Model> for RunRow {
    fn from(m: workflow_run::Model) -> Self {
        Self {
            id: m.id,
            workflow: m.workflow,
            title: m.title,
            status: m.status,
            created_at: m.created_at,
            updated_at: m.updated_at,
        }
    }
}

/// A step-entry row of a run's protocol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepRow {
    pub run_id: String,
    pub seq: i32,
    pub step_id: String,
    pub title: String,
    pub mode: String,
    pub command: String,
    pub description: String,
    pub status: String,
    pub output: String,
    pub started_at: String,
    pub finished_at: String,
}

impl From<run_step::Model> for StepRow {
    fn from(m: run_step::Model) -> Self {
        Self {
            run_id: m.run_id,
            seq: m.seq,
            step_id: m.step_id,
            title: m.title,
            mode: m.mode,
            command: m.command,
            description: m.description,
            status: m.status,
            output: m.output,
            started_at: m.started_at,
            finished_at: m.finished_at,
        }
    }
}

/// One step to seed into a fresh run — its definition id, title, and resolved
/// mode. All entries start `pending`.
#[derive(Clone, Debug)]
pub struct NewStep {
    pub step_id: String,
    pub title: String,
    pub mode: String,
    pub command: String,
    pub description: String,
}

/// Open a sea-orm connection and schema-sync this crate's entities. Scoped to
/// `not_yet_done_workflow::entity::*`, so a shared database's other tables are
/// left untouched.
pub async fn connect(url: &str) -> std::result::Result<DatabaseConnection, sea_orm::DbErr> {
    let db = Database::connect(url).await?;
    db.get_schema_registry("not_yet_done_workflow::entity::*")
        .sync(&db)
        .await?;
    Ok(db)
}

/// The default run-store URL: `<XDG_DATA_HOME>/not_yet_done/workflow_runs.sqlite`,
/// creating the parent directory. `mode=rwc` so the file is created on first
/// connect.
pub fn default_sqlite_url() -> Result<String> {
    let dir: PathBuf = dirs::data_local_dir()
        .ok_or_else(|| ContentError::Other("cannot resolve XDG data-local dir".into()))?
        .join("not_yet_done");
    std::fs::create_dir_all(&dir)
        .map_err(|e| ContentError::Other(format!("create {}: {e}", dir.display()).into()))?;
    let path = dir.join("workflow_runs.sqlite");
    Ok(format!("sqlite://{}?mode=rwc", path.display()))
}

/// Normalise a config `database:` value into a sea-orm URL: an existing
/// `sqlite:`/`postgres:`/`mysql:` scheme is used as-is, a bare path becomes a
/// `sqlite://<path>?mode=rwc`.
pub fn normalize_db_url(value: &str) -> String {
    let v = value.trim();
    if v.contains("://") || v.starts_with("sqlite:") {
        v.to_string()
    } else {
        format!("sqlite://{v}?mode=rwc")
    }
}

/// Bridge async [`connect`] into a sync caller via `block_in_place` + the
/// current Tokio handle — the path a synchronous [`AdapterFactory::create`]
/// takes to open the store. Requires a multi-threaded runtime (every front-end
/// runs one), exactly as the custom-columns and Jira factories do.
pub fn open_blocking(url: &str) -> std::result::Result<std::sync::Arc<DatabaseConnection>, String> {
    let handle = tokio::runtime::Handle::try_current()
        .map_err(|_| "workflow store needs a Tokio runtime".to_string())?;
    if handle.runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
        return Err("workflow store needs a multi-threaded Tokio runtime".into());
    }
    let url_owned = url.to_string();
    tokio::task::block_in_place(|| handle.block_on(connect(&url_owned)))
        .map(Arc::new)
        .map_err(|e| format!("open workflow db ({url_owned}): {e}"))
}

/// The run/protocol store over an opened, schema-synced connection.
pub struct RunStore {
    conn: Option<Arc<DatabaseConnection>>,
}

impl RunStore {
    /// A store over an already-opened connection.
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn: Some(conn) }
    }

    /// An inert store that persists nothing and lists no runs — the degraded
    /// fallback when a connection can't be opened.
    pub fn inert() -> Self {
        Self { conn: None }
    }

    /// Whether this store is backed by a real connection.
    pub fn is_live(&self) -> bool {
        self.conn.is_some()
    }

    fn conn(&self) -> Option<&DatabaseConnection> {
        self.conn.as_deref()
    }

    /// Create a fresh run of `workflow` (title snapshotted), seeding every step
    /// as a `pending` entry in order. Returns the new run id. On an inert store
    /// this is a no-op returning an empty id.
    pub async fn create_run(
        &self,
        scope: &str,
        workflow: &str,
        title: &str,
        steps: &[NewStep],
        now_rfc3339: &str,
    ) -> Result<String> {
        let Some(db) = self.conn() else {
            return Ok(String::new());
        };
        let id = format!("{workflow}-{now_rfc3339}");
        let run = workflow_run::ActiveModel {
            id: Set(id.clone()),
            scope: Set(scope.to_string()),
            workflow: Set(workflow.to_string()),
            title: Set(title.to_string()),
            status: Set(run_status::PENDING.to_string()),
            created_at: Set(now_rfc3339.to_string()),
            updated_at: Set(now_rfc3339.to_string()),
        };
        workflow_run::Entity::insert(run)
            .exec(db)
            .await
            .map_err(db_err)?;

        if !steps.is_empty() {
            let rows: Vec<run_step::ActiveModel> = steps
                .iter()
                .enumerate()
                .map(|(i, s)| run_step::ActiveModel {
                    run_id: Set(id.clone()),
                    seq: Set(i as i32),
                    step_id: Set(s.step_id.clone()),
                    title: Set(s.title.clone()),
                    mode: Set(s.mode.clone()),
                    command: Set(s.command.clone()),
                    description: Set(s.description.clone()),
                    status: Set(step_status::PENDING.to_string()),
                    output: Set(String::new()),
                    started_at: Set(String::new()),
                    finished_at: Set(String::new()),
                })
                .collect();
            run_step::Entity::insert_many(rows)
                .exec(db)
                .await
                .map_err(db_err)?;
        }
        Ok(id)
    }

    /// List a workflow's runs within `scope`, newest first.
    pub async fn list_runs(&self, scope: &str, workflow: &str) -> Result<Vec<RunRow>> {
        let Some(db) = self.conn() else {
            return Ok(Vec::new());
        };
        let models = workflow_run::Entity::find()
            .filter(workflow_run::Column::Scope.eq(scope))
            .filter(workflow_run::Column::Workflow.eq(workflow))
            .order_by_desc(workflow_run::Column::CreatedAt)
            .all(db)
            .await
            .map_err(db_err)?;
        Ok(models.into_iter().map(RunRow::from).collect())
    }

    /// Fetch a single run by id.
    pub async fn get_run(&self, id: &str) -> Result<Option<RunRow>> {
        let Some(db) = self.conn() else {
            return Ok(None);
        };
        let model = workflow_run::Entity::find_by_id(id.to_string())
            .one(db)
            .await
            .map_err(db_err)?;
        Ok(model.map(RunRow::from))
    }

    /// List a run's step-entry protocol, in execution order.
    pub async fn list_steps(&self, run_id: &str) -> Result<Vec<StepRow>> {
        let Some(db) = self.conn() else {
            return Ok(Vec::new());
        };
        let models = run_step::Entity::find()
            .filter(run_step::Column::RunId.eq(run_id))
            .order_by_asc(run_step::Column::Seq)
            .all(db)
            .await
            .map_err(db_err)?;
        Ok(models.into_iter().map(StepRow::from).collect())
    }

    /// Delete a run and its step protocol.
    pub async fn delete_run(&self, id: &str) -> Result<()> {
        let Some(db) = self.conn() else {
            return Ok(());
        };
        run_step::Entity::delete_many()
            .filter(run_step::Column::RunId.eq(id))
            .exec(db)
            .await
            .map_err(db_err)?;
        workflow_run::Entity::delete_by_id(id.to_string())
            .exec(db)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    // -- Manual-execution state machine (Phase 3) ---------------------------

    /// Set one step's status (stamping start/finish timestamps as the transition
    /// implies), then recompute and persist the owning run's aggregate status.
    /// Returns the run's new status, or `None` if the step (or store) is absent.
    pub async fn set_step_status(
        &self,
        run_id: &str,
        seq: i32,
        status: &str,
        now: &str,
    ) -> Result<Option<String>> {
        let Some(db) = self.conn() else {
            return Ok(None);
        };
        let Some(model) = run_step::Entity::find_by_id((run_id.to_string(), seq))
            .one(db)
            .await
            .map_err(db_err)?
        else {
            return Ok(None);
        };
        let started = model.started_at.clone();
        let mut am: run_step::ActiveModel = model.into();
        stamp_step(&mut am, status, &started, now);
        am.update(db).await.map_err(db_err)?;
        Ok(Some(recompute_run(db, run_id, now).await?))
    }

    /// The run's first non-terminal step (`pending`/`running`) in `seq` order,
    /// without mutating anything — the step a mode-aware "carry out the next
    /// step" driver acts on. `None` when the run is finished or the store inert.
    pub async fn next_pending_step(&self, run_id: &str) -> Result<Option<StepRow>> {
        let Some(db) = self.conn() else {
            return Ok(None);
        };
        let model = run_step::Entity::find()
            .filter(run_step::Column::RunId.eq(run_id))
            .filter(run_step::Column::Status.is_in([step_status::PENDING, step_status::RUNNING]))
            .order_by_asc(run_step::Column::Seq)
            .one(db)
            .await
            .map_err(db_err)?;
        Ok(model.map(StepRow::from))
    }

    /// Persist an executed step's terminal `status` and captured `output`,
    /// stamping timestamps, **without** touching the run's aggregate status —
    /// the caller decides the run's next state (the routing-aware
    /// append-per-visit driver in the adapter does this so it can set the run to
    /// `running` after queueing successors, `done`/`failed` at a terminal).
    /// Returns whether the step row existed.
    pub async fn write_step_result(
        &self,
        run_id: &str,
        seq: i32,
        status: &str,
        output: &str,
        now: &str,
    ) -> Result<bool> {
        let Some(db) = self.conn() else {
            return Ok(false);
        };
        let Some(model) = run_step::Entity::find_by_id((run_id.to_string(), seq))
            .one(db)
            .await
            .map_err(db_err)?
        else {
            return Ok(false);
        };
        let started = model.started_at.clone();
        let mut am: run_step::ActiveModel = model.into();
        stamp_step(&mut am, status, &started, now);
        am.output = Set(output.to_string());
        am.update(db).await.map_err(db_err)?;
        Ok(true)
    }

    /// Record an executed step's result — its terminal `status` and captured
    /// `output` — stamping timestamps and recomputing the run's aggregate status.
    /// Returns the run's new status, or `None` if the step (or store) is absent.
    pub async fn record_step_result(
        &self,
        run_id: &str,
        seq: i32,
        status: &str,
        output: &str,
        now: &str,
    ) -> Result<Option<String>> {
        if !self
            .write_step_result(run_id, seq, status, output, now)
            .await?
        {
            return Ok(None);
        }
        let Some(db) = self.conn() else {
            return Ok(None);
        };
        Ok(Some(recompute_run(db, run_id, now).await?))
    }

    // -- Dynamic append-per-visit (Phase 6b) --------------------------------

    /// Append a fresh `pending` step-visit row to the tail of a run's protocol,
    /// its `seq` one past the current maximum (so the same definition step can be
    /// visited — and recorded — more than once across a loop). Returns the new
    /// `seq`, or `0` on an inert store.
    pub async fn append_step(&self, run_id: &str, step: &NewStep) -> Result<i32> {
        let Some(db) = self.conn() else {
            return Ok(0);
        };
        let last = run_step::Entity::find()
            .filter(run_step::Column::RunId.eq(run_id))
            .order_by_desc(run_step::Column::Seq)
            .one(db)
            .await
            .map_err(db_err)?;
        let seq = last.map(|m| m.seq + 1).unwrap_or(0);
        let am = run_step::ActiveModel {
            run_id: Set(run_id.to_string()),
            seq: Set(seq),
            step_id: Set(step.step_id.clone()),
            title: Set(step.title.clone()),
            mode: Set(step.mode.clone()),
            command: Set(step.command.clone()),
            description: Set(step.description.clone()),
            status: Set(step_status::PENDING.to_string()),
            output: Set(String::new()),
            started_at: Set(String::new()),
            finished_at: Set(String::new()),
        };
        run_step::Entity::insert(am)
            .exec(db)
            .await
            .map_err(db_err)?;
        Ok(seq)
    }

    /// How many times a definition step has been visited (recorded as a row) in
    /// this run — the loop-cycle guard's counter.
    pub async fn count_step_visits(&self, run_id: &str, step_id: &str) -> Result<u64> {
        let Some(db) = self.conn() else {
            return Ok(0);
        };
        run_step::Entity::find()
            .filter(run_step::Column::RunId.eq(run_id))
            .filter(run_step::Column::StepId.eq(step_id))
            .count(db)
            .await
            .map_err(db_err)
    }

    /// Set a run's aggregate status directly (used by the routing driver, which
    /// decides `running`/`done`/`failed` from the control flow rather than a
    /// pure per-step aggregate). Stamps `updated_at`. No-op on an inert store.
    pub async fn set_run_status(&self, run_id: &str, status: &str, now: &str) -> Result<()> {
        let Some(db) = self.conn() else {
            return Ok(());
        };
        if let Some(run) = workflow_run::Entity::find_by_id(run_id.to_string())
            .one(db)
            .await
            .map_err(db_err)?
        {
            let mut am: workflow_run::ActiveModel = run.into();
            am.status = Set(status.to_string());
            am.updated_at = Set(now.to_string());
            am.update(db).await.map_err(db_err)?;
        }
        Ok(())
    }

    /// Delete every step row of a run, leaving the run itself. The routing
    /// driver uses this to `reset` a run back to a single freshly-seeded entry.
    pub async fn clear_steps(&self, run_id: &str) -> Result<()> {
        let Some(db) = self.conn() else {
            return Ok(());
        };
        run_step::Entity::delete_many()
            .filter(run_step::Column::RunId.eq(run_id))
            .exec(db)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Advance a run by applying `status` (`done` to complete, `skipped` to skip)
    /// to its first non-terminal step in `seq` order, then recompute the run.
    /// Returns the affected `seq` and the run's new status, or `None` when no
    /// step remained to advance (or the store is inert).
    pub async fn advance_run(
        &self,
        run_id: &str,
        status: &str,
        now: &str,
    ) -> Result<Option<(i32, String)>> {
        let Some(db) = self.conn() else {
            return Ok(None);
        };
        let next = run_step::Entity::find()
            .filter(run_step::Column::RunId.eq(run_id))
            .filter(run_step::Column::Status.is_in([step_status::PENDING, step_status::RUNNING]))
            .order_by_asc(run_step::Column::Seq)
            .one(db)
            .await
            .map_err(db_err)?;
        let Some(model) = next else {
            return Ok(None);
        };
        let seq = model.seq;
        let started = model.started_at.clone();
        let mut am: run_step::ActiveModel = model.into();
        stamp_step(&mut am, status, &started, now);
        am.update(db).await.map_err(db_err)?;
        let run_status = recompute_run(db, run_id, now).await?;
        Ok(Some((seq, run_status)))
    }

    /// Reset every step of a run back to `pending` (clearing output/timestamps)
    /// and the run itself to `pending`. Returns the run's new status.
    pub async fn reset_run(&self, run_id: &str, now: &str) -> Result<String> {
        let Some(db) = self.conn() else {
            return Ok(String::new());
        };
        let steps = run_step::Entity::find()
            .filter(run_step::Column::RunId.eq(run_id))
            .all(db)
            .await
            .map_err(db_err)?;
        for model in steps {
            let started = model.started_at.clone();
            let mut am: run_step::ActiveModel = model.into();
            stamp_step(&mut am, step_status::PENDING, &started, now);
            am.update(db).await.map_err(db_err)?;
        }
        recompute_run(db, run_id, now).await
    }
}

fn db_err(e: sea_orm::DbErr) -> ContentError {
    ContentError::Other(Box::new(e))
}

/// Apply a step-status transition to an active model, stamping the timestamps the
/// transition implies (idempotent on the `started_at` stamp — it is only set the
/// first time the step leaves `pending`). Resetting to `pending` clears output
/// and both timestamps so a re-run starts from a clean slate.
fn stamp_step(am: &mut run_step::ActiveModel, status: &str, started_at: &str, now: &str) {
    am.status = Set(status.to_string());
    match status {
        step_status::RUNNING => {
            if started_at.is_empty() {
                am.started_at = Set(now.to_string());
            }
        }
        step_status::DONE | step_status::FAILED => {
            if started_at.is_empty() {
                am.started_at = Set(now.to_string());
            }
            am.finished_at = Set(now.to_string());
        }
        step_status::SKIPPED => {
            am.finished_at = Set(now.to_string());
        }
        step_status::PENDING => {
            am.output = Set(String::new());
            am.started_at = Set(String::new());
            am.finished_at = Set(String::new());
        }
        _ => {}
    }
}

/// Aggregate a run's status from its step statuses: any `failed` fails the run;
/// all steps terminal (`done`/`skipped`) completes it; any progress at all
/// (something done/skipped/running) marks it `running`; otherwise it is still
/// `pending`. An empty run stays `pending`.
fn aggregate_run_status(steps: &[run_step::Model]) -> &'static str {
    if steps.is_empty() {
        return run_status::PENDING;
    }
    if steps.iter().any(|s| s.status == step_status::FAILED) {
        return run_status::FAILED;
    }
    let all_terminal = steps
        .iter()
        .all(|s| s.status == step_status::DONE || s.status == step_status::SKIPPED);
    if all_terminal {
        return run_status::DONE;
    }
    let any_progress = steps.iter().any(|s| {
        s.status == step_status::DONE
            || s.status == step_status::SKIPPED
            || s.status == step_status::RUNNING
    });
    if any_progress {
        run_status::RUNNING
    } else {
        run_status::PENDING
    }
}

/// Recompute a run's aggregate status from its current steps and persist it
/// (stamping `updated_at`). Returns the new status.
async fn recompute_run(db: &DatabaseConnection, run_id: &str, now: &str) -> Result<String> {
    let steps = run_step::Entity::find()
        .filter(run_step::Column::RunId.eq(run_id))
        .all(db)
        .await
        .map_err(db_err)?;
    let status = aggregate_run_status(&steps);
    if let Some(run) = workflow_run::Entity::find_by_id(run_id.to_string())
        .one(db)
        .await
        .map_err(db_err)?
    {
        let mut am: workflow_run::ActiveModel = run.into();
        am.status = Set(status.to_string());
        am.updated_at = Set(now.to_string());
        am.update(db).await.map_err(db_err)?;
    }
    Ok(status.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mem_store(name: &str) -> RunStore {
        let url = format!("sqlite:file:{name}?mode=memory&cache=shared");
        RunStore::new(Arc::new(connect(&url).await.unwrap()))
    }

    #[test]
    fn normalize_db_url_variants() {
        assert_eq!(
            normalize_db_url("/tmp/x.sqlite"),
            "sqlite:///tmp/x.sqlite?mode=rwc"
        );
        assert_eq!(
            normalize_db_url("sqlite:///tmp/x.sqlite?mode=rwc"),
            "sqlite:///tmp/x.sqlite?mode=rwc"
        );
        assert_eq!(normalize_db_url("postgres://h/db"), "postgres://h/db");
    }

    #[test]
    fn inert_store_no_ops() {
        let store = RunStore::inert();
        assert!(!store.is_live());
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            assert_eq!(store.create_run("s", "w", "W", &[], "t").await.unwrap(), "");
            assert!(store.list_runs("s", "w").await.unwrap().is_empty());
            assert!(store.get_run("x").await.unwrap().is_none());
            assert!(store.list_steps("x").await.unwrap().is_empty());
        });
    }

    #[tokio::test]
    async fn create_list_and_read_run_with_steps() {
        let store = mem_store("wf_runs_roundtrip").await;
        let steps = vec![
            NewStep {
                step_id: "build".into(),
                title: "Build".into(),
                mode: "auto".into(),
                command: "make".into(),
                description: String::new(),
            },
            NewStep {
                step_id: "test".into(),
                title: "Test".into(),
                mode: "manual".into(),
                command: String::new(),
                description: "run the tests".into(),
            },
        ];
        let id = store
            .create_run(
                "workflow/main",
                "release",
                "Release cutting",
                &steps,
                "2026-07-20T10:00:00Z",
            )
            .await
            .unwrap();
        assert_eq!(id, "release-2026-07-20T10:00:00Z");

        let runs = store.list_runs("workflow/main", "release").await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, run_status::PENDING);
        assert_eq!(runs[0].title, "Release cutting");

        // Scope isolation: another scope sees nothing.
        assert!(store
            .list_runs("workflow/other", "release")
            .await
            .unwrap()
            .is_empty());

        let got = store.get_run(&id).await.unwrap().unwrap();
        assert_eq!(got.workflow, "release");

        let protocol = store.list_steps(&id).await.unwrap();
        assert_eq!(protocol.len(), 2);
        assert_eq!(protocol[0].seq, 0);
        assert_eq!(protocol[0].step_id, "build");
        assert_eq!(protocol[0].command, "make");
        assert_eq!(protocol[0].status, step_status::PENDING);
        assert_eq!(protocol[1].step_id, "test");
        assert_eq!(protocol[1].description, "run the tests");

        store.delete_run(&id).await.unwrap();
        assert!(store
            .list_runs("workflow/main", "release")
            .await
            .unwrap()
            .is_empty());
        assert!(store.list_steps(&id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn manual_execution_state_machine() {
        let store = mem_store("wf_runs_state_machine").await;
        let steps = vec![
            NewStep {
                step_id: "a".into(),
                title: "A".into(),
                mode: "manual".into(),
                command: String::new(),
                description: String::new(),
            },
            NewStep {
                step_id: "b".into(),
                title: "B".into(),
                mode: "manual".into(),
                command: String::new(),
                description: String::new(),
            },
            NewStep {
                step_id: "c".into(),
                title: "C".into(),
                mode: "manual".into(),
                command: String::new(),
                description: String::new(),
            },
        ];
        let id = store
            .create_run(
                "workflow/main",
                "flow",
                "Flow",
                &steps,
                "2026-07-20T00:00:00Z",
            )
            .await
            .unwrap();
        assert_eq!(
            store.get_run(&id).await.unwrap().unwrap().status,
            run_status::PENDING
        );

        // Advance completes the first pending step and moves the run to running.
        let (seq, run) = store
            .advance_run(&id, step_status::DONE, "t1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(seq, 0);
        assert_eq!(run, run_status::RUNNING);
        let rows = store.list_steps(&id).await.unwrap();
        assert_eq!(rows[0].status, step_status::DONE);
        assert_eq!(rows[0].started_at, "t1");
        assert_eq!(rows[0].finished_at, "t1");
        assert_eq!(rows[1].status, step_status::PENDING);

        // Skip the next step (finished stamped, no start), run still running.
        let (seq, run) = store
            .advance_run(&id, step_status::SKIPPED, "t2")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(seq, 1);
        assert_eq!(run, run_status::RUNNING);
        let rows = store.list_steps(&id).await.unwrap();
        assert_eq!(rows[1].status, step_status::SKIPPED);
        assert_eq!(rows[1].started_at, "");
        assert_eq!(rows[1].finished_at, "t2");

        // Completing the last step finishes the run.
        let (seq, run) = store
            .advance_run(&id, step_status::DONE, "t3")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(seq, 2);
        assert_eq!(run, run_status::DONE);
        // Nothing left to advance.
        assert!(store
            .advance_run(&id, step_status::DONE, "t4")
            .await
            .unwrap()
            .is_none());

        // A per-step failure fails the whole run.
        let run = store
            .set_step_status(&id, 1, step_status::FAILED, "t5")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run, run_status::FAILED);

        // Reset returns everything to pending and clears stamps.
        let run = store.reset_run(&id, "t6").await.unwrap();
        assert_eq!(run, run_status::PENDING);
        let rows = store.list_steps(&id).await.unwrap();
        assert!(rows.iter().all(|r| r.status == step_status::PENDING
            && r.started_at.is_empty()
            && r.finished_at.is_empty()));

        // Setting the status of an unknown step is a no-op (None).
        assert!(store
            .set_step_status(&id, 99, step_status::DONE, "t7")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn append_per_visit_grows_the_protocol() {
        let store = mem_store("wf_runs_append").await;
        // Seed a run with only its entry step (append-per-visit style).
        let entry = vec![NewStep {
            step_id: "a".into(),
            title: "A".into(),
            mode: "manual".into(),
            command: String::new(),
            description: String::new(),
        }];
        let id = store
            .create_run(
                "workflow/main",
                "loop",
                "Loop",
                &entry,
                "2026-07-20T00:00:00Z",
            )
            .await
            .unwrap();
        assert_eq!(store.list_steps(&id).await.unwrap().len(), 1);
        assert_eq!(store.count_step_visits(&id, "a").await.unwrap(), 1);

        // Settle the entry and append two more visits of the same step (a loop).
        store
            .write_step_result(&id, 0, step_status::DONE, "run 1", "t1")
            .await
            .unwrap();
        let step_a = NewStep {
            step_id: "a".into(),
            title: "A".into(),
            mode: "manual".into(),
            command: String::new(),
            description: String::new(),
        };
        assert_eq!(store.append_step(&id, &step_a).await.unwrap(), 1);
        assert_eq!(store.append_step(&id, &step_a).await.unwrap(), 2);
        assert_eq!(store.count_step_visits(&id, "a").await.unwrap(), 3);

        // write_step_result did not aggregate the run; the driver sets it.
        store
            .set_run_status(&id, run_status::RUNNING, "t2")
            .await
            .unwrap();
        assert_eq!(
            store.get_run(&id).await.unwrap().unwrap().status,
            run_status::RUNNING
        );

        // The first visit kept its recorded output; the fresh visits are pending.
        let rows = store.list_steps(&id).await.unwrap();
        assert_eq!(rows[0].status, step_status::DONE);
        assert_eq!(rows[0].output, "run 1");
        assert_eq!(rows[1].status, step_status::PENDING);
        assert_eq!(rows[2].status, step_status::PENDING);

        // clear_steps empties the protocol but keeps the run.
        store.clear_steps(&id).await.unwrap();
        assert!(store.list_steps(&id).await.unwrap().is_empty());
        assert!(store.get_run(&id).await.unwrap().is_some());
    }
}
