use sea_orm::Set;
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "jira_user")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// ID of the jira_connection this user belongs to.
    pub connection_id: Uuid,
    /// Jira username / account name (e.g. "JDOE1").
    pub username: String,
    /// Display name (e.g. "Doe, Jane (EXT_TEAM)").
    pub display_name: String,
    /// Normalized slug for search/autocomplete (e.g. "doe-jane-ext-team").
    pub normalized: String,
    /// Email address (if available).
    #[sea_orm(nullable)]
    pub email: Option<String>,
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
