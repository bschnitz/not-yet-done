use sea_orm::{ConnectionTrait, Database, DbBackend, DbErr, Statement};

pub use sea_orm::DatabaseConnection;

pub async fn connect(db_url: &str, sync_schema: bool) -> Result<DatabaseConnection, DbErr> {
    let db = Database::connect(db_url).await?;

    if sync_schema {
        db.get_schema_registry("not_yet_done_core::entity::*")
            .sync(&db)
            .await?;
    }

    // Auto-migration: add `path` column if missing.
    migrate_path_column(&db).await?;

    // Auto-migration: migrate saved_filter → saved_query.
    migrate_saved_filter_to_saved_query(&db).await?;

    // Auto-migration: 2-component scopes ("jira:tickets") → 3-component
    // ("jira:jira:tickets") to make room for the per-instance id segment.
    migrate_saved_query_scope_with_instance_id(&db).await?;

    // Auto-migration: tag `color` → `fg_color` + `bg_color` + `symbol`.
    migrate_tag_color_split(&db).await?;

    Ok(db)
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
            db.execute_unprepared(&format!(
                "ALTER TABLE {table} ADD COLUMN fg_color TEXT"
            ))
            .await?;
        }
        if !names.iter().any(|n| n == "bg_color") {
            db.execute_unprepared(&format!(
                "ALTER TABLE {table} ADD COLUMN bg_color TEXT"
            ))
            .await?;
        }
        if !names.iter().any(|n| n == "symbol") {
            db.execute_unprepared(&format!(
                "ALTER TABLE {table} ADD COLUMN symbol TEXT"
            ))
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

/// Upgrade old `<adapter_type>:<view_name>` saved-query scopes to the
/// new `<adapter_type>:<instance_id>:<view_name>` shape. The default
/// instance id equals the adapter type, so the rewrite duplicates the
/// first segment. Idempotent: scopes with two or more `:`s, or with
/// none at all (e.g. `task`, `tracking` favorites), are left alone.
async fn migrate_saved_query_scope_with_instance_id(
    db: &DatabaseConnection,
) -> Result<(), DbErr> {
    // Count rows that need rewriting (exactly one ':' in scope).
    let to_migrate = db
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COUNT(*) AS c FROM saved_query \
             WHERE scope LIKE '%:%' \
             AND length(scope) - length(replace(scope, ':', '')) = 1",
        ))
        .await?
        .map(|r| r.try_get::<i32>("", "c").unwrap_or(0))
        .unwrap_or(0);

    if to_migrate == 0 {
        return Ok(());
    }

    db.execute_unprepared(
        "UPDATE saved_query \
         SET scope = substr(scope, 1, instr(scope, ':') - 1) || ':' || scope \
         WHERE scope LIKE '%:%' \
         AND length(scope) - length(replace(scope, ':', '')) = 1",
    )
    .await?;
    eprintln!("nyd: upgraded {to_migrate} saved_query scopes to <type>:<instance_id>:<view> format");
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

