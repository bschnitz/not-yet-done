use sea_orm::{ConnectionTrait, Database, DbBackend, DbErr, Statement};

pub use sea_orm::DatabaseConnection;

pub async fn connect(db_url: &str, sync_schema: bool) -> Result<DatabaseConnection, DbErr> {
    let db = Database::connect(db_url).await?;

    // Auto-migration: record which store owns a shortcut's query body. Runs
    // *before* the schema sync — the sync would otherwise see the entity's
    // new non-null `kind` and emit a plain `ADD COLUMN kind text NOT NULL`,
    // which SQLite rejects on a table that already has rows ("Cannot add a
    // NOT NULL column with default value NULL"). Adding it with its default
    // first makes the sync a no-op.
    add_query_shortcut_kind(&db).await?;

    if sync_schema {
        // App-shell entities (link / settings / query_shortcut)
        // still live in not-yet-done-core; the task domain moved to
        // not-yet-done-task-core (C2). Both currently share one database, so
        // sync both registries into this connection. C6 splits the storage.
        db.get_schema_registry("not_yet_done_core::entity::*")
            .sync(&db)
            .await?;
        db.get_schema_registry("not_yet_done_task_core::entity::*")
            .sync(&db)
            .await?;
    }

    // Auto-migration: add `path` column if missing.
    migrate_path_column(&db).await?;

    // Auto-migration: drop the retired `saved_query` table.
    drop_legacy_saved_query(&db).await?;

    // Auto-migration: tag `color` → `fg_color` + `bg_color` + `symbol`.
    migrate_tag_color_split(&db).await?;

    // Auto-migration: convert any text-encoded `query_shortcut.id` (left by
    // an out-of-band insert or older writer) to the blob format SeaORM
    // decodes. A single text-encoded id makes the whole scope's
    // `list_by_scope` query fail to decode, silently dropping every
    // shortcut in that scope.
    fix_text_uuids_in_query_shortcut(&db).await?;

    Ok(db)
}

/// SeaORM binds `Uuid` primary keys as 16-byte blobs, but the
/// `query_shortcut.id` column has TEXT affinity, so a value written as a
/// 36-char string literal (e.g. by an out-of-band `sqlite3` insert or an
/// older code path) sticks around as text. `Entity::find()` then fails to
/// decode `id` (`expected 16 bytes, found 36`) for the *entire* result
/// set of any scope containing such a row — so every shortcut in that
/// scope silently vanishes. Rewrite those rows with proper blob ids.
/// `id` is a pure surrogate key (no foreign keys reference it), so a fresh
/// `randomblob(16)` is fine — only `(scope, name) → shortcut` matters.
/// (The retired `saved_query` table once needed the same treatment.)
async fn fix_text_uuids_in_query_shortcut(db: &DatabaseConnection) -> Result<(), DbErr> {
    // Carry `kind` along when the column is there. It is added by
    // `add_query_shortcut_kind` just before this runs, so in practice it
    // always is — but the copy is a delete-and-reinsert, and a hard-coded
    // column list would quietly reset every rewritten row to the default.
    let payload = if column_names(db, "query_shortcut")
        .await?
        .iter()
        .any(|n| n == "kind")
    {
        "scope, name, shortcut, kind"
    } else {
        "scope, name, shortcut"
    };

    let text_count = db
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COUNT(*) as c FROM query_shortcut WHERE typeof(id) = 'text'",
        ))
        .await?
        .map(|r| r.try_get::<i32>("", "c").unwrap_or(0))
        .unwrap_or(0);

    if text_count > 0 {
        db.execute_unprepared(&format!(
            "CREATE TEMP TABLE _qs_fix AS \
             SELECT {payload} FROM query_shortcut WHERE typeof(id) = 'text'"
        ))
        .await?;
        db.execute_unprepared("DELETE FROM query_shortcut WHERE typeof(id) = 'text'")
            .await?;
        db.execute_unprepared(&format!(
            "INSERT OR IGNORE INTO query_shortcut (id, {payload}) \
             SELECT randomblob(16), {payload} FROM _qs_fix"
        ))
        .await?;
        db.execute_unprepared("DROP TABLE _qs_fix").await?;
        eprintln!("nyd: converted {text_count} text UUIDs in query_shortcut to blob format");
    }

    Ok(())
}

