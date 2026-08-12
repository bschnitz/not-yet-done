//! The per-column *schema* table: one row per `(scope, node_type, column_key)`.
//!
//! This is what makes the store self-describing. The first time a value is
//! written for a column its type is recorded here (type-on-first-write); from
//! then on the store is authoritative — a later write with a different type is
//! rejected, and every front-end can discover a scope's custom columns and
//! their types without needing the view YAML. `node_type` is part of the key so
//! the same `column_key` can mean different things on different node types of
//! one adapter instance (e.g. Jira `issue` vs `bookmark`).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "custom_column")]
pub struct Model {
    /// `"<adapter_type>/<instance_id>"` — isolates one adapter instance's
    /// schema from another's (same key as the cell table's `scope`).
    #[sea_orm(primary_key, auto_increment = false)]
    pub scope: String,
    /// The `type_id` of the node type this column is defined on.
    #[sea_orm(primary_key, auto_increment = false)]
    pub node_type: String,
    /// The custom column's key.
    #[sea_orm(primary_key, auto_increment = false)]
    pub column_key: String,
    /// Authoritative value type: `text` / `number` / `duration` / `datetime`.
    /// Fixed on first write; later writes must match.
    pub value_type: String,
    /// Optional human label. Reserved for a future front-end that introduces a
    /// column without a view YAML; `None` today.
    pub label: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
