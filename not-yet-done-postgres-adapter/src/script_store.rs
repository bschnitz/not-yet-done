//! [`ScriptStore`] implementation backing the Postgres adapter's
//! editable SQL scripts.
//!
//! This is the seam that keeps the TUI host from reaching into
//! `crate::query::*` directly: every script CRUD operation the App layer
//! used to perform inline now goes through the [`ScriptStore`] trait, and
//! this struct is the only place that knows the on-disk layout
//! (`<instance_data_dir>/db_scripts/…` for database-level scripts,
//! `<instance_data_dir>/queries/<db>/<schema>/<table>/…` for the flat
//! per-table query scripts). All the actual filesystem work still lives
//! in [`crate::query`]; this type just adapts those `io::Result`
//! signatures to the backend-opaque [`ScriptStore`] contract.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use not_yet_done_content::{ContentError, Result, ScriptStore};

/// Filesystem-backed [`ScriptStore`] for one Postgres adapter instance.
///
/// Holds the instance's data directory (the same path
/// [`crate::PostgresAdapter::instance_data_dir`] resolves to) and
/// delegates every operation to the free functions in [`crate::query`].
pub struct PostgresScriptStore {
    instance_data_dir: PathBuf,
}

impl PostgresScriptStore {
    pub fn new(instance_data_dir: PathBuf) -> Self {
        Self { instance_data_dir }
    }

    /// Map a `std::io::Error` into the contract's error type, preserving
    /// the `Display` text (e.g. the "not empty (N entries)" message that
    /// `delete_db_script_dir` produces) so the TUI can surface it verbatim.
    fn io_err(e: std::io::Error) -> ContentError {
        ContentError::Other(Box::new(e))
    }

    /// Parse a Layer-2 node id (`<db>/schemas/<schema>/tables/<table>`)
    /// back into its `(database, schema, table)` coordinates. Returns
    /// `None` when the id doesn't match that shape.
    fn parse_table_node_id(node_id: &str) -> Option<(String, String, String)> {
        let mut parts = node_id.split('/');
        let db = parts.next()?;
        if parts.next()? != "schemas" {
            return None;
        }
        let schema = parts.next()?;
        if parts.next()? != "tables" {
            return None;
        }
        let table = parts.next()?;
        if parts.next().is_some() || db.is_empty() || schema.is_empty() || table.is_empty() {
            return None;
        }
        Some((db.to_string(), schema.to_string(), table.to_string()))
    }
}

#[async_trait]
impl ScriptStore for PostgresScriptStore {
    // --- Database-level (hierarchical) --------------------------------

    async fn db_entry_is_dir(&self, database: &str, rel_path: &str) -> bool {
        let dir_path = crate::query::db_script_dir_path(
            &self.instance_data_dir,
            database,
            Path::new(rel_path),
        );
        tokio::fs::metadata(&dir_path)
            .await
            .map(|m| m.is_dir())
            .unwrap_or(false)
    }

