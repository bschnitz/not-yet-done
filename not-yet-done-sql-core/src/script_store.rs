//! [`ScriptStore`] implementation shared by the SQL adapters.
//!
//! This is the seam that keeps the TUI host from reaching into the file
//! layout directly: every script CRUD operation the App layer used to
//! perform inline goes through the [`ScriptStore`] trait, and this type
//! is the only place that knows where the files live.
//!
//! Everything below `db_scripts/` is backend-agnostic — the container
//! key is an opaque directory name. The node-scoped half is not, because
//! only the adapter knows how its node ids decompose; that part is
//! delegated to a [`NodeScriptLayout`].

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use not_yet_done_content::{ContentError, Result, ScriptStore};

use crate::script_files as files;

/// Where one adapter's node-scoped scripts live, and what a fresh one
/// contains.
///
/// Node ids are opaque to the host and differ per backend
/// (`<db>/schemas/<schema>/tables/<table>` for Postgres, something
/// flatter for a single-file database), so decomposing one into path
/// segments is the adapter's job. Everything else about the layout —
/// the `queries/` root, the `.sql` extension, the default script name —
/// is shared and handled by [`SqlScriptStore`].
pub trait NodeScriptLayout: Send + Sync {
    /// Path segments under `queries/` for this node, or `None` when the
    /// id doesn't address anything script-able. `None` makes the script
    /// operations no-ops rather than errors: an id we can't place is
    /// indistinguishable from a node that simply has no scripts.
    fn node_segments(&self, node_id: &str) -> Option<Vec<String>>;

    /// Body for a freshly created default script. Implementations
    /// typically seed a `SELECT * FROM <table>` for the addressed node;
    /// an unplaceable id should still yield a usable buffer rather than
    /// an empty one, so the marker the executor looks for is present
    /// either way.
    fn default_node_script_body(&self, node_id: &str) -> String;
}

/// Filesystem-backed [`ScriptStore`] for one SQL adapter instance.
///
/// Holds the instance's data directory and delegates every operation to
/// the free functions in [`crate::script_files`], adapting their
/// `io::Result` signatures to the backend-opaque [`ScriptStore`]
/// contract.
pub struct SqlScriptStore {
    instance_data_dir: PathBuf,
    layout: Arc<dyn NodeScriptLayout>,
}

impl SqlScriptStore {
    pub fn new(instance_data_dir: PathBuf, layout: Arc<dyn NodeScriptLayout>) -> Self {
        Self {
            instance_data_dir,
            layout,
        }
    }

    /// The instance directory every path this store hands out is rooted
    /// in. Exposed for callers that need the plain
    /// [`crate::script_files`] functions (reading and writing a script
    /// body is not part of the [`ScriptStore`] contract).
    pub fn instance_data_dir(&self) -> &std::path::Path {
        &self.instance_data_dir
    }

    /// Map a `std::io::Error` into the contract's error type, preserving
    /// the `Display` text (e.g. the "not empty (N entries)" message that
    /// `delete_db_script_dir` produces) so the TUI can surface it verbatim.
    fn io_err(e: std::io::Error) -> ContentError {
        ContentError::Other(Box::new(e))
    }
}

#[async_trait]
impl ScriptStore for SqlScriptStore {
    // --- Addressing and templates -------------------------------------

    fn db_script_path(&self, database: &str, rel_path: &str) -> PathBuf {
        files::db_script_path(
            &self.instance_data_dir,
            database,
            std::path::Path::new(rel_path),
        )
    }

    fn default_db_script_body(&self, database: &str, rel_path: &str) -> String {
        files::default_db_script_file(database, rel_path)
    }

    fn node_script_path(&self, node_id: &str, name: &str) -> Option<PathBuf> {
        let segments = self.layout.node_segments(node_id)?;
        Some(files::node_script_file_path(
            &self.instance_data_dir,
            &segments,
            name,
        ))
    }

    fn default_node_script_body(&self, node_id: &str) -> String {
        self.layout.default_node_script_body(node_id)
    }

    fn default_node_script_name(&self) -> &str {
        files::DEFAULT_SCRIPT_NAME
    }

    // --- Container-level (hierarchical) -------------------------------

    async fn db_entry_is_dir(&self, database: &str, rel_path: &str) -> bool {
        let dir_path = files::db_script_dir_path(
            &self.instance_data_dir,
            database,
            std::path::Path::new(rel_path),
        );
        tokio::fs::metadata(&dir_path)
            .await
            .map(|m| m.is_dir())
            .unwrap_or(false)
    }

    async fn create_db_script(&self, database: &str, rel_path: &str) -> Result<bool> {
        let path = files::db_script_path(
            &self.instance_data_dir,
            database,
            std::path::Path::new(rel_path),
        );
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
        let template = files::default_db_script_file(database, rel_path);
        tokio::fs::write(&path, template.as_bytes())
            .await
            .map_err(Self::io_err)?;
        Ok(true)
    }

    async fn create_db_dir(&self, database: &str, rel_path: &str) -> Result<()> {
        files::create_db_script_dir(
            &self.instance_data_dir,
            database,
            std::path::Path::new(rel_path),
        )
        .await
        .map_err(Self::io_err)
    }

    async fn rename_db_entry(&self, database: &str, rel_path: &str, new_name: &str) -> Result<()> {
        files::rename_db_script_entry(
            &self.instance_data_dir,
            database,
            std::path::Path::new(rel_path),
            new_name,
        )
        .await
        .map_err(Self::io_err)
    }