/// Column names of `table`, or an empty list when the table does not exist
/// (`PRAGMA table_info` on a missing table returns no rows rather than
/// failing, which is exactly the "nothing to migrate" answer callers want).
async fn column_names(db: &DatabaseConnection, table: &str) -> Result<Vec<String>, DbErr> {
    let cols = db
        .query_all_raw(Statement::from_string(
            DbBackend::Sqlite,
            format!("PRAGMA table_info({table})"),
        ))
        .await?;
    Ok(cols
        .iter()
        .filter_map(|r| r.try_get::<String>("", "name").ok())
        .collect())
}

/// Add `query_shortcut.kind`: which store owns the body of the query this
/// shortcut points at (`saved` | `extended`).
///
/// Every pre-existing row is a saved query — extended queries did not exist
/// when it was written — so the column default does the whole migration.
/// Without it a shortcut would have to be resolved by probing both stores,
/// and a name that exists in neither (the body was deleted behind the app's
/// back) would be indistinguishable from one whose store simply failed to
/// list. Idempotent: skips when the column is already there, and when the
/// table does not exist yet — the schema sync creates it with the column.
async fn add_query_shortcut_kind(db: &DatabaseConnection) -> Result<(), DbErr> {
    let names = column_names(db, "query_shortcut").await?;
    if names.is_empty() || names.iter().any(|n| n == "kind") {
        return Ok(());
    }
    db.execute_unprepared(
        "ALTER TABLE query_shortcut ADD COLUMN kind TEXT NOT NULL DEFAULT 'saved'",
    )
    .await?;
    eprintln!("nyd: added query_shortcut.kind (existing shortcuts marked as saved queries)");
    Ok(())
}

/// Replace the single `color` column on `global_tag` / `project_tag`
/// with `fg_color`, `bg_color`, `symbol`. The legacy value is copied
/// into `bg_color` (tag chips are typically identified by their
/// background). Idempotent: skips per-table if `color` is gone.
async fn migrate_tag_color_split(db: &DatabaseConnection) -> Result<(), DbErr> {
    for table in ["global_tag", "project_tag"] {
        let cols = db
            .query_all_raw(Statement::from_string(
                DbBackend::Sqlite,
                format!("PRAGMA table_info({table})"),
            ))
            .await?;
        let names: Vec<String> = cols
            .iter()
            .filter_map(|r| r.try_get::<String>("", "name").ok())
            .collect();
        if !names.iter().any(|n| n == "color") {
            continue;
        }
        if !names.iter().any(|n| n == "fg_color") {
            db.execute_unprepared(&format!("ALTER TABLE {table} ADD COLUMN fg_color TEXT"))
                .await?;
        }
        if !names.iter().any(|n| n == "bg_color") {
            db.execute_unprepared(&format!("ALTER TABLE {table} ADD COLUMN bg_color TEXT"))
                .await?;
        }
        if !names.iter().any(|n| n == "symbol") {
            db.execute_unprepared(&format!("ALTER TABLE {table} ADD COLUMN symbol TEXT"))
                .await?;
        }
        db.execute_unprepared(&format!(
            "UPDATE {table} SET bg_color = color WHERE bg_color IS NULL"
        ))
        .await?;
        db.execute_unprepared(&format!("ALTER TABLE {table} DROP COLUMN color"))
            .await?;
        eprintln!("nyd: split {table}.color into fg_color/bg_color/symbol");
    }
    Ok(())
}

/// Drop the retired `saved_query` table.
///
/// Saved-query *bodies* have long lived in adapter-managed storage
/// (`SavedQueryStore`, i.e. `<instance_data_dir>/queries/<name>.<suffix>`)
/// and the keyboard bindings in `query_shortcut`; this table was the last
/// remnant of the pre-adapter era and no code read it any more. Dropping it
/// here — rather than only in the user's database — makes the removal stick:
/// the entity is gone, so schema-sync can never recreate the table, and an
/// older binary run once against the same file leaves nothing behind.
/// Idempotent: `IF EXISTS`, so it is a no-op from the second start on.
async fn drop_legacy_saved_query(db: &DatabaseConnection) -> Result<(), DbErr> {
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA table_info(saved_query)",
        ))
        .await?;
    if rows.is_empty() {
        return Ok(());
    }

    db.execute_unprepared("DROP TABLE IF EXISTS saved_query")
        .await?;
    eprintln!("nyd: dropped the retired saved_query table");
    Ok(())
}

