use async_trait::async_trait;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, DatabaseConnection,
    EntityTrait, QueryFilter, Set,
};
use shaku::Component;
use uuid::Uuid;

use crate::entity::{global_tag, project_tag};
use crate::error::AppError;

/// Aufgelöster Tag — entweder global oder projektspezifisch
#[derive(Debug, Clone)]
pub enum ResolvedTag {
    Global(global_tag::Model),
    Project(project_tag::Model),
}

impl ResolvedTag {
    pub fn id(&self) -> Uuid {
        match self {
            ResolvedTag::Global(t) => t.id,
            ResolvedTag::Project(t) => t.id,
        }
    }
}

#[async_trait]
pub trait TagRepository: shaku::Interface {
    async fn insert_global(
        &self,
        name: String,
        color: Option<String>,
    ) -> Result<global_tag::Model, AppError>;

    async fn insert_project(
        &self,
        name: String,
        color: Option<String>,
        project_id: Uuid,
    ) -> Result<project_tag::Model, AppError>;

    async fn find_all_global(&self) -> Result<Vec<global_tag::Model>, AppError>;
    async fn find_all_by_project(&self, project_id: Uuid) -> Result<Vec<project_tag::Model>, AppError>;
    async fn find_all_project_tags(&self) -> Result<Vec<project_tag::Model>, AppError>;

    async fn find_global_by_id(&self, id: Uuid) -> Result<global_tag::Model, AppError>;
    async fn find_project_tag_by_id(&self, id: Uuid) -> Result<project_tag::Model, AppError>;

    async fn find_global_by_name(&self, name: &str) -> Result<Option<global_tag::Model>, AppError>;
    async fn find_project_tags_by_name(
        &self,
        name: &str,
        project_ids: &[Uuid],
    ) -> Result<Vec<project_tag::Model>, AppError>;

    async fn update_global(
        &self,
        id: Uuid,
        name: Option<String>,
        color: Option<String>,
    ) -> Result<global_tag::Model, AppError>;

    async fn update_project_tag(
        &self,
        id: Uuid,
        name: Option<String>,
        color: Option<String>,
    ) -> Result<project_tag::Model, AppError>;

    async fn delete_global(&self, id: Uuid) -> Result<(), AppError>;
    async fn delete_project_tag(&self, id: Uuid) -> Result<(), AppError>;
}

#[derive(Component)]
#[shaku(interface = TagRepository)]
pub struct TagRepositoryImpl {
    #[shaku(default)]
    db: Option<DatabaseConnection>,
}

#[async_trait]
impl TagRepository for TagRepositoryImpl {
    async fn insert_global(
        &self,
        name: String,
        color: Option<String>,
    ) -> Result<global_tag::Model, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");

        if let Some(existing) = self.find_global_by_name(&name).await? {
            return Err(AppError::DuplicateGlobalTag {
                name,
                id: existing.id,
            });
        }

        let model = global_tag::ActiveModel {
            name: Set(name),
            color: Set(color),
            ..global_tag::ActiveModel::new()
        };
        Ok(model.insert(db).await?)
    }

    async fn insert_project(
        &self,
        name: String,
        color: Option<String>,
        project_id: Uuid,
    ) -> Result<project_tag::Model, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");

        // Prüfen ob (name, project_id) schon existiert
        let existing = project_tag::Entity::find()
            .filter(project_tag::Column::Name.eq(&name))
            .filter(project_tag::Column::ProjectId.eq(project_id))
            .one(db)
        .await?;

        if let Some(existing) = existing {
            return Err(AppError::DuplicateProjectTag {
                name,
                id: existing.id,
            });
        }

        let model = project_tag::ActiveModel {
            name: Set(name),
            color: Set(color),
            project_id: Set(project_id),
            ..project_tag::ActiveModel::new()
        };
        Ok(model.insert(db).await?)
    }

    async fn find_all_global(&self) -> Result<Vec<global_tag::Model>, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        Ok(global_tag::Entity::find().all(db).await?)
    }

    async fn find_all_by_project(
        &self,
        project_id: Uuid,
    ) -> Result<Vec<project_tag::Model>, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        Ok(project_tag::Entity::find()
            .filter(project_tag::Column::ProjectId.eq(project_id))
            .all(db)
            .await?)
    }

    async fn find_all_project_tags(&self) -> Result<Vec<project_tag::Model>, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        Ok(project_tag::Entity::find().all(db).await?)
    }

    async fn find_global_by_id(&self, id: Uuid) -> Result<global_tag::Model, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        global_tag::Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or_else(|| AppError::TagNotFound(id.to_string()))
    }

    async fn find_project_tag_by_id(&self, id: Uuid) -> Result<project_tag::Model, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        project_tag::Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or_else(|| AppError::TagNotFound(id.to_string()))
    }

    async fn find_global_by_name(
        &self,
        name: &str,
    ) -> Result<Option<global_tag::Model>, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        Ok(global_tag::Entity::find()
            .filter(global_tag::Column::Name.eq(name))
            .one(db)
            .await?)
    }

    async fn find_project_tags_by_name(
        &self,
        name: &str,
        project_ids: &[Uuid],
    ) -> Result<Vec<project_tag::Model>, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        Ok(project_tag::Entity::find()
            .filter(project_tag::Column::Name.eq(name))
            .filter(project_tag::Column::ProjectId.is_in(project_ids.to_vec()))
            .all(db)
            .await?)
    }

    async fn update_global(
        &self,
        id: Uuid,
        name: Option<String>,
        color: Option<String>,
    ) -> Result<global_tag::Model, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        let tag = self.find_global_by_id(id).await?;
        let mut model: global_tag::ActiveModel = tag.into();
        if let Some(n) = name { model.name = Set(n); }
        if let Some(c) = color { model.color = Set(Some(c)); }
        Ok(model.update(db).await?)
    }

    async fn update_project_tag(
        &self,
        id: Uuid,
        name: Option<String>,
        color: Option<String>,
    ) -> Result<project_tag::Model, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        let tag = self.find_project_tag_by_id(id).await?;
        let mut model: project_tag::ActiveModel = tag.into();
        if let Some(n) = name { model.name = Set(n); }
        if let Some(c) = color { model.color = Set(Some(c)); }
        Ok(model.update(db).await?)
    }

    async fn delete_global(&self, id: Uuid) -> Result<(), AppError> {
        use sea_orm::ModelTrait;
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        let tag = self.find_global_by_id(id).await?;
        tag.delete(db).await?;
        Ok(())
    }

    async fn delete_project_tag(&self, id: Uuid) -> Result<(), AppError> {
        use sea_orm::ModelTrait;
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        let tag = self.find_project_tag_by_id(id).await?;
        tag.delete(db).await?;
        Ok(())
    }
}
