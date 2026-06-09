use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "jira_view_sort_state")]
pub struct Model {
    /// View scope key chosen by the frontend (e.g. `"jira:items"`).
    #[sea_orm(primary_key, auto_increment = false)]
    pub scope: String,
    /// Compact `col:dir,col:dir` form. See `parse_sort_state` /
    /// `serialize_sort_state` in the adapter.
    #[sea_orm(column_type = "Text")]
    pub sort: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
