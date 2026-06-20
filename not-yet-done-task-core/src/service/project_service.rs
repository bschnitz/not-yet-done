use async_trait::async_trait;
use shaku::Component;
use std::sync::Arc;
use uuid::Uuid;

use crate::entity::project;
use crate::error::AppError;
use crate::repository::{ProjectRepository, TaskRepository};

#[async_trait]
pub trait ProjectService: shaku::Interface {
    async fn add_project(
        &self,
        name: String,
        description: Option<String>,
    ) -> Result<project::Model, AppError>;
    async fn list_projects(&self) -> Result<Vec<project::Model>, AppError>;
    async fn edit_project(
        &self,
        id: Uuid,
        name: Option<String>,
        description: Option<String>,
    ) -> Result<project::Model, AppError>;
    async fn delete_project(&self, id: Uuid, cascade: bool) -> Result<(), AppError>;
}

#[derive(Component)]
#[shaku(interface = ProjectService)]
pub struct ProjectServiceImpl {
    #[shaku(inject)]
    project_repository: Arc<dyn ProjectRepository>,
    #[shaku(inject)]
    task_repository: Arc<dyn TaskRepository>,
}

#[async_trait]
impl ProjectService for ProjectServiceImpl {
    async fn add_project(
        &self,
        name: String,
        description: Option<String>,
    ) -> Result<project::Model, AppError> {
        self.project_repository.insert(name, description).await
    }

    async fn list_projects(&self) -> Result<Vec<project::Model>, AppError> {
        self.project_repository.find_all().await
    }

    async fn edit_project(
        &self,
        id: Uuid,
        name: Option<String>,
        description: Option<String>,
    ) -> Result<project::Model, AppError> {
        self.project_repository.update(id, name, description).await
    }

    async fn delete_project(&self, id: Uuid, cascade: bool) -> Result<(), AppError> {
        if cascade {
            self.task_repository.soft_delete_by_project(id).await?;
        }
        self.project_repository.delete(id).await
    }
}
