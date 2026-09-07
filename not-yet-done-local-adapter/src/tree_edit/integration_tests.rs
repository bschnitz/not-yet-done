//! Integration tests for tree_edit: serialize → edit → parse → diff → apply
//! against a real in-memory SQLite database.

#![cfg(test)]

use std::collections::HashSet;
use std::sync::Arc;

use chrono::Utc;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ConnectionTrait, Database, DbBackend, Schema};
use shaku::HasComponent;
use uuid::Uuid;

use not_yet_done_task_core::entity::{
    global_tag, project, project_tag,
    task::{self, TaskStatus},
    task_global_tag, task_project_tag, tracking,
};
use not_yet_done_task_core::module::TaskDomainModule;
use not_yet_done_task_core::repository::{
    ProjectRepositoryImpl, ProjectRepositoryImplParameters, TagRepositoryImpl,
    TagRepositoryImplParameters, TaskRepositoryImpl, TaskRepositoryImplParameters,
    TrackingRepository, TrackingRepositoryImpl, TrackingRepositoryImplParameters,
};
use not_yet_done_task_core::service::{TaskService, TrackingPolicy, TrackingService};

use super::diff::apply_changes;
use super::serialize::{serialize, short_id};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

async fn setup() -> (
    Arc<dyn TaskService>,
    Arc<dyn TrackingService>,
    Arc<dyn not_yet_done_task_core::service::TagService>,
    sea_orm::DatabaseConnection,
) {
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("failed to open in-memory SQLite");

    let schema = Schema::new(DbBackend::Sqlite);
    for stmt in [
        schema.create_table_from_entity(task::Entity),
        schema.create_table_from_entity(tracking::Entity),
        // Tag tables: the `tasks` adapter's snapshot load batch-resolves
        // tags for every task, so the join + tag tables must exist even
        // when a test creates no tags.
        schema.create_table_from_entity(global_tag::Entity),
        schema.create_table_from_entity(project::Entity),
        schema.create_table_from_entity(project_tag::Entity),
        schema.create_table_from_entity(task_global_tag::Entity),
        schema.create_table_from_entity(task_project_tag::Entity),
    ] {
        db.execute(&stmt).await.expect("schema creation failed");
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

    let service: Arc<dyn TaskService> = module.resolve();
    let tracking: Arc<dyn TrackingService> = module.resolve();
    let tag_service: Arc<dyn not_yet_done_task_core::service::TagService> = module.resolve();
    (service, tracking, tag_service, db)
}

async fn insert_task(
    db: &sea_orm::DatabaseConnection,
    desc: &str,
    parent_id: Option<Uuid>,
    status: TaskStatus,
    priority: i32,
) -> task::Model {
    let now = Utc::now();
    let model = task::ActiveModel {
        id: Set(Uuid::new_v4()),
        description: Set(desc.to_string()),
        status: Set(status),
        deleted: Set(false),
        deleted_at: Set(None),
        priority: Set(priority),
        parent_id: Set(parent_id),
        created_at: Set(now),
        updated_at: Set(now),
        last_tracked_at: Set(None),
        path: Set(None),
    };
    model.insert(db).await.expect("insert failed")
}

async fn all_tasks(db: &sea_orm::DatabaseConnection) -> Vec<task::Model> {
    use sea_orm::EntityTrait;
    task::Entity::find().all(db).await.expect("query failed")
}

fn find_by_desc<'a>(tasks: &'a [task::Model], desc: &str) -> &'a task::Model {
    tasks
        .iter()
        .find(|t| t.description == desc)
        .unwrap_or_else(|| panic!("Task '{}' not found", desc))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn round_trip_no_changes() {
    let (service, tracking, _tag_service, db) = setup().await;
    let root = insert_task(&db, "Root", None, TaskStatus::Todo, 0).await;
    let child = insert_task(&db, "Child", Some(root.id), TaskStatus::Done, 3).await;

    let subtree = vec![root.clone(), child.clone()];
    let content = serialize(&root, &subtree);

    let result = apply_changes(
        &content,
        &subtree,
        root.id,
        &service,
        &tracking,
        &HashSet::new(),
        &TrackingPolicy::Parallel,
    )
    .await;
    assert_eq!(result.unwrap(), "No changes");
}

#[tokio::test]
async fn rename_task() {
    let (service, tracking, _tag_service, db) = setup().await;
    let root = insert_task(&db, "Old Name", None, TaskStatus::Todo, 0).await;

    let subtree = vec![root.clone()];
    let sid = short_id(root.id);
    let content = format!("- [ ] New Name  (p=0  id={sid})\n");

    let result = apply_changes(
        &content,
        &subtree,
        root.id,
        &service,
        &tracking,
        &HashSet::new(),
        &TrackingPolicy::Parallel,
    )
    .await;
    assert!(result.unwrap().contains("updated"));

    let tasks = all_tasks(&db).await;
    assert_eq!(tasks[0].description, "New Name");
}