/// Migrate saved_filter rows and favorites (from settings) into saved_query.
/// Runs once: skips if saved_filter table is already gone or empty and
/// favorites have been migrated.
/// Also fixes blob UUIDs left over from the initial migration.
async fn migrate_saved_filter_to_saved_query(db: &DatabaseConnection) -> Result<(), DbErr> {
    // Check if saved_filter table exists.
    let rows = db
        .query_all_raw(Statement::from_string(
            DbBackend::Sqlite,
            "PRAGMA table_info(saved_filter)",
        ))
        .await?;

    if !rows.is_empty() {
        // Copy saved_filter rows, converting blob UUIDs to text format.
        let count = db
            .query_one_raw(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT COUNT(*) as c FROM saved_filter",
            ))
            .await?
            .map(|r| r.try_get::<i32>("", "c").unwrap_or(0))
            .unwrap_or(0);

        if count > 0 {
            // Insert with fresh blob UUIDs (randomblob(16) = SeaORM Uuid format).
            db.execute_unprepared(
                "INSERT INTO saved_query (id, scope, name, query, shortcut) \
                 SELECT randomblob(16), entity, name, filter_json, NULL \
                 FROM saved_filter",
            )
            .await?;
            eprintln!("nyd: migrated {count} saved filters → saved_query");
        }

        // Drop old table.
        db.execute_unprepared("DROP TABLE saved_filter").await?;
        eprintln!("nyd: dropped saved_filter table");
    }

    // Fix any text UUIDs left over from a previous buggy migration.
    fix_text_uuids_in_saved_query(db).await?;

    // Migrate favorites from settings table into saved_query.
    for (settings_key, scope) in [
        ("favorite_filters_task", "task"),
        ("favorite_filters_tracking", "tracking"),
    ] {
        let row = db
            .query_one_raw(Statement::from_string(
                DbBackend::Sqlite,
                format!("SELECT value FROM settings WHERE key = '{settings_key}'"),
            ))
            .await?;

        if let Some(row) = row {
            let json_str: String = row.try_get("", "value").unwrap_or_default();
            if !json_str.is_empty() {
                // Parse [{name, shortcut, filter_json}, ...]
                if let Ok(favs) = serde_json::from_str::<Vec<FavoriteMigration>>(&json_str) {
                    for fav in &favs {
                        let escaped_name = fav.name.replace('\'', "''");
                        let escaped_query = fav.filter_json.replace('\'', "''");
                        let escaped_shortcut = fav.shortcut.replace('\'', "''");
                        // Try to update shortcut on existing entry (from saved_filter migration).
                        let updated = db.execute_unprepared(&format!(
                            "UPDATE saved_query SET shortcut = '{escaped_shortcut}' \
                             WHERE scope = '{scope}' AND name = '{escaped_name}' AND shortcut IS NULL",
                        )).await?;
                        // If no existing entry, insert new with blob UUID.
                        if updated.rows_affected() == 0 {
                            db.execute_unprepared(&format!(
                                "INSERT OR IGNORE INTO saved_query (id, scope, name, query, shortcut) \
                                 VALUES (randomblob(16), \
                                 '{scope}', '{escaped_name}', '{escaped_query}', '{escaped_shortcut}')",
                            ))
                            .await?;
                        }
                    }
                    if !favs.is_empty() {
                        eprintln!("nyd: migrated {} {scope} favorites → saved_query", favs.len());
                    }
                }
            }
            // Remove migrated settings key.
            db.execute_unprepared(&format!(
                "DELETE FROM settings WHERE key = '{settings_key}'"
            ))
            .await?;
        }
    }

    Ok(())
}

/// Fix text UUIDs in saved_query table (left over from a previous migration
/// that stored UUIDs as text strings instead of 16-byte blobs).
async fn fix_text_uuids_in_saved_query(db: &DatabaseConnection) -> Result<(), DbErr> {
    let text_count = db
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT COUNT(*) as c FROM saved_query WHERE typeof(id) = 'text'",
        ))
        .await?
        .map(|r| r.try_get::<i32>("", "c").unwrap_or(0))
        .unwrap_or(0);

    if text_count > 0 {
        // Save data, delete text rows, re-insert with blob UUIDs.
        db.execute_unprepared(
            "CREATE TEMP TABLE _sq_fix AS \
             SELECT scope, name, query, shortcut FROM saved_query WHERE typeof(id) = 'text'",
        )
        .await?;

        db.execute_unprepared(
            "DELETE FROM saved_query WHERE typeof(id) = 'text'",
        )
        .await?;

        // Re-insert with proper blob UUIDs, skip duplicates by scope+name.
        db.execute_unprepared(
            "INSERT OR IGNORE INTO saved_query (id, scope, name, query, shortcut) \
             SELECT randomblob(16), scope, name, query, shortcut FROM _sq_fix",
        )
        .await?;

        db.execute_unprepared("DROP TABLE _sq_fix").await?;
        eprintln!("nyd: converted {text_count} text UUIDs in saved_query to blob format");
    }

    Ok(())
}

#[derive(serde::Deserialize)]
struct FavoriteMigration {
    name: String,
    shortcut: String,
    filter_json: String,
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
            if !seen.insert(pid.clone()) { break; }
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
