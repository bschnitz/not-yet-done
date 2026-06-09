use async_trait::async_trait;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, DatabaseConnection, EntityTrait, Set,
};
use shaku::Component;
use uuid::Uuid;

use crate::entity::project::{self, ActiveModel};
use crate::error::AppError;

#[async_trait]
pub trait ProjectRepository: shaku::Interface {
    async fn insert(&self, name: String, description: Option<String>) -> Result<project::Model, AppError>;
    async fn find_all(&self) -> Result<Vec<project::Model>, AppError>;
    async fn find_by_id(&self, id: Uuid) -> Result<project::Model, AppError>;
    async fn find_by_name(&self, name: &str) -> Result<Option<project::Model>, AppError>;
    async fn update(
        &self,
        id: Uuid,
        name: Option<String>,
        description: Option<String>,
    ) -> Result<project::Model, AppError>;
    async fn delete(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Component)]
#[shaku(interface = ProjectRepository)]
pub struct ProjectRepositoryImpl {
    #[shaku(default)]
    db: Option<DatabaseConnection>,
}

#[async_trait]
impl ProjectRepository for ProjectRepositoryImpl {
    async fn insert(
        &self,
        name: String,
        description: Option<String>,
    ) -> Result<project::Model, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        let model = ActiveModel {
            name: Set(name),
            description: Set(description),
            ..ActiveModel::new()
        };
        Ok(model.insert(db).await?)
    }

    async fn find_all(&self) -> Result<Vec<project::Model>, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        Ok(project::Entity::find().all(db).await?)
    }

    async fn find_by_id(&self, id: Uuid) -> Result<project::Model, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        project::Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or(AppError::ProjectNotFound(id))
    }

    async fn find_by_name(&self, name: &str) -> Result<Option<project::Model>, AppError> {
        use sea_orm::ColumnTrait;
        use sea_orm::QueryFilter;
        use crate::entity::project::Column;
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        Ok(project::Entity::find()
            .filter(Column::Name.eq(name))
            .one(db)
            .await?)
    }

    async fn update(
        &self,
        id: Uuid,
        name: Option<String>,
        description: Option<String>,
    ) -> Result<project::Model, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        let project = self.find_by_id(id).await?;
        let mut model: ActiveModel = project.into();
        if let Some(n) = name {
            model.name = Set(n);
        }
        if let Some(d) = description {
            model.description = Set(Some(d));
        }
        Ok(model.update(db).await?)
    }

    async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        use sea_orm::ModelTrait;
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        let project = self.find_by_id(id).await?;
        project.delete(db).await?;
        Ok(())
    }
}