#[tokio::test]
async fn toggle_status() {
    let (service, tracking, _tag_service, db) = setup().await;
    let root = insert_task(&db, "Task", None, TaskStatus::Todo, 5).await;

    let subtree = vec![root.clone()];
    let sid = short_id(root.id);
    let content = format!("- [x] Task  (p=5  id={sid})\n");

    let result = apply_changes(
        &content,
        &subtree,
        root.id,
        &service,
        &tracking,
        &HashSet::new(),
        &TrackingPolicy::Parallel,
    )
    .await;
    assert!(result.unwrap().contains("updated"));

    let tasks = all_tasks(&db).await;
    assert_eq!(tasks[0].status, TaskStatus::Done);
}

#[tokio::test]
async fn create_new_child() {
    let (service, tracking, _tag_service, db) = setup().await;
    let root = insert_task(&db, "Root", None, TaskStatus::Todo, 0).await;

    let subtree = vec![root.clone()];
    let sid = short_id(root.id);
    let content = format!("- [ ] Root  (p=0  id={sid})\n  - [ ] New child\n");

    let result = apply_changes(
        &content,
        &subtree,
        root.id,
        &service,
        &tracking,
        &HashSet::new(),
        &TrackingPolicy::Parallel,
    )
    .await;
    assert!(result.unwrap().contains("created"));

    let tasks = all_tasks(&db).await;
    assert_eq!(tasks.len(), 2);
    let child = find_by_desc(&tasks, "New child");
    assert_eq!(child.parent_id, Some(root.id));
}

#[tokio::test]
async fn create_nested_new_items() {
    let (service, tracking, _tag_service, db) = setup().await;
    let root = insert_task(&db, "Root", None, TaskStatus::Todo, 0).await;

    let subtree = vec![root.clone()];
    let sid = short_id(root.id);
    let content = format!(
        "- [ ] Root  (p=0  id={sid})\n\
         \x20 - [ ] New parent\n\
         \x20   - [x] New grandchild\n"
    );

    let result = apply_changes(
        &content,
        &subtree,
        root.id,
        &service,
        &tracking,
        &HashSet::new(),
        &TrackingPolicy::Parallel,
    )
    .await;
    let msg = result.unwrap();
    assert!(msg.contains("2 created"), "expected 2 created, got: {msg}");

    let tasks = all_tasks(&db).await;
    assert_eq!(tasks.len(), 3);

    let parent = find_by_desc(&tasks, "New parent");
    assert_eq!(parent.parent_id, Some(root.id));

    let grandchild = find_by_desc(&tasks, "New grandchild");
    assert_eq!(grandchild.parent_id, Some(parent.id));
    assert_eq!(grandchild.status, TaskStatus::Done);
}

#[tokio::test]
async fn soft_delete_missing_item() {
    let (service, tracking, _tag_service, db) = setup().await;
    let root = insert_task(&db, "Root", None, TaskStatus::Todo, 0).await;
    let child = insert_task(&db, "Will be deleted", Some(root.id), TaskStatus::Todo, 0).await;

    let subtree = vec![root.clone(), child.clone()];
    let sid = short_id(root.id);
    // Only root in the editor — child is removed.
    let content = format!("- [ ] Root  (p=0  id={sid})\n");

    let result = apply_changes(
        &content,
        &subtree,
        root.id,
        &service,
        &tracking,
        &HashSet::new(),
        &TrackingPolicy::Parallel,
    )
    .await;
    assert!(result.unwrap().contains("deleted"));

    let tasks = all_tasks(&db).await;
    let deleted = find_by_desc(&tasks, "Will be deleted");
    assert!(deleted.deleted);
}

#[tokio::test]
async fn reparent_task() {
    let (service, tracking, _tag_service, db) = setup().await;
    let root = insert_task(&db, "Root", None, TaskStatus::Todo, 0).await;
    let child_a = insert_task(&db, "A", Some(root.id), TaskStatus::Todo, 0).await;
    let child_b = insert_task(&db, "B", Some(root.id), TaskStatus::Todo, 0).await;

    let subtree = vec![root.clone(), child_a.clone(), child_b.clone()];
    let r_sid = short_id(root.id);
    let a_sid = short_id(child_a.id);
    let b_sid = short_id(child_b.id);

    // Move B under A.
    let content = format!(
        "- [ ] Root  (p=0  id={r_sid})\n\
         \x20 - [ ] A  (p=0  id={a_sid})\n\
         \x20   - [ ] B  (p=0  id={b_sid})\n"
    );

    let result = apply_changes(
        &content,
        &subtree,
        root.id,
        &service,
        &tracking,
        &HashSet::new(),
        &TrackingPolicy::Parallel,
    )
    .await;
    assert!(result.unwrap().contains("updated"));

    let tasks = all_tasks(&db).await;
    let b = find_by_desc(&tasks, "B");
    assert_eq!(b.parent_id, Some(child_a.id));
}

