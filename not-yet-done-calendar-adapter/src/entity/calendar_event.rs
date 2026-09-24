//! One stored event per `(scope, global_id)`.
//!
//! `scope` is the adapter instance id, so two calendar instances pointed at the
//! same database never see each other's events. `global_id` is the adapter's
//! `calendar:<connection>:<uid>` id; `connection` is denormalised out of it so
//! a connection's slice can be replaced without parsing ids. Instants are Unix
//! seconds (UTC) — plain integers keep the table portable across SQLite and
//! Postgres.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "calendar_event")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub scope: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub global_id: String,
    pub connection: String,
    pub uid: String,
    /// The calendar/mailbox display name (the "Account" column).
    pub calendar: String,
    pub title: String,
    pub start_unix: i64,
    pub end_unix: i64,
    pub all_day: bool,
    pub location: Option<String>,
    pub organizer: Option<String>,
    /// [`ShowAs::as_str`](not_yet_done_calendar_core::ShowAs::as_str) token.
    pub show_as: String,
    pub body: Option<String>,
    pub url: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
