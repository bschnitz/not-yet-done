use sea_orm::entity::prelude::*;

/// One persisted auth session per Confluence connection. The blob is
/// opaque adapter-side JSON (`{cookie: "..."}` for the cookie mechanism)
/// — the orchestrator hands it back as-is on cache hit. `created_at_unix`
/// is seconds since the Unix epoch so the orchestrator's TTL policies
/// can be evaluated without a chrono round-trip in the entity.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "auth_session")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub connection_id: Uuid,
    pub blob: String,
    pub created_at_unix: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
