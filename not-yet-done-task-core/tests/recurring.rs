//! Recurring tasks: closing an occurrence creates its successor, reopening it
//! withdraws an untouched successor again (`docs/plan-recurring-tasks.md`).

use std::sync::Arc;

use chrono::Duration;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, Database, DatabaseConnection, DbBackend,
    EntityTrait, QueryFilter, Schema, Set,
};
use shaku::HasComponent;
use uuid::Uuid;

use not_yet_done_task_core::entity::task::{self, TaskStatus};
use not_yet_done_task_core::entity::{global_tag, task_global_tag, task_project, tracking};
use not_yet_done_task_core::entity::{project, project_tag, task_project_tag};
use not_yet_done_task_core::error::AppError;
use not_yet_done_task_core::module::TaskDomainModule;
use not_yet_done_task_core::repository::{
    ProjectRepositoryImpl, ProjectRepositoryImplParameters, TagRepositoryImpl,
    TagRepositoryImplParameters, TaskOp, TaskRepository, TaskRepositoryImpl,
    TaskRepositoryImplParameters, TrackingRepositoryImpl, TrackingRepositoryImplParameters,
};
use not_yet_done_task_core::service::TaskService;

struct Fixture {
    repo: Arc<dyn TaskRepository>,
    service: Arc<dyn TaskService>,
    db: DatabaseConnection,
}

async fn setup() -> Fixture {
    let db = Database::connect("sqlite::memory:").await.expect("in-memory SQLite");
    let schema = Schema::new(DbBackend::Sqlite);
    for stmt in [
        schema.create_table_from_entity(task::Entity),
        schema.create_table_from_entity(tracking::Entity),
        schema.create_table_from_entity(project::Entity),
        schema.create_table_from_entity(task_project::Entity),
        schema.create_table_from_entity(global_tag::Entity),
        schema.create_table_from_entity(task_global_tag::Entity),
        schema.create_table_from_entity(project_tag::Entity),
        schema.create_table_from_entity(task_project_tag::Entity),
    ] {
        db.execute(&stmt).await.expect("create table");
    }
    let module = TaskDomainModule::builder()
        .with_component_parameters::<TaskRepositoryImpl>(TaskRepositoryImplParameters {
            db: Some(db.clone()),
        })
        .with_component_parameters::<ProjectRepositoryImpl>(ProjectRepositoryImplParameters {
            db: Some(db.clone()),
        })
        .with_component_parameters::<TagRepositoryImpl>(TagRepositoryImplParameters {
            db: Some(db.clone()),
        })
        .with_component_parameters::<TrackingRepositoryImpl>(TrackingRepositoryImplParameters {
            db: Some(db.clone()),
        })
        .build();
    Fixture { repo: module.resolve(), service: module.resolve(), db }
}

impl Fixture {
    async fn recurring(&self, rule: &str) -> task::Model {
        let t = self.repo.insert("Sport".into(), None, None, Some(3)).await.unwrap();
        self.service.set_task_recur(t.id, Some(rule.into())).await.unwrap()
    }

    async fn set_status(&self, id: Uuid, status: TaskStatus) -> task::Model {
        self.repo.update_task(id, None, Some(status), None, None, None).await.unwrap()
    }

    async fn series(&self, series_id: Uuid) -> Vec<task::Model> {
        let mut all = task::Entity::find()
            .filter(task::Column::SeriesId.eq(series_id))
            .all(&self.db)
            .await
            .unwrap();
        all.sort_by_key(|t| t.created_at);
        all
    }

    async fn open_in(&self, series_id: Uuid) -> Vec<task::Model> {
        self.series(series_id)
            .await
            .into_iter()
            .filter(|t| matches!(t.status, TaskStatus::Todo | TaskStatus::InProgress))
            .collect()
    }
}

#[tokio::test]
async fn set_recur_starts_a_series_and_refuses_a_bad_rule() {
    let f = setup().await;
    let t = f.recurring("12h after done").await;
    assert_eq!(t.recur.as_deref(), Some("12h after done"));
    assert_eq!(t.series_id, Some(t.id));

    let err = f.service.set_task_recur(t.id, Some("whenever".into())).await.unwrap_err();
    assert!(matches!(err, AppError::InvalidRecurrence(_)), "{err:?}");

    let cleared = f.service.set_task_recur(t.id, Some("  ".into())).await.unwrap();
    assert_eq!(cleared.recur, None);
    assert_eq!(cleared.series_id, Some(t.id), "history keeps its series");
}

