//! File-level backups of a SQLite task database.
//!
//! *Why this lives in `task-core`:* after the DB-split (Block C) the task
//! domain owns its own `tasks.db`, decoupled from the legacy app-shell crate
//! `not-yet-done-core`. Both consumers of the task domain — the `nyd-t` CLI and
//! the in-process Tasks/Trackings adapters — need to back that file up without
//! re-introducing a dependency on `not-yet-done-core`. The logic is a plain
//! timestamped file copy plus retention pruning, so it belongs next to the
//! domain it serves.
//!
//! Backups are named `<YYYYMMDD-HHMMSS>-<original-filename>` and dropped into a
//! caller-chosen directory. Retention and the daily-once check are
//! **suffix-aware**: they only ever consider files ending in
//! `-<original-filename>`. That keeps several databases backed up into the same
//! directory (e.g. the legacy `nyd.db` and the new `tasks.db`) independent —
//! pruning `tasks.db` backups never evicts a `nyd.db` backup, and a `nyd.db`
//! backup made today does not satisfy the `tasks.db` daily check.

use crate::error::AppError;
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};

/// Extract the on-disk file path from a `sqlite://…` DSN, validating that it
/// names an existing regular file.
fn extract_db_path(db_url: &str) -> Result<PathBuf, AppError> {
    let path_str = db_url
        .strip_prefix("sqlite://")
        .ok_or(AppError::NotFileBasedDatabase)?;
    // Strip any `?mode=rwc`-style query string.
    let path_str = path_str.split('?').next().unwrap_or(path_str);
    let path = Path::new(path_str);

    if !path.exists() {
        return Err(AppError::DatabaseFileNotFound(path.to_path_buf()));
    }
    if !path.is_file() {
        return Err(AppError::NotFileBasedDatabase);
    }
    Ok(path.to_path_buf())
}

fn original_filename(db_path: &Path) -> Result<String, AppError> {
    db_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .ok_or_else(|| AppError::BackupFailed("Invalid database filename".to_string()))
}

/// Create a timestamped copy of the database named by `db_url` into
/// `backup_dir`, then prune that database's own backups down to `max_count`
/// (`0` disables pruning). Returns the full path of the new backup file.
pub fn create_backup_at(
    db_url: &str,
    backup_dir: &Path,
    max_count: usize,
) -> Result<String, AppError> {
    let db_path = extract_db_path(db_url)?;
    let original = original_filename(&db_path)?;

    fs::create_dir_all(backup_dir).map_err(|e| {
        AppError::BackupFailed(format!("Failed to create backup directory: {e}"))
    })?;

    let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
    let backup_filename = format!("{timestamp}-{original}");
    let backup_path = backup_dir.join(&backup_filename);

    fs::copy(&db_path, &backup_path)
        .map_err(|e| AppError::BackupFailed(format!("Failed to copy database file: {e}")))?;

    cleanup_old_backups(backup_dir, &original, max_count)?;

    Ok(backup_path.to_string_lossy().to_string())
}

/// Restore `filename` (relative to `backup_dir`) into the database named by
/// `db_url`, overwriting it. Returns the path of the restored database.
pub fn restore_backup_at(
    db_url: &str,
    backup_dir: &Path,
    filename: &str,
) -> Result<String, AppError> {
    let db_path = extract_db_path(db_url)?;
    let backup_path = backup_dir.join(filename);

    if !backup_path.exists() {
        return Err(AppError::BackupFailed(format!(
            "Backup file not found: {filename}"
        )));
    }
    if !backup_path.is_file() {
        return Err(AppError::BackupFailed(format!(
            "Backup is not a file: {filename}"
        )));
    }

    fs::copy(&backup_path, &db_path)
        .map_err(|e| AppError::BackupFailed(format!("Failed to restore database file: {e}")))?;

    Ok(db_path.to_string_lossy().to_string())
}

/// List the backup filenames in `backup_dir`, sorted ascending (so oldest
/// timestamp first). Returns an empty list if the directory does not exist.
/// Not suffix-filtered — callers that want only one database's backups can
/// filter on the `-<filename>` suffix themselves.
pub fn list_backups(backup_dir: &Path) -> Result<Vec<String>, AppError> {
    if !backup_dir.exists() {
        return Ok(Vec::new());
    }
    let mut backups: Vec<String> = fs::read_dir(backup_dir)
        .map_err(|e| AppError::BackupFailed(format!("Failed to read backup directory: {e}")))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    backups.sort();
    Ok(backups)
}

