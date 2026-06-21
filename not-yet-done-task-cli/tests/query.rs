mod common;

use predicates::prelude::*;
use std::io::Write;

#[test]
fn query_tasks_from_file() {
    let (_dir, db_url) = common::setup();
    let _id = common::create_task(&db_url, "Test task for query");

    let filter_file = tempfile::NamedTempFile::new().unwrap();
    write!(filter_file.as_file(), "query:\n  [deleted, =, false]\n").unwrap();

    common::nyd(&db_url)
        .args(["query", "run", "--entity", "task", "--file", filter_file.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Test task for query"));
}

#[test]
fn query_tasks_json_output() {
    let (_dir, db_url) = common::setup();
    common::create_task(&db_url, "JSON output test");

    let filter_file = tempfile::NamedTempFile::new().unwrap();
    write!(filter_file.as_file(), "query:\n  [deleted, =, false]\n").unwrap();

    let output = common::nyd(&db_url)
        .args(["query", "run", "--entity", "task", "--file", filter_file.path().to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Vec<serde_json::Value> = serde_json::from_slice(&output).expect("Valid JSON array");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0]["description"], "JSON output test");
}

#[test]
fn query_trackings_from_file() {
    let (_dir, db_url) = common::setup();
    let task_id = common::create_task(&db_url, "Tracked task");
    common::start_tracking(&db_url, &task_id);

    let filter_file = tempfile::NamedTempFile::new().unwrap();
    write!(filter_file.as_file(), "query:\n  [deleted, =, false]\n").unwrap();

    let output = common::nyd(&db_url)
        .args(["query", "run", "--entity", "tracking", "--file", filter_file.path().to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Vec<serde_json::Value> = serde_json::from_slice(&output).expect("Valid JSON array");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0]["task_id"], task_id);
}

#[test]
fn query_with_date_filter() {
    let (_dir, db_url) = common::setup();
    let task_id = common::create_task(&db_url, "Date filtered task");
    common::start_tracking(&db_url, &task_id);

    // Filter with natural-language date — "yesterday" should include today's tracking
    let filter_file = tempfile::NamedTempFile::new().unwrap();
    write!(
        filter_file.as_file(),
        "query:\n  and:\n    - [deleted, =, false]\n    - [started_at, '>=', yesterday]\n"
    )
    .unwrap();

    let output = common::nyd(&db_url)
        .args(["query", "run", "--entity", "tracking", "--file", filter_file.path().to_str().unwrap()])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: Vec<serde_json::Value> = serde_json::from_slice(&output).expect("Valid JSON array");
    assert_eq!(parsed.len(), 1);
}

#[test]
fn query_debug_shows_resolved_filter() {
    let (_dir, db_url) = common::setup();

    let filter_file = tempfile::NamedTempFile::new().unwrap();
    write!(
        filter_file.as_file(),
        "query:\n  [started_at, '>=', yesterday]\n"
    )
    .unwrap();

    common::nyd(&db_url)
        .args([
            "query", "run", "--entity", "tracking", "--file",
            filter_file.path().to_str().unwrap(), "--debug",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("Resolved filter"))
        .stderr(predicate::str::contains("FilterExpr"));
}

#[test]
fn query_invalid_entity() {
    let (_dir, db_url) = common::setup();

    let filter_file = tempfile::NamedTempFile::new().unwrap();
    write!(filter_file.as_file(), "query:\n  [deleted, =, false]\n").unwrap();

    common::nyd(&db_url)
        .args(["query", "run", "--entity", "bogus", "--file", filter_file.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Unknown entity"));
}

#[test]
fn query_invalid_yaml() {
    let (_dir, db_url) = common::setup();

    let filter_file = tempfile::NamedTempFile::new().unwrap();
    write!(filter_file.as_file(), "{{{{not yaml").unwrap();

    common::nyd(&db_url)
        .args(["query", "run", "--entity", "task", "--file", filter_file.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Filter parse error"));
}

#[test]
fn query_missing_file() {
    let (_dir, db_url) = common::setup();

    common::nyd(&db_url)
        .args(["query", "run", "--entity", "task", "--file", "/nonexistent/filter.yaml"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error reading"));
}
