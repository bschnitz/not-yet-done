//! [`LocalColumnStore`] — the single, lib-owned SQLite holding every custom
//! cell, plus the process-wide shared handle every decorator reads through.
//!
//! The connection is opened once, eagerly, when [`shared_store`] is first
//! called (from the sync `AdapterFactory::create` path) — bridged into async
//! via `block_in_place` + the ambient Tokio handle, exactly as the Jira
//! adapter's factory does. The store then holds an `Arc<DatabaseConnection>`
//! and its query methods await plain sea-orm futures (which are `Send`, as
//! `async_trait` requires); wrapping the schema-sync future in a lazy
//! `OnceCell` instead would infect the decorator's futures with `!Send`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, Database, DatabaseConnection, EntityTrait, QueryFilter,
};

use not_yet_done_content::{ColumnSchema, ContentError, Result};

use crate::entity::{custom_cell, custom_column};

/// One stored cell, as handed to a decorator for injection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub column_key: String,
    pub value: String,
    pub value_type: String,
}

/// The value types the store understands. `text` is the permissive default;
/// the rest are validated on write.
pub const VALUE_TYPES: [&str; 5] = ["text", "number", "duration", "datetime", "json"];

/// Validate a value string against its declared type. An empty value is always
/// accepted (it represents "unset"); `text` and any unknown type never fail.
/// `number` parses as a decimal, `duration` as integer seconds, `datetime` as
/// RFC 3339 — the canonical inputs the view column kinds format. `json` parses
/// as any JSON value; it is the type for a cell holding structured data (a
/// list of tags, a list of records) that no scalar type describes, and unlike
/// the others it says nothing about how the value compares — a `json` column
/// still sorts as text.
fn validate_value(value_type: &str, value: &str) -> Result<()> {
    let v = value.trim();
    if v.is_empty() {
        return Ok(());
    }
    let ok = match value_type {
        "number" => v.parse::<f64>().is_ok(),
        "duration" => v.parse::<i64>().is_ok(),
        "datetime" => chrono::DateTime::parse_from_rfc3339(v).is_ok(),
        "json" => serde_json::from_str::<serde_json::Value>(v).is_ok(),
        _ => true,
    };
    if ok {
        Ok(())
    } else {
        Err(ContentError::Other(
            format!("`{value}` is not a valid {value_type} value").into(),
        ))
    }
}

/// Open a sea-orm connection and schema-sync this crate's entities. The sync
/// is scoped to `not_yet_done_custom_columns::entity::*`, so pointing the store
/// at a shared database leaves other tables untouched.
pub async fn connect(url: &str) -> std::result::Result<DatabaseConnection, sea_orm::DbErr> {
    let db = Database::connect(url).await?;
    db.get_schema_registry("not_yet_done_custom_columns::entity::*")
        .sync(&db)
        .await?;
    Ok(db)
}

/// The lib-owned store of custom cells. Holds an already-opened, schema-synced
/// connection shared for the process; query methods await plain sea-orm futures
/// (which are `Send`, as `async_trait` requires).
///
/// `conn` is `None` only in the degraded fallback (no runtime / open failed):
/// every method then no-ops, so the feature is simply inert rather than
/// breaking adapter construction.
pub struct LocalColumnStore {
    conn: Option<Arc<DatabaseConnection>>,
}

