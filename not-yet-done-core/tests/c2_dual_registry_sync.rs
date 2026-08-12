//! C2 guard: db::connect with sync_schema must create BOTH the task-domain
//! tables (registered under not_yet_done_task_core::entity::*) and the
//! app-shell tables (not_yet_done_core::entity::*) into the one database.
//!
//! `not-yet-done-core` does NOT depend on `not-yet-done-task-core` (C3 of
//! the DB-split): `db::connect` syncs the task registry purely by the
//! module-path glob string, relying on the *calling binary* to have linked
//! task-core so its entities self-register. The `extern crate` below makes
//! this test binary link task-core exactly as the real hosts (TUI/CLI/
//! Waybar) do — without it the linker drops the unreferenced crate and the
//! task entities never register, so the glob would find nothing.
extern crate not_yet_done_task_core;

use sea_orm::{ConnectionTrait, DbBackend, Statement};

#[tokio::test]
async fn dual_registry_creates_task_and_shell_tables() {
    let dir = std::env::temp_dir().join(format!("nyd_c2_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("c2.db");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());

    let db = not_yet_done_core::db::connect(&url, true)
        .await
        .expect("connect+sync");

    for table in [
        "task",
        "tracking",
        "global_tag",
        "query_shortcut",
        "settings",
        "link",
    ] {
        let row = db
            .query_one_raw(Statement::from_string(
                DbBackend::Sqlite,
                format!("SELECT name FROM sqlite_master WHERE type='table' AND name='{table}'"),
            ))
            .await
            .unwrap();
        assert!(
            row.is_some(),
            "table '{table}' was not created by schema sync"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
