//! Snowball cache of observed Jira workflow transitions.
//!
//! Whenever the adapter calls `GET /issue/{key}/transitions` for the
//! transition picker, every returned edge is upserted here. Over time
//! this builds up a partial picture of the project's workflow graph
//! from which the picker can enumerate multi-hop chains
//! (e.g. `Ready -> In Progress -> Done`) — without ever needing the
//! admin-only `/workflowscheme/project` endpoint.
//!
//! `required_fields` is a JSON-encoded `Vec<String>` of field names
//! the API reported as `required: true` (from
//! `expand=transitions.fields`). The chain executor reads this so it
//! can refuse to start a chain that would stall on a required prompt.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "jira_workflow_edge")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub connection_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub project_key: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub issuetype_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub from_status_id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub transition_id: String,
    pub from_status_name: String,
    pub transition_name: String,
    pub to_status_id: String,
    pub to_status_name: String,
    /// JSON-encoded `Vec<String>` — names of fields the API flagged
    /// `required: true`. Empty `[]` for unconditional transitions.
    pub required_fields: String,
    pub last_seen_unix: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
