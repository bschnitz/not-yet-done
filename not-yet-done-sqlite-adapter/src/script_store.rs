//! SQLite side of the shared [`SqlScriptStore`].
//!
//! The store itself is backend-agnostic and lives in
//! `not-yet-done-sql-core`. All that's left here is the one thing only
//! this adapter knows: how a table or view node id
//! (`<key>/tables/<table>`, `<key>/views/<view>`) decomposes into the path
//! segments its scripts are filed under, and what a fresh default script
//! for such a node contains.
//!
//! Two segments, not three as in Postgres — SQLite has no schema
//! namespace, so `<key>` (the source key, see
//! [`crate::sources::source_key`]) takes the place of both the database
//! and the schema level. The key is used verbatim as a directory name:
//! it is already sanitized to `[A-Za-z0-9_-]` plus the path hash, which
//! is precisely why scripts of two same-named files in different folders
//! land in different directories.
//!
//! Tables and views share one script directory per name. That stays
//! unambiguous because SQLite keeps both in `sqlite_master` under a single
//! unique `name`, so one database file cannot hold a table and a view of
//! the same name.

use std::path::PathBuf;
use std::sync::Arc;

use not_yet_done_sql_core::{NodeScriptLayout, SqlScriptStore, quote_ident};

/// Build the script store for one SQLite adapter instance.
///
/// `instance_data_dir` is the same path
/// [`not_yet_done_content::ContentAdapter::instance_data_dir`] resolves
/// to for this adapter.
pub fn sqlite_script_store(instance_data_dir: PathBuf) -> SqlScriptStore {
    SqlScriptStore::new(instance_data_dir, Arc::new(SqliteNodeScriptLayout))
}

/// Maps SQLite table and view node ids onto `queries/<key>/<name>/`.
pub struct SqliteNodeScriptLayout;

impl SqliteNodeScriptLayout {
    /// Parse a table or view node id (`<key>/tables/<table>`,
    /// `<key>/views/<view>`) back into its `(source key, name)`
    /// coordinates. Returns `None` when the id doesn't match that shape —
    /// the level above (`<key>`, `<key>/tables`) and the level below
    /// (`…/rows/<n>`) own no scripts.
    fn parse_table_node_id(node_id: &str) -> Option<(String, String)> {
        let mut parts = node_id.split('/');
        let key = parts.next()?;
        let group = parts.next()?;
        if group != crate::adapter::TABLES_GROUP_ID && group != crate::adapter::VIEWS_GROUP_ID {
            return None;
        }
        let table = parts.next()?;
        if parts.next().is_some() || key.is_empty() || table.is_empty() {
            return None;
        }
        Some((key.to_string(), table.to_string()))
    }
}

impl NodeScriptLayout for SqliteNodeScriptLayout {
    fn node_segments(&self, node_id: &str) -> Option<Vec<String>> {
        let (key, table) = Self::parse_table_node_id(node_id)?;
        Some(vec![key, table])
    }

    fn default_node_script_body(&self, node_id: &str) -> String {
        // An unplaceable id still gets a usable buffer rather than an
        // empty one: the user can type their own SQL, and the marker the
        // executor looks for is present either way.
        match Self::parse_table_node_id(node_id) {
            Some((_, table)) => not_yet_done_content::script_buffer::default_buffer(&format!(
                "SELECT * FROM {};\n",
                quote_ident(&table)
            )),
            None => not_yet_done_content::script_buffer::default_buffer("SELECT 1;\n"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::ScriptStore;

    #[test]
    fn parse_table_node_id_happy_path() {
        assert_eq!(
            SqliteNodeScriptLayout::parse_table_node_id("notes-1a2b3c4d/tables/widgets"),
            Some(("notes-1a2b3c4d".into(), "widgets".into()))
        );
    }

    /// A view is as queryable as a table, so it owns its `Q` scripts the
    /// same way — and files them under its own name.
    #[test]
    fn parse_table_node_id_accepts_a_view() {
        assert_eq!(
            SqliteNodeScriptLayout::parse_table_node_id("notes-1a2b3c4d/views/v_recent"),
            Some(("notes-1a2b3c4d".into(), "v_recent".into()))
        );
    }

    #[test]
    fn parse_table_node_id_rejects_wrong_shape() {
        for id in [
            "notes-1a2b3c4d",
            "notes-1a2b3c4d/tables",
            "notes-1a2b3c4d/tables/widgets/rows/0",
            "notes-1a2b3c4d/views/v_recent/rows/0",
            "notes-1a2b3c4d/schemas/widgets",
            "/tables/widgets",
        ] {
            assert!(
                SqliteNodeScriptLayout::parse_table_node_id(id).is_none(),
                "should reject: {id}"
            );
        }
    }

    #[test]
    fn node_script_path_follows_the_sqlite_layout() {
        let store = sqlite_script_store(PathBuf::from("/tmp/nyd/sqlite"));
        assert_eq!(
            store.node_script_path("notes-1a2b3c4d/tables/widgets", "default"),
            Some(PathBuf::from(
                "/tmp/nyd/sqlite/queries/notes-1a2b3c4d/widgets/default.sql"
            ))
        );
    }

    /// Same file name in two directories: the key's path hash differs, so
    /// their scripts must not share a directory.
    #[test]
    fn same_named_databases_get_separate_script_dirs() {
        let store = sqlite_script_store(PathBuf::from("/tmp/nyd/sqlite"));
        let a = store.node_script_path("data-aaaaaaaa/tables/widgets", "default");
        let b = store.node_script_path("data-bbbbbbbb/tables/widgets", "default");
        assert_ne!(a, b);
    }

    #[test]
    fn default_node_script_body_selects_from_the_addressed_table() {
        let store = sqlite_script_store(PathBuf::from("/tmp/nyd/sqlite"));
        let body = store.default_node_script_body("notes-1a2b3c4d/tables/widgets");
        assert!(body.contains("SELECT * FROM \"widgets\";"), "{body}");
        // An id we can't place still yields a runnable placeholder.
        let fallback = store.default_node_script_body("nonsense");
        assert!(fallback.contains("SELECT 1;"), "{fallback}");
    }
}
