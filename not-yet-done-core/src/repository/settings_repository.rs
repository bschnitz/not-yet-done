use async_trait::async_trait;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, DatabaseConnection,
    EntityTrait, QueryFilter, Set,
};
use shaku::Component;

use crate::entity::settings::{self, ActiveModel};
use crate::error::AppError;

#[async_trait]
pub trait SettingsRepository: shaku::Interface {
    async fn get(&self, key: &str) -> Result<Option<String>, AppError>;
    async fn set(&self, key: &str, value: &str) -> Result<(), AppError>;
    async fn delete(&self, key: &str) -> Result<(), AppError>;
}

#[derive(Component)]
#[shaku(interface = SettingsRepository)]
pub struct SettingsRepositoryImpl {
    #[shaku(default)]
    db: Option<DatabaseConnection>,
}

#[async_trait]
impl SettingsRepository for SettingsRepositoryImpl {
    async fn get(&self, key: &str) -> Result<Option<String>, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        let row = settings::Entity::find()
            .filter(settings::Column::Key.eq(key))
            .one(db)
            .await?;
        Ok(row.map(|r| r.value))
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");

        // Try to find existing.
        if let Some(existing) = settings::Entity::find()
            .filter(settings::Column::Key.eq(key))
            .one(db)
            .await?
        {
            let mut model: ActiveModel = existing.into();
            model.value = Set(value.to_string());
            model.update(db).await?;
        } else {
            let model = ActiveModel {
                key: Set(key.to_string()),
                value: Set(value.to_string()),
                ..ActiveModel::new()
            };
            model.insert(db).await?;
        }
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        settings::Entity::delete_many()
            .filter(settings::Column::Key.eq(key))
            .exec(db)
            .await?;
        Ok(())
    }
}
