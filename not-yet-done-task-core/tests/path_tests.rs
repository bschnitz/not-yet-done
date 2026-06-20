//! Comprehensive tests for the materialized path system.
//!
//! Tests cover: path computation, insert, reparent, batch operations,
//! deep trees, multi-reparent in batch, delete + undelete, and rebuild.

use std::sync::Arc;

use sea_orm::{
    ColumnTrait, ConnectionTrait, Database, DbBackend, EntityTrait,
    QueryFilter, Schema,
};
use shaku::HasComponent;
use uuid::Uuid;

use not_yet_done_task_core::entity::task::{self, TaskStatus};
use not_yet_done_task_core::module::TaskDomainModule;
use not_yet_done_task_core::repository::{
    TaskOp, TaskRepository, TaskRepositoryImpl, TaskRepositoryImplParameters,
    ProjectRepositoryImpl, ProjectRepositoryImplParameters,
    TagRepositoryImpl, TagRepositoryImplParameters,
    TrackingRepositoryImpl, TrackingRepositoryImplParameters,
    compute_path, task_short_id,
};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

async fn setup() -> (Arc<dyn TaskRepository>, sea_orm::DatabaseConnection) {
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("in-memory SQLite");

    let schema = Schema::new(DbBackend::Sqlite);
    db.execute(&schema.create_table_from_entity(task::Entity))
        .await
        .expect("create table");

    let module = TaskDomainModule::builder()
        .with_component_parameters::<TaskRepositoryImpl>(
            TaskRepositoryImplParameters { db: Some(db.clone()) },
        )
        .with_component_parameters::<ProjectRepositoryImpl>(
            ProjectRepositoryImplParameters { db: Some(db.clone()) },
        )
        .with_component_parameters::<TagRepositoryImpl>(
            TagRepositoryImplParameters { db: Some(db.clone()) },
        )
        .with_component_parameters::<TrackingRepositoryImpl>(
            TrackingRepositoryImplParameters { db: Some(db.clone()) },
        )
        .build();

    let repo: Arc<dyn TaskRepository> = module.resolve();
    (repo, db)
}

fn sid(id: Uuid) -> String {
    task_short_id(id)
}

// ---------------------------------------------------------------------------
// Unit tests: compute_path
// ---------------------------------------------------------------------------

#[test]
fn compute_path_root() {
    use std::collections::HashMap;
    let id = Uuid::new_v4();
    let parents = HashMap::from([(id, None)]);
    let path = compute_path(id, &parents);
    assert_eq!(path, format!("/{}/", sid(id)));
}

#[test]
fn compute_path_two_levels() {
    use std::collections::HashMap;
    let root = Uuid::new_v4();
    let child = Uuid::new_v4();
    let parents = HashMap::from([
        (root, None),
        (child, Some(root)),
    ]);
    assert_eq!(compute_path(child, &parents), format!("/{}/{}/", sid(root), sid(child)));
}

#[test]
fn compute_path_deep_chain() {
    use std::collections::HashMap;
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let c = Uuid::new_v4();
    let d = Uuid::new_v4();
    let parents = HashMap::from([
        (a, None),
        (b, Some(a)),
        (c, Some(b)),
        (d, Some(c)),
    ]);
    assert_eq!(
        compute_path(d, &parents),
        format!("/{}/{}/{}/{}/", sid(a), sid(b), sid(c), sid(d))
    );
}

#[test]
fn compute_path_cycle_guard() {
    use std::collections::HashMap;
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let parents = HashMap::from([
        (a, Some(b)),
        (b, Some(a)),
    ]);
    let path = compute_path(a, &parents);
    assert!(path.contains(&sid(a)));
}

// ---------------------------------------------------------------------------
// Integration tests: insert sets path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn insert_root_task_has_path() {
    let (repo, _db) = setup().await;
    let task = repo.insert("Root".into(), None, None, None).await.unwrap();
    assert!(task.path.is_some());
    assert_eq!(task.path.unwrap(), format!("/{}/", sid(task.id)));
}