#[tokio::test]
async fn change_priority() {
    let (service, tracking, _tag_service, db) = setup().await;
    let root = insert_task(&db, "Task", None, TaskStatus::Todo, 0).await;

    let subtree = vec![root.clone()];
    let sid = short_id(root.id);
    let content = format!("- [ ] Task  (p=9  id={sid})\n");

    let result = apply_changes(
        &content,
        &subtree,
        root.id,
        &service,
        &tracking,
        &HashSet::new(),
        &TrackingPolicy::Parallel,
    )
    .await;
    assert!(result.unwrap().contains("updated"));

    let tasks = all_tasks(&db).await;
    assert_eq!(tasks[0].priority, 9);
}

#[tokio::test]
async fn multiple_changes_at_once() {
    let (service, tracking, _tag_service, db) = setup().await;
    let root = insert_task(&db, "Root", None, TaskStatus::Todo, 0).await;
    let keep = insert_task(&db, "Keep", Some(root.id), TaskStatus::Todo, 0).await;
    let remove = insert_task(&db, "Remove", Some(root.id), TaskStatus::Todo, 0).await;

    let subtree = vec![root.clone(), keep.clone(), remove.clone()];
    let r_sid = short_id(root.id);
    let k_sid = short_id(keep.id);

    let content = format!(
        "- [~] Root  (p=5  id={r_sid})\n\
         \x20 - [x] Renamed  (p=1  id={k_sid})\n\
         \x20 - [ ] Brand new\n"
    );

    let result = apply_changes(
        &content,
        &subtree,
        root.id,
        &service,
        &tracking,
        &HashSet::new(),
        &TrackingPolicy::Parallel,
    )
    .await;
    let msg = result.unwrap();
    // Should have updates, creates, and deletes.
    assert!(msg.contains("created"), "msg: {msg}");
    assert!(msg.contains("updated"), "msg: {msg}");
    assert!(msg.contains("deleted"), "msg: {msg}");

    let tasks = all_tasks(&db).await;
    let root_task = find_by_desc(&tasks, "Root");
    assert_eq!(root_task.status, TaskStatus::InProgress);
    assert_eq!(root_task.priority, 5);

    let renamed = find_by_desc(&tasks, "Renamed");
    assert_eq!(renamed.status, TaskStatus::Done);
    assert_eq!(renamed.priority, 1);

    let removed = find_by_desc(&tasks, "Remove");
    assert!(removed.deleted);

    assert!(tasks.iter().any(|t| t.description == "Brand new"));
}

#[tokio::test]
async fn delete_subtree_by_removing_from_editor() {
    let (service, tracking, _tag_service, db) = setup().await;
    let root = insert_task(&db, "Root", None, TaskStatus::Todo, 0).await;
    let child_a = insert_task(&db, "Child A", Some(root.id), TaskStatus::Todo, 0).await;
    let grandchild_a =
        insert_task(&db, "Grandchild A", Some(child_a.id), TaskStatus::Todo, 0).await;
    let child_b = insert_task(&db, "Child B", Some(root.id), TaskStatus::Todo, 0).await;
    let grandchild_b =
        insert_task(&db, "Grandchild B", Some(child_b.id), TaskStatus::Todo, 0).await;

    let subtree = vec![
        root.clone(),
        child_a.clone(),
        grandchild_a.clone(),
        child_b.clone(),
        grandchild_b.clone(),
    ];

    // Remove child_a and grandchild_a from the editor — keep only root, child_b, grandchild_b.
    let r_sid = short_id(root.id);
    let b_sid = short_id(child_b.id);
    let gb_sid = short_id(grandchild_b.id);
    let content = format!(
        "- [ ] Root  (p=0  id={r_sid})\n\
         \x20 - [ ] Child B  (p=0  id={b_sid})\n\
         \x20   - [ ] Grandchild B  (p=0  id={gb_sid})\n"
    );

    let result = apply_changes(
        &content,
        &subtree,
        root.id,
        &service,
        &tracking,
        &HashSet::new(),
        &TrackingPolicy::Parallel,
    )
    .await;
    let msg = result.unwrap();
    assert!(msg.contains("deleted"), "expected deletions, got: {msg}");

    let tasks = all_tasks(&db).await;
    let a = find_by_desc(&tasks, "Child A");
    assert!(a.deleted, "Child A should be soft-deleted");
    let ga = find_by_desc(&tasks, "Grandchild A");
    assert!(ga.deleted, "Grandchild A should be soft-deleted");
    let b = find_by_desc(&tasks, "Child B");
    assert!(!b.deleted, "Child B should NOT be deleted");
}

