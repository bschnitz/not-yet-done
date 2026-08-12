//! The per-step execution protocol of a run: one row per step *entry*.
//!
//! Keyed by `(run_id, seq)` rather than `(run_id, step_id)` so a step reached
//! more than once (a loop or a re-run in a DAG) records each visit as its own
//! row in execution order. `step_id` carries the definition's step id for
//! joining back to structure; `seq` is the monotonic visit order within the run.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "run_step")]
pub struct Model {
    /// The owning run ([`workflow_run::Model::id`](super::workflow_run::Model)).
    #[sea_orm(primary_key, auto_increment = false)]
    pub run_id: String,
    /// Monotonic visit order within the run (0-based).
    #[sea_orm(primary_key, auto_increment = false)]
    pub seq: i32,
    /// The definition step's id this entry executed.
    pub step_id: String,
    /// Step title, snapshotted from the definition.
    pub title: String,
    /// Resolved mode this entry ran in: `manual` / `auto` / `ai`.
    pub mode: String,
    /// Command snapshotted from the definition for `auto` execution — captured at
    /// run-creation so a later edit of the file can't rewrite what a run ran.
    /// Empty for manual/ai steps or steps without a command.
    pub command: String,
    /// Prose instruction snapshotted from the definition — shown for a manual
    /// step and fed as the prompt to the `ai` runner. Empty when the step has
    /// none.
    pub description: String,
    /// Lifecycle: `pending` / `running` / `done` / `skipped` / `failed`.
    pub status: String,
    /// Captured output / log for the entry (command stdout+stderr, AI transcript,
    /// or a manual note). Empty until the step runs.
    pub output: String,
    /// RFC 3339 start timestamp, or empty while still pending.
    pub started_at: String,
    /// RFC 3339 finish timestamp, or empty while unfinished.
    pub finished_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
