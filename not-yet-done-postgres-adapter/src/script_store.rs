//! Postgres side of the shared [`SqlScriptStore`].
//!
//! The store itself is backend-agnostic and lives in
//! `not-yet-done-sql-core`. All that's left here is the one thing only
//! this adapter knows: how a Layer-2 node id
//! (`<db>/schemas/<schema>/tables/<table>`) decomposes into the path
//! segments its scripts are filed under, and what a fresh default
//! script for such a node contains.
//!
//! Tables and views share one script directory per name. That stays
//! unambiguous because postgres keeps both in `pg_class`, where a name is
//! unique per schema — so one schema cannot hold a table and a view of the
//! same name.

use std::path::PathBuf;
use std::sync::Arc;

use not_yet_done_sql_core::{NodeScriptLayout, SqlScriptStore};

/// Build the script store for one Postgres adapter instance.
///
/// `instance_data_dir` is the same path
/// [`crate::PostgresAdapter::instance_data_dir`] resolves to.
pub fn postgres_script_store(instance_data_dir: PathBuf) -> SqlScriptStore {
    SqlScriptStore::new(instance_data_dir, Arc::new(PostgresNodeScriptLayout))
}

/// Maps Postgres table node ids onto `queries/<db>/<schema>/<table>/`.
pub struct PostgresNodeScriptLayout;

impl PostgresNodeScriptLayout {
    /// Parse a Layer-2 node id (`<db>/schemas/<schema>/tables/<table>` or
    /// `…/views/<view>`) back into its `(database, schema, relation)`
    /// coordinates. Returns `None` when the id doesn't match that shape.
    ///
    /// Both group segments are accepted: a view would otherwise silently
    /// lose its per-node scripts, since an unparseable id has no place to
    /// file them.
    fn parse_table_node_id(node_id: &str) -> Option<(String, String, String)> {
        let mut parts = node_id.split('/');
        let db = parts.next()?;
        if parts.next()? != "schemas" {
            return None;
        }
        let schema = parts.next()?;
        let group = parts.next()?;
        if group != crate::adapter::TABLES_GROUP_ID && group != crate::adapter::VIEWS_GROUP_ID {
            return None;
        }
        let table = parts.next()?;
        if parts.next().is_some() || db.is_empty() || schema.is_empty() || table.is_empty() {
            return None;
        }
        Some((db.to_string(), schema.to_string(), table.to_string()))
    }
}

impl NodeScriptLayout for PostgresNodeScriptLayout {
    fn node_segments(&self, node_id: &str) -> Option<Vec<String>> {
        let (database, schema, table) = Self::parse_table_node_id(node_id)?;
        Some(vec![database, schema, table])
    }

    fn default_node_script_body(&self, node_id: &str) -> String {
        // An unparseable id still gets a usable buffer rather than an
        // empty one: the user can type their own SQL, and the marker the
        // executor looks for is present either way.
        match Self::parse_table_node_id(node_id) {
            Some((_, schema, table)) => crate::query::default_query_file(&schema, &table),
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
            PostgresNodeScriptLayout::parse_table_node_id("mydb/schemas/public/tables/users"),
            Some(("mydb".into(), "public".into(), "users".into()))
        );
    }

    /// A view files its scripts in the same layout as a table — same
    /// directory per name, which postgres' per-schema name uniqueness makes
    /// unambiguous.
    #[test]
    fn parse_table_node_id_accepts_a_view() {
        assert_eq!(
            PostgresNodeScriptLayout::parse_table_node_id("mydb/schemas/public/views/v_recent"),
            Some(("mydb".into(), "public".into(), "v_recent".into()))
        );
    }

    #[test]
    fn parse_table_node_id_rejects_wrong_shape() {
        for id in [
            "mydb",
            "mydb/schemas/public",
            "mydb/schemas/public/tables/users/extra",
            "mydb/foo/public/tables/users",
            "/schemas/public/tables/users",
            // A row is not a place to file scripts.
            "mydb/schemas/public/views/v_recent/rows/0",
        ] {
            assert!(
                PostgresNodeScriptLayout::parse_table_node_id(id).is_none(),
                "should reject: {id}"
            );
        }
    }

    #[test]
    fn node_script_path_follows_the_postgres_layout() {
        let store = postgres_script_store(PathBuf::from("/tmp/nyd/pg"));
        assert_eq!(
            store.node_script_path("mydb/schemas/public/tables/users", "default"),
            Some(PathBuf::from(
                "/tmp/nyd/pg/queries/mydb/public/users/default.sql"
            ))
        );
    }

    #[test]
    fn default_node_script_body_selects_from_the_addressed_table() {
        let store = postgres_script_store(PathBuf::from("/tmp/nyd/pg"));
        let body = store.default_node_script_body("mydb/schemas/public/tables/users");
        assert!(
            body.contains("SELECT * FROM \"public\".\"users\";"),
            "{body}"
        );
        // An id we can't place still yields a runnable placeholder.
        let fallback = store.default_node_script_body("nonsense");
        assert!(fallback.contains("SELECT 1;"), "{fallback}");
    }
}
