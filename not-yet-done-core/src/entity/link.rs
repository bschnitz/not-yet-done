//! App-wide directed link between two [`not_yet_done_content::NodeRef`]s.
//!
//! Source and target are stored as their canonical string form
//! (`<tab>/<rest>`). The TUI parses them back with `NodeRef::parse`
//! when needed; the DB layer treats them as opaque text.

use sea_orm::Set;
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "link")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// Canonical `NodeRef` of the node the link points *from*.
    pub source_ref: String,
    /// Canonical `NodeRef` of the node the link points *to*.
    pub target_ref: String,
    pub created_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {
    fn new() -> Self {
        Self {
            id: Set(Uuid::new_v4()),
            created_at: Set(chrono::Utc::now()),
            ..ActiveModelTrait::default()
        }
    }
}
