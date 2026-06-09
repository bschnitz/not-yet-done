use sea_orm::entity::prelude::*;

/// One persisted auth session per Stoat connection. The blob is opaque
/// adapter-side JSON (the `X-Session-Token` plus the resolved user
/// identity) — the orchestrator hands it back as-is on cache hit. We
/// persist only the session token, **never** the password.
/// `created_at_unix` is seconds since the Unix epoch so the
/// orchestrator's TTL policies can be evaluated without a chrono
/// round-trip in the entity.
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