async fn migrate_path_column(db: &DatabaseConnection) -> Result<(), DbErr> {
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA table_info(task)",
        ))
        .await?;

    let has_path = rows.iter().any(|row| {
        row.try_get::<String>("", "name")
            .map(|n| n == "path")
            .unwrap_or(false)
    });

    if !has_path {
        db.execute_unprepared("ALTER TABLE task ADD COLUMN path TEXT")
            .await?;
        eprintln!("nyd: added 'path' column to task table");
        // Initial population will happen via rebuild_all_paths on first use.
    }

    // Check if any tasks have NULL path and rebuild if needed.
    let null_count = db
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COUNT(*) as c FROM task WHERE path IS NULL",
        ))
        .await?
        .map(|r| r.try_get::<i32>("", "c").unwrap_or(0))
        .unwrap_or(0);

    if null_count > 0 {
        eprintln!("nyd: rebuilding paths for {null_count} tasks...");
        rebuild_paths_raw(db).await?;
    }

    Ok(())
}

/// Rebuild all materialized paths using raw SQL.
/// Used during migration before the repository is available.
async fn rebuild_paths_raw(db: &DatabaseConnection) -> Result<(), DbErr> {
    use std::collections::HashMap;

    // Load all id + parent_id pairs.
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT hex(id) as id_hex, hex(parent_id) as parent_hex FROM task",
        ))
        .await?;

    let mut parents: HashMap<String, Option<String>> = HashMap::new();
    for row in &rows {
        let id_hex: String = row.try_get("", "id_hex").unwrap_or_default();
        let parent_hex: Option<String> = row.try_get("", "parent_hex").ok();
        // Filter empty strings from hex(NULL).
        let parent = parent_hex.filter(|s| !s.is_empty());
        parents.insert(id_hex, parent);
    }

    // Compute path for each task.
    fn compute(id: &str, parents: &HashMap<String, Option<String>>) -> String {
        let mut chain = vec![id.to_string()];
        let mut current = parents.get(id).cloned().flatten();
        let mut seen = std::collections::HashSet::new();
        seen.insert(id.to_string());
        while let Some(pid) = current {
            if !seen.insert(pid.clone()) {
                break;
            }
            chain.push(pid.clone());
            current = parents.get(&pid).cloned().flatten();
        }
        chain.reverse();
        let mut path = String::from("/");
        for nid in chain {
            // Use first 8 chars of hex ID (matching short_id).
            path.push_str(&nid[..8.min(nid.len())]);
            path.push('/');
        }
        path
    }

    for row in &rows {
        let id_hex: String = row.try_get("", "id_hex").unwrap_or_default();
        let path = compute(&id_hex, &parents);
        let sql = format!(
            "UPDATE task SET path = '{}' WHERE hex(id) = '{}'",
            path, id_hex
        );
        db.execute_unprepared(&sql).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A single text-encoded `id` (as left by an out-of-band insert) must
    /// be rewritten to a blob so SeaORM can decode the whole scope, while
    /// well-formed blob rows and the `(scope, name, shortcut)` payload are
    /// left untouched. This is the exact shape of the Tasks-tab bug where
    /// two text rows made every shortcut in `tasks/tasks/tasks` vanish.
    #[tokio::test]
    async fn fix_text_uuids_in_query_shortcut_rewrites_only_text_rows() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        db.execute_unprepared(
            "CREATE TABLE query_shortcut ( \
               id uuid_text NOT NULL PRIMARY KEY, \
               scope varchar NOT NULL, \
               name varchar NOT NULL, \
               shortcut varchar NOT NULL )",
        )
        .await
        .unwrap();
        // One correct blob row, two malformed text rows (36-char uuid).
        db.execute_unprepared(
            "INSERT INTO query_shortcut (id, scope, name, shortcut) VALUES \
               (randomblob(16), 'jira/jira/tickets', 'My Tickets', 'ctrl+i'), \
               ('bb516688-6983-48b3-bb74-45c6f825a5cb', 'tasks/tasks/tasks', 'Recent', 'ctrl+m'), \
               ('cd9665d8-290c-4859-b0c0-4fdce2c716e1', 'tasks/tasks/tasks', 'All', 'ü')",
        )
        .await
        .unwrap();

        fix_text_uuids_in_query_shortcut(&db).await.unwrap();

        // No text-encoded ids remain.
        let text_left = db
            .query_one_raw(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT COUNT(*) AS c FROM query_shortcut WHERE typeof(id) = 'text'",
            ))
            .await
            .unwrap()
            .map(|r| r.try_get::<i32>("", "c").unwrap_or(-1))
            .unwrap();
        assert_eq!(text_left, 0, "text-encoded ids must be gone");

        // All three rows survive with their payload intact.
        let rows = db
            .query_all_raw(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT scope, name, shortcut, length(id) AS len FROM query_shortcut \
                 ORDER BY scope, name",
            ))
            .await
            .unwrap();
        let got: Vec<(String, String, String, i32)> = rows
            .iter()
            .map(|r| {
                (
                    r.try_get::<String>("", "scope").unwrap(),
                    r.try_get::<String>("", "name").unwrap(),
                    r.try_get::<String>("", "shortcut").unwrap(),
                    r.try_get::<i32>("", "len").unwrap(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "jira/jira/tickets".into(),
                    "My Tickets".into(),
                    "ctrl+i".into(),
                    16
                ),
                ("tasks/tasks/tasks".into(), "All".into(), "ü".into(), 16),
                (
                    "tasks/tasks/tasks".into(),
                    "Recent".into(),
                    "ctrl+m".into(),
                    16
                ),
            ]
        );
    }

    /// Every shortcut written before extended queries existed points at a
    /// saved query, so the column default is the whole migration — and
    /// running it twice must not fail (it runs on every startup).
    #[tokio::test]
    async fn add_query_shortcut_kind_defaults_existing_rows_to_saved() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        db.execute_unprepared(
            "CREATE TABLE query_shortcut ( \
               id uuid_text NOT NULL PRIMARY KEY, \
               scope varchar NOT NULL, \
               name varchar NOT NULL, \
               shortcut varchar NOT NULL )",
        )
        .await
        .unwrap();
        db.execute_unprepared(
            "INSERT INTO query_shortcut (id, scope, name, shortcut) VALUES \
               (randomblob(16), 'jira/jira/tickets', 'Mine', 'ctrl+i')",
        )
        .await
        .unwrap();

        add_query_shortcut_kind(&db).await.unwrap();
        add_query_shortcut_kind(&db).await.unwrap();

        let kind = db
            .query_one_raw(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT kind FROM query_shortcut",
            ))
            .await
            .unwrap()
            .map(|r| r.try_get::<String>("", "kind").unwrap())
            .unwrap();
        assert_eq!(kind, "saved");
    }

    /// No table yet (fresh database, schema sync creates it with the column
    /// already present): nothing to alter, and no error either.
    #[tokio::test]
    async fn add_query_shortcut_kind_skips_a_missing_table() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        add_query_shortcut_kind(&db).await.unwrap();
    }

    /// The uuid fix is a delete-and-reinsert. Once `kind` exists it has to
    /// travel with the row, or a rewritten shortcut would silently fall back
    /// to the saved store and stop finding its body.
    #[tokio::test]
    async fn fix_text_uuids_preserves_the_kind() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        db.execute_unprepared(
            "CREATE TABLE query_shortcut ( \
               id uuid_text NOT NULL PRIMARY KEY, \
               scope varchar NOT NULL, \
               name varchar NOT NULL, \
               shortcut varchar NOT NULL, \
               kind TEXT NOT NULL DEFAULT 'saved' )",
        )
        .await
        .unwrap();
        db.execute_unprepared(
            "INSERT INTO query_shortcut (id, scope, name, shortcut, kind) VALUES \
               ('bb516688-6983-48b3-bb74-45c6f825a5cb', 'jira/jira/tickets', 'Mine', 'ctrl+i', 'extended')",
        )
        .await
        .unwrap();

        fix_text_uuids_in_query_shortcut(&db).await.unwrap();

        let row = db
            .query_one_raw(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT kind, length(id) AS len FROM query_shortcut",
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.try_get::<String>("", "kind").unwrap(), "extended");
        assert_eq!(row.try_get::<i32>("", "len").unwrap(), 16);
    }

    /// Idempotent: a table that is already all-blob is left unchanged and
    /// the migration is a no-op (safe to run on every startup).
    #[tokio::test]
    async fn fix_text_uuids_in_query_shortcut_is_noop_when_clean() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        db.execute_unprepared(
            "CREATE TABLE query_shortcut ( \
               id uuid_text NOT NULL PRIMARY KEY, \
               scope varchar NOT NULL, \
               name varchar NOT NULL, \
               shortcut varchar NOT NULL )",
        )
        .await
        .unwrap();
        db.execute_unprepared(
            "INSERT INTO query_shortcut (id, scope, name, shortcut) VALUES \
               (randomblob(16), 'taiga/taiga/items', 'Open items', 'ctrl+i')",
        )
        .await
        .unwrap();

        fix_text_uuids_in_query_shortcut(&db).await.unwrap();

        let count = db
            .query_one_raw(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT COUNT(*) AS c FROM query_shortcut",
            ))
            .await
            .unwrap()
            .map(|r| r.try_get::<i32>("", "c").unwrap_or(-1))
            .unwrap();
        assert_eq!(count, 1);
    }
}