    async fn create_db_script(&self, database: &str, rel_path: &str) -> Result<bool> {
        let path =
            crate::query::db_script_path(&self.instance_data_dir, database, Path::new(rel_path));
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(false);
        }
        // Nested scripts need their parent directories created first;
        // `write_db_script` only mkdir's the flat-root parent.
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(Self::io_err)?;
        }
        let template = crate::query::default_db_script_file(database, rel_path);
        tokio::fs::write(&path, template.as_bytes())
            .await
            .map_err(Self::io_err)?;
        Ok(true)
    }

    async fn create_db_dir(&self, database: &str, rel_path: &str) -> Result<()> {
        crate::query::create_db_script_dir(&self.instance_data_dir, database, Path::new(rel_path))
            .await
            .map_err(Self::io_err)
    }

    async fn rename_db_entry(&self, database: &str, rel_path: &str, new_name: &str) -> Result<()> {
        crate::query::rename_db_script_entry(
            &self.instance_data_dir,
            database,
            Path::new(rel_path),
            new_name,
        )
        .await
        .map_err(Self::io_err)
    }

    async fn move_db_entry(&self, database: &str, src: &str, dst: &str) -> Result<()> {
        crate::query::move_db_script_entry(
            &self.instance_data_dir,
            database,
            Path::new(src),
            Path::new(dst),
        )
        .await
        .map_err(Self::io_err)
    }

    async fn delete_db_script(&self, database: &str, rel_path: &str) -> Result<()> {
        crate::query::delete_db_script(&self.instance_data_dir, database, rel_path)
            .await
            .map_err(Self::io_err)
    }

    async fn delete_db_dir(&self, database: &str, rel_path: &str) -> Result<()> {
        crate::query::delete_db_script_dir(&self.instance_data_dir, database, Path::new(rel_path))
            .await
            .map_err(Self::io_err)
    }

    // --- Node-scoped (flat) -------------------------------------------

    async fn list_node_scripts(&self, node_id: &str) -> Result<Vec<String>> {
        let Some((database, schema, table)) = Self::parse_table_node_id(node_id) else {
            return Ok(Vec::new());
        };
        crate::query::list_scripts_in_table(&self.instance_data_dir, &database, &schema, &table)
            .await
            .map_err(Self::io_err)
    }

    async fn delete_node_script(&self, node_id: &str, name: &str) -> Result<()> {
        let Some((database, schema, table)) = Self::parse_table_node_id(node_id) else {
            return Ok(());
        };
        let path = crate::query::query_file_path(
            &self.instance_data_dir,
            &database,
            &schema,
            &table,
            name,
        );
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Self::io_err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal self-cleaning temp directory (the crate has no `tempfile`
    /// dev-dep; `query.rs` uses the same pattern in its own tests).
    mod tempdir_ish {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};

        static COUNTER: AtomicU64 = AtomicU64::new(0);

        pub struct TempDir {
            path: PathBuf,
        }

        impl TempDir {
            pub fn new() -> Self {
                let nanos = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                let n = COUNTER.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir()
                    .join(format!("nyd-scriptstore-test-{nanos}-{n}-{}", std::process::id()));
                std::fs::create_dir_all(&path).unwrap();
                Self { path }
            }

            pub fn path(&self) -> &Path {
                &self.path
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }
    }

    #[test]
    fn parse_table_node_id_happy_path() {
        assert_eq!(
            PostgresScriptStore::parse_table_node_id("mydb/schemas/public/tables/users"),
            Some(("mydb".to_string(), "public".to_string(), "users".to_string()))
        );
    }

    #[test]
    fn parse_table_node_id_rejects_wrong_shape() {
        assert!(PostgresScriptStore::parse_table_node_id("mydb").is_none());
        assert!(PostgresScriptStore::parse_table_node_id("mydb/schemas/public").is_none());
        assert!(
            PostgresScriptStore::parse_table_node_id("mydb/schemas/public/tables/users/extra")
                .is_none()
        );
        assert!(
            PostgresScriptStore::parse_table_node_id("mydb/foo/public/tables/users").is_none()
        );
        assert!(PostgresScriptStore::parse_table_node_id("/schemas/public/tables/users").is_none());
    }

    #[tokio::test]
    async fn create_db_script_is_idempotent_and_seeds_template() {
        let dir = tempdir_ish::TempDir::new();
        let store = PostgresScriptStore::new(dir.path().to_path_buf());
        // First create: succeeds and seeds a SQL template.
        assert!(store.create_db_script("mydb", "audit.sql").await.unwrap());
        let on_disk = crate::query::read_db_script(dir.path(), "mydb", "audit.sql")
            .await
            .unwrap();
        assert!(on_disk.contains(crate::query::QUERY_MARKER));
        // Second create: file exists, returns false, leaves it untouched.
        assert!(!store.create_db_script("mydb", "audit.sql").await.unwrap());
    }

    #[tokio::test]
    async fn create_db_script_makes_nested_parents() {
        let dir = tempdir_ish::TempDir::new();
        let store = PostgresScriptStore::new(dir.path().to_path_buf());
        assert!(store
            .create_db_script("mydb", "util/reports/audit.sql")
            .await
            .unwrap());
        assert!(store.db_entry_is_dir("mydb", "util").await);
        assert!(store.db_entry_is_dir("mydb", "util/reports").await);
        assert!(!store.db_entry_is_dir("mydb", "util/reports/audit.sql").await);
    }

    #[tokio::test]
    async fn list_node_scripts_empty_for_unknown_node() {
        let dir = tempdir_ish::TempDir::new();
        let store = PostgresScriptStore::new(dir.path().to_path_buf());
        let v = store.list_node_scripts("mydb/schemas/public/tables/users").await.unwrap();
        assert!(v.is_empty());
        // Malformed node id ⇒ empty, not an error.
        let v = store.list_node_scripts("not-a-table").await.unwrap();
        assert!(v.is_empty());
    }
}