#[tokio::test]
async fn insert_child_task_has_parent_path() {
    let (repo, _db) = setup().await;
    let root = repo.insert("Root".into(), None, None, None).await.unwrap();
    let child = repo.insert("Child".into(), Some(root.id), None, None).await.unwrap();
    assert_eq!(child.path.unwrap(), format!("/{}/{}/", sid(root.id), sid(child.id)));
}

#[tokio::test]
async fn insert_grandchild_path() {
    let (repo, _db) = setup().await;
    let a = repo.insert("A".into(), None, None, None).await.unwrap();
    let b = repo.insert("B".into(), Some(a.id), None, None).await.unwrap();
    let c = repo.insert("C".into(), Some(b.id), None, None).await.unwrap();
    assert_eq!(c.path.unwrap(), format!("/{}/{}/{}/", sid(a.id), sid(b.id), sid(c.id)));
}

// ---------------------------------------------------------------------------
// Integration tests: reparent updates path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reparent_updates_self_and_children() {
    let (repo, _db) = setup().await;
    let a = repo.insert("A".into(), None, None, None).await.unwrap();
    let b = repo.insert("B".into(), Some(a.id), None, None).await.unwrap();
    let c = repo.insert("C".into(), Some(b.id), None, None).await.unwrap();
    let d = repo.insert("D".into(), None, None, None).await.unwrap();

    // Move B under D
    let b_updated = repo.update_task(b.id, None, None, None, Some(Some(d.id)), None).await.unwrap();
    assert_eq!(b_updated.path.unwrap(), format!("/{}/{}/", sid(d.id), sid(b.id)));

    let c_updated = repo.find_by_id(c.id).await.unwrap();
    assert_eq!(c_updated.path.unwrap(), format!("/{}/{}/{}/", sid(d.id), sid(b.id), sid(c.id)));
}

#[tokio::test]
async fn reparent_to_root() {
    let (repo, _db) = setup().await;
    let a = repo.insert("A".into(), None, None, None).await.unwrap();
    let b = repo.insert("B".into(), Some(a.id), None, None).await.unwrap();
    let c = repo.insert("C".into(), Some(b.id), None, None).await.unwrap();

    repo.update_task(b.id, None, None, None, Some(None), None).await.unwrap();

    let b = repo.find_by_id(b.id).await.unwrap();
    assert_eq!(b.path.unwrap(), format!("/{}/", sid(b.id)));

    let c = repo.find_by_id(c.id).await.unwrap();
    assert_eq!(c.path.unwrap(), format!("/{}/{}/", sid(b.id), sid(c.id)));
}

// ---------------------------------------------------------------------------
// Integration tests: batch operations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_insert_builds_paths() {
    let (repo, _db) = setup().await;
    let root = repo.insert("Root".into(), None, None, None).await.unwrap();

    let results = repo.apply_batch(vec![
        TaskOp::Insert {
            description: "Child A".into(),
            parent_id: Some(root.id),
            status: TaskStatus::Todo,
            priority: 0,
        },
        TaskOp::Insert {
            description: "Child B".into(),
            parent_id: Some(root.id),
            status: TaskStatus::Todo,
            priority: 0,
        },
    ]).await.unwrap();

    assert_eq!(results.len(), 2);

    let a = repo.find_by_id(results[0].id).await.unwrap();
    assert_eq!(a.path.unwrap(), format!("/{}/{}/", sid(root.id), sid(results[0].id)));
}

