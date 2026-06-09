//! Keyboard shortcut bound to a named saved query.
//!
//! Saved-query *bodies* live in adapter-managed storage
//! (`SavedQueryStore`, e.g. `<XDG_DATA_HOME>/not_yet_done/<adapter>/
//! <instance>/queries/<name>.yaml`). This table holds only the
//! TUI-side overlay: which key chord, if any, should apply the query
//! named `name` while the user is on a particular view scope.
//!
//! `scope` is a `NodeRef`-style path string identifying the
//! hierarchy level the shortcut hangs on. Concrete forms today:
//!
//! - View root (Jira/Taiga saved queries):
//!   `<adapter>/<instance>/<view>` — e.g. `jira/jira/tickets`.
//! - Postgres table scripts:
//!   `postgres/<instance>/<...path-of-configured-node-types.../>` —
//!   the exact segments depend on the user's `postgres.yaml` view
//!   hierarchy (e.g. `postgres/postgres/db1/schemas/public/tables/users`
//!   when a `schemas/tables` group hierarchy is configured, or
//!   `postgres/postgres/db1/public/users` when it is not).
//! - Postgres db-level scripts: same scheme, terminating at the DB-level
//!   `db_scripts` group node.
//!
//! Path-segment form mirrors the app-wide `NodeRef` convention so a
//! shortcut row can in principle live on any level of the hierarchy,
//! identified by its full path. There is no FK to `saved_query` because
//! the body lives outside the DB entirely; an orphan shortcut (whose
//! target query was deleted from the filesystem) is silently ignored by
//! the frontend when it builds the menu.
use sea_orm::Set;
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "query_shortcut")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub scope: String,
    pub name: String,
    pub shortcut: String,
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
