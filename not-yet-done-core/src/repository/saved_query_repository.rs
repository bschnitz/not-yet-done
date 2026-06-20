use async_trait::async_trait;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, DatabaseConnection,
    EntityTrait, QueryFilter, Set,
};
use shaku::Component;
use uuid::Uuid;

use crate::entity::saved_query::{self, ActiveModel};
use crate::error::CoreError;

#[async_trait]
pub trait SavedQueryRepository: shaku::Interface {
    /// Upsert a saved query. On update, `shortcut: None` preserves the
    /// existing shortcut (only the query is overwritten); `shortcut: Some(_)`
    /// sets it. On insert, the shortcut is stored as given (None or Some).
    /// To clear an existing shortcut, use [`update_shortcut`] with `None`.
    async fn upsert(
        &self,
        scope: &str,
        name: &str,
        query: &str,
        shortcut: Option<&str>,
    ) -> Result<saved_query::Model, CoreError>;
    async fn find_by_id(&self, id: Uuid) -> Result<Option<saved_query::Model>, CoreError>;
    async fn find_by_scope_and_name(
        &self,
        scope: &str,
        name: &str,
    ) -> Result<Option<saved_query::Model>, CoreError>;
    async fn list_by_scope(&self, scope: &str) -> Result<Vec<saved_query::Model>, CoreError>;
    async fn update_shortcut(&self, id: Uuid, shortcut: Option<&str>) -> Result<(), CoreError>;
    async fn delete(&self, id: Uuid) -> Result<(), CoreError>;
}

#[derive(Component)]
#[shaku(interface = SavedQueryRepository)]
pub struct SavedQueryRepositoryImpl {
    #[shaku(default)]
    db: Option<DatabaseConnection>,
}

#[async_trait]
impl SavedQueryRepository for SavedQueryRepositoryImpl {
    async fn upsert(
        &self,
        scope: &str,
        name: &str,
        query: &str,
        shortcut: Option<&str>,
    ) -> Result<saved_query::Model, CoreError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");

        if let Some(existing) = self.find_by_scope_and_name(scope, name).await? {
            let mut model: ActiveModel = existing.into();
            model.query = Set(query.to_string());
            if let Some(s) = shortcut {
                model.shortcut = Set(Some(s.to_string()));
            }
            return Ok(model.update(db).await?);
        }

        let model = ActiveModel {
            scope: Set(scope.to_string()),
            name: Set(name.to_string()),
            query: Set(query.to_string()),
            shortcut: Set(shortcut.map(|s| s.to_string())),
            ..ActiveModel::new()
        };
        Ok(model.insert(db).await?)
    }

    async fn find_by_id(&self, id: Uuid) -> Result<Option<saved_query::Model>, CoreError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        Ok(saved_query::Entity::find_by_id(id).one(db).await?)
    }

    async fn find_by_scope_and_name(
        &self,
        scope: &str,
        name: &str,
    ) -> Result<Option<saved_query::Model>, CoreError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        Ok(saved_query::Entity::find()
            .filter(saved_query::Column::Scope.eq(scope))
            .filter(saved_query::Column::Name.eq(name))
            .one(db)
            .await?)
    }

    async fn list_by_scope(&self, scope: &str) -> Result<Vec<saved_query::Model>, CoreError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        Ok(saved_query::Entity::find()
            .filter(saved_query::Column::Scope.eq(scope))
            .all(db)
            .await?)
    }

    async fn update_shortcut(&self, id: Uuid, shortcut: Option<&str>) -> Result<(), CoreError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        if let Some(existing) = saved_query::Entity::find_by_id(id).one(db).await? {
            let mut model: ActiveModel = existing.into();
            model.shortcut = Set(shortcut.map(|s| s.to_string()));
            model.update(db).await?;
        }
        Ok(())
    }

    async fn delete(&self, id: Uuid) -> Result<(), CoreError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        saved_query::Entity::delete_by_id(id).exec(db).await?;
        Ok(())
    }
}