#[tokio::test]
async fn batch_reparent_multiple_tasks() {
    let (repo, _db) = setup().await;
    let root = repo.insert("Root".into(), None, None, None).await.unwrap();
    let a = repo.insert("A".into(), Some(root.id), None, None).await.unwrap();
    let b = repo.insert("B".into(), Some(a.id), None, None).await.unwrap();
    let c = repo.insert("C".into(), Some(b.id), None, None).await.unwrap();

    // Batch: move A to root, move C under root
    repo.apply_batch(vec![
        TaskOp::Update { id: a.id, description: None, status: None, priority: None, parent_id: Some(None), deleted: None },
        TaskOp::Update { id: c.id, description: None, status: None, priority: None, parent_id: Some(Some(root.id)), deleted: None },
    ]).await.unwrap();

    let a = repo.find_by_id(a.id).await.unwrap();
    assert_eq!(a.path.unwrap(), format!("/{}/", sid(a.id)));

    let b = repo.find_by_id(b.id).await.unwrap();
    assert_eq!(b.path.unwrap(), format!("/{}/{}/", sid(a.id), sid(b.id)));

    let c = repo.find_by_id(c.id).await.unwrap();
    assert_eq!(c.path.unwrap(), format!("/{}/{}/", sid(root.id), sid(c.id)));
}

#[tokio::test]
async fn batch_insert_then_reparent_under_new() {
    let (repo, _db) = setup().await;
    let root = repo.insert("Root".into(), None, None, None).await.unwrap();
    let child = repo.insert("Child".into(), Some(root.id), None, None).await.unwrap();

    // Insert new parent, then move child under it (two batches since we need the ID)
    let results = repo.apply_batch(vec![
        TaskOp::Insert {
            description: "New Parent".into(),
            parent_id: Some(root.id),
            status: TaskStatus::Todo,
            priority: 0,
        },
    ]).await.unwrap();
    let new_parent_id = results[0].id;

    repo.apply_batch(vec![
        TaskOp::Update {
            id: child.id,
            description: None, status: None, priority: None,
            parent_id: Some(Some(new_parent_id)),
            deleted: None,
        },
    ]).await.unwrap();

    let child = repo.find_by_id(child.id).await.unwrap();
    assert_eq!(
        child.path.unwrap(),
        format!("/{}/{}/{}/", sid(root.id), sid(new_parent_id), sid(child.id))
    );
}

// ---------------------------------------------------------------------------
// Integration tests: deep trees
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deep_tree_10_levels() {
    let (repo, _db) = setup().await;
    let mut parent_id: Option<Uuid> = None;
    let mut ids = Vec::new();

    for i in 0..10 {
        let task = repo.insert(format!("Level {i}"), parent_id, None, None).await.unwrap();
        ids.push(task.id);
        parent_id = Some(task.id);
    }

    let deepest = repo.find_by_id(*ids.last().unwrap()).await.unwrap();
    let expected: String = format!(
        "/{}/",
        ids.iter().map(|id| sid(*id)).collect::<Vec<_>>().join("/")
    );
    assert_eq!(deepest.path.unwrap(), expected);
}

#[tokio::test]
async fn wide_tree_20_children() {
    let (repo, _db) = setup().await;
    let root = repo.insert("Root".into(), None, None, None).await.unwrap();

    for i in 0..20 {
        let child = repo.insert(format!("Child {i}"), Some(root.id), None, None).await.unwrap();
        assert_eq!(child.path.unwrap(), format!("/{}/{}/", sid(root.id), sid(child.id)));
    }
}

// ---------------------------------------------------------------------------
// Integration tests: rebuild_all_paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rebuild_all_paths_fixes_missing() {
    let (repo, db) = setup().await;
    let root = repo.insert("Root".into(), None, None, None).await.unwrap();
    let _child = repo.insert("Child".into(), Some(root.id), None, None).await.unwrap();

    db.execute_unprepared("UPDATE task SET path = NULL").await.unwrap();

    let count = repo.rebuild_all_paths().await.unwrap();
    assert_eq!(count, 2);

    let root = repo.find_by_id(root.id).await.unwrap();
    assert!(root.path.is_some());
}

#[tokio::test]
async fn rebuild_skips_correct_paths() {
    let (repo, _db) = setup().await;
    repo.insert("Root".into(), None, None, None).await.unwrap();

    let count = repo.rebuild_all_paths().await.unwrap();
    assert_eq!(count, 0);
}