impl LocalColumnStore {
    /// Construct a store over an already-opened connection.
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn: Some(conn) }
    }

    /// An inert store that persists nothing and injects no cells — the
    /// degraded fallback when a connection can't be opened.
    pub fn inert() -> Self {
        Self { conn: None }
    }

    /// All cells for the given `scope` and set of `row_ids`, grouped by row id,
    /// in a single query. Rows with no stored cells are simply absent from the
    /// map. An empty `row_ids` short-circuits to an empty map (no query).
    pub async fn get_for_rows(
        &self,
        scope: &str,
        row_ids: &[&str],
    ) -> Result<HashMap<String, Vec<Cell>>> {
        let Some(conn) = &self.conn else {
            return Ok(HashMap::new());
        };
        if row_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let ids: Vec<String> = row_ids.iter().map(|s| s.to_string()).collect();
        let models = custom_cell::Entity::find()
            .filter(custom_cell::Column::Scope.eq(scope))
            .filter(custom_cell::Column::RowId.is_in(ids))
            .all(conn.as_ref())
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;

        let mut out: HashMap<String, Vec<Cell>> = HashMap::new();
        for m in models {
            out.entry(m.row_id).or_default().push(Cell {
                column_key: m.column_key,
                value: m.value,
                value_type: m.value_type,
            });
        }
        Ok(out)
    }

    /// Convenience wrapper over [`Self::get_for_rows`] for a single row.
    pub async fn get_for_row(&self, scope: &str, row_id: &str) -> Result<Vec<Cell>> {
        Ok(self
            .get_for_rows(scope, &[row_id])
            .await?
            .remove(row_id)
            .unwrap_or_default())
    }

    /// Upsert one cell, enforcing the column's type.
    ///
    /// The column's type is fixed on first write (type-on-first-write): the
    /// first `set_cell` for a `(scope, node_type, column_key)` records
    /// `value_type` in the schema table. From then on the store is
    /// **authoritative** — a later write whose `value_type` differs is rejected
    /// (`Err`), never silently coerced. The value is validated against the
    /// effective type before it is stored.
    pub async fn set_cell(
        &self,
        scope: &str,
        node_type: &str,
        row_id: &str,
        column_key: &str,
        value: &str,
        value_type: &str,
    ) -> Result<()> {
        let Some(conn) = &self.conn else {
            return Ok(());
        };

        // Resolve the authoritative type: the schema wins once the column
        // exists; otherwise this write introduces it.
        let existing = custom_column::Entity::find_by_id((
            scope.to_string(),
            node_type.to_string(),
            column_key.to_string(),
        ))
        .one(conn.as_ref())
        .await
        .map_err(|e| ContentError::Other(Box::new(e)))?;

        let effective_type = match &existing {
            Some(schema) => {
                if schema.value_type != value_type {
                    return Err(ContentError::Other(
                        format!(
                            "custom column `{column_key}` is type `{}`; cannot store a `{value_type}` value \
                             (use the `retype-column` action to change the column's type)",
                            schema.value_type
                        )
                        .into(),
                    ));
                }
                schema.value_type.clone()
            }
            None => value_type.to_string(),
        };

        validate_value(&effective_type, value)?;

        // Register the column schema on first write.
        if existing.is_none() {
            custom_column::Entity::insert(custom_column::ActiveModel {
                scope: Set(scope.to_string()),
                node_type: Set(node_type.to_string()),
                column_key: Set(column_key.to_string()),
                value_type: Set(effective_type.clone()),
                label: Set(None),
            })
            .exec(conn.as_ref())
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;
        }

        let am = custom_cell::ActiveModel {
            scope: Set(scope.to_string()),
            row_id: Set(row_id.to_string()),
            column_key: Set(column_key.to_string()),
            value: Set(value.to_string()),
            value_type: Set(effective_type),
        };
        custom_cell::Entity::insert(am)
            .on_conflict(
                OnConflict::columns([
                    custom_cell::Column::Scope,
                    custom_cell::Column::RowId,
                    custom_cell::Column::ColumnKey,
                ])
                .update_columns([custom_cell::Column::Value, custom_cell::Column::ValueType])
                .to_owned(),
            )
            .exec(conn.as_ref())
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;
        Ok(())
    }

    /// Change a defined column's value type, but only if every stored value
    /// still fits.
    ///
    /// Type-on-first-write ([`Self::set_cell`]) pins a column's type to
    /// whatever the very first write happened to declare — usually `text`,
    /// because that is the default the forms offer. This is the one way back:
    /// the new type is accepted when *all* the column's stored values validate
    /// under it, and rejected otherwise with an error naming each offending row
    /// id and value, so they can be corrected and the retype retried. Nothing
    /// is written on the failure path.
    ///
    /// Cells are keyed by `(scope, row_id, column_key)` — without a
    /// `node_type` — so a column key defined on two node types of one instance
    /// shares one pool of cells. The denormalised per-cell `value_type` is the
    /// only thing separating them, so the retype covers exactly the cells that
    /// currently carry the column's *old* type.
    ///
    /// Returns the number of cells migrated. Retyping to the type a column
    /// already has is a no-op success.
    pub async fn retype_column(
        &self,
        scope: &str,
        node_type: &str,
        column_key: &str,
        new_type: &str,
    ) -> Result<usize> {
        let Some(conn) = &self.conn else {
            return Ok(0);
        };
        if !VALUE_TYPES.contains(&new_type) {
            return Err(ContentError::Other(
                format!(
                    "`{new_type}` is not a value type ({})",
                    VALUE_TYPES.join(", ")
                )
                .into(),
            ));
        }

        let schema = custom_column::Entity::find_by_id((
            scope.to_string(),
            node_type.to_string(),
            column_key.to_string(),
        ))
        .one(conn.as_ref())
        .await
        .map_err(|e| ContentError::Other(Box::new(e)))?
        .ok_or_else(|| {
            ContentError::Other(
                format!("no custom column `{column_key}` is defined on `{node_type}`").into(),
            )
        })?;

        let old_type = schema.value_type.clone();
        if old_type == new_type {
            return Ok(0);
        }

        let cells = custom_cell::Entity::find()
            .filter(custom_cell::Column::Scope.eq(scope))
            .filter(custom_cell::Column::ColumnKey.eq(column_key))
            .filter(custom_cell::Column::ValueType.eq(old_type.as_str()))
            .all(conn.as_ref())
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;

        // Name every value that does not fit, not just the first — the point is
        // to hand back a complete correction list.
        let mut offenders: Vec<String> = cells
            .iter()
            .filter(|c| validate_value(new_type, &c.value).is_err())
            .map(|c| format!("{}: `{}`", c.row_id, c.value))
            .collect();
        if !offenders.is_empty() {
            offenders.sort();
            return Err(ContentError::Other(
                format!(
                    "cannot retype custom column `{column_key}` from `{old_type}` to \
                     `{new_type}`: {} value(s) do not fit — {}",
                    offenders.len(),
                    offenders.join(", ")
                )
                .into(),
            ));
        }

        let mut am: custom_column::ActiveModel = schema.into();
        am.value_type = Set(new_type.to_string());
        custom_column::Entity::update(am)
            .exec(conn.as_ref())
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;

        custom_cell::Entity::update_many()
            .col_expr(
                custom_cell::Column::ValueType,
                Expr::value(new_type.to_string()),
            )
            .filter(custom_cell::Column::Scope.eq(scope))
            .filter(custom_cell::Column::ColumnKey.eq(column_key))
            .filter(custom_cell::Column::ValueType.eq(old_type.as_str()))
            .exec(conn.as_ref())
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;

        Ok(cells.len())
    }

    /// Remove one cell. Missing cells are not an error (idempotent). The
    /// column's schema entry is intentionally left in place — a column stays
    /// defined (and typed) even when no row currently carries a value.
    pub async fn clear_cell(&self, scope: &str, row_id: &str, column_key: &str) -> Result<()> {
        let Some(conn) = &self.conn else {
            return Ok(());
        };
        custom_cell::Entity::delete_many()
            .filter(custom_cell::Column::Scope.eq(scope))
            .filter(custom_cell::Column::RowId.eq(row_id))
            .filter(custom_cell::Column::ColumnKey.eq(column_key))
            .exec(conn.as_ref())
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;
        Ok(())
    }

    /// The custom columns defined for a `(scope, node_type)`, in stable
    /// `column_key` order. This is the discovery surface a front-end reads to
    /// learn which columns exist and how they are typed.
    pub async fn columns(&self, scope: &str, node_type: &str) -> Result<Vec<ColumnSchema>> {
        let Some(conn) = &self.conn else {
            return Ok(Vec::new());
        };
        let mut models = custom_column::Entity::find()
            .filter(custom_column::Column::Scope.eq(scope))
            .filter(custom_column::Column::NodeType.eq(node_type))
            .all(conn.as_ref())
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;
        models.sort_by(|a, b| a.column_key.cmp(&b.column_key));
        Ok(models
            .into_iter()
            .map(|m| ColumnSchema {
                label: m.label,
                // Sortable and present in every row: the decorator projects
                // each defined column into every row it injects into, blank
                // where no cell is stored — so a sort or a filter over a
                // custom column sees a real, if empty, cell everywhere.
                ..ColumnSchema::new(m.column_key, "").typed(m.value_type)
            })
            .collect())
    }
}

