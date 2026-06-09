//! DB connection setup for the Stoat adapter.
//!
//! Chat state (servers / channels / messages) is **not** cached in
//! SQLite — it is highly volatile and lives in [`crate::gateway::StoatState`]
//! in memory (rebuilt from the WS `Ready` event on every connect). The
//! only things we persist are the session token (so a restart doesn't
//! force a fresh login) and per-view sort state. Default backing store:
//! a private SQLite file under `~/.local/share/not_yet_done/stoat.sqlite`.
//! Override via `db.url` in the view's adapter YAML.

use std::path::PathBuf;

use sea_orm::{Database, DatabaseConnection, DbErr};

pub fn default_sqlite_url() -> Result<String, String> {
    let dir: PathBuf = dirs::data_local_dir()
        .ok_or_else(|| "cannot resolve XDG data-local dir".to_string())?
        .join("not_yet_done");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = dir.join("stoat.sqlite");
    Ok(format!("sqlite://{}?mode=rwc", path.display()))
}

/// Open a sea-orm connection and sync this crate's entity schema.
pub async fn connect(db_url: &str) -> Result<DatabaseConnection, DbErr> {
    let db = Database::connect(db_url).await?;
    db.get_schema_registry("not_yet_done_stoat_adapter::entity::*")
        .sync(&db)
        .await?;
    Ok(db)
}

/// Stable per-connection scope id derived from the Stoat base URL via
/// UUID v5 against `NAMESPACE_URL`. Same URL → same id across restarts,
/// so the persisted session token survives a restart.
pub fn scope_id_for_url(url: &str) -> uuid::Uuid {
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, url.as_bytes())
}