// ---------------------------------------------------------------------------
// Integration tests: path querying with LIKE
// ---------------------------------------------------------------------------

#[tokio::test]
async fn path_like_finds_descendants() {
    let (repo, db) = setup().await;
    let root = repo.insert("Root".into(), None, None, None).await.unwrap();
    let a = repo.insert("A".into(), Some(root.id), None, None).await.unwrap();
    let _b = repo.insert("B".into(), Some(a.id), None, None).await.unwrap();
    let _c = repo.insert("C".into(), Some(root.id), None, None).await.unwrap();

    // in_tree(A): path LIKE '%/<a-sid>%'
    let pattern = format!("%/{}%", sid(a.id));
    let found: Vec<task::Model> = task::Entity::find()
        .filter(task::Column::Path.like(pattern))
        .all(&db)
        .await
        .unwrap();

    let names: Vec<&str> = found.iter().map(|t| t.description.as_str()).collect();
    assert!(names.contains(&"A"));
    assert!(names.contains(&"B"));
    assert!(!names.contains(&"C"));
    assert!(!names.contains(&"Root"));
}

#[tokio::test]
async fn path_like_has_ancestor() {
    let (repo, db) = setup().await;
    let root = repo.insert("Root".into(), None, None, None).await.unwrap();
    let a = repo.insert("A".into(), Some(root.id), None, None).await.unwrap();
    let _b = repo.insert("B".into(), Some(a.id), None, None).await.unwrap();
    let _c = repo.insert("C".into(), Some(a.id), None, None).await.unwrap();

    // has_ancestor(A): path contains /<a-sid>/ but is not A itself
    let pattern = format!("%/{}/_%", sid(a.id));
    let found: Vec<task::Model> = task::Entity::find()
        .filter(task::Column::Path.like(pattern))
        .all(&db)
        .await
        .unwrap();

    let names: Vec<&str> = found.iter().map(|t| t.description.as_str()).collect();
    assert!(names.contains(&"B"));
    assert!(names.contains(&"C"));
    assert!(!names.contains(&"A"));
    assert!(!names.contains(&"Root"));
}

// ---------------------------------------------------------------------------
// Integration tests: complex scenario
// ---------------------------------------------------------------------------

