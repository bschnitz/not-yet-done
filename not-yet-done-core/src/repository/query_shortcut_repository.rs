//! Repository for the [`query_shortcut`](crate::entity::query_shortcut)
//! table. The companion to adapter-managed `SavedQueryStore` (which
//! holds the *body* of each named query): this DB layer stores the
//! `(scope, name) → key chord` mapping that the TUI uses to bind a
//! global shortcut to a saved query. Splitting body and shortcut into
//! separate storage lets adapters own their data and the TUI own its
//! input bindings without either side reaching into the other.
//!
//! `scope` is a `NodeRef`-style `/`-separated path identifying the
//! hierarchy level the binding belongs to (e.g. `jira/jira/tickets`
//! for a view-root shortcut, or
//! `postgres/<inst>/<db>/schemas/<s>/tables/<t>` for a per-table
//! Postgres script shortcut). See the entity-level docs for the
//! convention.

use async_trait::async_trait;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait,
    QueryFilter, Set,
};
use shaku::Component;

use crate::entity::query_shortcut::{self, ActiveModel};
use crate::error::AppError;

#[async_trait]
pub trait QueryShortcutRepository: shaku::Interface {
    /// All `(name, shortcut)` pairs registered for `scope`. Order is
    /// unspecified — callers should sort if they want stable UI.
    async fn list_by_scope(
        &self,
        scope: &str,
    ) -> Result<Vec<query_shortcut::Model>, AppError>;

    /// Set or replace the shortcut for `(scope, name)`. Upsert — no
    /// distinction between "create" and "update".
    async fn set(
        &self,
        scope: &str,
        name: &str,
        shortcut: &str,
    ) -> Result<query_shortcut::Model, AppError>;

    /// Remove the shortcut for `(scope, name)`. Missing rows are not an
    /// error (idempotent).
    async fn unset(&self, scope: &str, name: &str) -> Result<(), AppError>;

    /// Rename the query referenced by all shortcuts under `scope`. Used
    /// when the user renames a saved query via the menu — the body
    /// moves on the filesystem, the shortcut row tracks the new name.
    async fn rename(
        &self,
        scope: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), AppError>;
}

#[derive(Component)]
#[shaku(interface = QueryShortcutRepository)]
pub struct QueryShortcutRepositoryImpl {
    #[shaku(default)]
    db: Option<DatabaseConnection>,
}

#[async_trait]
impl QueryShortcutRepository for QueryShortcutRepositoryImpl {
    async fn list_by_scope(
        &self,
        scope: &str,
    ) -> Result<Vec<query_shortcut::Model>, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        Ok(query_shortcut::Entity::find()
            .filter(query_shortcut::Column::Scope.eq(scope))
            .all(db)
            .await?)
    }

    async fn set(
        &self,
        scope: &str,
        name: &str,
        shortcut: &str,
    ) -> Result<query_shortcut::Model, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        let existing = query_shortcut::Entity::find()
            .filter(query_shortcut::Column::Scope.eq(scope))
            .filter(query_shortcut::Column::Name.eq(name))
            .one(db)
            .await?;
        if let Some(model) = existing {
            let mut active: ActiveModel = model.into();
            active.shortcut = Set(shortcut.to_string());
            return Ok(active.update(db).await?);
        }
        let model = ActiveModel {
            scope: Set(scope.to_string()),
            name: Set(name.to_string()),
            shortcut: Set(shortcut.to_string()),
            ..ActiveModel::new()
        };
        Ok(model.insert(db).await?)
    }

    async fn unset(&self, scope: &str, name: &str) -> Result<(), AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        query_shortcut::Entity::delete_many()
            .filter(query_shortcut::Column::Scope.eq(scope))
            .filter(query_shortcut::Column::Name.eq(name))
            .exec(db)
            .await?;
        Ok(())
    }

    async fn rename(
        &self,
        scope: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        let row = query_shortcut::Entity::find()
            .filter(query_shortcut::Column::Scope.eq(scope))
            .filter(query_shortcut::Column::Name.eq(old_name))
            .one(db)
            .await?;
        if let Some(model) = row {
            let mut active: ActiveModel = model.into();
            active.name = Set(new_name.to_string());
            active.update(db).await?;
        }
        Ok(())
    }
}
