//! Reproduction probe for the "cross-process CLI-created task not visible until
//! restart" bug.
//!
//! The TUI holds a long-lived `sea-orm` connection opened at startup. A Jira
//! script creates a task in a *separate* process (`nyd-t task add`) and then
//! asks the TUI to jump to it. The task only shows up after an app restart.
//!
//! This test isolates the single untested data-layer scenario: does a
//! long-lived `sea-orm`/`sqlx` SQLite connection see a row committed by another
//! OS process, when it re-queries *without* reconnecting?
//!
//! Setup: open a domain module over a temp DB (connection A, held open), record
//! the baseline task count, spawn the real `nyd-t` binary to add a task (a true
//! second process, connection B), then re-query connection A. If A does *not*
//! see the new row, the long-lived-connection-staleness hypothesis is confirmed.

use std::process::Command;

mod common;

#[test]
fn long_lived_connection_sees_cross_process_insert() {
    let (dir, db_url) = common::setup();
    // Keep the TempDir alive for the whole test.
    let _dir = dir;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    // Connection A: opened once, held open across the external write — mirrors
    // the TUI's startup connection.
    let domain = rt
        .block_on(not_yet_done_task_core::bootstrap::open(&db_url))
        .expect("open domain over temp db");

    let baseline = rt
        .block_on(domain.task_service.list_tasks_including_deleted(None))
        .expect("baseline list")
        .len();

    // Connection B: a genuinely separate OS process writes to the same file.
    let status = Command::new(env!("CARGO_BIN_EXE_nyd-t"))
        .env("NYD_TASKS_DB", &db_url)
        .args(["task", "add", "cross-process-probe"])
        .status()
        .expect("run nyd-t task add");
    assert!(status.success(), "nyd-t task add failed");

    // Re-query connection A *without* reconnecting.
    let after = rt
        .block_on(domain.task_service.list_tasks_including_deleted(None))
        .expect("post-insert list")
        .len();

    assert_eq!(
        after,
        baseline + 1,
        "long-lived connection A did not see the row inserted by external \
         process B (baseline={baseline}, after={after}) — cross-process \
         staleness confirmed"
    );
}
