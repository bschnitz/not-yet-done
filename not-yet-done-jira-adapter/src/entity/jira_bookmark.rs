//! Per-server bookmarked issues.
//!
//! Keyed by `(connection_id, issue_key)` — `connection_id` is the
//! connection's `scope_id` (UUID v5 of the base URL), so bookmarks belong
//! to the *Jira server*, not to any one view-file instance id. This lets the
//! normal tickets subtab and the bookmarks subtab (which may carry different
//! `adapter.id`s) share the exact same set as long as they point at the same
//! server. `bookmarked_at_unix` records the add time so the bookmarks view
//! can offer a locally-sortable "Bookmarked" column.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "jira_bookmark")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub connection_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub issue_key: String,
    /// Unix seconds (UTC) when the bookmark was added.
    pub bookmarked_at_unix: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
