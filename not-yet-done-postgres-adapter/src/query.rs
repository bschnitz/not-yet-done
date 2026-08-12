//! Postgres-shaped view of the shared script layout.
//!
//! The layout itself — which directory holds which script, how the
//! container-level script tree is walked and mutated — is backend
//! agnostic and lives in [`not_yet_done_sql_core::script_files`]; those
//! items are re-exported here so the adapter's many call sites keep one
//! import path.
//!
//! What this module adds is the Postgres shape on top of it: node
//! scripts are addressed by the `(database, schema, table)` triple, and
//! the default template is a `SELECT * FROM "<schema>"."<table>"`.

use std::path::{Path, PathBuf};

use not_yet_done_content::script_buffer::default_buffer;
use not_yet_done_sql_core::script_files as files;

use crate::client::quote_ident;

pub use not_yet_done_sql_core::script_files::{
    DEFAULT_SCRIPT_NAME, DbScriptEntry, DbScriptTreeEntry, create_db_script_dir,
    db_script_dir_path, db_script_file_path, db_script_path, db_scripts_dir, delete_db_script,
    delete_db_script_dir, is_sql_extension, list_all_db_scripts, list_db_script_entries,
    list_db_scripts_in_database, move_db_script_entry, read_db_script, rename_db_script_entry,
    walk_db_script_entries, write_db_script,
};

/// Path segments under `queries/` for one Postgres table.
fn table_segments(database: &str, schema: &str, table: &str) -> Vec<String> {
    vec![database.to_string(), schema.to_string(), table.to_string()]
}

/// Resolve the on-disk path of a named persisted query:
/// `<instance_data_dir>/queries/<database>/<schema>/<table>/<script>.sql`.
pub fn query_file_path(
    instance_data_dir: &Path,
    database: &str,
    schema: &str,
    table: &str,
    script: &str,
) -> PathBuf {
    files::node_script_file_path(
        instance_data_dir,
        &table_segments(database, schema, table),
        script,
    )
}

/// Directory holding all named scripts for `(database, schema, table)`.
pub fn table_scripts_dir(
    instance_data_dir: &Path,
    database: &str,
    schema: &str,
    table: &str,
) -> PathBuf {
    files::node_scripts_dir(instance_data_dir, &table_segments(database, schema, table))
}

/// One on-disk script file found under
/// `<instance_data_dir>/queries/<database>/<schema>/<table>/<script>.sql`.
#[derive(Debug, Clone)]
pub struct ScriptEntry {
    pub database: String,
    pub schema: String,
    pub table: String,
    pub script: String,
}

