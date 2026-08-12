//! One row per **run** of a workflow — an execution instance of a definition.
//!
//! A run is created from a [`WorkflowDef`](crate::model::WorkflowDef) (Phase 3);
//! its per-step protocol lives in [`run_step`](super::run_step), joined on
//! `run_id`. The definition file stays the single source of truth for structure;
//! a run only records *what happened* when that structure was executed.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "workflow_run")]
pub struct Model {
    /// Opaque unique run id (also the node id fragment addressing this run).
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    /// The adapter instance scope (`<adapter_type>/<instance_id>`), so runs from
    /// two workflow instances pointed at one shared database never mix.
    pub scope: String,
    /// The workflow's `name` (file stem) this run was started from.
    pub workflow: String,
    /// Human title snapshotted from the definition at run-creation time, so a
    /// later rename of the file doesn't rewrite history.
    pub title: String,
    /// Lifecycle: `pending` / `running` / `done` / `failed` / `cancelled`.
    pub status: String,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
    /// RFC 3339 last-update timestamp.
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