    async fn move_db_entry(&self, database: &str, src: &str, dst: &str) -> Result<()> {
        files::move_db_script_entry(
            &self.instance_data_dir,
            database,
            std::path::Path::new(src),
            std::path::Path::new(dst),
        )
        .await
        .map_err(Self::io_err)
    }

    async fn delete_db_script(&self, database: &str, rel_path: &str) -> Result<()> {
        files::delete_db_script(&self.instance_data_dir, database, rel_path)
            .await
            .map_err(Self::io_err)
    }

    async fn delete_db_dir(&self, database: &str, rel_path: &str) -> Result<()> {
        files::delete_db_script_dir(
            &self.instance_data_dir,
            database,
            std::path::Path::new(rel_path),
        )
        .await
        .map_err(Self::io_err)
    }

    // --- Node-scoped (flat) -------------------------------------------

    async fn list_node_scripts(&self, node_id: &str) -> Result<Vec<String>> {
        let Some(segments) = self.layout.node_segments(node_id) else {
            return Ok(Vec::new());
        };
        files::list_node_scripts(&self.instance_data_dir, &segments)
            .await
            .map_err(Self::io_err)
    }

    async fn delete_node_script(&self, node_id: &str, name: &str) -> Result<()> {
        let Some(segments) = self.layout.node_segments(node_id) else {
            return Ok(());
        };
        let path = files::node_script_file_path(&self.instance_data_dir, &segments, name);
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
    use not_yet_done_content::script_buffer::QUERY_MARKER;

    /// Two-segment layout: `<container>/<table>`. Enough to exercise the
    /// shared half without pulling in a real adapter's id grammar.
    struct TwoSegmentLayout;

    impl NodeScriptLayout for TwoSegmentLayout {
        fn node_segments(&self, node_id: &str) -> Option<Vec<String>> {
            let mut parts = node_id.split('/');
            let a = parts.next()?;
            let b = parts.next()?;
            if parts.next().is_some() || a.is_empty() || b.is_empty() {
                return None;
            }
            Some(vec![a.to_string(), b.to_string()])
        }

        fn default_node_script_body(&self, node_id: &str) -> String {
            not_yet_done_content::script_buffer::default_buffer(&format!(
                "SELECT * FROM {node_id};\n"
            ))
        }
    }

    fn store(dir: &std::path::Path) -> SqlScriptStore {
        SqlScriptStore::new(dir.to_path_buf(), Arc::new(TwoSegmentLayout))
    }

    #[test]
    fn node_script_path_follows_the_layout() {
        let s = store(std::path::Path::new("/tmp/nyd/instance"));
        assert_eq!(
            s.node_script_path("main/users", "default"),
            Some(PathBuf::from(
                "/tmp/nyd/instance/queries/main/users/default.sql"
            ))
        );
    }

    #[test]
    fn unplaceable_node_id_has_no_script_path() {
        let s = store(std::path::Path::new("/tmp/nyd/instance"));
        assert_eq!(s.node_script_path("main", "default"), None);
        assert_eq!(s.node_script_path("a/b/c", "default"), None);
    }

    #[tokio::test]
    async fn unplaceable_node_id_makes_script_ops_no_ops() {
        let dir = tempfile::tempdir().expect("temp dir");
        let s = store(dir.path());
        assert!(s.list_node_scripts("main").await.unwrap().is_empty());
        // Deleting a script of an unplaceable node must not error either.
        s.delete_node_script("main", "default").await.unwrap();
    }

    #[tokio::test]
    async fn create_db_script_is_idempotent_and_seeds_template() {
        let dir = tempfile::tempdir().expect("temp dir");
        let s = store(dir.path());
        assert!(s.create_db_script("mydb", "audit.sql").await.unwrap());
        let path = s.db_script_path("mydb", "audit.sql");
        let body = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(body.contains(QUERY_MARKER));
        // Second call reports "already there" and leaves the body alone.
        assert!(!s.create_db_script("mydb", "audit.sql").await.unwrap());
    }

    #[tokio::test]
    async fn create_db_script_makes_nested_parents() {
        let dir = tempfile::tempdir().expect("temp dir");
        let s = store(dir.path());
        assert!(
            s.create_db_script("mydb", "util/deep/audit.sql")
                .await
                .unwrap()
        );
        let path = s.db_script_path("mydb", "util/deep/audit.sql");
        assert!(tokio::fs::try_exists(&path).await.unwrap());
    }

    #[tokio::test]
    async fn node_scripts_round_trip_through_the_store() {
        let dir = tempfile::tempdir().expect("temp dir");
        let s = store(dir.path());
        let path = s
            .node_script_path("main/users", "active")
            .expect("placeable");
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&path, s.default_node_script_body("main/users"))
            .await
            .unwrap();

        assert_eq!(
            s.list_node_scripts("main/users").await.unwrap(),
            vec!["active".to_string()]
        );
        s.delete_node_script("main/users", "active").await.unwrap();
        assert!(s.list_node_scripts("main/users").await.unwrap().is_empty());
        // Idempotent delete.
        s.delete_node_script("main/users", "active").await.unwrap();
    }

    #[test]
    fn default_node_script_body_comes_from_the_layout() {
        let s = store(std::path::Path::new("/tmp/nyd/instance"));
        let body = s.default_node_script_body("main/users");
        assert!(body.contains(QUERY_MARKER));
        assert!(body.contains("SELECT * FROM main/users;"));
    }
}
