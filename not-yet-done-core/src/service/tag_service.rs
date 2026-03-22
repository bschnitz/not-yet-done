use async_trait::async_trait;
use shaku::Component;
use std::sync::Arc;
use uuid::Uuid;

use crate::entity::{global_tag, project_tag};
use crate::error::AppError;
use crate::repository::{ProjectRepository, TagRepository};

/// Darstellung eines Tags in der Listenausgabe
#[derive(Debug)]
pub enum TagItem {
    Global(global_tag::Model),
    Project { tag: project_tag::Model, project_name: String },
}

enum TagId {
    Global(Uuid),
    Project(Uuid),
}

#[async_trait]
pub trait TagService: shaku::Interface {
    async fn add_global(&self, name: String, color: Option<String>) -> Result<global_tag::Model, AppError>;
    async fn add_project_tag(
        &self,
        name: String,
        color: Option<String>,
        project_ref: String,
    ) -> Result<project_tag::Model, AppError>;
    async fn list_all(&self) -> Result<Vec<TagItem>, AppError>;
    async fn list_global(&self) -> Result<Vec<global_tag::Model>, AppError>;
    async fn list_by_project(&self, project_ref: String) -> Result<Vec<project_tag::Model>, AppError>;
    async fn edit(&self, id: String, name: Option<String>, color: Option<String>) -> Result<TagItem, AppError>;
    async fn delete(&self, id: String) -> Result<(), AppError>;
}

#[derive(Component)]
#[shaku(interface = TagService)]
pub struct TagServiceImpl {
    #[shaku(inject)]
    tag_repository: Arc<dyn TagRepository>,
    #[shaku(inject)]
    project_repository: Arc<dyn ProjectRepository>,
}

#[async_trait]
impl TagService for TagServiceImpl {
    async fn add_global(
        &self,
        name: String,
        color: Option<String>,
    ) -> Result<global_tag::Model, AppError> {
        validate_color_opt(&color)?;
        self.tag_repository.insert_global(name, color).await
    }

    async fn add_project_tag(
        &self,
        name: String,
        color: Option<String>,
        project_ref: String,
    ) -> Result<project_tag::Model, AppError> {
        validate_color_opt(&color)?;
        let project = resolve_project(&*self.project_repository, &project_ref).await?;
        self.tag_repository.insert_project(name, color, project.id).await
    }

    async fn list_all(&self) -> Result<Vec<TagItem>, AppError> {
        let globals = self.tag_repository.find_all_global().await?;
        let project_tags = self.tag_repository.find_all_project_tags().await?;

        let mut items: Vec<TagItem> = globals.into_iter().map(TagItem::Global).collect();

        for tag in project_tags {
            let project = self.project_repository.find_by_id(tag.project_id).await?;
            items.push(TagItem::Project { tag, project_name: project.name });
        }

        Ok(items)
    }

    async fn list_global(&self) -> Result<Vec<global_tag::Model>, AppError> {
        self.tag_repository.find_all_global().await
    }

    async fn list_by_project(
        &self,
        project_ref: String,
    ) -> Result<Vec<project_tag::Model>, AppError> {
        let project = resolve_project(&*self.project_repository, &project_ref).await?;
        self.tag_repository.find_all_by_project(project.id).await
    }

    async fn edit(
        &self,
        id: String,
        name: Option<String>,
        color: Option<String>,
    ) -> Result<TagItem, AppError> {
        validate_color_opt(&color)?;
        match parse_tag_id(&id)? {
            TagId::Global(uuid) => {
                let tag = self.tag_repository.update_global(uuid, name, color).await?;
                Ok(TagItem::Global(tag))
            }
            TagId::Project(uuid) => {
                let tag = self.tag_repository.update_project_tag(uuid, name, color).await?;
                let project = self.project_repository.find_by_id(tag.project_id).await?;
                Ok(TagItem::Project { tag, project_name: project.name })
            }
        }
    }

    async fn delete(&self, id: String) -> Result<(), AppError> {
        match parse_tag_id(&id)? {
            TagId::Global(uuid) => self.tag_repository.delete_global(uuid).await,
            TagId::Project(uuid) => self.tag_repository.delete_project_tag(uuid).await,
        }
    }
}

// ── Hilfsfunktionen ────────────────────────────────────────────────────────
fn validate_color_opt(color: &Option<String>) -> Result<(), AppError> {
    if let Some(c) = color {
        let re = regex::Regex::new(r"^#[0-9A-Fa-f]{3,8}$").unwrap();
        if !re.is_match(c) {
            return Err(AppError::InvalidColor(c.clone()));
        }
    }
    Ok(())
}

async fn resolve_project(
    repo: &dyn crate::repository::ProjectRepository,
    proj_ref: &str,
) -> Result<crate::entity::project::Model, AppError> {
    if let Ok(id) = Uuid::parse_str(proj_ref) {
        repo.find_by_id(id).await
    } else {
        repo.find_by_name(proj_ref)
            .await?
            .ok_or_else(|| AppError::ProjectNotFound(Uuid::nil()))
    }
}

/// Löst einen Tag-ID-String auf.
/// Formate: "global-tag:<uuid>", "project-tag:<uuid>", oder plain "<uuid>" (→ global)
fn parse_tag_id(id: &str) -> Result<TagId, AppError> {
    if let Some(rest) = id.strip_prefix("global-tag:") {
        let uuid = Uuid::parse_str(rest).map_err(|_| AppError::InvalidId(rest.to_string()))?;
        Ok(TagId::Global(uuid))
    } else if let Some(rest) = id.strip_prefix("project-tag:") {
        let uuid = Uuid::parse_str(rest).map_err(|_| AppError::InvalidId(rest.to_string()))?;
        Ok(TagId::Project(uuid))
    } else {
        let uuid = Uuid::parse_str(id).map_err(|_| AppError::InvalidId(id.to_string()))?;
        Ok(TagId::Global(uuid))
    }
}
