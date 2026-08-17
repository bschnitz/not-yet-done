//! Repository for the [`script_hook`](crate::entity::script_hook) table:
//! the `(scope, name) → hook` mapping that turns a view script into one
//! that runs on its own, triggered by an event instead of a key press.
//!
//! The sibling of [`QueryShortcutRepository`](super::QueryShortcutRepository)
//! — same scope key, same storage split (the script body stays on disk,
//! only the binding lives in the DB), and the same idempotent upsert
//! semantics. Kept in its own table rather than folded into
//! `query_shortcut` because the two bindings are independent: a script can
//! have a chord, a hook, both, or neither.

use async_trait::async_trait;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait,
    QueryFilter, Set,
};
use shaku::Component;

use crate::entity::script_hook::{self, ActiveModel};
use crate::error::CoreError;

#[async_trait]
pub trait ScriptHookRepository: shaku::Interface {
    /// All `(name, hook)` pairs registered for `scope`. Order is
    /// unspecified — callers should sort if they want stable UI.
    async fn list_by_scope(&self, scope: &str) -> Result<Vec<script_hook::Model>, CoreError>;

    /// Set or replace the hook for `(scope, name)`. Upsert — no
    /// distinction between "create" and "update".
    async fn set(
        &self,
        scope: &str,
        name: &str,
        hook: &str,
    ) -> Result<script_hook::Model, CoreError>;

    /// Remove the hook for `(scope, name)`. Missing rows are not an error
    /// (idempotent), so deleting a script can unconditionally clear it.
    async fn unset(&self, scope: &str, name: &str) -> Result<(), CoreError>;

    /// Follow a renamed script within `scope`, so its hook keeps firing
    /// under the new name.
    async fn rename(&self, scope: &str, old_name: &str, new_name: &str) -> Result<(), CoreError>;
}

#[derive(Component)]
#[shaku(interface = ScriptHookRepository)]
pub struct ScriptHookRepositoryImpl {
    #[shaku(default)]
    db: Option<DatabaseConnection>,
}

#[async_trait]
impl ScriptHookRepository for ScriptHookRepositoryImpl {
    async fn list_by_scope(&self, scope: &str) -> Result<Vec<script_hook::Model>, CoreError> {
        let db = self.db.as_ref().expect("DB not initialized");
        Ok(script_hook::Entity::find()
            .filter(script_hook::Column::Scope.eq(scope))
            .all(db)
            .await?)
    }

    async fn set(
        &self,
        scope: &str,
        name: &str,
        hook: &str,
    ) -> Result<script_hook::Model, CoreError> {
        let db = self.db.as_ref().expect("DB not initialized");
        let existing = script_hook::Entity::find()
            .filter(script_hook::Column::Scope.eq(scope))
            .filter(script_hook::Column::Name.eq(name))
            .one(db)
            .await?;
        if let Some(model) = existing {
            let mut active: ActiveModel = model.into();
            active.hook = Set(hook.to_string());
            return Ok(active.update(db).await?);
        }
        let model = ActiveModel {
            scope: Set(scope.to_string()),
            name: Set(name.to_string()),
            hook: Set(hook.to_string()),
            ..ActiveModel::new()
        };
        Ok(model.insert(db).await?)
    }

    async fn unset(&self, scope: &str, name: &str) -> Result<(), CoreError> {
        let db = self.db.as_ref().expect("DB not initialized");
        script_hook::Entity::delete_many()
            .filter(script_hook::Column::Scope.eq(scope))
            .filter(script_hook::Column::Name.eq(name))
            .exec(db)
            .await?;
        Ok(())
    }

    async fn rename(&self, scope: &str, old_name: &str, new_name: &str) -> Result<(), CoreError> {
        let db = self.db.as_ref().expect("DB not initialized");
        let row = script_hook::Entity::find()
            .filter(script_hook::Column::Scope.eq(scope))
            .filter(script_hook::Column::Name.eq(old_name))
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