/// Reproduces the bug: original had items that were then removed in editor,
/// but apply_changes returned "No changes".
#[tokio::test]
async fn delete_items_exact_user_scenario() {
    let (service, tracking, _tag_service, db) = setup().await;
    // Build exact tree from bug report.
    let root = insert_task(&db, "Build compost bin system", None, TaskStatus::Todo, 0).await;
    let item_a = insert_task(&db, "A new Item", Some(root.id), TaskStatus::Todo, 0).await;
    let sub_a = insert_task(&db, "A new Subitem", Some(item_a.id), TaskStatus::Todo, 0).await;
    let item_b = insert_task(&db, "A new Item 2", Some(root.id), TaskStatus::Todo, 0).await;
    let sub_b = insert_task(&db, "A new Subitem 2", Some(item_b.id), TaskStatus::Todo, 0).await;
    let renamed = insert_task(
        &db,
        "Build wooden frame structure renamed",
        Some(item_b.id),
        TaskStatus::Todo,
        0,
    )
    .await;
    let design = insert_task(
        &db,
        "Design compost bin with multiple compartments",
        Some(root.id),
        TaskStatus::Todo,
        0,
    )
    .await;

    let subtree = vec![
        root.clone(),
        item_a.clone(),
        sub_a.clone(),
        item_b.clone(),
        sub_b.clone(),
        renamed.clone(),
        design.clone(),
    ];

    // Serialize original (should show all 7 items).
    let original_content = crate::tree_edit::serialize(&root, &subtree);
    assert!(
        original_content.contains("A new Item"),
        "original should contain A new Item"
    );

    // Edited: remove item_a and sub_a (keep item_b, sub_b, renamed, design).
    let r_sid = short_id(root.id);
    let b_sid = short_id(item_b.id);
    let sb_sid = short_id(sub_b.id);
    let rn_sid = short_id(renamed.id);
    let d_sid = short_id(design.id);

    let content = format!(
        "- [ ] Build compost bin system  (p=0  id={r_sid})\n\
         \x20 - [ ] A new Item 2  (p=0  id={b_sid})\n\
         \x20   - [ ] A new Subitem 2  (p=0  id={sb_sid})\n\
         \x20   - [ ] Build wooden frame structure renamed  (p=0  id={rn_sid})\n\
         \x20 - [ ] Design compost bin with multiple compartments  (p=0  id={d_sid})\n"
    );

    let result = apply_changes(
        &content,
        &subtree,
        root.id,
        &service,
        &tracking,
        &HashSet::new(),
        &TrackingPolicy::Parallel,
    )
    .await;
    let msg = result.unwrap();
    assert!(msg.contains("deleted"), "expected deletions, got: {msg}");

    let tasks = all_tasks(&db).await;
    let a = tasks.iter().find(|t| t.id == item_a.id).unwrap();
    assert!(a.deleted, "item_a should be soft-deleted");
    let sa = tasks.iter().find(|t| t.id == sub_a.id).unwrap();
    assert!(sa.deleted, "sub_a should be soft-deleted");
}

/// Test with duplicate names — the diff should use IDs, not descriptions.
#[tokio::test]
async fn delete_with_duplicate_names() {
    let (service, tracking, _tag_service, db) = setup().await;
    let root = insert_task(&db, "Root", None, TaskStatus::Todo, 0).await;
    let dup_a = insert_task(&db, "A new Item", Some(root.id), TaskStatus::Todo, 0).await;
    let child_of_a = insert_task(&db, "A new Subitem", Some(dup_a.id), TaskStatus::Todo, 0).await;
    let dup_b = insert_task(&db, "A new Item", Some(root.id), TaskStatus::Todo, 0).await;
    let child_of_b = insert_task(&db, "A new Subitem", Some(dup_b.id), TaskStatus::Todo, 0).await;

    let subtree = vec![
        root.clone(),
        dup_a.clone(),
        child_of_a.clone(),
        dup_b.clone(),
        child_of_b.clone(),
    ];

    // Remove dup_a (first "A new Item") and its child.
    let r_sid = short_id(root.id);
    let b_sid = short_id(dup_b.id);
    let cb_sid = short_id(child_of_b.id);
    let content = format!(
        "- [ ] Root  (p=0  id={r_sid})\n\
         \x20 - [ ] A new Item  (p=0  id={b_sid})\n\
         \x20   - [ ] A new Subitem  (p=0  id={cb_sid})\n"
    );

    let result = apply_changes(
        &content,
        &subtree,
        root.id,
        &service,
        &tracking,
        &HashSet::new(),
        &TrackingPolicy::Parallel,
    )
    .await;
    let msg = result.unwrap();
    assert!(msg.contains("deleted"), "expected deletions, got: {msg}");

    let tasks = all_tasks(&db).await;
    let a = tasks.iter().find(|t| t.id == dup_a.id).unwrap();
    assert!(a.deleted, "dup_a should be soft-deleted");
    let ca = tasks.iter().find(|t| t.id == child_of_a.id).unwrap();
    assert!(ca.deleted, "child_of_a should be soft-deleted");
    let b = tasks.iter().find(|t| t.id == dup_b.id).unwrap();
    assert!(!b.deleted, "dup_b should NOT be deleted");
}