/// Resolve the default SQLite URL for the single lib-owned store:
/// `<XDG_DATA_HOME>/not_yet_done/custom_columns.sqlite`, creating the parent
/// directory. `mode=rwc` so the file is created on first connect.
pub fn default_sqlite_url() -> Result<String> {
    let dir: PathBuf = dirs::data_local_dir()
        .ok_or_else(|| ContentError::Other("cannot resolve XDG data-local dir".into()))?
        .join("not_yet_done");
    std::fs::create_dir_all(&dir)
        .map_err(|e| ContentError::Other(format!("create {}: {e}", dir.display()).into()))?;
    let path = dir.join("custom_columns.sqlite");
    Ok(format!("sqlite://{}?mode=rwc", path.display()))
}

/// The process-wide shared store, backed by the default path. Opened once
/// (bridging async `connect` into this sync path via `block_in_place` + the
/// ambient multi-threaded Tokio handle, exactly as the Jira factory does) and
/// shared by every decorator so they all use the one connection.
///
/// If the connection can't be opened (path unresolvable, no runtime, open
/// error) this falls back to an in-memory database — the feature is then inert
/// (nothing persists) rather than breaking adapter construction. Every read is
/// already best-effort, so a degraded store simply injects no cells.
pub fn shared_store() -> Arc<LocalColumnStore> {
    static STORE: std::sync::OnceLock<Arc<LocalColumnStore>> = std::sync::OnceLock::new();
    STORE
        .get_or_init(|| {
            let url = default_sqlite_url().unwrap_or_else(|_| in_memory_url());
            match open_blocking(&url).or_else(|_| open_blocking(&in_memory_url())) {
                Ok(conn) => Arc::new(LocalColumnStore::new(conn)),
                Err(_) => Arc::new(LocalColumnStore::inert()),
            }
        })
        .clone()
}

