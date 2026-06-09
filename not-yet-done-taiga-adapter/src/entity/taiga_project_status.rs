use sea_orm::Set;
use sea_orm::entity::prelude::*;

/// One status row per (connection_id, project_id, item_type, status_id).
/// `item_type` holds the Taiga ItemType as a short string ("task", "issue",
/// "epic", "userstory") — matches `ItemType::as_str()`.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "taiga_project_status")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub connection_id: Uuid,
    pub project_id: i64,
    pub item_type: String,
    pub status_id: i64,
    pub name: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {
    fn new() -> Self {
        Self {
            id: Set(Uuid::new_v4()),
            ..ActiveModelTrait::default()
        }
    }
}
