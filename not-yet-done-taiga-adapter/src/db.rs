//! DB connection setup for the Taiga adapter.
//!
//! Each adapter instance opens its own connection (cached per-URL inside the
//! factory). Default backing store: a private SQLite file under
//! `~/.local/share/not_yet_done/taiga-cache.sqlite`. Override via `db.url`
//! in the view's adapter YAML.

use std::path::PathBuf;

use sea_orm::{Database, DatabaseConnection, DbErr};

pub fn default_sqlite_url() -> Result<String, String> {
    let dir: PathBuf = dirs::data_local_dir()
        .ok_or_else(|| "cannot resolve XDG data-local dir".to_string())?
        .join("not_yet_done");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = dir.join("taiga-cache.sqlite");
    Ok(format!("sqlite://{}?mode=rwc", path.display()))
}

/// Open a sea-orm connection and sync this crate's entity schema.
pub async fn connect(db_url: &str) -> Result<DatabaseConnection, DbErr> {
    let db = Database::connect(db_url).await?;
    db.get_schema_registry("not_yet_done_taiga_adapter::entity::*")
        .sync(&db)
        .await?;
    Ok(db)
}
