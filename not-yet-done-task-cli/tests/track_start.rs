mod common;

use predicates::prelude::*;

// ── track start <valid-task-uuid> ────────────────────────────────────────────

#[test]
fn start_tracking_for_existing_task_succeeds() {
    let (_dir, db_url) = common::setup();
    let task_id = common::create_task(&db_url, "My test task");

    common::nyd(&db_url)
        .args(["track", "start", &task_id])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ Tracking started:"))
        .stdout(predicate::str::contains(&task_id).not()); // output shows tracking id, not task id
}

#[test]
fn start_tracking_output_contains_tracking_id() {
    let (_dir, db_url) = common::setup();
    let task_id = common::create_task(&db_url, "Task with tracking");

    let output = common::nyd(&db_url)
        .args(["track", "start", &task_id])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).unwrap();
    // The output must contain a bracketed UUID for the new tracking entry
    assert!(
        common::parse_bracketed_uuid(&stdout).is_some(),
        "Expected a bracketed UUID in output, got: {stdout}"
    );
}

// ── track start <non-existent-uuid> ─────────────────────────────────────────

#[test]
fn start_tracking_for_nonexistent_task_fails() {
    let (_dir, db_url) = common::setup();
    let nonexistent = "00000000-0000-0000-0000-000000000000";

    common::nyd(&db_url)
        .args(["track", "start", nonexistent])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error:"))
        .stderr(predicate::str::contains("Task not found").or(predicate::str::contains("not found")));
}

// ── track start <invalid uuid> ───────────────────────────────────────────────

#[test]
fn start_tracking_with_invalid_uuid_fails() {
    let (_dir, db_url) = common::setup();

    common::nyd(&db_url)
        .args(["track", "start", "not-a-uuid"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error:"))
        .stderr(predicate::str::contains("Invalid ID"));
}

// ── double start on same task ────────────────────────────────────────────────

#[test]
fn start_tracking_twice_on_same_task_fails() {
    let (_dir, db_url) = common::setup();
    let task_id = common::create_task(&db_url, "Double-start task");

    common::nyd(&db_url)
        .args(["track", "start", &task_id])
        .assert()
        .success();

    common::nyd(&db_url)
        .args(["track", "start", &task_id])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already has an active tracking"));
}

// ── sequential start without --parallel stops the first ─────────────────────

#[test]
fn start_second_task_without_parallel_stops_first() {
    let (_dir, db_url) = common::setup();
    let task_a = common::create_task(&db_url, "Task A");
    let task_b = common::create_task(&db_url, "Task B");

    common::nyd(&db_url)
        .args(["track", "start", &task_a])
        .assert()
        .success();

    // Starting task B without --parallel should succeed (stops A first)
    common::nyd(&db_url)
        .args(["track", "start", &task_b])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ Tracking started:"));

    // Task A should no longer be active: starting it again must succeed
    // (if it were still active, this would fail with "already has an active tracking")
    common::nyd(&db_url)
        .args(["track", "start", &task_a])
        .assert()
        .success();
}

// ── sequential start with --parallel keeps both active ───────────────────────

#[test]
fn start_second_task_with_parallel_keeps_first_active() {
    let (_dir, db_url) = common::setup();
    let task_a = common::create_task(&db_url, "Parallel A");
    let task_b = common::create_task(&db_url, "Parallel B");

    common::nyd(&db_url)
        .args(["track", "start", &task_a])
        .assert()
        .success();

    common::nyd(&db_url)
        .args(["track", "start", "--parallel", &task_b])
        .assert()
        .success();

    // Task A is still active: starting it again must fail
    common::nyd(&db_url)
        .args(["track", "start", &task_a])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already has an active tracking"));
}