#[tokio::test]
async fn complex_tree_restructure() {
    let (repo, _db) = setup().await;

    let work = repo.insert("Work".into(), None, None, None).await.unwrap();
    let acmecorp = repo.insert("AcmeCorp".into(), Some(work.id), None, None).await.unwrap();
    let globex = repo.insert("Globex".into(), Some(acmecorp.id), None, None).await.unwrap();
    let globex_tickets = repo.insert("Tickets".into(), Some(globex.id), None, None).await.unwrap();
    let proj_101 = repo.insert("PROJ-101".into(), Some(globex_tickets.id), None, None).await.unwrap();
    let proj_202 = repo.insert("PROJ-202".into(), Some(globex_tickets.id), None, None).await.unwrap();
    let daily = repo.insert("Daily".into(), Some(globex.id), None, None).await.unwrap();
    let initech = repo.insert("Initech".into(), Some(acmecorp.id), None, None).await.unwrap();
    let initech_tickets = repo.insert("Tickets".into(), Some(initech.id), None, None).await.unwrap();
    let ticket_42 = repo.insert("#42".into(), Some(initech_tickets.id), None, None).await.unwrap();
    let _private = repo.insert("Private".into(), Some(work.id), None, None).await.unwrap();

    // Verify deep path before restructure.
    let proj = repo.find_by_id(proj_101.id).await.unwrap();
    assert_eq!(
        proj.path.unwrap(),
        format!("/{}/{}/{}/{}/{}/",
            sid(work.id), sid(acmecorp.id), sid(globex.id),
            sid(globex_tickets.id), sid(proj_101.id))
    );

    // Batch: move Initech under Work directly, move PROJ-202 under Daily
    repo.apply_batch(vec![
        TaskOp::Update {
            id: initech.id,
            description: None, status: None, priority: None,
            parent_id: Some(Some(work.id)),
            deleted: None,
        },
        TaskOp::Update {
            id: proj_202.id,
            description: None, status: None, priority: None,
            parent_id: Some(Some(daily.id)),
            deleted: None,
        },
    ]).await.unwrap();

    // Initech: Work -> Initech
    let initech = repo.find_by_id(initech.id).await.unwrap();
    assert_eq!(initech.path.unwrap(), format!("/{}/{}/", sid(work.id), sid(initech.id)));

    // #116: Work -> Initech -> Tickets -> #116
    let t42 = repo.find_by_id(ticket_42.id).await.unwrap();
    assert_eq!(
        t42.path.unwrap(),
        format!("/{}/{}/{}/{}/",
            sid(work.id), sid(initech.id), sid(initech_tickets.id), sid(ticket_42.id))
    );

    // PROJ-202 under Daily
    let p202 = repo.find_by_id(proj_202.id).await.unwrap();
    assert_eq!(
        p202.path.unwrap(),
        format!("/{}/{}/{}/{}/{}/",
            sid(work.id), sid(acmecorp.id), sid(globex.id),
            sid(daily.id), sid(proj_202.id))
    );

    // PROJ-101 unchanged
    let p101 = repo.find_by_id(proj_101.id).await.unwrap();
    assert_eq!(
        p101.path.unwrap(),
        format!("/{}/{}/{}/{}/{}/",
            sid(work.id), sid(acmecorp.id), sid(globex.id),
            sid(globex_tickets.id), sid(proj_101.id))
    );
}

#[tokio::test]
async fn batch_delete_preserves_paths() {
    let (repo, _db) = setup().await;
    let root = repo.insert("Root".into(), None, None, None).await.unwrap();
    let child = repo.insert("Child".into(), Some(root.id), None, None).await.unwrap();

    repo.apply_batch(vec![TaskOp::Delete { id: child.id }]).await.unwrap();

    let deleted = repo.find_by_id(child.id).await.unwrap();
    assert!(deleted.deleted);
    assert!(deleted.path.is_some());
}

#[tokio::test]
async fn undelete_preserves_paths() {
    let (repo, _db) = setup().await;
    let root = repo.insert("Root".into(), None, None, None).await.unwrap();
    let child = repo.insert("Child".into(), Some(root.id), None, None).await.unwrap();
    let original_path = child.path.clone();

    repo.soft_delete(child.id).await.unwrap();
    repo.undelete_last().await.unwrap();

    let restored = repo.find_by_id(child.id).await.unwrap();
    assert!(!restored.deleted);
    assert_eq!(restored.path, original_path);
}

// ---------------------------------------------------------------------------
// Integration tests: include_ancestors option
// ---------------------------------------------------------------------------

#[tokio::test]
async fn include_ancestors_adds_parent_chain() {
    let (repo, _db) = setup().await;

    // Tree: root -> a -> b -> c, root -> d
    let root = repo.insert("Root".into(), None, None, None).await.unwrap();
    let a = repo.insert("A".into(), Some(root.id), None, None).await.unwrap();
    let b = repo.insert("B".into(), Some(a.id), None, None).await.unwrap();
    let _c = repo.insert("C".into(), Some(b.id), None, None).await.unwrap();
    let _d = repo.insert("D".into(), Some(root.id), None, None).await.unwrap();

    // Filter: only C matches (by description).
    use not_yet_done_task_core::filter::{FilterExpr, FilterLeaf, ColRef, Operator, Rhs, Literal};
    use not_yet_done_task_core::filter::query_filter::QueryOptions;

    let expr = FilterExpr::Leaf(FilterLeaf {
        lhs: ColRef::unqualified("description"),
        op: Operator::Eq,
        rhs: Rhs::Lit(Literal::String("C".into())),
    });

    // Without include_ancestors: only C.
    let without = repo.find_filtered_with_options(&expr, &QueryOptions::default()).await.unwrap();
    assert_eq!(without.len(), 1);
    assert_eq!(without[0].description, "C");

    // With include_ancestors: C + B + A + Root.
    let opts = QueryOptions { include_ancestors: true };
    let with = repo.find_filtered_with_options(&expr, &opts).await.unwrap();
    let names: Vec<&str> = with.iter().map(|t| t.description.as_str()).collect();
    assert_eq!(with.len(), 4);
    assert!(names.contains(&"C"));
    assert!(names.contains(&"B"));
    assert!(names.contains(&"A"));
    assert!(names.contains(&"Root"));
    // D should NOT be included (sibling, not ancestor).
    assert!(!names.contains(&"D"));
}

