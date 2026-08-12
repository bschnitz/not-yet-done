//! DB connection setup for the jira adapter.
//!
//! Each adapter instance opens its own connection (cached per-URL inside the
//! factory). The default backing store is a private SQLite file under
//! `~/.local/share/not_yet_done/jira-cache.sqlite`; users can override this
//! with any sea-orm-compatible URL (`sqlite://...`, `postgres://...`) via
//! the `db.url` field in the view's adapter YAML config.

use std::path::PathBuf;

use sea_orm::{Database, DatabaseConnection, DbErr};

/// Resolve the default SQLite path under the user's local data dir,
/// creating the parent directory if needed. Returns a sea-orm-compatible
/// URL with `mode=rwc` so the file is created on first connect.
pub fn default_sqlite_url() -> Result<String, String> {
    let dir: PathBuf = dirs::data_local_dir()
        .ok_or_else(|| "cannot resolve XDG data-local dir".to_string())?
        .join("not_yet_done");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = dir.join("jira-cache.sqlite");
    Ok(format!("sqlite://{}?mode=rwc", path.display()))
}

/// Open a sea-orm connection and sync this crate's entity schema.
/// Schema sync is scoped to `not_yet_done_jira_adapter::entity::*`, so the
/// host application's own tables (if pointing at the same DB) are untouched.
pub async fn connect(db_url: &str) -> Result<DatabaseConnection, DbErr> {
    let db = Database::connect(db_url).await?;
    db.get_schema_registry("not_yet_done_jira_adapter::entity::*")
        .sync(&db)
        .await?;
    Ok(db)
}
