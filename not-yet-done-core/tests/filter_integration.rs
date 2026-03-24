// not-yet-done-core/tests/filter_integration.rs

//! Integration tests for the filter DSL.
//!
//! Each test:
//!   1. Spins up a fresh in-memory SQLite database
//!   2. Seeds it with tasks from `fixtures/tasks.yaml`
//!   3. Loads a filter fixture (FilterExpr + expected descriptions)
//!   4. Runs `find_filtered` and asserts the result matches expectations
//!
//! Tests are order-independent — every test gets its own DB.

use std::collections::HashSet;
use std::path::Path;

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, Database, DatabaseConnection,
    DbBackend, Schema,
};
use serde::Deserialize;
use uuid::Uuid;

use not_yet_done_core::entity::task::{self, ActiveModel, TaskStatus};
use not_yet_done_core::filter::FilterExpr;
use not_yet_done_core::repository::{TaskRepository, TaskRepositoryImpl, TaskRepositoryImplParameters};

// ---------------------------------------------------------------------------
// Fixture types
// ---------------------------------------------------------------------------

/// One task entry from `fixtures/tasks.yaml`.
#[derive(Debug, Deserialize)]
struct TaskSeed {
    description: String,
    #[serde(default = "default_status")]
    status: String,
    #[serde(default)]
    priority: i32,
    #[serde(default)]
    deleted: bool,
    /// Optional: description of the parent task (resolved after all tasks inserted).
    #[serde(default)]
    parent_description: Option<String>,
}

fn default_status() -> String { "todo".to_string() }

/// One filter fixture file.
#[derive(Debug, Deserialize)]
struct FilterFixture {
    description: String,
    filter: FilterExpr,
    expected: Vec<String>,
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Create a fresh in-memory SQLite DB and sync the task schema.
async fn setup_db() -> DatabaseConnection {
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("failed to open in-memory SQLite");

    let schema = Schema::new(DbBackend::Sqlite);
    let stmt = schema.create_table_from_entity(task::Entity);

    db.execute(&stmt)
        .await
        .expect("failed to create task table");

    db
}

/// Seed the database from `fixtures/tasks.yaml` and return a map of
/// description → inserted Uuid (needed to resolve parent references).
async fn seed_tasks(db: &DatabaseConnection) -> std::collections::HashMap<String, Uuid> {
    let yaml = std::fs::read_to_string(fixture_path("tasks.yaml"))
        .expect("tasks.yaml not found");
    let seeds: Vec<TaskSeed> = serde_yaml::from_str(&yaml)
        .expect("failed to parse tasks.yaml");

    let mut desc_to_id: std::collections::HashMap<String, Uuid> = Default::default();

    // First pass: insert tasks without parent_id
    for seed in &seeds {
        if seed.parent_description.is_some() { continue; }

        let status = parse_status(&seed.status);
        let id = Uuid::new_v4();
        let now = Utc::now();

        let model = ActiveModel {
            id: Set(id),
            description: Set(seed.description.clone()),
            status: Set(status),
            priority: Set(seed.priority),
            deleted: Set(seed.deleted),
            parent_id: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        };
        model.insert(db).await.expect("seed insert failed");
        desc_to_id.insert(seed.description.clone(), id);
    }

    // Second pass: insert tasks that have a parent
    for seed in &seeds {
        let Some(ref parent_desc) = seed.parent_description else { continue };

        let parent_id = *desc_to_id.get(parent_desc)
            .unwrap_or_else(|| panic!("parent '{}' not found in seed data", parent_desc));

        let status = parse_status(&seed.status);
        let id = Uuid::new_v4();
        let now = Utc::now();

        let model = ActiveModel {
            id: Set(id),
            description: Set(seed.description.clone()),
            status: Set(status),
            priority: Set(seed.priority),
            deleted: Set(seed.deleted),
            parent_id: Set(Some(parent_id)),
            created_at: Set(now),
            updated_at: Set(now),
        };
        model.insert(db).await.expect("seed insert (child) failed");
        desc_to_id.insert(seed.description.clone(), id);
    }

    desc_to_id
}

fn parse_status(s: &str) -> TaskStatus {
    match s {
        "todo"        => TaskStatus::Todo,
        "in_progress" => TaskStatus::InProgress,
        "done"        => TaskStatus::Done,
        "cancelled"   => TaskStatus::Cancelled,
        other         => panic!("unknown status in fixture: '{other}'"),
    }
}

fn fixture_path(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Load a filter fixture file and return it.
fn load_fixture(name: &str) -> FilterFixture {
    let yaml = std::fs::read_to_string(fixture_path(name))
        .unwrap_or_else(|_| panic!("fixture '{name}' not found"));
    serde_yaml::from_str(&yaml)
        .unwrap_or_else(|e| panic!("failed to parse fixture '{name}': {e}"))
}

/// Run a single filter fixture against the database and assert the results.
async fn run_fixture(repo: &dyn TaskRepository, fixture_name: &str) {
    let fixture = load_fixture(fixture_name);

    let results = repo
        .find_filtered(&fixture.filter)
        .await
        .unwrap_or_else(|e| panic!("[{}] find_filtered failed: {e}", fixture.description));

    let actual: HashSet<String> = results
        .into_iter()
        .map(|t| t.description)
        .collect();

    let expected: HashSet<String> = fixture.expected.into_iter().collect();

    assert_eq!(
        actual, expected,
        "\n[{}]\n  missing:    {:?}\n  unexpected: {:?}",
        fixture.description,
        expected.difference(&actual).collect::<Vec<_>>(),
        actual.difference(&expected).collect::<Vec<_>>(),
    );
}

// ---------------------------------------------------------------------------
// Test setup macro — reduces boilerplate per test
// ---------------------------------------------------------------------------

/// Sets up a fresh DB, seeds it, builds a repo, and returns it as a
/// `Box<dyn TaskRepository>` together with the module that owns it.
///
/// We use `AppModule` so the repo is properly wired; we extract it via
/// `HasComponent::resolve`.
macro_rules! fresh_repo {
    () => {{
        use shaku::HasComponent;
        use not_yet_done_core::module::AppModule;

        let db = setup_db().await;
        seed_tasks(&db).await;

        let module = AppModule::builder()
            .with_component_parameters::<TaskRepositoryImpl>(
                TaskRepositoryImplParameters { db: Some(db) }
            )
            .build();

        let repo: std::sync::Arc<dyn TaskRepository> = module.resolve();
        repo
    }};
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_filter_like() {
    let repo = fresh_repo!();
    run_fixture(repo.as_ref(), "filter_like.yaml").await;
}

#[tokio::test]
async fn test_filter_priority_range() {
    let repo = fresh_repo!();
    run_fixture(repo.as_ref(), "filter_priority_range.yaml").await;
}

#[tokio::test]
async fn test_filter_status_in() {
    let repo = fresh_repo!();
    run_fixture(repo.as_ref(), "filter_status_in.yaml").await;
}

#[tokio::test]
async fn test_filter_not() {
    let repo = fresh_repo!();
    run_fixture(repo.as_ref(), "filter_not.yaml").await;
}

#[tokio::test]
async fn test_filter_compound() {
    let repo = fresh_repo!();
    run_fixture(repo.as_ref(), "filter_compound.yaml").await;
}

#[tokio::test]
async fn test_filter_col_vs_col() {
    let repo = fresh_repo!();
    run_fixture(repo.as_ref(), "filter_col_vs_col.yaml").await;
}

#[tokio::test]
async fn test_filter_is_null() {
    let repo = fresh_repo!();
    run_fixture(repo.as_ref(), "filter_is_null.yaml").await;
}
