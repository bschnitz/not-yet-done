//! One row per `(scope, connection)`: when the connection was last fetched
//! successfully, which span that fetch covered, and how the latest attempt
//! ended. Readers compare `revision` against the one they loaded to notice
//! that another process has stored newer events.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "calendar_sync")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub scope: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub connection: String,
    /// Bumped by every store of this connection's events — the change marker
    /// readers compare (timestamps can collide within a second).
    pub revision: i64,
    /// Last successful fetch (Unix seconds); `None` until one succeeded.
    pub synced_at_unix: Option<i64>,
    /// The span the last successful fetch covered.
    pub range_start_unix: Option<i64>,
    pub range_end_unix: Option<i64>,
    /// Latest attempt, successful or not.
    pub attempted_at_unix: i64,
    /// Why the latest attempt failed; `None` when it succeeded.
    pub last_error: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