/// Restore a deleted task by changing [D] to [ ].
#[tokio::test]
async fn restore_deleted_task_via_marker() {
    let (service, tracking, _tag_service, db) = setup().await;
    let root = insert_task(&db, "Root", None, TaskStatus::Todo, 0).await;
    // Manually soft-delete a child.
    let child = insert_task(&db, "Deleted child", Some(root.id), TaskStatus::Todo, 0).await;
    service.delete_task(child.id).await.unwrap();

    // Reload to get deleted=true.
    let tasks = all_tasks(&db).await;
    let subtree: Vec<_> = tasks.iter().cloned().collect();

    let r_sid = short_id(root.id);
    let c_sid = short_id(child.id);

    // Editor shows [D], user changes to [ ] to restore.
    let content = format!(
        "- [ ] Root  (p=0  id={r_sid})\n\
         \x20 - [ ] Deleted child  (p=0  id={c_sid})\n"
    );

    let result = apply_changes(
        &content,
        &subtree,
        root.id,
        &service,
        &tracking,
        &HashSet::new(),
        &TrackingPolicy::Parallel,
    )
    .await;
    let msg = result.unwrap();
    assert!(
        msg.contains("updated"),
        "expected update for restore, got: {msg}"
    );

    let tasks = all_tasks(&db).await;
    let restored = find_by_desc(&tasks, "Deleted child");
    assert!(
        !restored.deleted,
        "child should be restored (deleted=false)"
    );
}

/// Delete a task by changing [ ] to [D].
#[tokio::test]
async fn delete_task_via_d_marker() {
    let (service, tracking, _tag_service, db) = setup().await;
    let root = insert_task(&db, "Root", None, TaskStatus::Todo, 0).await;
    let child = insert_task(&db, "Active child", Some(root.id), TaskStatus::Todo, 0).await;

    let subtree = vec![root.clone(), child.clone()];
    let r_sid = short_id(root.id);
    let c_sid = short_id(child.id);

    // User changes [ ] to [D].
    let content = format!(
        "- [ ] Root  (p=0  id={r_sid})\n\
         \x20 - [D] Active child  (p=0  id={c_sid})\n"
    );

    let result = apply_changes(
        &content,
        &subtree,
        root.id,
        &service,
        &tracking,
        &HashSet::new(),
        &TrackingPolicy::Parallel,
    )
    .await;
    let msg = result.unwrap();
    assert!(
        msg.contains("updated"),
        "expected update for delete, got: {msg}"
    );

    let tasks = all_tasks(&db).await;
    let deleted = find_by_desc(&tasks, "Active child");
    assert!(deleted.deleted, "child should be soft-deleted");
}

/// Exact reproduction: two siblings named "A new Item", each with a child
/// named "A new Subitem". Remove one sibling and its child.
/// The serialize→edit→apply round trip must detect the deletion.
#[tokio::test]
async fn serialize_edit_delete_round_trip() {
    let (service, tracking, _tag_service, db) = setup().await;
    let root = insert_task(&db, "Build compost bin system", None, TaskStatus::Todo, 0).await;
    let dup_a = insert_task(&db, "A new Item", Some(root.id), TaskStatus::Todo, 0).await;
    let child_a = insert_task(&db, "A new Subitem", Some(dup_a.id), TaskStatus::Todo, 0).await;
    let dup_b = insert_task(&db, "A new Item", Some(root.id), TaskStatus::Todo, 0).await;
    let child_b = insert_task(&db, "A new Subitem", Some(dup_b.id), TaskStatus::Todo, 0).await;
    let extra = insert_task(
        &db,
        "Build wooden frame structure renamed",
        Some(dup_b.id),
        TaskStatus::Todo,
        0,
    )
    .await;
    let design = insert_task(
        &db,
        "Design compost bin with multiple compartments",
        Some(root.id),
        TaskStatus::Todo,
        0,
    )
    .await;

    let subtree = vec![
        root.clone(),
        dup_a.clone(),
        child_a.clone(),
        dup_b.clone(),
        child_b.clone(),
        extra.clone(),
        design.clone(),
    ];

    // Step 1: Serialize (what the user sees in their editor).
    let original_content = crate::tree_edit::serialize(&root, &subtree);
    eprintln!("=== SERIALIZED ===\n{original_content}");

    // Step 2: The user removes dup_a and child_a lines, keeping the rest.
    // We need to find and remove the lines for dup_a and child_a by their short IDs.
    let a_sid = short_id(dup_a.id);
    let ca_sid = short_id(child_a.id);
    let edited: String = original_content
        .lines()
        .filter(|line| !line.contains(&a_sid) && !line.contains(&ca_sid))
        .map(|l| format!("{l}\n"))
        .collect();
    eprintln!("=== EDITED ===\n{edited}");

    // Step 3: Apply.
    let result = apply_changes(
        &edited,
        &subtree,
        root.id,
        &service,
        &tracking,
        &HashSet::new(),
        &TrackingPolicy::Parallel,
    )
    .await;
    let msg = result.unwrap();
    eprintln!("=== RESULT: {msg}");
    assert!(msg.contains("deleted"), "expected deletions, got: {msg}");
}