/// Walk `<instance_data_dir>/queries/` and return every `.sql` file as
/// a [`ScriptEntry`]. The directory layout is fixed at four levels
/// (db / schema / table / `<script>.sql`). Anything that doesn't match
/// (stray files, missing levels) is silently skipped — the directory
/// is user-writable and we treat unknown structure as not-our-problem
/// rather than surfacing it as an error in the UI. Returns an empty
/// list if the `queries/` root doesn't exist yet.
pub async fn list_all_scripts(instance_data_dir: &Path) -> std::io::Result<Vec<ScriptEntry>> {
    let root = instance_data_dir.join("queries");
    let mut out = Vec::new();
    let dbs = match files::read_subdirs(&root).await? {
        Some(v) => v,
        None => return Ok(out),
    };
    for db in dbs {
        let schemas = match files::read_subdirs(&root.join(&db)).await? {
            Some(v) => v,
            None => continue,
        };
        for schema in schemas {
            let tables = match files::read_subdirs(&root.join(&db).join(&schema)).await? {
                Some(v) => v,
                None => continue,
            };
            for table in tables {
                for script in files::list_node_scripts(
                    instance_data_dir,
                    &table_segments(&db, &schema, &table),
                )
                .await?
                {
                    out.push(ScriptEntry {
                        database: db.clone(),
                        schema: schema.clone(),
                        table: table.clone(),
                        script,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| {
        (&a.database, &a.schema, &a.table, &a.script).cmp(&(
            &b.database,
            &b.schema,
            &b.table,
            &b.script,
        ))
    });
    Ok(out)
}

/// List `.sql` script names in a single `(database, schema, table)`
/// directory. Missing directory ⇒ empty list. Sorted alphabetically.
/// Shortcut bindings live in the `query_shortcut` DB table, not on
/// disk — the TUI merges them in at listing time.
pub async fn list_scripts_in_table(
    instance_data_dir: &Path,
    database: &str,
    schema: &str,
    table: &str,
) -> std::io::Result<Vec<String>> {
    files::list_node_scripts(instance_data_dir, &table_segments(database, schema, table)).await
}

/// Default file contents shown the first time the editor opens for
/// `(schema, table)`: the shared buffer skeleton wrapped around a
/// `SELECT * FROM <schema>.<table>` body, both identifiers quoted via
/// [`crate::client::quote_ident`].
pub fn default_query_file(schema: &str, table: &str) -> String {
    let qschema = quote_ident(schema);
    let qtable = quote_ident(table);
    default_buffer(&format!("SELECT * FROM {qschema}.{qtable};\n"))
}

/// Default file contents shown the first time the editor opens for a
/// DB-level script. Delegates to the shared template selector — kept as
/// a named wrapper because the adapter references it in several places.
pub fn default_db_script_file(database: &str, script: &str) -> String {
    files::default_db_script_file(database, script)
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::script_buffer::QUERY_MARKER;

    #[test]
    fn path_is_under_queries_subdir() {
        let base = Path::new("/tmp/nyd/postgres/work");
        let p = query_file_path(base, "mydb", "public", "users", DEFAULT_SCRIPT_NAME);
        assert_eq!(
            p,
            Path::new("/tmp/nyd/postgres/work/queries/mydb/public/users/default.sql")
        );
    }

    #[test]
    fn named_script_path_is_alongside_default() {
        let base = Path::new("/tmp/nyd/postgres/work");
        let p = query_file_path(base, "mydb", "public", "users", "active_only");
        assert_eq!(
            p,
            Path::new("/tmp/nyd/postgres/work/queries/mydb/public/users/active_only.sql")
        );
    }

    #[test]
    fn default_template_contains_marker_and_quoted_select() {
        let s = default_query_file("public", "users");
        assert!(s.contains(QUERY_MARKER));
        assert!(s.contains("SELECT * FROM \"public\".\"users\";"));
    }

    #[test]
    fn default_template_quotes_special_identifiers() {
        let s = default_query_file("we\"ird", "tbl");
        // Embedded `"` is doubled.
        assert!(s.contains("\"we\"\"ird\".\"tbl\""));
    }

    #[tokio::test]
    async fn list_scripts_in_table_returns_empty_when_dir_missing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let v = list_scripts_in_table(dir.path(), "db", "public", "users")
            .await
            .unwrap();
        assert!(v.is_empty());
    }

    #[tokio::test]
    async fn list_scripts_in_table_returns_sql_files_only_sorted() {
        let dir = tempfile::tempdir().expect("temp dir");
        let td = table_scripts_dir(dir.path(), "db", "public", "users");
        tokio::fs::create_dir_all(&td).await.unwrap();
        tokio::fs::write(td.join("zeta.sql"), "select 1")
            .await
            .unwrap();
        tokio::fs::write(td.join("alpha.sql"), "select 2")
            .await
            .unwrap();
        tokio::fs::write(td.join("notes.txt"), "ignore me")
            .await
            .unwrap();
        let v = list_scripts_in_table(dir.path(), "db", "public", "users")
            .await
            .unwrap();
        assert_eq!(v, vec!["alpha".to_string(), "zeta".to_string()]);
    }

    #[tokio::test]
    async fn list_all_scripts_walks_the_four_level_layout() {
        let dir = tempfile::tempdir().expect("temp dir");
        for (db, schema, table, script) in [
            ("beta", "public", "orders", "default"),
            ("alpha", "public", "users", "zeta"),
            ("alpha", "public", "users", "alpha"),
        ] {
            let p = query_file_path(dir.path(), db, schema, table, script);
            tokio::fs::create_dir_all(p.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(&p, "select 1").await.unwrap();
        }
        // A stray file at the wrong depth is skipped rather than erroring.
        tokio::fs::write(dir.path().join("queries/stray.sql"), "x")
            .await
            .unwrap();

        let v = list_all_scripts(dir.path()).await.unwrap();
        let quads: Vec<(&str, &str, &str, &str)> = v
            .iter()
            .map(|e| {
                (
                    e.database.as_str(),
                    e.schema.as_str(),
                    e.table.as_str(),
                    e.script.as_str(),
                )
            })
            .collect();
        assert_eq!(
            quads,
            vec![
                ("alpha", "public", "users", "alpha"),
                ("alpha", "public", "users", "zeta"),
                ("beta", "public", "orders", "default"),
            ]
        );
    }
}
