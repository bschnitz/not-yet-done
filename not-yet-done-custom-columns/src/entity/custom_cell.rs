//! The single table backing custom user columns: one row per
//! `(scope, row_id, column_key)` cell.
//!
//! `scope` isolates one adapter instance's cells from another's — it is
//! `"<adapter_type>/<instance_id>"`, so the same Jira ticket key stored under
//! two different Jira instances never collides. `row_id` is the content node's
//! stable id (the same id used for addressing / `get_by_id`), which is how a
//! cell is matched back onto its row. `value_type` mirrors the column's
//! authoritative type from the [`custom_column`](super::custom_column) schema
//! table (fixed on first write); it is denormalised onto the cell so a read
//! carries the type without a schema join.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "custom_cell")]
pub struct Model {
    /// `"<adapter_type>/<instance_id>"` — isolates one adapter instance's
    /// annotations from another's.
    #[sea_orm(primary_key, auto_increment = false)]
    pub scope: String,
    /// The content node's stable id — matched against a row's `id` on read.
    #[sea_orm(primary_key, auto_increment = false)]
    pub row_id: String,
    /// The custom column's key — the same key a `source: custom` view column
    /// declares, so the injected metadata field lands in that column.
    #[sea_orm(primary_key, auto_increment = false)]
    pub column_key: String,
    /// The stored value, verbatim. The view column's `kind:` does the display
    /// formatting.
    pub value: String,
    /// Intended value type: `text` / `number` / `duration` / `datetime`.
    pub value_type: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