/// A unique shared-cache in-memory URL for the degraded fallback.
fn in_memory_url() -> String {
    "sqlite:file:nyd_custom_columns_fallback?mode=memory&cache=shared".to_string()
}

/// Bridge async [`connect`] into a sync caller via `block_in_place` + the
/// current Tokio handle. Requires a multi-threaded runtime (all front-ends use
/// one, as the Jira factory already relies on).
fn open_blocking(url: &str) -> std::result::Result<Arc<DatabaseConnection>, String> {
    let handle = tokio::runtime::Handle::try_current()
        .map_err(|_| "custom-columns store needs a Tokio runtime".to_string())?;
    if handle.runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
        return Err("custom-columns store needs a multi-threaded Tokio runtime".into());
    }
    let url_owned = url.to_string();
    tokio::task::block_in_place(|| handle.block_on(connect(&url_owned)))
        .map(Arc::new)
        .map_err(|e| format!("open custom-columns db ({url_owned}): {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mem_store(name: &str) -> LocalColumnStore {
        // A *shared-cache* named in-memory DB: every connection sea-orm's pool
        // opens sees the same data (a bare `sqlite::memory:` gives each pool
        // connection its own empty DB). A unique name per test isolates them.
        let url = format!("sqlite:file:{name}?mode=memory&cache=shared");
        LocalColumnStore::new(Arc::new(connect(&url).await.unwrap()))
    }

    #[tokio::test]
    async fn set_get_clear_roundtrip() {
        let store = mem_store("cc_roundtrip").await;
        let scope = "jira/acme";
        let nt = "jira:issue";
        store
            .set_cell(scope, nt, "ISS-1", "estimate", "5", "number")
            .await
            .unwrap();
        store
            .set_cell(scope, nt, "ISS-1", "note", "look into", "text")
            .await
            .unwrap();

        let cells = store.get_for_row(scope, "ISS-1").await.unwrap();
        assert_eq!(cells.len(), 2);
        assert!(
            cells
                .iter()
                .any(|c| c.column_key == "estimate" && c.value == "5")
        );

        // Upsert overwrites in place, no duplicate row.
        store
            .set_cell(scope, nt, "ISS-1", "estimate", "8", "number")
            .await
            .unwrap();
        let cells = store.get_for_row(scope, "ISS-1").await.unwrap();
        assert_eq!(cells.len(), 2);
        assert_eq!(
            cells
                .iter()
                .find(|c| c.column_key == "estimate")
                .unwrap()
                .value,
            "8"
        );

        store.clear_cell(scope, "ISS-1", "note").await.unwrap();
        let cells = store.get_for_row(scope, "ISS-1").await.unwrap();
        assert_eq!(cells.len(), 1);
        // Clearing a missing cell is a no-op, not an error.
        store.clear_cell(scope, "ISS-1", "note").await.unwrap();
    }

    #[tokio::test]
    async fn scope_isolates_and_batch_groups_by_row() {
        let store = mem_store("cc_scope").await;
        let nt = "t";
        store
            .set_cell("jira/a", nt, "K-1", "c", "va", "text")
            .await
            .unwrap();
        store
            .set_cell("jira/b", nt, "K-1", "c", "vb", "text")
            .await
            .unwrap();
        store
            .set_cell("jira/a", nt, "K-2", "c", "v2", "text")
            .await
            .unwrap();

        // Scope isolation: same row id under a different scope is invisible.
        let a = store.get_for_rows("jira/a", &["K-1", "K-2"]).await.unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(a["K-1"][0].value, "va");
        assert_eq!(a["K-2"][0].value, "v2");

        // Empty id set short-circuits.
        assert!(store.get_for_rows("jira/a", &[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn type_is_fixed_on_first_write() {
        let store = mem_store("cc_type_lock").await;
        let scope = "jira/acme";
        let nt = "jira:issue";
        // First write pins `estimate` to number.
        store
            .set_cell(scope, nt, "K-1", "estimate", "5", "number")
            .await
            .unwrap();
        // Same type on another row is fine.
        store
            .set_cell(scope, nt, "K-2", "estimate", "8", "number")
            .await
            .unwrap();
        // A conflicting type is rejected — store wins, error returned.
        let err = store
            .set_cell(scope, nt, "K-1", "estimate", "x", "text")
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("number"), "got: {err}");
    }

    #[tokio::test]
    async fn value_is_validated_against_type() {
        let store = mem_store("cc_validate").await;
        let scope = "jira/acme";
        let nt = "jira:issue";
        // Non-numeric value for a number column is rejected on first write.
        assert!(
            store
                .set_cell(scope, nt, "K-1", "est", "abc", "number")
                .await
                .is_err()
        );
        // Valid number is accepted and pins the type.
        store
            .set_cell(scope, nt, "K-1", "est", "42", "number")
            .await
            .unwrap();
        // An empty value is always accepted (represents "unset").
        store
            .set_cell(scope, nt, "K-2", "est", "", "number")
            .await
            .unwrap();
        // Bad RFC3339 datetime rejected; good one accepted.
        assert!(
            store
                .set_cell(scope, nt, "K-1", "due", "not-a-date", "datetime")
                .await
                .is_err()
        );
        store
            .set_cell(scope, nt, "K-1", "due", "2026-07-16T10:00:00Z", "datetime")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn json_accepts_any_json_value_and_rejects_the_rest() {
        let store = mem_store("cc_json").await;
        let scope = "jira/acme";
        let nt = "jira:issue";
        // A list is the shape this type exists for, but any JSON value fits.
        for (row, value) in [
            ("K-1", r#"["a", "b"]"#),
            ("K-2", r#"[{"dir": "x", "clean": true}]"#),
            ("K-3", r#"{"a": 1}"#),
            ("K-4", "42"),
        ] {
            store
                .set_cell(scope, nt, row, "tags", value, "json")
                .await
                .unwrap();
        }
        // Bare text is not JSON — the gap that let `json` in unvalidated.
        assert!(
            store
                .set_cell(scope, nt, "K-5", "tags", "a, b", "json")
                .await
                .is_err()
        );
        // Truncated JSON is caught too.
        assert!(
            store
                .set_cell(scope, nt, "K-5", "tags", r#"["a", "#, "json")
                .await
                .is_err()
        );
        // A json column still compares as text — the type describes the
        // payload, not an order.
        assert_eq!(
            store.columns(scope, nt).await.unwrap()[0].sort_kind(),
            not_yet_done_content::SortKind::Text
        );
    }

    #[tokio::test]
    async fn a_text_column_of_json_values_can_be_retyped_to_json() {
        let store = mem_store("cc_retype_json").await;
        let scope = "jira/acme";
        let nt = "jira:issue";
        store
            .set_cell(scope, nt, "K-1", "tags", r#"["k"]"#, "text")
            .await
            .unwrap();
        store
            .set_cell(scope, nt, "K-2", "tags", "kf1", "text")
            .await
            .unwrap();

        // `kf1` is a fine string but not JSON, so it blocks — and is named.
        let err = format!(
            "{}",
            store
                .retype_column(scope, nt, "tags", "json")
                .await
                .unwrap_err()
        );
        assert!(err.contains("K-2: `kf1`"), "got: {err}");

        // Quoted, it is a JSON string, and the retype goes through.
        store
            .set_cell(scope, nt, "K-2", "tags", r#""kf1""#, "text")
            .await
            .unwrap();
        assert_eq!(
            store
                .retype_column(scope, nt, "tags", "json")
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            store.columns(scope, nt).await.unwrap()[0].value_type,
            "json"
        );
    }

    #[tokio::test]
    async fn columns_discovery_is_scoped_by_node_type() {
        let store = mem_store("cc_discovery").await;
        let scope = "jira/acme";
        store
            .set_cell(scope, "jira:issue", "K-1", "estimate", "5", "number")
            .await
            .unwrap();
        store
            .set_cell(scope, "jira:issue", "K-2", "note", "hi", "text")
            .await
            .unwrap();
        store
            .set_cell(scope, "jira:bookmark", "B-1", "estimate", "abc", "text")
            .await
            .unwrap();

        let issue_cols = store.columns(scope, "jira:issue").await.unwrap();
        assert_eq!(issue_cols.len(), 2);
        // Stable column_key order.
        assert_eq!(issue_cols[0].key, "estimate");
        assert_eq!(issue_cols[0].value_type, "number");
        assert_eq!(issue_cols[1].key, "note");

        // Same key, different node type, is an independent definition — here
        // `estimate` is text on bookmarks, so a text value is fine.
        let bm_cols = store.columns(scope, "jira:bookmark").await.unwrap();
        assert_eq!(bm_cols.len(), 1);
        assert_eq!(bm_cols[0].value_type, "text");

        // Unknown node type → no columns.
        assert!(store.columns(scope, "jira:nope").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn retype_migrates_the_column_and_its_cells() {
        let store = mem_store("cc_retype_ok").await;
        let scope = "jira/acme";
        let nt = "jira:issue";
        for (row, rank) in [("K-1", "30"), ("K-2", "10"), ("K-3", "")] {
            store
                .set_cell(scope, nt, row, "rank", rank, "text")
                .await
                .unwrap();
        }

        // Only the non-empty cells count as migrated; an empty value fits any
        // type but is still carried over.
        assert_eq!(
            store
                .retype_column(scope, nt, "rank", "number")
                .await
                .unwrap(),
            3
        );

        // The schema now reports number …
        let cols = store.columns(scope, nt).await.unwrap();
        assert_eq!(cols[0].key, "rank");
        assert_eq!(cols[0].value_type, "number");
        // … and so does every cell (the denormalised copy moved with it).
        let cells = store.get_for_row(scope, "K-1").await.unwrap();
        assert_eq!(cells[0].value_type, "number");

        // A write in the new type is now accepted, the old one rejected.
        store
            .set_cell(scope, nt, "K-4", "rank", "40", "number")
            .await
            .unwrap();
        assert!(
            store
                .set_cell(scope, nt, "K-4", "rank", "40", "text")
                .await
                .is_err()
        );

        // Retyping to the type it already has is a no-op success.
        assert_eq!(
            store
                .retype_column(scope, nt, "rank", "number")
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn retype_names_every_value_that_does_not_fit() {
        let store = mem_store("cc_retype_bad").await;
        let scope = "jira/acme";
        let nt = "jira:issue";
        for (row, rank) in [("K-1", "30"), ("K-2", "n/a"), ("K-3", "later")] {
            store
                .set_cell(scope, nt, row, "rank", rank, "text")
                .await
                .unwrap();
        }

        let err = format!(
            "{}",
            store
                .retype_column(scope, nt, "rank", "number")
                .await
                .unwrap_err()
        );
        // Both offenders are named with id *and* value, so they can be fixed.
        assert!(err.contains("K-2: `n/a`"), "got: {err}");
        assert!(err.contains("K-3: `later`"), "got: {err}");
        // The row that was fine is not listed.
        assert!(!err.contains("K-1"), "got: {err}");

        // Nothing was written: the column is still text and still accepts text.
        assert_eq!(
            store.columns(scope, nt).await.unwrap()[0].value_type,
            "text"
        );
        store
            .set_cell(scope, nt, "K-4", "rank", "soon", "text")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn retype_rejects_an_unknown_column_or_type() {
        let store = mem_store("cc_retype_args").await;
        let scope = "jira/acme";
        let nt = "jira:issue";
        store
            .set_cell(scope, nt, "K-1", "rank", "30", "text")
            .await
            .unwrap();

        // Undefined column.
        assert!(
            store
                .retype_column(scope, nt, "nope", "number")
                .await
                .is_err()
        );
        // Defined column, but not on that node type.
        assert!(
            store
                .retype_column(scope, "jira:bookmark", "rank", "number")
                .await
                .is_err()
        );
        // Not a value type at all.
        let err = format!(
            "{}",
            store
                .retype_column(scope, nt, "rank", "integer")
                .await
                .unwrap_err()
        );
        assert!(err.contains("number"), "got: {err}");
    }

    #[tokio::test]
    async fn retype_leaves_a_same_named_column_of_another_type_alone() {
        let store = mem_store("cc_retype_sibling").await;
        let scope = "jira/acme";
        // One instance, two node types, same key — separate definitions that
        // nonetheless share the cell table. The per-cell type keeps them apart.
        store
            .set_cell(scope, "jira:issue", "K-1", "rank", "30", "text")
            .await
            .unwrap();
        store
            .set_cell(scope, "jira:bookmark", "B-1", "rank", "600", "duration")
            .await
            .unwrap();

        assert_eq!(
            store
                .retype_column(scope, "jira:issue", "rank", "number")
                .await
                .unwrap(),
            1
        );

        // The bookmark column and its cell are untouched.
        assert_eq!(
            store.columns(scope, "jira:bookmark").await.unwrap()[0].value_type,
            "duration"
        );
        assert_eq!(
            store.get_for_row(scope, "B-1").await.unwrap()[0].value_type,
            "duration"
        );
    }
}