// ---------------------------------------------------------------------------
// Adapter-level delete: recursive (tree view) vs single (flat list view)
// ---------------------------------------------------------------------------

/// Build a `tasks` adapter over the test DB so we can drive `invoke_action`
/// / `execute` exactly as the TUI does.
fn build_task_adapter(
    service: Arc<dyn TaskService>,
    tracking_service: Arc<dyn TrackingService>,
    tag_service: Arc<dyn not_yet_done_task_core::service::TagService>,
    project_service: Arc<dyn not_yet_done_task_core::service::ProjectService>,
    db: &sea_orm::DatabaseConnection,
    policy: TrackingPolicy,
) -> Box<dyn not_yet_done_content::ContentAdapter> {
    use not_yet_done_content::InMemoryHostBus;
    // The self-contained factory would open its *own* database; the test
    // needs the adapter over the very `service`/`tracking` of its in-memory
    // DB, so build the handle and adapter directly.
    let bus = std::sync::Arc::new(InMemoryHostBus::default());
    let tracking: Arc<dyn TrackingRepository> = tracking_repo_for(&db);
    let handle = crate::CoreHandle::new(
        service,
        tracking,
        tracking_service,
        tag_service,
        project_service,
        bus,
        "test".to_string(),
        policy,
    );
    Box::new(crate::task::TaskAdapter::new("test", handle))
}

/// Resolve a `TrackingRepository` over the test's in-memory `db` for the
/// `CoreHandle`'s direct stop/lookup path.
fn tracking_repo_for(db: &sea_orm::DatabaseConnection) -> Arc<dyn TrackingRepository> {
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
    module.resolve()
}

/// Resolve a `ProjectService` over the test's in-memory `db` — the
/// `CoreHandle` now carries one; these task-adapter tests don't exercise
/// project actions, so any service over the same DB satisfies the constructor.
fn project_service_for(
    db: &sea_orm::DatabaseConnection,
) -> Arc<dyn not_yet_done_task_core::service::ProjectService> {
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
    module.resolve()
}

/// Flat list `delete-single`: only the invoking task is deleted; its child
/// survives (and re-roots to the forest top on the next load). The confirm
/// prompt is the generic one (`None` from the adapter).
#[tokio::test]
async fn adapter_delete_single_leaves_children() {
    use not_yet_done_content::{ActionContext, ActionDispatch, ActionInput};
    let (service, tracking, _tag_service, db) = setup().await;
    let parent = insert_task(&db, "Parent", None, TaskStatus::Todo, 0).await;
    let _child = insert_task(&db, "Child", Some(parent.id), TaskStatus::Todo, 0).await;

    let adapter = build_task_adapter(
        service,
        tracking,
        _tag_service,
        project_service_for(&db),
        &db,
        TrackingPolicy::Exclusive,
    );
    let mut node = adapter.get_by_id(&parent.id.to_string()).await.unwrap();

    // No recursive warning for the single-delete action.
    let dispatch = node
        .invoke_action("delete-single", &ActionContext::default())
        .await
        .unwrap();
    assert!(matches!(
        dispatch,
        ActionDispatch::DeleteSelf { confirm: None }
    ));

    node.execute("delete-single", ActionInput::None, &Default::default())
        .await
        .unwrap();

    let tasks = all_tasks(&db).await;
    assert!(find_by_desc(&tasks, "Parent").deleted, "parent deleted");
    assert!(
        !find_by_desc(&tasks, "Child").deleted,
        "child must survive a single (non-recursive) delete"
    );
}

