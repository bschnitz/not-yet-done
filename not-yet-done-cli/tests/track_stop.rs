mod common;

use predicates::prelude::*;

// ── track stop with no active tracking ──────────────────────────────────────

#[test]
fn stop_with_no_active_tracking_fails() {
    let (_dir, db_url) = common::setup();

    // The service returns NoActiveTrackingAny when nothing is running
    common::nyd(&db_url)
        .args(["track", "stop"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error:"));
}

// ── track start + track stop ─────────────────────────────────────────────────

#[test]
fn stop_active_tracking_succeeds() {
    let (_dir, db_url) = common::setup();
    let task_id = common::create_task(&db_url, "Stoppable task");
    common::start_tracking(&db_url, &task_id);

    common::nyd(&db_url)
        .args(["track", "stop"])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ Tracking stopped:"))
        .stdout(predicate::str::contains("Stoppable task"));
}

#[test]
fn stop_output_contains_start_and_end_time() {
    let (_dir, db_url) = common::setup();
    let task_id = common::create_task(&db_url, "Timed task");
    common::start_tracking(&db_url, &task_id);

    // Output format: ✓ Tracking stopped: [<uuid>] <description> | <start> → <end>
    common::nyd(&db_url)
        .args(["track", "stop"])
        .assert()
        .success()
        .stdout(predicate::str::contains("→"));
}

// ── track stop --task-id <uuid> ──────────────────────────────────────────────

#[test]
fn stop_specific_task_by_id_succeeds() {
    let (_dir, db_url) = common::setup();
    let task_a = common::create_task(&db_url, "Stop A");
    let task_b = common::create_task(&db_url, "Stop B");

    common::nyd(&db_url)
        .args(["track", "start", "--parallel", &task_a])
        .assert()
        .success();
    common::nyd(&db_url)
        .args(["track", "start", "--parallel", &task_b])
        .assert()
        .success();

    // Stop only task A
    common::nyd(&db_url)
        .args(["track", "stop", "--task-id", &task_a])
        .assert()
        .success()
        .stdout(predicate::str::contains("Stop A"))
        .stdout(predicate::str::contains("Stop B").not());
}

#[test]
fn stop_specific_task_leaves_other_active() {
    let (_dir, db_url) = common::setup();
    let task_a = common::create_task(&db_url, "Stay active");
    let task_b = common::create_task(&db_url, "Get stopped");

    common::nyd(&db_url)
        .args(["track", "start", "--parallel", &task_a])
        .assert()
        .success();
    common::nyd(&db_url)
        .args(["track", "start", "--parallel", &task_b])
        .assert()
        .success();

    common::nyd(&db_url)
        .args(["track", "stop", "--task-id", &task_b])
        .assert()
        .success();

    // Task A is still active: starting it again must fail
    common::nyd(&db_url)
        .args(["track", "start", &task_a])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already has an active tracking"));
}

// ── track stop --task-id on inactive task ────────────────────────────────────

#[test]
fn stop_task_without_active_tracking_fails() {
    let (_dir, db_url) = common::setup();
    let task_id = common::create_task(&db_url, "Never tracked");

    common::nyd(&db_url)
        .args(["track", "stop", "--task-id", &task_id])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error:"))
        .stderr(predicate::str::contains("No active tracking"));
}

#[test]
fn stop_task_with_invalid_uuid_fails() {
    let (_dir, db_url) = common::setup();

    common::nyd(&db_url)
        .args(["track", "stop", "--task-id", "not-a-uuid"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error:"))
        .stderr(predicate::str::contains("Invalid ID"));
}

// ── track stop all: two parallel trackings ───────────────────────────────────

#[test]
fn stop_all_stops_multiple_active_trackings() {
    let (_dir, db_url) = common::setup();
    let task_a = common::create_task(&db_url, "Multi A");
    let task_b = common::create_task(&db_url, "Multi B");

    common::nyd(&db_url)
        .args(["track", "start", "--parallel", &task_a])
        .assert()
        .success();
    common::nyd(&db_url)
        .args(["track", "start", "--parallel", &task_b])
        .assert()
        .success();

    let output = common::nyd(&db_url)
        .args(["track", "stop"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).unwrap();
    let stopped_count = stdout.matches("✓ Tracking stopped:").count();
    assert_eq!(stopped_count, 2, "Expected 2 stopped trackings, got:\n{stdout}");
}
