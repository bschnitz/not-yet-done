mod common;

use predicates::prelude::*;

// ── track summary with no trackings today ────────────────────────────────────

#[test]
fn summary_with_no_trackings_prints_empty_message() {
    let (_dir, db_url) = common::setup();
    // No tasks, no trackings — should print the "nothing found" message
    common::nyd(&db_url)
        .args(["track", "summary"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No tracked time found"));
}

// ── track summary after start + stop ─────────────────────────────────────────

#[test]
fn summary_shows_completed_tracking() {
    let (_dir, db_url) = common::setup();
    let task_id = common::create_task(&db_url, "Summary task");
    common::start_tracking(&db_url, &task_id);

    common::nyd(&db_url)
        .args(["track", "stop"])
        .assert()
        .success();

    common::nyd(&db_url)
        .args(["track", "summary"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Summary task"))
        .stdout(predicate::str::contains("No tracked time found").not());
}

#[test]
fn summary_output_contains_total_line() {
    let (_dir, db_url) = common::setup();
    let task_id = common::create_task(&db_url, "Total task");
    common::start_tracking(&db_url, &task_id);
    common::nyd(&db_url).args(["track", "stop"]).assert().success();

    common::nyd(&db_url)
        .args(["track", "summary"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Total"));
}

#[test]
fn summary_includes_active_tracking_in_duration() {
    let (_dir, db_url) = common::setup();
    let task_id = common::create_task(&db_url, "Active summary task");
    common::start_tracking(&db_url, &task_id);

    // Active tracking (no ended_at) is included, clamped to now
    common::nyd(&db_url)
        .args(["track", "summary"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Active summary task"));
}

// ── --from after --to ────────────────────────────────────────────────────────

#[test]
fn summary_from_after_to_fails() {
    let (_dir, db_url) = common::setup();

    common::nyd(&db_url)
        .args(["track", "summary", "--from", "2026-03-22", "--to", "2026-03-01"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--from must not be after --to"));
}

// ── --from and --to range filter ─────────────────────────────────────────────

#[test]
fn summary_with_explicit_range_containing_no_trackings_prints_empty_message() {
    let (_dir, db_url) = common::setup();
    let task_id = common::create_task(&db_url, "Out of range task");
    common::start_tracking(&db_url, &task_id);
    common::nyd(&db_url).args(["track", "stop"]).assert().success();

    // Query a range far in the past
    common::nyd(&db_url)
        .args(["track", "summary", "--from", "2000-01-01", "--to", "2000-01-02"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No tracked time found"));
}

// ── --task-id filter ─────────────────────────────────────────────────────────

#[test]
fn summary_task_id_filter_shows_only_matching_task() {
    let (_dir, db_url) = common::setup();
    let task_a = common::create_task(&db_url, "Filter A");
    let task_b = common::create_task(&db_url, "Filter B");

    common::nyd(&db_url)
        .args(["track", "start", "--parallel", &task_a])
        .assert()
        .success();
    common::nyd(&db_url)
        .args(["track", "start", "--parallel", &task_b])
        .assert()
        .success();
    common::nyd(&db_url).args(["track", "stop"]).assert().success();

    common::nyd(&db_url)
        .args(["track", "summary", "--task-id", &task_a])
        .assert()
        .success()
        .stdout(predicate::str::contains("Filter A"))
        .stdout(predicate::str::contains("Filter B").not());
}

#[test]
fn summary_task_id_filter_with_invalid_uuid_fails() {
    let (_dir, db_url) = common::setup();

    common::nyd(&db_url)
        .args(["track", "summary", "--task-id", "not-a-uuid"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid ID"));
}

// ── multiple tasks appear in summary ─────────────────────────────────────────

#[test]
fn summary_shows_multiple_tasks() {
    let (_dir, db_url) = common::setup();
    let task_a = common::create_task(&db_url, "Alpha task");
    let task_b = common::create_task(&db_url, "Beta task");

    common::nyd(&db_url)
        .args(["track", "start", "--parallel", &task_a])
        .assert()
        .success();
    common::nyd(&db_url)
        .args(["track", "start", "--parallel", &task_b])
        .assert()
        .success();
    common::nyd(&db_url).args(["track", "stop"]).assert().success();

    common::nyd(&db_url)
        .args(["track", "summary"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Alpha task"))
        .stdout(predicate::str::contains("Beta task"));
}