#[tokio::test]
async fn include_ancestors_no_duplicates() {
    let (repo, _db) = setup().await;

    // Tree: root -> a -> b, root -> a -> c
    let root = repo.insert("Root".into(), None, None, None).await.unwrap();
    let a = repo.insert("A".into(), Some(root.id), None, None).await.unwrap();
    let _b = repo.insert("B".into(), Some(a.id), None, None).await.unwrap();
    let _c = repo.insert("C".into(), Some(a.id), None, None).await.unwrap();

    // Filter matches B and C (both children of A).
    use not_yet_done_task_core::filter::{FilterExpr, FilterLeaf, ColRef, Operator, Rhs, Literal};
    use not_yet_done_task_core::filter::query_filter::QueryOptions;

    let expr = FilterExpr::Or(vec![
        FilterExpr::Leaf(FilterLeaf {
            lhs: ColRef::unqualified("description"),
            op: Operator::Eq,
            rhs: Rhs::Lit(Literal::String("B".into())),
        }),
        FilterExpr::Leaf(FilterLeaf {
            lhs: ColRef::unqualified("description"),
            op: Operator::Eq,
            rhs: Rhs::Lit(Literal::String("C".into())),
        }),
    ]);

    let opts = QueryOptions { include_ancestors: true };
    let results = repo.find_filtered_with_options(&expr, &opts).await.unwrap();
    // B, C, A, Root — no duplicates of A or Root.
    assert_eq!(results.len(), 4);
    let ids: std::collections::HashSet<Uuid> = results.iter().map(|t| t.id).collect();
    assert_eq!(ids.len(), 4); // all unique
}

#[tokio::test]
async fn include_ancestors_with_already_matching_parent() {
    let (repo, _db) = setup().await;

    // Tree: root -> a -> b
    let root = repo.insert("Root".into(), None, None, None).await.unwrap();
    let _a = repo.insert("A".into(), Some(root.id), None, None).await.unwrap();
    let _b = repo.insert("B".into(), Some(_a.id), None, None).await.unwrap();

    // Filter matches A and B (A is already in results, should not duplicate).
    use not_yet_done_task_core::filter::{FilterExpr, FilterLeaf, ColRef, Operator, Rhs, Literal};
    use not_yet_done_task_core::filter::query_filter::QueryOptions;

    let expr = FilterExpr::Or(vec![
        FilterExpr::Leaf(FilterLeaf {
            lhs: ColRef::unqualified("description"),
            op: Operator::Eq,
            rhs: Rhs::Lit(Literal::String("A".into())),
        }),
        FilterExpr::Leaf(FilterLeaf {
            lhs: ColRef::unqualified("description"),
            op: Operator::Eq,
            rhs: Rhs::Lit(Literal::String("B".into())),
        }),
    ]);

    let opts = QueryOptions { include_ancestors: true };
    let results = repo.find_filtered_with_options(&expr, &opts).await.unwrap();
    // A, B, Root — A already in results, Root added as ancestor.
    assert_eq!(results.len(), 3);
}