/// Tree `delete`: recursive — the whole subtree is soft-deleted, and the
/// confirm prompt spells out the cascade with the descendant count.
#[tokio::test]
async fn adapter_delete_recursive_warns_and_cascades() {
    use not_yet_done_content::{ActionContext, ActionDispatch, ActionInput};
    let (service, tracking, _tag_service, db) = setup().await;
    let parent = insert_task(&db, "Parent", None, TaskStatus::Todo, 0).await;
    let child = insert_task(&db, "Child", Some(parent.id), TaskStatus::Todo, 0).await;
    let _grandchild = insert_task(&db, "Grandchild", Some(child.id), TaskStatus::Todo, 0).await;

    let adapter = build_task_adapter(
        service,
        tracking,
        _tag_service,
        project_service_for(&db),
        &db,
        TrackingPolicy::Exclusive,
    );
    let mut node = adapter.get_by_id(&parent.id.to_string()).await.unwrap();

    // The prompt names the cascade and the subtask count (2 descendants).
    let dispatch = node
        .invoke_action("delete", &ActionContext::default())
        .await
        .unwrap();
    let ActionDispatch::DeleteSelf { confirm: Some(msg) } = dispatch else {
        panic!("expected a recursive-delete confirmation prompt");
    };
    assert!(msg.contains("2 subtasks"), "prompt names the count: {msg}");
    assert!(msg.contains("recursive"), "prompt flags the cascade: {msg}");

    node.execute("delete", ActionInput::None, &Default::default())
        .await
        .unwrap();

    let tasks = all_tasks(&db).await;
    assert!(find_by_desc(&tasks, "Parent").deleted, "parent deleted");
    assert!(find_by_desc(&tasks, "Child").deleted, "child cascaded");
    assert!(
        find_by_desc(&tasks, "Grandchild").deleted,
        "grandchild cascaded"
    );
}

// ---------------------------------------------------------------------------
// `toggle-tracking` with `group_paths`
// ---------------------------------------------------------------------------

/// Ids of the tasks currently tracking, by description.
async fn tracking_now(repo: &Arc<dyn TrackingRepository>, tasks: &[task::Model]) -> Vec<String> {
    let active = repo.find_all_active().await.unwrap();
    let mut names: Vec<String> = active
        .iter()
        .map(|t| {
            tasks
                .iter()
                .find(|x| x.id == t.task_id)
                .map(|x| x.description.clone())
                .unwrap_or_default()
        })
        .collect();
    names.sort();
    names
}

/// `toggle-tracking` on `task` with the given `group_paths` argument.
async fn toggle_grouped(
    adapter: &dyn not_yet_done_content::ContentAdapter,
    task: &task::Model,
    group_paths: &[&str],
) -> not_yet_done_content::ActionDispatch {
    use not_yet_done_content::{ActionContext, ArgValue};
    let node = adapter.get_by_id(&task.id.to_string()).await.unwrap();
    let ctx = ActionContext {
        args: not_yet_done_content::ActionArgs::new().with(
            "group_paths",
            ArgValue::List(group_paths.iter().map(|s| s.to_string()).collect()),
        ),
        ..Default::default()
    };
    node.invoke_action("toggle-tracking", &ctx).await.unwrap()
}

/// A start stops only the running trackings of its own group: `/Work/*` is
/// one group, everything else the shared rest group.
#[tokio::test]
async fn grouped_toggle_stops_only_the_same_group() {
    use not_yet_done_content::ActionDispatch;
    let (service, tracking, tag_service, db) = setup().await;
    let work = insert_task(&db, "Work", None, TaskStatus::Todo, 0).await;
    let a = insert_task(&db, "A", Some(work.id), TaskStatus::Todo, 0).await;
    let b = insert_task(&db, "B", Some(work.id), TaskStatus::Todo, 0).await;
    let other = insert_task(&db, "Other", None, TaskStatus::Todo, 0).await;
    let c = insert_task(&db, "C", Some(other.id), TaskStatus::Todo, 0).await;
    let d = insert_task(&db, "D", Some(other.id), TaskStatus::Todo, 0).await;
    let repo = tracking_repo_for(&db);
    let tasks = all_tasks(&db).await;

    let adapter = build_task_adapter(
        service,
        tracking,
        tag_service,
        project_service_for(&db),
        &db,
        TrackingPolicy::Exclusive,
    );
    let groups = ["^/Work"];

    assert!(matches!(
        toggle_grouped(adapter.as_ref(), &c, &groups).await,
        ActionDispatch::Noop
    ));
    toggle_grouped(adapter.as_ref(), &a, &groups).await;
    assert_eq!(
        tracking_now(&repo, &tasks).await,
        vec!["A", "C"],
        "different groups track side by side"
    );

    toggle_grouped(adapter.as_ref(), &b, &groups).await;
    assert_eq!(
        tracking_now(&repo, &tasks).await,
        vec!["B", "C"],
        "B replaces A inside /Work, C outside is untouched"
    );

    toggle_grouped(adapter.as_ref(), &d, &groups).await;
    assert_eq!(
        tracking_now(&repo, &tasks).await,
        vec!["B", "D"],
        "the rest group is one group: D replaces C"
    );

    // Toggling a tracking task stops it, regardless of groups.
    toggle_grouped(adapter.as_ref(), &b, &groups).await;
    assert_eq!(tracking_now(&repo, &tasks).await, vec!["D"]);

    // An invalid regex is refused before anything changes, naming the pattern.
    match toggle_grouped(adapter.as_ref(), &a, &["("]).await {
        ActionDispatch::Error(msg) => assert!(msg.contains("group_paths[0]"), "{msg}"),
        other => panic!("expected an error, got {other:?}"),
    }
    assert_eq!(tracking_now(&repo, &tasks).await, vec!["D"]);
}

