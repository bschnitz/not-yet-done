mod common;

use predicates::prelude::*;

// ── Helper: create a task, track it, stop it, return (task_id, tracking_id) ──

fn setup_completed_tracking(db_url: &str, description: &str) -> (String, String) {
    let task_id = common::create_task(db_url, description);
    let tracking_id = common::start_tracking(db_url, &task_id);
    common::nyd(db_url).args(["track", "stop"]).assert().success();
    (task_id, tracking_id)
}

// ── invalid / nonexistent tracking ID ────────────────────────────────────────

#[test]
fn move_with_invalid_uuid_fails() {
    let (_dir, db_url) = common::setup();

    common::nyd(&db_url)
        .args(["track", "move", "not-a-uuid", "yesterday 9am"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid tracking ID"));
}

#[test]
fn move_nonexistent_tracking_fails() {
    let (_dir, db_url) = common::setup();
    let nonexistent = "00000000-0000-0000-0000-000000000000";

    common::nyd(&db_url)
        .args(["track", "move", nonexistent, "yesterday 9am"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error:"))
        .stderr(predicate::str::contains("Tracking not found").or(predicate::str::contains("not found")));
}

// ── move an active (non-completed) tracking ───────────────────────────────────

#[test]
fn move_still_active_tracking_fails() {
    let (_dir, db_url) = common::setup();
    let task_id = common::create_task(&db_url, "Active move task");
    let tracking_id = common::start_tracking(&db_url, &task_id);

    // Tracking has no ended_at → TrackingStillActive
    common::nyd(&db_url)
        .args(["track", "move", &tracking_id, "yesterday 9am"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error:"));
}

// ── successful move ───────────────────────────────────────────────────────────

#[test]
fn move_completed_tracking_succeeds() {
    let (_dir, db_url) = common::setup();
    let (_task_id, tracking_id) = setup_completed_tracking(&db_url, "Movable task");

    common::nyd(&db_url)
        .args(["track", "move", &tracking_id, "yesterday 9am"])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ Tracking moved:"))
        .stdout(predicate::str::contains("Movable task"));
}

#[test]
fn move_output_contains_old_and_new_times() {
    let (_dir, db_url) = common::setup();
    let (_task_id, tracking_id) = setup_completed_tracking(&db_url, "Time move task");

    common::nyd(&db_url)
        .args(["track", "move", &tracking_id, "yesterday 9am"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Old:"))
        .stdout(predicate::str::contains("New:"));
}

#[test]
fn move_returns_new_tracking_id_different_from_old() {
    let (_dir, db_url) = common::setup();
    let (_task_id, old_tracking_id) = setup_completed_tracking(&db_url, "ID change task");

    let output = common::nyd(&db_url)
        .args(["track", "move", &old_tracking_id, "yesterday 9am"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).unwrap();

    // The output contains two UUIDs: old and new. The new one must differ.
    let uuids: Vec<&str> = stdout
        .lines()
        .filter_map(|line| {
            let start = line.find('[')?;
            let end = line.find(']')?;
            if end > start + 1 { Some(&line[start + 1..end]) } else { None }
        })
        .collect();

    assert!(uuids.len() >= 2, "Expected at least two UUIDs in output:\n{stdout}");
    assert_ne!(uuids[0], uuids[1], "Old and new tracking IDs should differ");
}

// ── move into the future ──────────────────────────────────────────────────────

#[test]
fn move_into_future_without_flag_fails() {
    let (_dir, db_url) = common::setup();
    let (_task_id, tracking_id) = setup_completed_tracking(&db_url, "Future task");

    common::nyd(&db_url)
        .args(["track", "move", &tracking_id, "tomorrow 9am"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error:"));
}

#[test]
fn move_into_future_with_allow_future_succeeds() {
    let (_dir, db_url) = common::setup();
    let (_task_id, tracking_id) = setup_completed_tracking(&db_url, "Future allowed task");

    common::nyd(&db_url)
        .args(["track", "move", &tracking_id, "tomorrow 9am", "--allow-future"])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ Tracking moved:"));
}

// ── --gravity ─────────────────────────────────────────────────────────────────

#[test]
fn move_with_gravity_start_succeeds() {
    let (_dir, db_url) = common::setup();
    let (_task_id, tracking_id) = setup_completed_tracking(&db_url, "Gravity start task");

    common::nyd(&db_url)
        .args(["track", "move", &tracking_id, "yesterday", "--gravity", "start"])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ Tracking moved:"));
}

#[test]
fn move_with_gravity_end_succeeds() {
    let (_dir, db_url) = common::setup();
    let (_task_id, tracking_id) = setup_completed_tracking(&db_url, "Gravity end task");

    common::nyd(&db_url)
        .args(["track", "move", &tracking_id, "yesterday", "--gravity", "end"])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ Tracking moved:"));
}

// ── --offset ──────────────────────────────────────────────────────────────────

#[test]
fn move_with_offset_succeeds() {
    let (_dir, db_url) = common::setup();
    let (_task_id, tracking_id) = setup_completed_tracking(&db_url, "Offset task");

    common::nyd(&db_url)
        .args(["track", "move", &tracking_id, "yesterday 9am", "--offset", "+1h"])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ Tracking moved:"));
}

#[test]
fn move_with_invalid_offset_fails() {
    let (_dir, db_url) = common::setup();
    let (_task_id, tracking_id) = setup_completed_tracking(&db_url, "Bad offset task");

    common::nyd(&db_url)
        .args(["track", "move", &tracking_id, "yesterday 9am", "--offset", "1hour"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error:").or(predicate::str::contains("invalid")));
}

// ── overlap without --allow-overlap ──────────────────────────────────────────

#[test]
fn move_overlapping_without_flag_fails() {
    let (_dir, db_url) = common::setup();

    // Create two tasks: A tracked yesterday 8-9am, B tracked yesterday 8:30-9:30am
    // Moving B to 8am should overlap with A
    let task_a = common::create_task(&db_url, "Overlap A");
    let task_b = common::create_task(&db_url, "Overlap B");

    // Track A: start it, stop it, move it to a known slot
    let tracking_a = common::start_tracking(&db_url, &task_a);
    common::nyd(&db_url).args(["track", "stop"]).assert().success();
    common::nyd(&db_url)
        .args(["track", "move", &tracking_a, "yesterday 8am", "--allow-future"])
        .assert()
        .success();

    // Track B: start, stop, then try to move to 8am (overlaps with A)
    let tracking_b = common::start_tracking(&db_url, &task_b);
    common::nyd(&db_url).args(["track", "stop"]).assert().success();

    common::nyd(&db_url)
        .args(["track", "move", &tracking_b, "yesterday 8am"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error:"));
}

#[test]
fn move_overlapping_with_allow_overlap_succeeds() {
    let (_dir, db_url) = common::setup();

    let task_a = common::create_task(&db_url, "Allow Overlap A");
    let task_b = common::create_task(&db_url, "Allow Overlap B");

    let tracking_a = common::start_tracking(&db_url, &task_a);
    common::nyd(&db_url).args(["track", "stop"]).assert().success();
    common::nyd(&db_url)
        .args(["track", "move", &tracking_a, "yesterday 8am", "--allow-future"])
        .assert()
        .success();

    let tracking_b = common::start_tracking(&db_url, &task_b);
    common::nyd(&db_url).args(["track", "stop"]).assert().success();

    common::nyd(&db_url)
        .args(["track", "move", &tracking_b, "yesterday 8am", "--allow-overlap"])
        .assert()
        .success()
        .stdout(predicate::str::contains("✓ Tracking moved:"));
}
