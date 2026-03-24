// not-yet-done-core/src/repository/task_repository.rs

use sea_orm::prelude::Expr;
use async_trait::async_trait;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, DatabaseConnection,
    EntityTrait, QueryFilter, Set,
};
use shaku::Component;
use uuid::Uuid;

use crate::entity::task::{self, ActiveModel, TaskColumnRegistry};
use crate::entity::task_project;
use crate::error::AppError;
use crate::filter::{FilterBuilder, FilterExpr};

#[async_trait]
pub trait TaskRepository: shaku::Interface {
    async fn insert(&self, description: String, parent_id: Option<Uuid>) -> Result<task::Model, AppError>;
    async fn find_all(&self, project_id: Option<Uuid>) -> Result<Vec<task::Model>, AppError>;
    async fn find_by_id(&self, id: Uuid) -> Result<task::Model, AppError>;
    async fn soft_delete(&self, id: Uuid) -> Result<(), AppError>;
    async fn update_description(&self, id: Uuid, description: String) -> Result<task::Model, AppError>;
    async fn assign_project(&self, task_id: Uuid, project_id: Uuid) -> Result<(), AppError>;
    async fn unassign_project(&self, task_id: Uuid, project_id: Uuid) -> Result<(), AppError>;
    async fn soft_delete_by_project(&self, project_id: Uuid) -> Result<(), AppError>;
    async fn assign_global_tag(&self, task_id: Uuid, tag_id: Uuid) -> Result<(), AppError>;
    async fn unassign_global_tag(&self, task_id: Uuid, tag_id: Uuid) -> Result<(), AppError>;
    async fn assign_project_tag(&self, task_id: Uuid, tag_id: Uuid) -> Result<(), AppError>;
    async fn unassign_project_tag(&self, task_id: Uuid, tag_id: Uuid) -> Result<(), AppError>;
    async fn find_project_ids_for_task(&self, task_id: Uuid) -> Result<Vec<Uuid>, AppError>;

    /// Return all tasks matching the given filter expression.
    ///
    /// The expression is compiled entirely to SQL.  Only columns of the `task`
    /// entity may be referenced; unknown column names produce
    /// [`AppError::FilterError`].
    async fn find_filtered(&self, expr: &FilterExpr) -> Result<Vec<task::Model>, AppError>;
}

#[derive(Component)]
#[shaku(interface = TaskRepository)]
pub struct TaskRepositoryImpl {
    #[shaku(default)]
    db: Option<DatabaseConnection>,
}