/// `group_paths` refines exclusivity; over a parallel policy it is refused
/// instead of silently picking one of the two.
#[tokio::test]
async fn group_paths_over_parallel_policy_is_an_error() {
    use not_yet_done_content::ActionDispatch;
    let (service, tracking, tag_service, db) = setup().await;
    let a = insert_task(&db, "A", None, TaskStatus::Todo, 0).await;
    let repo = tracking_repo_for(&db);

    let adapter = build_task_adapter(
        service,
        tracking,
        tag_service,
        project_service_for(&db),
        &db,
        TrackingPolicy::Parallel,
    );
    match toggle_grouped(adapter.as_ref(), &a, &["^/Work"]).await {
        ActionDispatch::Error(msg) => assert!(msg.contains("allow_parallel"), "{msg}"),
        other => panic!("expected an error, got {other:?}"),
    }
    assert!(repo.find_all_active().await.unwrap().is_empty());
}

/// Without `group_paths` the adapter's configured policy applies: exclusive
/// stops everything else.
#[tokio::test]
async fn plain_toggle_keeps_the_configured_exclusive_policy() {
    use not_yet_done_content::ActionContext;
    let (service, tracking, tag_service, db) = setup().await;
    let a = insert_task(&db, "A", None, TaskStatus::Todo, 0).await;
    let b = insert_task(&db, "B", None, TaskStatus::Todo, 0).await;
    let repo = tracking_repo_for(&db);
    let tasks = all_tasks(&db).await;

    let adapter = build_task_adapter(
        service,
        tracking,
        tag_service,
        project_service_for(&db),
        &db,
        TrackingPolicy::Exclusive,
    );
    for t in [&a, &b] {
        let node = adapter.get_by_id(&t.id.to_string()).await.unwrap();
        node.invoke_action("toggle-tracking", &ActionContext::default())
            .await
            .unwrap();
    }
    assert_eq!(tracking_now(&repo, &tasks).await, vec!["B"]);
}

/// The outline editor's `-t` flags run under the same policy: two `-t` in
/// different groups both start; under the exclusive policy that is refused
/// up front.
#[tokio::test]
async fn outline_tracking_flags_follow_the_policy() {
    let (service, tracking, _tag_service, db) = setup().await;
    let root = insert_task(&db, "Root", None, TaskStatus::Todo, 0).await;
    let work = insert_task(&db, "Work", Some(root.id), TaskStatus::Todo, 0).await;
    let a = insert_task(&db, "A", Some(work.id), TaskStatus::Todo, 0).await;
    let other = insert_task(&db, "Other", Some(root.id), TaskStatus::Todo, 0).await;
    let c = insert_task(&db, "C", Some(other.id), TaskStatus::Todo, 0).await;
    let repo = tracking_repo_for(&db);
    let tasks = all_tasks(&db).await;

    let subtree = vec![
        root.clone(),
        work.clone(),
        a.clone(),
        other.clone(),
        c.clone(),
    ];
    let content = serialize(&root, &subtree);
    let flagged = content
        .lines()
        .map(|l| {
            if l.contains(&short_id(a.id)) || l.contains(&short_id(c.id)) {
                l.replacen("] ", "] -t ", 1)
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let err = apply_changes(
        &flagged,
        &subtree,
        root.id,
        &service,
        &tracking,
        &HashSet::new(),
        &TrackingPolicy::Exclusive,
    )
    .await
    .unwrap_err();
    assert!(err.contains("Only one task"), "{err}");
    assert!(repo.find_all_active().await.unwrap().is_empty());

    let grouped = TrackingPolicy::grouped(&["^/Root/Work"]).unwrap();
    apply_changes(
        &flagged,
        &subtree,
        root.id,
        &service,
        &tracking,
        &HashSet::new(),
        &grouped,
    )
    .await
    .unwrap();
    assert_eq!(tracking_now(&repo, &tasks).await, vec!["A", "C"]);
}