/// Create a backup of the database named by `db_url` only if none exists for
/// today (matched on both the `YYYYMMDD` prefix **and** the `-<filename>`
/// suffix, so a different database's backup today does not count). Returns the
/// new backup path, or `None` if today's backup already existed.
pub fn ensure_daily_backup_at(
    db_url: &str,
    backup_dir: &Path,
    max_count: usize,
) -> Result<Option<String>, AppError> {
    let db_path = extract_db_path(db_url)?;
    let original = original_filename(&db_path)?;
    let suffix = format!("-{original}");
    let today_prefix = Utc::now().format("%Y%m%d").to_string();

    let already = list_backups(backup_dir)?
        .iter()
        .any(|b| b.starts_with(&today_prefix) && b.ends_with(&suffix));
    if already {
        return Ok(None);
    }
    create_backup_at(db_url, backup_dir, max_count).map(Some)
}

/// Remove the oldest backups of one database (matched on the `-<original>`
/// suffix) until at most `max_count` remain. `max_count == 0` disables pruning.
fn cleanup_old_backups(
    backup_dir: &Path,
    original: &str,
    max_count: usize,
) -> Result<(), AppError> {
    if max_count == 0 {
        return Ok(());
    }
    let suffix = format!("-{original}");

    let mut entries: Vec<_> = fs::read_dir(backup_dir)
        .map_err(|e| AppError::BackupFailed(format!("Failed to read backup directory: {e}")))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_file())
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.ends_with(&suffix))
        })
        .collect();

    entries.sort_by_key(|entry| {
        entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });

    while entries.len() > max_count {
        let oldest = entries.remove(0);
        fs::remove_file(oldest.path())
            .map_err(|e| AppError::BackupFailed(format!("Failed to remove old backup: {e}")))?;
    }
    Ok(())
}

/// The per-host default backup directory: `<data-local>/not_yet_done/backups`.
/// Matches the legacy core config default so backups continue to land in the
/// same place after the split.
pub fn default_backup_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("not_yet_done")
        .join("backups")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Write a throwaway "database" file and return its `sqlite://` DSN.
    fn fake_db(dir: &Path, name: &str, contents: &[u8]) -> String {
        let path = dir.join(name);
        fs::write(&path, contents).unwrap();
        format!("sqlite://{}", path.display())
    }

    #[test]
    fn create_backup_writes_a_timestamped_suffixed_copy() {
        let tmp = tempdir().unwrap();
        let dsn = fake_db(tmp.path(), "tasks.db", b"payload");
        let backup_dir = tmp.path().join("backups");

        let written = create_backup_at(&dsn, &backup_dir, 10).unwrap();
        let path = Path::new(&written);
        let name = path.file_name().unwrap().to_str().unwrap();

        // `<YYYYMMDD-HHMMSS>-tasks.db`
        assert!(name.ends_with("-tasks.db"), "got {name}");
        assert_eq!(name.len(), "YYYYMMDD-HHMMSS".len() + "-tasks.db".len());
        assert_eq!(fs::read(path).unwrap(), b"payload");
    }

    #[test]
    fn cleanup_is_suffix_aware_and_leaves_other_dbs_alone() {
        let tmp = tempdir().unwrap();
        let backup_dir = tmp.path();

        // Three tasks.db backups and one nyd.db backup in the same dir.
        for ts in ["20260101-000001", "20260102-000001", "20260103-000001"] {
            fs::write(backup_dir.join(format!("{ts}-tasks.db")), b"t").unwrap();
        }
        fs::write(backup_dir.join("20260101-000001-nyd.db"), b"n").unwrap();

        cleanup_old_backups(backup_dir, "tasks.db", 2).unwrap();

        let remaining: Vec<String> = fs::read_dir(backup_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        let tasks = remaining.iter().filter(|n| n.ends_with("-tasks.db")).count();
        // Pruned to the cap…
        assert_eq!(tasks, 2);
        // …without ever touching the unrelated database's backup.
        assert!(remaining.iter().any(|n| n.ends_with("-nyd.db")));
    }

    #[test]
    fn daily_check_runs_once_per_db_per_day() {
        let tmp = tempdir().unwrap();
        let backup_dir = tmp.path().join("backups");
        let tasks = fake_db(tmp.path(), "tasks.db", b"t");
        let nyd = fake_db(tmp.path(), "nyd.db", b"n");

        // First call of the day backs up; the second is a no-op.
        assert!(ensure_daily_backup_at(&tasks, &backup_dir, 10).unwrap().is_some());
        assert!(ensure_daily_backup_at(&tasks, &backup_dir, 10).unwrap().is_none());

        // A *different* database's backup today does not satisfy nyd.db's
        // daily check — the suffix differs.
        assert!(ensure_daily_backup_at(&nyd, &backup_dir, 10).unwrap().is_some());
    }
}