#[async_trait]
impl TaskRepository for TaskRepositoryImpl {
    async fn insert(&self, description: String, parent_id: Option<Uuid>) -> Result<task::Model, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        let model = ActiveModel {
            description: Set(description),
            parent_id: Set(parent_id),
            ..ActiveModel::new()
        };
        Ok(model.insert(db).await?)
    }

    async fn find_all(&self, project_id: Option<Uuid>) -> Result<Vec<task::Model>, AppError> {
        use crate::entity::task::Column;
        use sea_orm::QuerySelect;
        let db = self.db.as_ref().expect("DB nicht initialisiert");

        let query = task::Entity::find()
            .filter(Column::Deleted.eq(false));

        if let Some(pid) = project_id {
            use crate::entity::task_project::Column as TpCol;
            use sea_orm::JoinType;
            return Ok(query
                .join(
                    JoinType::InnerJoin,
                    task::Entity::belongs_to(crate::entity::task_project::Entity)
                        .from(Column::Id)
                        .to(TpCol::TaskId)
                        .into(),
                )
                .filter(TpCol::ProjectId.eq(pid))
                .all(db)
                .await?);
        }

        Ok(query.all(db).await?)
    }

    async fn find_by_id(&self, id: Uuid) -> Result<task::Model, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        task::Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or(AppError::TaskNotFound(id))
    }

    async fn soft_delete(&self, id: Uuid) -> Result<(), AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        let task = self.find_by_id(id).await?;
        let mut model: ActiveModel = task.into();
        model.deleted = Set(true);
        model.updated_at = Set(chrono::Utc::now());
        model.update(db).await?;
        Ok(())
    }

    async fn update_description(&self, id: Uuid, description: String) -> Result<task::Model, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        let task = self.find_by_id(id).await?;
        let mut model: ActiveModel = task.into();
        model.description = Set(description);
        model.updated_at = Set(chrono::Utc::now());
        Ok(model.update(db).await?)
    }

    async fn assign_project(&self, task_id: Uuid, project_id: Uuid) -> Result<(), AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        let join = task_project::ActiveModel {
            task_id: Set(task_id),
            project_id: Set(project_id),
        };
        use sea_orm::ActiveModelTrait;
        join.insert(db).await?;
        Ok(())
    }

    async fn unassign_project(&self, task_id: Uuid, project_id: Uuid) -> Result<(), AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        task_project::Entity::delete_many()
            .filter(task_project::Column::TaskId.eq(task_id))
            .filter(task_project::Column::ProjectId.eq(project_id))
            .exec(db)
            .await?;
        Ok(())
    }

    async fn soft_delete_by_project(&self, project_id: Uuid) -> Result<(), AppError> {
        use crate::entity::task::Column;
        let db = self.db.as_ref().expect("DB nicht initialisiert");

        let task_ids: Vec<Uuid> = task_project::Entity::find()
            .filter(task_project::Column::ProjectId.eq(project_id))
            .all(db)
            .await?
            .into_iter()
            .map(|tp| tp.task_id)
            .collect();

        if task_ids.is_empty() {
            return Ok(());
        }

        task::Entity::update_many()
            .col_expr(task::Column::Deleted, Expr::value(true))
            .col_expr(task::Column::UpdatedAt, Expr::value(chrono::Utc::now()))
            .filter(Column::Id.is_in(task_ids))
            .exec(db)
            .await?;

        Ok(())
    }

    async fn assign_global_tag(&self, task_id: Uuid, tag_id: Uuid) -> Result<(), AppError> {
        use crate::entity::task_global_tag;
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        task_global_tag::ActiveModel {
            task_id: Set(task_id),
            global_tag_id: Set(tag_id),
        }
        .insert(db)
        .await?;
        Ok(())
    }

    async fn unassign_global_tag(&self, task_id: Uuid, tag_id: Uuid) -> Result<(), AppError> {
        use crate::entity::task_global_tag;
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        task_global_tag::Entity::delete_many()
            .filter(task_global_tag::Column::TaskId.eq(task_id))
            .filter(task_global_tag::Column::GlobalTagId.eq(tag_id))
            .exec(db)
            .await?;
        Ok(())
    }

    async fn assign_project_tag(&self, task_id: Uuid, tag_id: Uuid) -> Result<(), AppError> {
        use crate::entity::task_project_tag;
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        task_project_tag::ActiveModel {
            task_id: Set(task_id),
            project_tag_id: Set(tag_id),
        }
        .insert(db)
        .await?;
        Ok(())
    }

    async fn unassign_project_tag(&self, task_id: Uuid, tag_id: Uuid) -> Result<(), AppError> {
        use crate::entity::task_project_tag;
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        task_project_tag::Entity::delete_many()
            .filter(task_project_tag::Column::TaskId.eq(task_id))
            .filter(task_project_tag::Column::ProjectTagId.eq(tag_id))
            .exec(db)
            .await?;
        Ok(())
    }

    async fn find_project_ids_for_task(&self, task_id: Uuid) -> Result<Vec<Uuid>, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        Ok(task_project::Entity::find()
            .filter(task_project::Column::TaskId.eq(task_id))
            .all(db)
            .await?
            .into_iter()
            .map(|tp| tp.project_id)
            .collect())
    }

    async fn find_filtered(&self, expr: &FilterExpr) -> Result<Vec<task::Model>, AppError> {
        let db = self.db.as_ref().expect("DB nicht initialisiert");
        let condition = FilterBuilder::new(&TaskColumnRegistry).build(expr)?;
        Ok(task::Entity::find().filter(condition).all(db).await?)
    }
}