#[tokio::test]
async fn done_creates_the_successor() {
    let f = setup().await;
    let first = f.recurring("12h after done, due +2h").await;
    let parent = f.repo.insert("Health".into(), None, None, None).await.unwrap();
    f.repo.update_task(first.id, None, None, None, Some(Some(parent.id)), None).await.unwrap();
    let tag = global_tag::ActiveModel {
        id: Set(Uuid::new_v4()),
        name: Set("todo".into()),
        ..Default::default()
    }
    .insert(&f.db)
    .await
    .unwrap();
    f.repo.assign_global_tag(first.id, tag.id).await.unwrap();

    let done = f.set_status(first.id, TaskStatus::Done).await;

    let open = f.open_in(first.id).await;
    assert_eq!(open.len(), 1);
    let next = &open[0];
    assert_ne!(next.id, first.id);
    assert_eq!(next.status, TaskStatus::Todo);
    assert_eq!(next.description, "Sport");
    assert_eq!(next.priority, 3);
    assert_eq!(next.parent_id, Some(parent.id));
    assert_eq!(next.recur.as_deref(), Some("12h after done, due +2h"));
    assert_eq!(next.series_id, Some(first.id));
    assert_eq!(next.not_before, Some(done.updated_at + Duration::hours(12)));
    assert_eq!(next.due, Some(done.updated_at + Duration::hours(14)));
    assert!(next.path.as_deref().unwrap().starts_with(parent.path.as_deref().unwrap()));
    let tags = f.repo.find_tags_by_task_ids(&[next.id]).await.unwrap();
    assert_eq!(tags.get(&next.id).map(Vec::len), Some(1), "tags are copied");
}

#[tokio::test]
async fn cancelled_also_advances_and_the_series_keeps_its_id() {
    let f = setup().await;
    let first = f.recurring("1d after done").await;
    f.set_status(first.id, TaskStatus::Cancelled).await;
    let second = f.open_in(first.id).await.remove(0);
    f.set_status(second.id, TaskStatus::Done).await;
    let all = f.series(first.id).await;
    assert_eq!(all.len(), 3);
    assert_eq!(f.open_in(first.id).await.len(), 1);
}

#[tokio::test]
async fn no_successor_without_a_rule_or_a_closing_transition() {
    let f = setup().await;
    let plain = f.repo.insert("Once".into(), None, None, None).await.unwrap();
    f.set_status(plain.id, TaskStatus::Done).await;
    assert_eq!(task::Entity::find().all(&f.db).await.unwrap().len(), 1);

    let t = f.recurring("1d after done").await;
    f.set_status(t.id, TaskStatus::InProgress).await;
    assert_eq!(f.series(t.id).await.len(), 1, "starting is not completing");
    f.set_status(t.id, TaskStatus::Done).await;
    f.set_status(t.id, TaskStatus::Cancelled).await;
    assert_eq!(f.series(t.id).await.len(), 2, "done → cancelled spawns nothing new");
}

#[tokio::test]
async fn reopening_withdraws_an_untouched_successor() {
    let f = setup().await;
    let first = f.recurring("1d after done").await;
    f.set_status(first.id, TaskStatus::Done).await;
    assert_eq!(f.series(first.id).await.len(), 2);

    f.set_status(first.id, TaskStatus::Todo).await;
    let all = f.series(first.id).await;
    assert_eq!(all.len(), 1, "successor removed for good");
    assert_eq!(all[0].id, first.id);
    assert!(task_global_tag::Entity::find().all(&f.db).await.unwrap().is_empty());
}

#[tokio::test]
async fn reopening_keeps_a_touched_successor() {
    let f = setup().await;
    let first = f.recurring("1d after done").await;
    f.set_status(first.id, TaskStatus::Done).await;
    let next = f.open_in(first.id).await.remove(0);
    f.repo.update_description(next.id, "Sport (long)".into()).await.unwrap();

    f.set_status(first.id, TaskStatus::Todo).await;
    assert_eq!(f.open_in(first.id).await.len(), 2);
}

#[tokio::test]
async fn reopening_keeps_a_successor_with_subtasks() {
    let f = setup().await;
    let first = f.recurring("1d after done").await;
    f.set_status(first.id, TaskStatus::Done).await;
    let next = f.open_in(first.id).await.remove(0);
    task::ActiveModel {
        description: Set("warm up".into()),
        parent_id: Set(Some(next.id)),
        ..sea_orm::ActiveModelBehavior::new()
    }
    .insert(&f.db)
    .await
    .unwrap();

    f.set_status(first.id, TaskStatus::Todo).await;
    assert_eq!(f.open_in(first.id).await.len(), 2);
}

#[tokio::test]
async fn tree_edit_batch_advances_too() {
    let f = setup().await;
    let first = f.recurring("1d after done").await;
    let update = |status| TaskOp::Update {
        id: first.id,
        description: None,
        status: Some(status),
        priority: None,
        parent_id: None,
        deleted: None,
    };
    f.repo.apply_batch(vec![update(TaskStatus::Done)]).await.unwrap();
    assert_eq!(f.series(first.id).await.len(), 2);
    f.repo.apply_batch(vec![update(TaskStatus::Todo)]).await.unwrap();
    assert_eq!(f.series(first.id).await.len(), 1);
}
