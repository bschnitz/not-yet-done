//! Generic content adapter abstraction.
//!
//! Provides a frontend-agnostic interface for connecting to remote content
//! systems (ticket trackers, wikis, databases). Each backend implements the
//! same trait interface so any frontend can work with any system uniformly.

#[cfg(any(test, feature = "mock"))]
pub mod mock;

pub mod anonymize;
pub mod auth;
pub mod children;
pub mod describe;
pub mod download;
pub mod grouping;
pub mod http_log;
pub mod link_route;
pub mod node_ref;
pub mod query_vars;
pub mod scaffold;
pub mod script_buffer;
pub mod slug;
pub mod sort_serde;
pub mod text;

pub use anonymize::{Anonymizer, StandardAnonymizer, anonymizing_factory};
pub use children::{BoxFuture, Child, check_rows, child_types, columns_for, list, list_subtree};
pub use describe::{
    HELP_ACTION_ID, TypeNode, child_types_of_type, help_action, is_builtin, level_actions,
    level_actions_for_type, render_level, render_level_for_type, run_builtin,
};
pub use grouping::{GroupBucket, GroupSpec};
pub use scaffold::{
    FileMeta as ScaffoldFileMeta, Selection as ScaffoldSelection, generate as generate_scaffold,
};

pub use auth::{
    AuthError, AuthFieldSpec, AuthOrchestrator, AuthSpec, CredentialBinding, CredentialProvider,
    InMemorySessionStore, MechanismSpec, ResolvedSession, SessionCachePolicy, SessionEntry,
    SessionStore,
};
pub use link_route::{LinkRoute, LinkRouteError};
pub use node_ref::{NodeRef, NodeRefParseError};

use async_trait::async_trait;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ContentError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("conflict: {0}")]
    Conflict(ConflictError),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("not supported: {0}")]
    NotSupported(String),

    #[error("{0}")]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

pub type Result<T> = std::result::Result<T, ContentError>;

#[derive(Debug)]
pub struct ConflictError {
    pub remote_version: String,
    pub remote_content: Option<Vec<u8>>,
    pub message: String,
}

impl std::fmt::Display for ConflictError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

// ---------------------------------------------------------------------------
// NodeType
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NodeType {
    /// Unique type identifier (e.g. "jira:issue", "wiki:page", "db:row").
    pub type_id: String,
    /// MIME type of the content body (e.g. "text/plain", "text/x-jira-wiki").
    pub mime_type: String,
    /// Editor syntax identifier (e.g. "markdown", "jira", "sql").
    pub syntax: Option<String>,
    /// File extension for temporary editor files (e.g. ".md", ".jira", ".sql").
    pub file_extension: String,
    /// Human-readable label (e.g. "Issue", "Page", "Row").
    pub display_name: String,
}

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metadata {
    pub fields: Vec<MetadataField>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetadataField {
    pub key: String,
    pub value: String,
    pub display_label: String,
    pub editable: bool,
    /// Allowed values (for dropdowns). None = free text.
    pub allowed_values: Option<Vec<String>>,
}

impl Metadata {
    /// Overwrite the `value` of an existing field by `key`, in place.
    ///
    /// A no-op if no field with that key exists (the caller asked to patch
    /// a column the row doesn't carry — silently ignored rather than
    /// inventing a field with an empty label). Lets a caller tweak a single
    /// column value on an existing summary without rebuilding the metadata.
    pub fn set_field(&mut self, key: &str, value: impl Into<String>) {
        if let Some(field) = self.fields.iter_mut().find(|f| f.key == key) {
            field.value = value.into();
        }
    }
}

#[cfg(test)]
mod metadata_tests {
    use super::*;

    fn field(key: &str, value: &str) -> MetadataField {
        MetadataField {
            key: key.into(),
            value: value.into(),
            display_label: key.into(),
            editable: false,
            allowed_values: None,
        }
    }

    #[test]
    fn set_field_overwrites_existing_value_in_place() {
        let mut md = Metadata {
            fields: vec![field("marker", ""), field("label", "keep")],
        };
        md.set_field("marker", "⏱");
        assert_eq!(md.fields[0].value, "⏱");
        // Other fields untouched; no field added.
        assert_eq!(md.fields[1].value, "keep");
        assert_eq!(md.fields.len(), 2);
    }

    #[test]
    fn set_field_is_noop_for_absent_key() {
        let mut md = Metadata {
            fields: vec![field("marker", "")],
        };
        md.set_field("missing", "x");
        assert_eq!(md.fields.len(), 1);
        assert_eq!(md.fields[0].value, "");
    }
}

#[cfg(test)]
mod apply_sort_tests {
    use super::*;

    fn col(key: &str, kind: SortKind) -> ColumnSchema {
        let value_type = match kind {
            SortKind::Text => "text",
            SortKind::Number => "number",
            SortKind::DateTime => "datetime",
        };
        ColumnSchema::new(key, key).typed(value_type)
    }

    fn key(column: &str, direction: SortDirection) -> SortKey {
        SortKey {
            column: column.into(),
            direction,
        }
    }

    /// Build a summary whose `label` is the id and which carries the given
    /// `(key, value)` metadata fields.
    fn summary(id: &str, fields: &[(&str, &str)]) -> NodeSummary {
        NodeSummary {
            id: id.into(),
            label: id.into(),
            node_type: NodeType {
                type_id: "test".into(),
                mime_type: "text/plain".into(),
                syntax: None,
                file_extension: String::new(),
                display_name: "Test".into(),
            },
            metadata: Metadata {
                fields: fields
                    .iter()
                    .map(|(k, v)| MetadataField {
                        key: (*k).into(),
                        value: (*v).into(),
                        display_label: (*k).into(),
                        editable: false,
                        allowed_values: None,
                    })
                    .collect(),
            },
            has_children: None,
        }
    }

    fn ids(items: &[NodeSummary]) -> Vec<&str> {
        items.iter().map(|s| s.id.as_str()).collect()
    }

    #[test]
    fn datetime_sort_orders_chronologically_both_directions() {
        let cols = [col("started", SortKind::DateTime)];
        let mut items = vec![
            summary("b", &[("started", "2026-06-02T10:00:00Z")]),
            summary("a", &[("started", "2026-06-01T10:00:00Z")]),
            summary("c", &[("started", "2026-06-03T10:00:00Z")]),
        ];
        let applied = apply_sort(&mut items, &[key("started", SortDirection::Asc)], &cols);
        assert_eq!(ids(&items), ["a", "b", "c"]);
        assert_eq!(applied, vec![key("started", SortDirection::Asc)]);

        apply_sort(&mut items, &[key("started", SortDirection::Desc)], &cols);
        assert_eq!(ids(&items), ["c", "b", "a"]);
    }

    #[test]
    fn number_sort_is_numeric_not_lexical() {
        let cols = [col("duration", SortKind::Number)];
        let mut items = vec![
            summary("x", &[("duration", "100")]),
            summary("y", &[("duration", "9")]),
            summary("z", &[("duration", "60")]),
        ];
        apply_sort(&mut items, &[key("duration", SortDirection::Asc)], &cols);
        // 9 < 60 < 100 numerically (lexically "100" would sort before "60").
        assert_eq!(ids(&items), ["y", "z", "x"]);
    }

    #[test]
    fn unparseable_typed_cells_sort_to_the_end_ascending() {
        let cols = [col("ended", SortKind::DateTime)];
        let mut items = vec![
            summary("running", &[("ended", "running")]),
            summary("late", &[("ended", "2026-06-02T10:00:00Z")]),
            summary("early", &[("ended", "2026-06-01T10:00:00Z")]),
        ];
        apply_sort(&mut items, &[key("ended", SortDirection::Asc)], &cols);
        assert_eq!(ids(&items), ["early", "late", "running"]);
    }

    #[test]
    fn a_label_backed_column_sorts_from_its_field() {
        // `description`/`summary`-style columns whose value is *also* the
        // label. They sort because the adapter carries the field as well —
        // not because the sort reaches into the label when a field is
        // missing.
        let cols = [col("description", SortKind::Text)];
        let mut items = vec![
            summary("Banana", &[("description", "Banana")]),
            summary("apple", &[("description", "apple")]),
        ];
        apply_sort(&mut items, &[key("description", SortDirection::Asc)], &cols);
        // Case-insensitive: "apple" before "Banana".
        assert_eq!(ids(&items), ["apple", "Banana"]);
    }

    #[test]
    fn multi_key_sort_breaks_ties_with_later_keys_and_is_stable() {
        let cols = [
            col("status", SortKind::Text),
            col("priority", SortKind::Number),
        ];
        let mut items = vec![
            summary("a", &[("status", "open"), ("priority", "2")]),
            summary("b", &[("status", "open"), ("priority", "1")]),
            summary("c", &[("status", "done"), ("priority", "1")]),
        ];
        let applied = apply_sort(
            &mut items,
            &[
                key("status", SortDirection::Asc),
                key("priority", SortDirection::Asc),
            ],
            &cols,
        );
        // Primary key status asc (done < open), secondary priority asc.
        assert_eq!(ids(&items), ["c", "b", "a"]);
        assert_eq!(applied.len(), 2);
    }

    #[test]
    fn unknown_columns_are_dropped_from_applied() {
        let cols = [col("started", SortKind::DateTime)];
        let mut items = vec![summary("a", &[("started", "2026-06-01T10:00:00Z")])];
        let applied = apply_sort(
            &mut items,
            &[
                key("nonsense", SortDirection::Asc),
                key("started", SortDirection::Desc),
            ],
            &cols,
        );
        // Only the recognised key is reported as applied.
        assert_eq!(applied, vec![key("started", SortDirection::Desc)]);
    }

    #[test]
    fn a_missing_cell_sorts_as_empty_and_never_as_the_label() {
        // `label` is the id here, so a fallback to it would order z/a/m —
        // exactly the wrong answer this used to give.
        let cols = [col("status", SortKind::Text)];
        let mut items = vec![
            summary("z", &[("status", "open")]),
            summary("a", &[]),
            summary("m", &[("status", "done")]),
        ];
        apply_sort(&mut items, &[key("status", SortDirection::Asc)], &cols);
        assert_eq!(ids(&items), ["a", "m", "z"]);
    }

    #[test]
    fn a_column_that_is_not_in_rows_is_not_sorted_on() {
        // Sortable, but the value lives server-side only: locally there is
        // nothing to compare, so the key is dropped rather than guessed at.
        let cols = [ColumnSchema::new("summary", "Summary").not_in_rows()];
        let mut items = vec![summary("b", &[]), summary("a", &[])];
        let applied = apply_sort(&mut items, &[key("summary", SortDirection::Asc)], &cols);
        assert!(applied.is_empty());
        assert_eq!(ids(&items), ["b", "a"]);
    }

    #[test]
    fn an_unsortable_column_is_dropped() {
        let cols = [ColumnSchema::new("notes", "Notes").unsortable()];
        let mut items = vec![
            summary("b", &[("notes", "x")]),
            summary("a", &[("notes", "a")]),
        ];
        let applied = apply_sort(&mut items, &[key("notes", SortDirection::Asc)], &cols);
        assert!(applied.is_empty());
        assert_eq!(ids(&items), ["b", "a"]);
    }

    /// The predicate `apply_sort` sorts by, asked without sorting. This is
    /// what lets a caller see that taking a sort over locally would *lose* a
    /// key the backend served server-side — a column with no cell in the rows
    /// is exactly that case — instead of finding out by having already
    /// reordered the list.
    #[test]
    fn honoured_keys_are_the_ones_a_local_sort_could_compare() {
        let cols = [
            ColumnSchema::new("rank", "Rank"),
            ColumnSchema::new("summary", "Summary").not_in_rows(),
            ColumnSchema::new("notes", "Notes").unsortable(),
        ];
        let asked = [
            key("summary", SortDirection::Asc),
            key("notes", SortDirection::Asc),
            key("rank", SortDirection::Asc),
            key("unknown", SortDirection::Asc),
        ];
        let honoured = honoured_sort_keys(&asked, &cols);
        assert_eq!(honoured.len(), 1);
        assert_eq!(honoured[0].column, "rank");

        // And it agrees with what `apply_sort` actually applies — one rule,
        // asked two ways.
        let mut items = vec![
            summary("b", &[("rank", "2")]),
            summary("a", &[("rank", "1")]),
        ];
        assert_eq!(apply_sort(&mut items, &asked, &cols), honoured);
    }

    #[test]
    fn sort_kind_follows_the_value_type() {
        assert_eq!(ColumnSchema::new("a", "A").sort_kind(), SortKind::Text);
        assert_eq!(
            ColumnSchema::new("a", "A").typed("number").sort_kind(),
            SortKind::Number
        );
        // Durations are integer seconds — numeric, not lexical.
        assert_eq!(
            ColumnSchema::new("a", "A").typed("duration").sort_kind(),
            SortKind::Number
        );
        assert_eq!(
            ColumnSchema::new("a", "A").typed("datetime").sort_kind(),
            SortKind::DateTime
        );
    }
}

// ---------------------------------------------------------------------------
// ListParams / ListResult / NodeSummary
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ListParams {
    /// Which child type to list.
    pub node_type: NodeType,
    /// Opaque query string in the backend's native language (JQL, SQL, etc.).
    pub query: Option<String>,
    /// Requested sort. Adapters honour what they can and report what was
    /// actually applied via [`ListResult::applied_sort`]. Empty = adapter
    /// chooses (typically a stable backend default).
    pub sort: Vec<SortKey>,
    /// Pagination request. `None` = adapter default page (often the first
    /// page of a sensible size). Adapters that cannot paginate ignore this.
    pub page: Option<PageRequest>,
    /// If true, download full content for each item (batch).
    /// If false, return NodeSummary only (lazy).
    pub download: bool,
    /// The pane's active grouping, for adapters that group **adapter-side**
    /// (capability [`AdapterCapabilities::group_by_via_adapter`]). Tree views
    /// can't group engine-side — the adapter owns the per-bucket fold — so
    /// the engine passes the active `group_by` along on the root `list()`
    /// and the adapter returns one bucket node per group as the root level.
    /// `None` = ungrouped (or an engine-side-grouping flat view); adapters
    /// without the capability never see `Some` and may ignore the field.
    pub group_by: Option<GroupSpec>,
}

/// One sort key in a (potentially multi-column) sort spec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SortKey {
    /// Column key as declared by [`children::columns_for`].
    pub column: String,
    pub direction: SortDirection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

/// Pagination request: a half-open window `[offset, offset + limit)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRequest {
    pub offset: u32,
    pub limit: u32,
}

/// Pagination state of a list result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageInfo {
    pub offset: u32,
    pub limit: u32,
    /// Total matching items, if the backend reports it.
    pub total: Option<u64>,
    pub has_next: bool,
    pub has_prev: bool,
}

/// How a column's values compare. Derived from a column's
/// [`ColumnSchema::value_type`] (see [`ColumnSchema::sort_kind`]) so the
/// generic [`apply_sort`] helper knows whether to compare cells lexically,
/// numerically, or as timestamps — only the adapter knows what a given column
/// actually holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortKind {
    /// Lexicographic (case-insensitive) string compare. The fallback for any
    /// free-text column.
    #[default]
    Text,
    /// Parse cells as `f64` and compare numerically. Unparseable cells
    /// (empty, non-numeric) sort to the end of an ascending run.
    Number,
    /// Parse cells as RFC 3339 timestamps and compare chronologically.
    /// Unparseable cells (empty, sentinels like `"running"`) sort to the
    /// end of an ascending run.
    DateTime,
}

/// The one declaration of a column: what it is called, how its values are
/// typed, whether every listed row carries it, and whether the adapter can
/// sort on it.
///
/// Adapters declare their own list columns per child type
/// ([`children::Child::columns`]); a decorator adds columns that aren't native
/// content but are carried, typed, alongside it via
/// [`ContentAdapter::describe_columns`] (today: the locally-stored custom
/// columns). Both channels speak this type, and
/// [`children::columns_for`] unions them — front-ends see one list.
///
/// This is also the framework's one channel for a **type flowing backend →
/// front-end**: row metadata is otherwise stringly-typed. A front-end merges
/// this schema over its own layout config — `value_type` is authoritative for
/// how a value is validated, compared and rendered, while width/order/
/// visibility stay a front-end concern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnSchema {
    /// Stable column key — the metadata-field key the value is carried under
    /// (when [`in_rows`](Self::in_rows)), the key referenced from
    /// [`SortKey::column`], and the view config's column `key`.
    pub key: String,
    /// Optional human label. `None` lets the front-end fall back to its own
    /// (e.g. the view YAML's `label:`).
    pub label: Option<String>,
    /// Canonical value type: `text` / `number` / `duration` / `datetime`.
    /// `text` is the permissive default; the others imply parsing/validation,
    /// typed comparison (see [`Self::sort_kind`]) and type-aware rendering
    /// (right-aligned number, formatted duration, …).
    pub value_type: String,
    /// Allowed values when the column is a closed set (drives a select on the
    /// edit side). Empty = free value.
    pub options: Vec<String>,
    /// The value is carried as a [`MetadataField`] under [`key`](Self::key) in
    /// **every** listed row. That makes the column locally filterable and
    /// locally sortable — and it is a promise the generic list path checks
    /// (see [`children::check_rows`]).
    ///
    /// `false` for a column that only exists on the detail projection, or one
    /// the backend can order by without ever shipping the value.
    pub in_rows: bool,
    /// The adapter undertakes to honour a [`SortKey`] on this column. *How* is
    /// its own business — a server-side `ORDER BY` or a local [`apply_sort`];
    /// the framework only needs to know whether to offer the column.
    ///
    /// Independent of [`in_rows`](Self::in_rows): a server-side sort needs no
    /// local cell, and a column present in every row may still not be
    /// sortable.
    pub sortable: bool,
}

impl ColumnSchema {
    /// A text column carried in every row and sortable — the common case for
    /// an adapter's own list columns. Refine with the builders below.
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: Some(label.into()),
            value_type: "text".into(),
            options: Vec::new(),
            in_rows: true,
            sortable: true,
        }
    }

    /// Set the canonical value type (`text` / `number` / `duration` /
    /// `datetime`).
    pub fn typed(mut self, value_type: impl Into<String>) -> Self {
        self.value_type = value_type.into();
        self
    }

    /// Restrict the column to a closed set of values.
    pub fn with_options(mut self, options: Vec<String>) -> Self {
        self.options = options;
        self
    }

    /// The column can be listed and filtered but not sorted on.
    pub fn unsortable(mut self) -> Self {
        self.sortable = false;
        self
    }

    /// The value is *not* carried in list rows — the column is sortable
    /// server-side only, or exists on the detail projection alone.
    pub fn not_in_rows(mut self) -> Self {
        self.in_rows = false;
        self
    }

    /// How values in this column compare, derived from
    /// [`value_type`](Self::value_type). Unknown types compare as text — the
    /// permissive default that can only mis-order, never fail.
    pub fn sort_kind(&self) -> SortKind {
        match self.value_type.as_str() {
            // Durations are stored as integer seconds, so they compare
            // numerically.
            "number" | "duration" => SortKind::Number,
            "datetime" => SortKind::DateTime,
            _ => SortKind::Text,
        }
    }

    /// The label a front-end shows, falling back to the key.
    pub fn display_label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.key)
    }

    /// Map this described column to a form field for an
    /// [`InputSpec::ColumnForm`] editor, shared by every front-end so the
    /// mapping stays uniform: a closed `options` set becomes a select,
    /// anything else a text field the backend validates against `value_type`.
    /// Always optional — an empty value clears the cell. The type is hinted in
    /// the label for the non-`text` kinds so the user knows the expected format.
    pub fn to_form_field(&self) -> FormFieldSpec {
        let base = self.label.clone().unwrap_or_else(|| self.key.clone());
        if self.options.is_empty() {
            let label = if self.value_type == "text" {
                base
            } else {
                format!("{base} ({})", self.value_type)
            };
            FormFieldSpec::text(self.key.clone(), label).optional()
        } else {
            FormFieldSpec::select(self.key.clone(), base, self.options.clone()).optional()
        }
    }
}

/// The value a row carries for `column`, or `None` when the row has no such
/// cell.
///
/// **The** cell lookup — sorting ([`apply_sort`]) and local filtering
/// (`not_yet_done_extended_query`) both go through it, so "filter and sort see
/// the same value" holds by shared code rather than by comment. An absent
/// field and a blank one are the same thing: no value. There is no fallback to
/// any other slot of the summary; a column whose value lives in the row is
/// declared [`ColumnSchema::in_rows`] and must be there.
pub fn cell<'a>(summary: &'a NodeSummary, column: &str) -> Option<&'a str> {
    summary
        .metadata
        .fields
        .iter()
        .find(|f| f.key == column)
        .map(|f| f.value.as_str())
        .filter(|v| !v.trim().is_empty())
}

/// Sort `items` in place by a multi-column `sort` spec, using `columns` to
/// resolve each requested key to a [`SortKind`].
///
/// This is the generic engine behind the `S` (sort) action: an adapter
/// declares its columns via [`children::Child::columns`] and calls this from
/// its `list()` before any grouping, so the within-group order follows the
/// requested item sort. The frontend stays adapter-agnostic — it just forwards
/// [`SortKey`]s and renders whatever the adapter reports as applied.
///
/// A cell's value is [`cell`]; a row without that cell sorts as empty. Keys
/// naming a column that is unknown, not [`sortable`](ColumnSchema::sortable),
/// or not [`in_rows`](ColumnSchema::in_rows) are **skipped** — this function
/// only sorts on values it can actually read, and never guesses one. The sort
/// is **stable** and applied least-significant-key-first, so a multi-key spec
/// orders by the first key with later keys breaking ties. Returns the subset
/// of `sort` keys that were honoured (suitable for
/// [`ListResult::applied_sort`]).
pub fn apply_sort(
    items: &mut [NodeSummary],
    sort: &[SortKey],
    columns: &[ColumnSchema],
) -> Vec<SortKey> {
    let resolved = resolve_sort(sort, columns);

    // Apply keys least-significant first; a stable sort preserves the order
    // established by earlier (more significant) passes for equal elements.
    for (key, kind) in resolved.iter().rev() {
        items.sort_by(|a, b| {
            let va = cell(a, &key.column).unwrap_or("");
            let vb = cell(b, &key.column).unwrap_or("");
            let ord = compare_cells(va, vb, *kind);
            match key.direction {
                SortDirection::Asc => ord,
                SortDirection::Desc => ord.reverse(),
            }
        });
    }

    resolved.into_iter().map(|(k, _)| k.clone()).collect()
}

/// Pair each sort key with the [`SortKind`] to compare it under, dropping the
/// keys no local comparison can serve. The one place that rule lives.
fn resolve_sort<'a>(sort: &'a [SortKey], columns: &[ColumnSchema]) -> Vec<(&'a SortKey, SortKind)> {
    sort.iter()
        .filter_map(|k| {
            columns
                .iter()
                .find(|c| c.key == k.column && c.sortable && c.in_rows)
                .map(|c| (k, c.sort_kind()))
        })
        .collect()
}

/// The subset of `sort` that [`apply_sort`] would honour against `columns`.
///
/// Same rule, asked without doing the work: a caller deciding *whether* to
/// take a sort over locally needs to know what a local sort would achieve
/// before it reorders anything it might have to put back.
pub fn honoured_sort_keys(sort: &[SortKey], columns: &[ColumnSchema]) -> Vec<SortKey> {
    resolve_sort(sort, columns)
        .into_iter()
        .map(|(k, _)| k.clone())
        .collect()
}

/// Compare two cell strings under a [`SortKind`]. Unparseable values under a
/// typed kind (Number/DateTime) sort *after* parseable ones in ascending
/// order, so sentinels like an empty `ended` or a literal `"running"` land at
/// the end rather than at an arbitrary position.
fn compare_cells(a: &str, b: &str, kind: SortKind) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match kind {
        SortKind::Text => a.to_lowercase().cmp(&b.to_lowercase()),
        SortKind::Number => {
            let pa = a.trim().parse::<f64>().ok();
            let pb = b.trim().parse::<f64>().ok();
            match (pa, pb) {
                (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
        }
        SortKind::DateTime => {
            let pa = chrono::DateTime::parse_from_rfc3339(a.trim()).ok();
            let pb = chrono::DateTime::parse_from_rfc3339(b.trim()).ok();
            match (pa, pb) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeSummary {
    pub id: String,
    pub label: String,
    pub node_type: NodeType,
    pub metadata: Metadata,
    /// Live expandability hint from the adapter. `None` = unknown
    /// (frontend falls back to the static "does this level have a
    /// tree-continuing child?" config check). `Some(true)` / `Some(false)`
    /// override that check, letting adapters whose backend reports
    /// per-row child counts (e.g. Confluence's `childTypes.page.value`)
    /// distinguish actually-leaf pages from expandable ones inside a
    /// `recursive: true` ChildDef where the static check would always
    /// say "expandable".
    pub has_children: Option<bool>,
}

pub struct ListResult {
    pub items: Vec<NodeSummary>,
    /// What sort the adapter actually applied. May differ from
    /// [`ListParams::sort`] (unsupported columns dropped, default added).
    pub applied_sort: Vec<SortKey>,
    /// Pagination state of this result, if the adapter paginates.
    pub page: Option<PageInfo>,
    /// Whether batch download is available for this node type.
    pub batch_download_available: bool,
    /// If download=true was requested, the full nodes.
    pub downloaded: Vec<Box<dyn Node>>,
}

// ---------------------------------------------------------------------------
// Subtree / SubtreeNode (eager multi-level expansion)
// ---------------------------------------------------------------------------

/// One **level** of an eagerly-expanded tree: the nodes at this level plus
/// that level's pagination state. Mirrors [`ListResult`] (`items` + `page`)
/// but recursively — each [`SubtreeNode`] carries its own children as a
/// nested `Subtree`. Returned by [`Node::list_subtree`].
///
/// The split (a level owns its `page`, a node owns its `children`) lets the
/// frontend ingest the whole structure into its per-parent tree cache in one
/// pass: one cache slot per node's `children` level, each with its own page.
#[derive(Clone, Debug, Default)]
pub struct Subtree {
    pub items: Vec<SubtreeNode>,
    /// Pagination state of THIS level, if the adapter paginates. Local
    /// adapters (the only ones that opt into eager subtrees) load
    /// all-or-nothing, so this is typically `None` for them.
    pub page: Option<PageInfo>,
}

/// A single node in an eagerly-expanded tree.
///
/// `children.items` is empty when the node is a genuine leaf **or** the
/// requested depth limit was reached (i.e. "not expanded here"). The two
/// cases are distinguished by `summary.has_children`: `Some(false)` = real
/// leaf, otherwise depth-limited and the frontend may still lazy-expand it
/// on demand via the ordinary cascade.
#[derive(Clone, Debug)]
pub struct SubtreeNode {
    /// Carries `id`, `label`, `node_type`, `metadata`, `has_children`.
    pub summary: NodeSummary,
    /// This node's already-expanded children (one tree level deeper).
    pub children: Subtree,
}

// ---------------------------------------------------------------------------
// QueryVariable (saved-query parameter binding)
// ---------------------------------------------------------------------------

/// A single variable extracted from a raw saved-query string by the adapter.
///
/// Adapters define their own inline syntax (e.g. `${name:default}` for
/// Taiga); the frontend stays syntax-agnostic and only knows that the
/// adapter reports a list of variables to gather before the query can be
/// rendered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryVariable {
    /// Variable name (as referenced in the raw query).
    pub name: String,
    /// Default value parsed out of the raw query. `None` means the
    /// adapter could not extract a default — the frontend must require
    /// input from the user.
    pub default: Option<String>,
}

// ---------------------------------------------------------------------------
// AdapterCapabilities
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct AdapterCapabilities {
    pub supports_create: bool,
    pub supports_delete: bool,
    pub supports_search: bool,
    pub supports_batch_download: bool,
    /// Whether list() can return total counts.
    pub supports_total_count: bool,
    /// Whether the adapter computes subtree-cumulated values for tree nodes
    /// (M4 tree-fold aggregation). When `true`, a `tree_aggregate` column may
    /// toggle between a node's own value and the adapter's cumulated field;
    /// the TUI never folds the (lazy-loaded) tree itself. Adapters that cannot
    /// cumulate leave this `false` and the `toggle_tree_aggregate` action and
    /// hint stay hidden.
    pub supports_tree_aggregation: bool,
    /// Whether the active query applies to child node lists at every
    /// depth, not just the root `list()`. When `true`, the engine
    /// threads the pane's active (rendered) query into tree-expansion
    /// and drill-down `list()` calls so a filtered tree stays filtered
    /// below the root. Adapters whose child node types carry *different*
    /// query semantics than their parent (e.g. Jira epic → story, where
    /// the parent's JQL must not leak onto the children) leave this
    /// `false` — child loads then receive `query: None` as before. Only
    /// homogeneous trees (the task forest: `task:item` → `task:item`,
    /// one `FilterExpr` valid at every depth) opt in.
    pub propagates_query_to_subtree: bool,
    /// Whether the adapter can group its **tree** root level itself when the
    /// engine passes the active grouping in [`ListParams::group_by`]: it then
    /// returns one bucket node per group (each holding that bucket's folded
    /// subtree) instead of the plain root rows. Engine-side grouping (M3)
    /// only partitions flat lists; a tree's per-bucket aggregates must be
    /// folded by whoever owns the data, so tree views group only through
    /// this capability. With it set, the `cycle_grouping` / group-menu keys
    /// become claimable in tree mode and trigger a *reload* (the adapter
    /// must re-list) instead of an in-memory rebuild.
    pub group_by_via_adapter: bool,
    /// Whether the adapter can build a whole multi-level subtree in one
    /// [`Node::list_subtree`] call without per-level round-trips. Local
    /// adapters that hold the full forest in memory (Tasks, Trackings) set
    /// this `true`; the engine then expands a tree's initial / reloaded
    /// state with a single eager call instead of the per-node `list()`
    /// cascade (which is O(N²) in tree-rebuilds for `expand_depth: all`).
    ///
    /// Remote adapters leave this `false` on purpose: a blocking
    /// `list_subtree` over a slow backend would freeze the UI, whereas the
    /// progressive cascade keeps it responsive and shows levels as they
    /// arrive. They still get a correct (but synchronous, round-trip-per-
    /// level) default `list_subtree` impl — it is simply never driven by the
    /// engine for them. Interactive single-node expansion always uses the
    /// cascade regardless of this flag.
    pub supports_eager_subtree: bool,
    /// Whether the adapter offers an editable, adapter-native query
    /// script per node — the `Q` editor and the `q` script menu.
    ///
    /// The host used to gate those keys on `adapter_type() == "postgres"`,
    /// which meant a second SQL backend could not have them without the
    /// host learning its name. Adapters that set this must also provide a
    /// [`ScriptStore`] (via [`ContentAdapter::script_store`]) whose
    /// node-scoped half is populated; the levels that own scripts are
    /// declared in the view config, not here.
    pub supports_node_query_editor: bool,
    /// Whether this adapter's node ids are *not* stable across reloads
    /// and processes.
    ///
    /// The link/mark features (`f`, `m`/`p`, saved `NodeRef`s) address
    /// nodes by id, which only works if an id still means the same row
    /// later. Adapters whose ids are positional or per-query — a SQL
    /// result row is `qrow:<n>`, meaningful only inside the query that
    /// produced it — set this and the host reports "no stable ids"
    /// instead of following a ref that would land on an unrelated row.
    ///
    /// Negated on purpose: stable ids are the norm (every ticket, task
    /// and page adapter has them), so the `Default` must be "stable" or
    /// adding this field would silently switch linking off everywhere.
    /// Only the exceptions opt in.
    pub unstable_node_ids: bool,
}

// ---------------------------------------------------------------------------
// Custom queries (free-form, adapter-native query strings)
// ---------------------------------------------------------------------------

/// Addressing data threaded into [`ContentAdapter::execute_custom_query`].
/// The map is opaque to the caller — each adapter declares the keys it
/// needs (postgres reads `database`; future adapters may read other keys).
/// The optional `page` is honoured by adapters that can paginate
/// resultset queries (postgres wraps a SELECT with `LIMIT/OFFSET`).
///
/// `cursor` is the cursor-pagination opt-in. When `Some(_)` the adapter
/// runs the cursor path instead of the regular execute; the variants
/// distinguish the three lifecycle steps (open / fetch next / close).
/// Adapters that don't implement cursor pagination return
/// `NotSupported` for `Some(Open)`.
#[derive(Clone, Debug, Default)]
pub struct CustomQueryContext {
    pub fields: std::collections::HashMap<String, String>,
    pub page: Option<PageRequest>,
    pub cursor: Option<CursorIntent>,
}

/// One step in the cursor-pagination lifecycle. The TUI threads these
/// through [`ContentAdapter::execute_custom_query`] when a view uses
/// `PaginationMode::Cursor`.
#[derive(Clone, Debug)]
pub enum CursorIntent {
    /// Open a fresh server-side cursor for the query and return the
    /// first page. The resulting [`CustomQueryResult::cursor_id`]
    /// carries the opaque handle the caller must pass back on the
    /// next steps.
    Open,
    /// Fetch the next page from an already-open cursor.
    Continue { cursor_id: String },
    /// Tear down a cursor. Returns an empty result (no rows, no
    /// `cursor_id`).
    Close { cursor_id: String },
}

impl CustomQueryContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }

    pub fn with_page(mut self, page: PageRequest) -> Self {
        self.page = Some(page);
        self
    }

    pub fn with_cursor(mut self, cursor: CursorIntent) -> Self {
        self.cursor = Some(cursor);
        self
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }
}

/// Result of a free-form custom query.
///
/// For result-set queries (SELECT…) `items` carries the rows and
/// `status` is `None`. For non-resultset statements (UPDATE/DELETE/INSERT
/// without RETURNING, or DDL) `items` is empty and `status` carries a
/// human-readable summary like `"5 row(s) affected"`.
///
/// When several `;`-separated statements are executed in one call, only
/// the **last** statement's output is reflected here.
pub struct CustomQueryResult {
    /// Column names of the last result set, in order. Empty when the
    /// last statement had no result set.
    pub columns: Vec<String>,
    /// Rows of the last result set, mapped to `NodeSummary`s with one
    /// `MetadataField` per column. IDs are `qrow:<row_index>`.
    pub items: Vec<NodeSummary>,
    /// Status text for non-resultset statements. `None` when the last
    /// statement returned a result set.
    pub status: Option<String>,
    /// Pagination state when the adapter wrapped the last statement
    /// with `LIMIT/OFFSET`. `None` when no pagination was applied
    /// (non-SELECT, multi-statement, or `context.page` was `None`).
    pub page: Option<PageInfo>,
    /// Opaque cursor handle when the adapter opened or continued a
    /// server-side cursor for this query. The caller stores this and
    /// threads it back on the next page via
    /// [`CursorIntent::Continue`]. `None` when no cursor is in play
    /// (LIMIT/OFFSET path, full-load, or cursor was just closed).
    pub cursor_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Tree search
// ---------------------------------------------------------------------------

/// Result set for an adapter-side tree search.
///
/// Returned by [`ContentAdapter::search_in_tree`] — used by tree-mode
/// views to perform a server-side search (e.g. CQL for Confluence,
/// JQL `text ~` for Jira) and locate each hit inside the lazy tree
/// without forcing the user to open every parent by hand.
///
/// Hits are pre-sorted in tree-render order by the adapter (e.g. for
/// Confluence: configured space order, then ancestor DFS, then title),
/// so the caller can step `current` forward / backward without
/// re-sorting.
#[derive(Debug, Clone)]
pub struct TreeSearchResults {
    pub hits: Vec<TreeFindHit>,
    /// The server reported more matches than `hits.len()` (we hit the
    /// per-call limit). UI surfaces this so the user knows to refine
    /// the query for completeness.
    pub truncated: bool,
}

/// One hit from [`ContentAdapter::search_in_tree`].
///
/// `path` is the chain of node ids from the tree root down to and
/// including the hit itself, ready to be fed into a lazy-expand
/// driver. For Confluence: `["SPACE", "ancestor_id_1", …, "page_id"]`.
#[derive(Debug, Clone)]
pub struct TreeFindHit {
    pub path: Vec<String>,
    /// Display title for status-bar feedback.
    pub label: String,
    /// Root-level grouping key (e.g. Confluence space key). Empty when
    /// the adapter has no such notion. Cosmetic only — `path[0]` is
    /// the canonical addressing.
    pub space_key: String,
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// An action a node exposes to the user (edit, delete, transition, …).
///
/// Adapters declare their actions; the TUI dispatches them along two paths:
///
/// 1. **Menu/picker path (existing)** — the user opens the action menu and
///    selects one. The TUI honours the [`InputSpec`] (editor / picker /
///    file-picker / none) and the result routes back through
///    [`Node::execute`] returning [`ActionOutcome`].
/// 2. **Shortcut/dispatch path (new)** — the YAML view config binds a key
///    directly to an action `id`. The TUI calls [`Node::invoke_action`]
///    which returns an [`ActionDispatch`] describing what UI flow to start
///    (open editor, execute query, create child, …).
///
/// The action carries no bar-placement hint: whether a shortcut-bound action
/// surfaces in the top action bar or the bottom status bar is a pure TUI
/// concern, derived from the action's [`InputSpec`] and id (activatable →
/// action bar, fire-and-forget → status bar), not declared here.
#[derive(Clone, Debug)]
pub struct NodeAction {
    /// Stable identifier referenced from view config (e.g. `"edit_full"`,
    /// `"edit_with_comments"`, `"transition"`, `"delete"`).
    pub id: String,
    /// Default human-readable label. The view config may override.
    pub label: String,
    /// What kind of input the action needs from the user.
    pub input: InputSpec,
}

impl NodeAction {
    /// Build an action from its id, label, and input shape.
    pub fn new(id: impl Into<String>, label: impl Into<String>, input: InputSpec) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            input,
        }
    }
}

/// Shape of input an action consumes.
#[derive(Clone, Debug)]
pub enum InputSpec {
    /// No user input — fire and forget (e.g. delete, refresh-style).
    None,
    /// Multi-line editor buffer. Adapter renders the template in
    /// [`Node::prepare`] and parses the result inside [`Node::execute`].
    Editor,
    /// Picker over an option list. Options come from
    /// [`Node::picker_options`] when triggered.
    Picker,
    /// File picker — the TUI opens its file-picker widget and returns the
    /// chosen path(s). `multi: false` constrains the user to one file.
    FilePicker { multi: bool },
    /// Structured multi-field form. The TUI renders the [`FormFieldSpec`]
    /// list generically (text / select / toggle widgets), collects the
    /// values, and delivers them via [`ActionInput::Form`]. Initial values
    /// for an edit flow come from [`Node::form_prep`]; static fallbacks
    /// come from each field's [`FormFieldSpec::default`].
    Form { fields: Vec<FormFieldSpec> },
    /// A form over a **dynamic, backend-owned column set** rather than a static
    /// field list — today: the lib-owned custom columns. A front-end builds the
    /// fields one of two ways and prefills current values via
    /// [`Node::form_prep`]:
    ///
    /// * from [`ContentAdapter::describe_columns`] (mapping each
    ///   [`ColumnSchema`] with [`ColumnSchema::to_form_field`]) and delivering
    ///   [`ActionInput::Form`] — the backend resolves each type from its own
    ///   schema, so only already-defined columns are editable; or
    /// * from the front-end's **own typed column config** (e.g. the TUI view
    ///   YAML's `source: custom` columns with their `kind:`) and delivering
    ///   [`ActionInput::ColumnForm`], carrying each cell's `value_type` so the
    ///   backend can create the column on first write (type-on-first-write) —
    ///   no `describe_columns` round-trip needed, so a column that has never
    ///   been written still gets an input.
    ColumnForm,
}

/// A predicate over another field's current value, gating a field's visibility
/// in a dynamic form. The front-end re-evaluates it after every value change: a
/// field whose condition no longer holds is hidden — dropped from the layout,
/// skipped by focus navigation, and excluded from the submitted
/// [`ActionInput::Form`] values (and required-checks). Comparison is against the
/// controller's current value string (a toggle yields `"true"`/`"false"`; a
/// select yields the option label).
#[derive(Clone, Debug)]
pub struct FieldCondition {
    /// [`FormFieldSpec::key`] of the field whose value is tested.
    pub field: String,
    /// Visible when the controller's value equals one of these.
    pub equals_any: Vec<String>,
    /// Invert the match: visible when the value is *not* among `equals_any`.
    pub negate: bool,
}

/// One field in an [`InputSpec::Form`].
#[derive(Clone, Debug)]
pub struct FormFieldSpec {
    /// Stable key the value is returned under in [`ActionInput::Form`] and
    /// the key [`Node::form_prep`] uses to prefill an initial value.
    pub key: String,
    /// Human-readable label shown next to the widget.
    pub label: String,
    /// Which widget renders this field.
    pub kind: FormFieldKind,
    /// When true the TUI rejects submission while the value is empty
    /// (text/select). Toggles are always satisfied.
    pub required: bool,
    /// Static initial value used when [`Node::form_prep`] supplies none.
    /// For a toggle this is `"true"`/`"false"`; for a select it must be
    /// one of `allowed_values`.
    pub default: Option<String>,
    /// Render the value as bullets rather than clear text — passwords, tokens,
    /// anything that must not stand on screen. Only text fields honour it.
    pub masked: bool,
    /// When set, the field is only shown (and collected/validated) while this
    /// condition holds against another field's current value — the basis for
    /// dynamic forms whose fields change with the selection. `None` → always
    /// visible.
    pub visible_when: Option<FieldCondition>,
}

impl FormFieldSpec {
    /// A required single-line text field.
    pub fn text(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FormFieldKind::Text,
            required: true,
            default: None,
            masked: false,
            visible_when: None,
        }
    }

    /// A select field over a fixed option list.
    pub fn select(
        key: impl Into<String>,
        label: impl Into<String>,
        allowed_values: Vec<String>,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FormFieldKind::Select { allowed_values },
            required: true,
            default: None,
            masked: false,
            visible_when: None,
        }
    }

    /// A boolean toggle.
    pub fn toggle(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FormFieldKind::Toggle,
            required: false,
            default: None,
            masked: false,
            visible_when: None,
        }
    }

    /// A required natural-language date(-time) text field. `with_time` selects
    /// whether the value carries a time-of-day (front-ends may render a live
    /// resolved preview). The submitted value stays the raw phrase the user
    /// typed; resolution happens in the backend.
    pub fn datetime(key: impl Into<String>, label: impl Into<String>, with_time: bool) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FormFieldKind::DateTime { with_time },
            required: true,
            default: None,
            masked: false,
            visible_when: None,
        }
    }

    /// Mark the field optional (empty values are accepted).
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    /// Mask the value on screen (passwords, tokens). No effect on anything but
    /// a text field, and none on what is submitted.
    pub fn masked(mut self) -> Self {
        self.masked = true;
        self
    }

    /// Set the static initial value.
    pub fn with_default(mut self, default: impl Into<String>) -> Self {
        self.default = Some(default.into());
        self
    }

    /// Show this field only while `field`'s value equals `value`.
    pub fn visible_when(mut self, field: impl Into<String>, value: impl Into<String>) -> Self {
        self.visible_when = Some(FieldCondition {
            field: field.into(),
            equals_any: vec![value.into()],
            negate: false,
        });
        self
    }

    /// Show this field only while `field`'s value is one of `values`.
    pub fn visible_when_any(
        mut self,
        field: impl Into<String>,
        values: impl IntoIterator<Item = String>,
    ) -> Self {
        self.visible_when = Some(FieldCondition {
            field: field.into(),
            equals_any: values.into_iter().collect(),
            negate: false,
        });
        self
    }

    /// Show this field only while `field`'s value is *not* `value`.
    pub fn hidden_when(mut self, field: impl Into<String>, value: impl Into<String>) -> Self {
        self.visible_when = Some(FieldCondition {
            field: field.into(),
            equals_any: vec![value.into()],
            negate: true,
        });
        self
    }
}

/// Widget kind for a [`FormFieldSpec`].
#[derive(Clone, Debug)]
pub enum FormFieldKind {
    /// Single-line free text.
    Text,
    /// One choice out of a fixed list.
    Select { allowed_values: Vec<String> },
    /// Boolean on/off.
    Toggle,
    /// Natural-language date(-time) text field. `with_time` distinguishes a
    /// day-only field from one that also carries a time-of-day. The value is
    /// the raw phrase the user typed; the backend resolves it.
    DateTime { with_time: bool },
}

/// The user's input for an action invocation.
pub enum ActionInput {
    /// `InputSpec::None` actions.
    None,
    /// `InputSpec::Editor` actions: the saved buffer plus the original
    /// template (for 3-way merge) and the version token returned by
    /// [`Node::prepare`].
    Edited {
        text: String,
        original: String,
        version: String,
    },
    /// `InputSpec::Picker` actions: the value of the selected option.
    Picked(String),
    /// `InputSpec::FilePicker` actions: the absolute paths the user chose.
    /// Always non-empty when delivered by the TUI.
    Files(Vec<std::path::PathBuf>),
    /// `InputSpec::Form` actions: the collected field values keyed by
    /// [`FormFieldSpec::key`]. Toggles deliver `"true"`/`"false"`; text and
    /// select fields deliver their string value (possibly empty for an
    /// optional field).
    Form(std::collections::HashMap<String, String>),
    /// `InputSpec::ColumnForm` actions delivered by a front-end that builds the
    /// form from **its own typed column config** (rather than the backend
    /// schema): each cell carries its `value_type`, so the backend can create a
    /// column on first write (type-on-first-write) without a pre-existing
    /// schema. An empty [`ColumnCellInput::value`] means "clear this cell".
    /// (A front-end that instead builds the form from
    /// [`ContentAdapter::describe_columns`] can still deliver the simpler
    /// [`ActionInput::Form`], where the backend resolves each type from its own
    /// schema.)
    ColumnForm(Vec<ColumnCellInput>),
}

/// One typed cell delivered by an [`InputSpec::ColumnForm`] submission via
/// [`ActionInput::ColumnForm`]. The `value_type` travels with the value so a
/// backend that stores the column (e.g. custom columns) can bootstrap it on
/// first write without the front-end having to define it beforehand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnCellInput {
    /// Column key — matches the view config column `key` and the metadata-field
    /// key the value is carried under.
    pub key: String,
    /// The user's value; empty means "clear this cell".
    pub value: String,
    /// Canonical value type from the front-end's column config
    /// (`text`/`number`/`duration`/`datetime`). Authoritative only when the
    /// backend has no column of this key yet; an existing column's stored type
    /// wins (the backend may reject a conflicting type).
    pub value_type: String,
}

/// What the adapter wants the TUI to do after executing an action.
pub enum ActionOutcome {
    /// Persisted. Optional notification text.
    Done { message: Option<String> },
    /// Validation, parse error, or conflict. The adapter has rendered a
    /// fresh buffer in its own syntax (banners, conflict markers); the
    /// TUI should reopen the editor with this content. `new_version`
    /// reflects an upstream re-fetch when applicable.
    Reopen {
        content: String,
        new_version: Option<String>,
    },
    /// Nothing changed — no roundtrip needed.
    NoChanges,
    /// The action created or surfaced a new node — caller may navigate.
    /// `message` is an optional adapter-supplied notification; when `None`
    /// the caller builds a default from `node_type`.
    Navigate {
        node_id: String,
        node_type: NodeType,
        message: Option<String>,
    },
    /// The action produced a local file (or URL) the frontend should hand
    /// to the OS — e.g. an adapter that downloaded an attachment and wants
    /// it shown in the platform image/PDF viewer. The TUI opens `target`
    /// with its configured link opener (`xdg-open` by default), detached so
    /// nothing blocks on the viewer.
    ///
    /// `target` is a single entry point on purpose: when an action fetches
    /// several files it writes them all into one directory and returns the
    /// first, so the opened viewer can page through its siblings. The
    /// adapter never spawns a process itself — *how* to open stays the
    /// frontend's decision (opener config), the adapter only says *what*.
    OpenExternal {
        target: String,
        message: Option<String>,
    },
    /// The action was a *menu step*, not a terminal mutation: it resolved
    /// which follow-up editor action should run next. The frontend opens the
    /// editor for `action_id` on the **same** node, reusing the standard
    /// editor flow ([`Node::prepare`] → edit → [`Node::execute`]) — no new
    /// editor plumbing.
    ///
    /// *Why it exists:* some conversions can't be shown in a single editor
    /// because the target type isn't known yet, and which source fields drop
    /// depends on that target. So a `Picker`/`Form` action first lets the user
    /// choose the target (the menu), then returns `OpenEditor { action_id }`
    /// pointing at a type-specific editor action (e.g. `"convert:userstory"`)
    /// whose `prepare` renders the exactly-right buffer. It generalises the
    /// "pick a variant, then edit it" flow for any adapter.
    ///
    /// Interactive-only: the flow needs `$EDITOR`, so non-interactive
    /// frontends (the CLI) reject it.
    OpenEditor { action_id: String },
}

/// Initial state for an `InputSpec::Editor` action.
#[derive(Default)]
pub struct EditorPrep {
    /// Initial buffer content written to the temp file.
    pub template: String,
    /// Backend version token — passed back via [`ActionInput::Edited`]
    /// for conflict detection.
    pub version: String,
    /// File suffix for `$EDITOR` syntax highlighting (e.g. `".jira"`).
    pub suffix: String,
    /// Optional **persistent** file the editor should open, instead of a
    /// throwaway temp file. `Some(path)` opts into "materialised" editing:
    /// the buffer lives at exactly this path, is *not* cleaned up when the
    /// editor closes, and sibling files (e.g. downloaded attachments) can be
    /// referenced by relative path from it. The adapter is responsible for
    /// creating parent directories and seeding the file's initial content via
    /// [`Self::template`] (the frontend writes `template` to `file_path`).
    /// `None` (the default) keeps the classic `$TMPDIR` temp-file behaviour.
    pub file_path: Option<std::path::PathBuf>,
}

/// A selectable option for an `InputSpec::Picker` action.
#[derive(Clone, Debug)]
pub struct ActionOption {
    /// Display label (e.g. "In Progress").
    pub label: String,
    /// Value passed back via [`ActionInput::Picked`] (e.g. transition ID).
    pub value: String,
}

/// One option in a named value list served by
/// [`ContentAdapter::list_values`].
///
/// Deliberately *not* tied to any action: it is a generic "here is a value
/// the user may pick" pair. A frontend menu (e.g. the TUI's `option_menu`)
/// fetches such a list, lets the user choose, and then feeds the chosen
/// `value` back into a value-accepting action via [`ActionContext::value`].
/// The adapter never learns which widget sourced the value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ValueOption {
    /// Stable value handed back to the adapter (e.g. a tag id like
    /// `global-tag:<uuid>`). Opaque to the frontend.
    pub value: String,
    /// Human-readable label shown in the menu.
    pub label: String,
    /// Optional structured detail carried alongside the value for consumers
    /// that need more than the display label (e.g. Kimai's `entry_combos`
    /// attaches `project` / `activity` clear names next to the slug token).
    /// Empty for sources with no such breakdown; frontends may ignore it.
    pub extra: std::collections::BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------
// Per-node action dispatch (shortcut path)
// ---------------------------------------------------------------------------

/// What [`Node::invoke_action`] tells the TUI to do.
///
/// Distinct from [`ActionOutcome`] (which describes the *result* of a
/// completed [`Node::execute`] roundtrip). `ActionDispatch` describes the
/// *intent* — what UI flow the TUI should start in response to a
/// shortcut press.
///
/// Note: the design doc (`docs/plan-cursor-pagination-and-node-actions.md`)
/// originally called this `ActionOutcome`; the name was already in use,
/// so we use `ActionDispatch` here. Semantics are identical.
#[derive(Clone, Debug)]
pub enum ActionDispatch {
    /// Open an editor session. `session_kind` is a *generic* role
    /// discriminator the TUI uses to pick the right `EditSession` impl —
    /// it MUST NOT encode an adapter name. The TUI understands a small
    /// fixed vocabulary; adapters reuse those role names rather than
    /// inventing adapter-prefixed ones:
    ///
    /// - `"query_editor"` — an editor that edits and (re)runs a query
    ///   against a backend (Postgres uses it for the per-table SQL editor).
    /// - `"script_editor"` — an editor for a named, persisted script that
    ///   the user re-executes separately (Postgres uses it for DB-level
    ///   scripts).
    ///
    /// `params` carries the session's setup data as opaque string
    /// key/value pairs (e.g. `{"database": "live", "script": "report"}`).
    OpenEditor {
        session_kind: String,
        params: std::collections::HashMap<String, String>,
    },
    /// Execute a query and (optionally) open a paginated result pane.
    /// Used by per-node `execute` shortcuts (e.g. `x` on a DB script).
    ExecuteQuery {
        database: String,
        sql: String,
        /// When true the TUI opens a paginated result pane. When false
        /// the query is fire-and-forget (e.g. DDL); status surfaces in
        /// the status bar.
        paged: bool,
    },
    /// Delete the invoking node. TUI confirms before the actual delete.
    ///
    /// `confirm` lets the adapter override the confirmation prompt — e.g.
    /// a node that deletes its whole subtree should say so ("Delete 'X'
    /// and its 3 subtasks (recursive)? (y/n)"). `None` falls back to the
    /// TUI's generic `Delete '<label>'? (y/n)`. The adapter is the only
    /// authority on whether its delete cascades, so it owns the wording.
    DeleteSelf { confirm: Option<String> },
    /// Reload the current pane.
    Reload,
    /// Ask the user to confirm before the action does its (often
    /// irreversible) work. The adapter returns this on the *first*
    /// invocation — when [`ActionContext::confirmed`] is still `false` —
    /// with a user-facing `prompt`. The TUI shows a `(y/n)` confirmation
    /// and, on "y", re-invokes the *same* action on the *same* node with
    /// `confirmed: true`, at which point the adapter performs the work.
    ///
    /// Unlike [`DeleteSelf`] (whose confirm/execute split lives in the
    /// frontend's delete plumbing), `Confirm` is generic: any action can
    /// gate itself behind a confirmation, and the adapter authors the
    /// wording because only it knows what the action will actually do
    /// (e.g. how many successor intervals a restore will purge).
    Confirm { prompt: String },
    /// No-op — useful as a default for adapters that haven't migrated.
    Noop,
    /// The action succeeded and has a user-facing result to report, but did
    /// *not* change the pane's data (so no reload is warranted). The frontend
    /// surfaces `message` — the TUI in its status bar, the CLI on stdout.
    ///
    /// *Why it exists:* some actions produce a result the user needs to see
    /// without mutating the view — e.g. `backup` returns the path of the file
    /// it just wrote. Neither [`ActionDispatch::Reload`] (implies a data
    /// change) nor [`ActionDispatch::Noop`] (says "nothing happened") fit;
    /// `Notify` is the generic success-with-a-message channel.
    Notify { message: String },
    /// Adapter rejected the action with a user-displayable error.
    Error(String),
}

/// A node the user marked for a subsequent move — the "clipboard" of the
/// generic mark-move / paste-move vocabulary (M7).
///
/// Marking is session state owned by the frontend; it travels into the
/// adapter only when a `paste-move` action fires, via
/// [`ActionContext::marked`]. The adapter uses it to perform the
/// structural move (reparent / relocate the marked node under or next to
/// the invoking node) and is free to reject an incompatible
/// [`MarkedNode::node_type`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkedNode {
    /// Adapter-local id of the marked node — the same id accepted by
    /// [`ContentAdapter::get_by_id`].
    pub node_id: String,
    /// Type of the marked node, so a `paste-move` target can validate
    /// the move (e.g. only accept a task under another task).
    pub node_type: NodeType,
    /// Human-readable label, for the frontend's "marked …" indicator.
    pub label: String,
}

/// Context passed into [`Node::invoke_action`].
///
/// Extended per phase; today it carries the [`MarkedNode`] for the
/// generic mark/paste-move flow. The frontend populates `marked` only on
/// a `paste-move` invocation (and only when something is marked); every
/// other action sees `None`.
#[derive(Clone, Debug, Default)]
pub struct ActionContext {
    /// The node the user previously marked for a move, if any. Populated
    /// by the frontend on a `paste-move` invocation so the adapter can
    /// relocate the marked node relative to the invoking (target) node.
    pub marked: Option<MarkedNode>,
    /// Whether the user has already confirmed this action. `false` on the
    /// first invocation; the adapter may return
    /// [`ActionDispatch::Confirm`] to request a `(y/n)` prompt. The TUI
    /// re-invokes with `confirmed: true` on "y", and the adapter then does
    /// the work instead of asking again.
    pub confirmed: bool,
    /// The pane's currently-active query text — the *same* filter string the
    /// frontend already hands [`ContentAdapter::list`] via
    /// [`LoadParams::query`] on every load. Carried here so a **set-scoped**
    /// action (one that operates on more than the invoking node — e.g. a
    /// container/list-wide `restore-all`, a bulk delete, an aggregate) can
    /// re-resolve the *visible* set and act only on it, never on the whole
    /// universe.
    ///
    /// This is the active query's *identity*, not a round-trip of rendered
    /// content: the adapter produced the listing from this same query, so it
    /// re-derives the set itself (just like `list`) rather than receiving a
    /// list of ids back. `None`/empty = no active filter → the whole list is
    /// in scope (which equals what the pane shows).
    ///
    /// Single-node actions (delete one row, restore one row, toggle) ignore
    /// it — their target is already the invoking node.
    pub query: Option<String>,
    /// A frontend-sourced value for a *value-accepting* action — the core of
    /// the decoupled-input design. Instead of the adapter prescribing a
    /// widget (`InputSpec::Picker`), an action simply consumes a value here
    /// and the frontend decides how to source it (free text, a node field,
    /// or a list fetched via [`ContentAdapter::list_values`]). The adapter
    /// accepts any value and rejects nonsense via [`ActionDispatch::Error`].
    ///
    /// `None` for actions that take no value (the common case). Example: the
    /// TUI's `option_menu` sets this to the focused option's `value` (e.g. a
    /// tag id) when invoking a `toggle`-style action.
    pub value: Option<String>,
    /// A frontend-sourced *free-text* input for a value-accepting action that
    /// needs the user to type something — the text companion to [`Self::value`].
    /// Where `value` carries a selected id (the focused option), `text` carries
    /// a typed string. The two combine: a `rename` action reads the tag id from
    /// `value` and the new name from `text`; a `create` action reads only `text`
    /// (the new name). The adapter validates and rejects nonsense (e.g. empty
    /// text) via [`ActionDispatch::Error`].
    ///
    /// `None` for actions that need no typed input (the common case). Example:
    /// the TUI's `option_menu` prompts for a line of text on a create/rename
    /// binding and sets this to what the user typed.
    pub text: Option<String>,
}

// ---------------------------------------------------------------------------
// AdapterStatus (live connection state, observable by frontends)
// ---------------------------------------------------------------------------

/// One field requested from the user during interactive credential entry.
/// Frontends render a form built from a `Vec<AuthField>` and pass the
/// collected values back via [`ContentAdapter::submit_credentials`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthField {
    /// Stable key the adapter uses to look the value up (e.g. "username").
    pub name: String,
    /// Display label for the form (e.g. "Username").
    pub label: String,
    /// If true, the frontend must mask the input (passwords).
    pub masked: bool,
    /// If true, the frontend may accept an empty answer.
    ///
    /// Fields the config binds are never optional — binding one is how
    /// the user says the value is needed. This exists for the form a
    /// credential script describes, where the script decides which of
    /// its own inputs it can do without.
    pub optional: bool,
    /// Optional pre-filled value (e.g. username from YAML config).
    pub prefill: Option<String>,
}

/// Live connection state of an adapter, observable through
/// [`ContentAdapter::subscribe_status`]. Adapters that need to surface
/// async login progress (cookie scripts, OAuth flows) publish updates
/// here; frontends render the current state in the relevant view.
// `Eq` is intentionally absent: `Busy.progress` is an `Option<f32>`, and `f32`
// is only `PartialEq`. Status comparisons (change detection) use `PartialEq`.
#[derive(Clone, Debug, PartialEq)]
pub enum AdapterStatus {
    /// Auth has not started yet.
    Idle,
    /// Auth is in progress. `retry` is 1-based; `max_retries` and
    /// `timeout_secs` come from the adapter's config so the view can
    /// render "Connecting… (retry/max_retries) Timeout: timeout_secs".
    Connecting {
        retry: u32,
        max_retries: u32,
        timeout_secs: u64,
    },
    /// Adapter needs interactive credentials. The frontend renders a form
    /// for `fields` and submits the collected values via
    /// [`ContentAdapter::submit_credentials`], or reports the user's
    /// refusal via [`ContentAdapter::cancel_credentials`].
    NeedsCreds {
        fields: Vec<AuthField>,
        /// What this form is for, when it is not simply "log in" — a
        /// credential script asking to unlock a password store says so
        /// here. Frontends use it as the dialog's title.
        header: Option<String>,
        /// Why the form is being shown again ("that passphrase was
        /// rejected"). Set by a credential script that wants another try;
        /// a fatal error ends the login instead and arrives as `Failed`.
        error: Option<String>,
    },
    /// Auth completed; the adapter is ready to serve requests.
    Ready,
    /// Auth gave up after exhausting retries (or hit a non-retryable
    /// error). `reason` is suitable for direct display.
    Failed { reason: String },
    /// A specific request is in flight against a (presumably) connected
    /// backend. Surfaces a countdown so users can see the adapter is
    /// still working before its timeout kicks in. Implies the connection
    /// is otherwise live — `Busy` only ever flips out of (and back to)
    /// `Ready` in practice, never out of `Connecting`/`NeedsCreds`.
    ///
    /// `started_at_unix_ms` is wall-clock at request start (so the
    /// frontend can compute `elapsed = now − started_at` on every
    /// render-tick without a separate counter). `timeout_secs` is the
    /// adapter-configured deadline that fires the reconnect.
    ///
    /// `progress` is an optional best-effort completion estimate in `[0, 1]`
    /// for loads that arrive incrementally (e.g. the calendar's browser
    /// backend paging month by month). `Some(f)` lets the frontend render a
    /// percentage; `None` means indeterminate (the countdown is the only cue),
    /// which is what every non-incremental `Busy` uses.
    Busy {
        label: String,
        started_at_unix_ms: u64,
        timeout_secs: u64,
        progress: Option<f32>,
    },
}

impl AdapterStatus {
    /// One-line rendering of a *transient* connection state, for frontends
    /// that only need to tell the user what the adapter is doing right now.
    /// `None` for the two resting states (`Idle`, `Ready`) — there is
    /// nothing to report about them.
    ///
    /// Shared so the TUI banner and the CLI's stderr progress line cannot
    /// drift apart. Frontends that can say more say it themselves: the TUI
    /// names the key that opens the credential form and renders `Busy` as a
    /// live countdown, neither of which a one-shot line can express.
    pub fn banner_text(&self) -> Option<String> {
        match self {
            Self::Connecting {
                retry,
                max_retries,
                timeout_secs,
            } => {
                // Both details are optional in the status: an adapter with a
                // single, open-ended attempt reports `1/1` and `0` — printing
                // "(1/1) Timeout: 0s" would state a limit that isn't there.
                let mut line = "Connecting…".to_string();
                if *max_retries > 1 {
                    line.push_str(&format!(" ({retry}/{max_retries})"));
                }
                if *timeout_secs > 0 {
                    line.push_str(&format!(" Timeout: {timeout_secs}s"));
                }
                Some(line)
            }
            Self::NeedsCreds { .. } => Some("Login required".into()),
            Self::Failed { reason } => Some(format!("Connection failed: {reason}")),
            Self::Busy { label, .. } => Some(format!("Working: {label}")),
            Self::Idle | Self::Ready => None,
        }
    }
}

/// Out-of-band signal from an adapter that its content changed and the
/// affected views should reload / redraw — the push counterpart to the
/// pull-based `list()`/`get_by_id()` model. Emitted by *streaming*
/// adapters (the Stoat gateway pushes one per relevant WebSocket event);
/// pull-only adapters never send (see [`ContentAdapter::subscribe_invalidations`]).
///
/// The payload is deliberately adapter-internal (a raw node id, not an
/// app-wide [`NodeRef`]): the frontend already routes each subscription
/// to a known view, so all it needs is *which level within that view*
/// changed. Keeping it internal avoids coupling the adapter to the
/// frontend's path encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invalidation {
    /// The child list of the node with this adapter-internal id is stale
    /// (e.g. a new message arrived in the channel with this id). The
    /// frontend reloads a view's current level **only** if that level's
    /// parent is this node — so a message in a channel nobody is looking
    /// at costs nothing.
    Node { id: String },
    /// The whole adapter tree is stale (reconnect, fresh bootstrap
    /// snapshot, or a structural change). The frontend reloads the
    /// current level of every view bound to this adapter.
    All,
    /// Redraw only — **no** refetch. The adapter's data is unchanged but a
    /// time-derived rendering (e.g. a live "elapsed since" cell) needs to
    /// re-render. The dirty-gated render loop is otherwise parked, so this
    /// is the wake that makes such cells tick. Bridged from a periodic
    /// domain heartbeat (`DomainEvent::TrackingTick`); cheap by design, so
    /// adapters may send it at ~1 Hz without touching their backend.
    Repaint,
    /// A single row's **complete** new state (M9 — adapter-driven live
    /// rows). Carries the full [`NodeSummary`], so the frontend replaces
    /// the matching row (by `id`) **in place** — no refetch, selection and
    /// scroll preserved. The push counterpart to [`Repaint`]: where
    /// `Repaint` re-renders a *time-derived* cell against a fresh `now`,
    /// `Row` lets the adapter itself compute the new value (e.g. a running
    /// tracking's elapsed duration, where the *same* column must read live
    /// for active rows and static for completed ones — something a single
    /// render-time `kind: elapsed` column cannot express). Generic over any
    /// adapter: a chat adapter can push an edited message, a CI adapter a
    /// build-progress row. A row whose `id` matches no visible item is a
    /// no-op.
    Row(NodeSummary),
    /// Re-pace the frontend's per-view **live-refresh timer** (M9). The
    /// adapter dictates the cadence at which the frontend should pull its
    /// [`live_rows`](ContentAdapter::live_rows) and patch them; `Some(d)`
    /// (re)starts the timer at interval `d`, `None` stops it. Sent whenever
    /// the adapter's set of live rows changes shape — e.g. the tracking
    /// adapter sends `Some(1s)` when a tracking starts and `None` when the
    /// last one stops, so the 1 Hz pull only runs while something actually
    /// ticks. The timer is owned by the frontend (one place to budget
    /// wake-ups against the dirty-gated render loop); the adapter only
    /// declares the interval.
    RefreshInterval(Option<std::time::Duration>),
    /// Data anchored to the **current instant** changed structurally, so a
    /// view grouped into buckets (one independently-aggregated subtree per
    /// group) must re-resolve and reload *only* the bucket that "now" falls
    /// into — not every bucket. Payload-free on purpose: the adapter knows
    /// *that* its now-anchored data moved, but not which bucket a given pane
    /// shows it under, because the active grouping ([`GroupSpec`]) is
    /// per-pane frontend state. So the frontend, on receiving this, asks the
    /// adapter [`bucket_for_now`](ContentAdapter::bucket_for_now) for each
    /// grouped pane's spec and reloads that one bucket's subtree in place.
    ///
    /// Emitted by the tracking adapter when a tracking starts/stops (the
    /// running interval's bucket totals shift) — the targeted counterpart to
    /// the coarse [`All`] a structural change (e.g. a deleted tracking) sends.
    /// Generic over any now-anchored grouping (e.g. a CI adapter grouping
    /// builds by day could re-fold today's bucket on a new build).
    NowAnchored,
}

/// A time-anchored heads-up that a scheduled item is about to happen — the
/// generic "reminder" event an adapter fires ahead of the moment (see
/// [`ContentAdapter::subscribe_reminders`]).
///
/// The payload is deliberately adapter-agnostic: a stable id, a human title,
/// an optional detail line, the instant the item occurs, and how far ahead
/// this fire is. That is enough for a frontend to run a user-configured
/// command (a desktop notification, a sound, …) **without** knowing what kind
/// of thing it is — a calendar event, a CI window, a countdown. Which items
/// get a reminder, and how far ahead, is the *adapter's* policy; what to *do*
/// when one fires is the *frontend's* — this type is the seam between them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reminder {
    /// Stable id of the underlying item (e.g. the event's global id), so a
    /// frontend can de-dupe repeated fires and correlate with a row.
    pub id: String,
    /// Human-readable subject (e.g. the event title).
    pub title: String,
    /// Optional secondary line (location, organiser, account, …).
    pub detail: Option<String>,
    /// RFC3339 instant the item occurs at, rendered in local time — for
    /// display and as a substitution value in the configured command.
    pub when: String,
    /// RFC3339 instant the item is *over*, rendered in local time, when the
    /// underlying item has a defined end (e.g. a calendar event's end). Lets a
    /// frontend keep a notification on screen until the moment has passed.
    /// `None` for point-in-time items with no natural end.
    pub until: Option<String>,
    /// Whole minutes from this fire until [`when`](Self::when) (the lead
    /// time). `0` when firing at or after the moment.
    pub lead_minutes: i64,
}

/// A backend-initiated request for user input, raised **mid-operation** — the
/// input-capturing counterpart to the fire-and-forget [`Reminder`] /
/// [`Invalidation`] streams. Where a [`NodeAction`] is *user*-initiated (the
/// user presses a key, the frontend collects input, the adapter executes), a
/// `PromptRequest` is *adapter*-initiated: a long-running async operation
/// (e.g. an interactive browser login) discovers that it needs the user to
/// provide — or merely acknowledge — something before it can continue, and
/// pushes this request up to the frontend, blocking until the answer arrives
/// on [`respond`](Self::respond).
///
/// It reuses the Action input vocabulary on purpose — [`InputSpec`] describes
/// the shape (from `None` = pure acknowledge, through `Editor`, to a
/// multi-field `Form`) and [`ActionInput`] carries the answer back — so the
/// frontend collects it with the *same* widgets it already uses for actions,
/// with no parallel input machinery. The one semantic difference from an
/// action: a prompt **always** shows its popup, even for [`InputSpec::None`]
/// (that is the acknowledge case), because presenting [`detail`](Self::detail)
/// — e.g. an MFA number to match — is the whole point.
///
/// Separation of concerns: the adapter owns *when* a prompt is raised and
/// *what* it asks; the frontend owns *how* it is rendered. Whether an
/// interactive frontend exists at all falls out of the channel: if none took
/// the stream (see [`ContentAdapter::take_prompt_requests`]) or the responder
/// is dropped unanswered, the raising side observes a closed channel and fails
/// loudly rather than hanging.
pub struct PromptRequest {
    /// Human-readable label of the raising instance/connection (e.g. the
    /// account name). Pure context for the frontend to show — it does **not**
    /// imply any view/tab switch; the originating tab is merely the context.
    pub source: String,
    /// The prompt text shown to the user. Adapter-supplied and typically
    /// user-configurable per callback.
    pub prompt: String,
    /// Optional read-only detail rendered above the input — e.g. the MFA
    /// number to match, or an instruction line. Display only.
    pub detail: Option<String>,
    /// Shape of the expected input, reusing the [`NodeAction`] vocabulary.
    /// [`InputSpec::None`] means "acknowledge only" — the frontend shows the
    /// prompt and waits for a bare confirm.
    pub input: InputSpec,
    /// One-shot channel the frontend answers on. Sending a [`PromptAnswer`]
    /// unblocks the raising operation; dropping it (or a send failure on the
    /// request stream) signals "no interactive handler" to that operation.
    pub respond: tokio::sync::oneshot::Sender<PromptAnswer>,
}

/// The frontend's answer to a [`PromptRequest`].
pub enum PromptAnswer {
    /// The user supplied input — reuses the same [`ActionInput`] the frontend
    /// produces for actions. A bare acknowledge (for [`InputSpec::None`]) is
    /// delivered as [`ActionInput::None`].
    Provided(ActionInput),
    /// The user dismissed the prompt without answering. The raising operation
    /// should treat this as "user declined" and unwind (and may retry).
    Cancelled,
}

// ---------------------------------------------------------------------------
// Core Traits
// ---------------------------------------------------------------------------

/// The entry point. One instance per configured connection.
#[async_trait]
pub trait ContentAdapter: Send + Sync {
    /// Stable type identifier of this adapter (e.g. "jira", "postgres",
    /// "taiga"). Used as the first path component in
    /// [`ContentAdapter::instance_data_dir`] and as a prefix in
    /// scope keys (e.g. saved-query scope).
    fn adapter_type(&self) -> &str;

    /// Stable per-instance identifier. Comes from the YAML
    /// `adapter.id:` field; defaults to [`adapter_type`](Self::adapter_type)
    /// when not set. Two adapter instances loaded into the same App run must
    /// have distinct `instance_id`s — the loader validates this.
    ///
    /// The default mirrors the "no `adapter.id:`" case: a single-instance
    /// adapter that stores no separate id reports its type. Multi-instance
    /// adapters override to return the configured id.
    fn instance_id(&self) -> &str {
        self.adapter_type()
    }

    /// Per-instance writable data directory. Default:
    /// `<XDG_DATA_HOME>/not_yet_done/<adapter_type>/<instance_id>/`.
    /// Caller is responsible for creating it on first write. Adapters
    /// may override if they need a different layout.
    fn instance_data_dir(&self) -> std::path::PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("not_yet_done")
            .join(self.adapter_type())
            .join(self.instance_id())
    }

    /// Navigate to the root node of the content tree.
    async fn root(&self) -> Result<Box<dyn Node>>;

    /// Direct access to a node by its ID (shortcut, avoids tree traversal).
    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>>;

    /// The single source of truth about `node`'s children: for each child kind
    /// its [`NodeType`], sortable columns, and a lazy fetcher. Everything a
    /// front-end asks about children is *derived* from this via the free
    /// functions in [`crate::children`] ([`crate::child_types`],
    /// [`crate::sortable_columns_for`], [`crate::list`], [`crate::list_subtree`]),
    /// so the type set, its sort columns and its fetch can never drift apart.
    ///
    /// Each fetcher is keyed only by `node.id()` + adapter state — no downcast.
    /// A leaf node returns an empty vec.
    fn childs<'a>(&'a self, node: &'a dyn Node) -> Vec<crate::children::Child<'a>>;

    /// Opt-in fast path for eager subtree expansion. Returns `Some(result)`
    /// when the adapter can build the whole expanded subtree in one pass;
    /// `None` (the default) makes [`crate::children::list_subtree`] fall back to
    /// its generic per-node recursion.
    ///
    /// This is the home for adapters that hold their tree in memory (local
    /// Tasks/Trackings, capability [`AdapterCapabilities::supports_eager_subtree`]):
    /// the generic recursion resolves each node via [`get_by_id`](Self::get_by_id)
    /// and — critically — cannot carry `params.sort` below the first level, so a
    /// sorted tree would lose its per-level sibling order. An adapter that owns
    /// the whole structure sorts every level here in a single projection walk.
    async fn eager_subtree(
        &self,
        _node: &dyn Node,
        _params: &ListParams,
        _depth: u32,
    ) -> Option<Result<Subtree>> {
        None
    }

    /// Download a binary asset this adapter serves (e.g. an image
    /// attachment), by absolute URL, authenticating as the adapter needs to.
    /// Returns the raw bytes.
    ///
    /// The frontend calls this when the user opens an *image* link (via the
    /// link-hop): rather than sending the URL to a browser, the file is
    /// fetched here and shown in the OS image viewer. Because attachment URLs
    /// commonly sit behind auth or on a separate file host, only the adapter
    /// can fetch them — hence the hook.
    ///
    /// The default declines every URL, so an adapter that hasn't opted in
    /// keeps the browser fallback. An adapter that overrides this SHOULD
    /// still decline (with [`ContentError::NotSupported`]) any URL it doesn't
    /// recognise as its own, so unrelated links fall back to the browser too.
    async fn download_asset(&self, _url: &str) -> Result<Vec<u8>> {
        Err(ContentError::NotSupported("download_asset".into()))
    }

    /// Synchronous, instance-free lookup of the action set for a given
    /// node type. Used by the TUI to render shortcut hints without
    /// instantiating a node — the alternative (`get_by_id(id).actions()`)
    /// triggers a full chain walk per cursor move, which on Postgres
    /// means a `list_databases` call just to read a constant list.
    ///
    /// Adapters return the same set their `Node::actions()` impl would
    /// return for a fresh instance of that type. Default is empty —
    /// adapters with no shortcuts don't need to override.
    fn actions_for_type(&self, _node_type: &NodeType) -> Vec<NodeAction> {
        Vec::new()
    }

    /// Environment variables to propagate to child processes (editors,
    /// scripts) spawned in this adapter's context. Default is empty —
    /// adapters without connection state (or that don't expose their
    /// credentials to child tools) don't need to override.
    ///
    /// Use case: the Postgres adapter exposes its live tunnel and
    /// credentials as libpq-style `PG*` vars so the editor's LSP
    /// (`postgres-language-server`) can talk to the same database the
    /// TUI is connected to without the user duplicating credentials in
    /// a sidecar config file.
    ///
    /// `node` identifies which item in the adapter's tree the spawn is
    /// for, so the env can be node-specific (e.g. `PGDATABASE` derived
    /// from the selected database row). The map is a snapshot at spawn
    /// time — adapters should not assume it will be re-queried if
    /// connection state changes later.
    ///
    /// Sync because the password/port are already resolved in adapter
    /// RAM; no I/O should be needed here. If the underlying connection
    /// is not yet open, return an empty map rather than blocking.
    fn child_process_env(&self, _node: &NodeRef) -> std::collections::HashMap<String, String> {
        std::collections::HashMap::new()
    }

    /// Augment an editor buffer with adapter-specific, **editor-only**
    /// completion hints before the external editor opens. `node` is the
    /// canonical [`NodeRef`] of the item being edited; the adapter scopes
    /// the hint from it. The returned string REPLACES the buffer: the
    /// adapter must first strip any stale hint it previously added (so
    /// repeated opens stay idempotent) and then append a fresh one.
    ///
    /// The hint is a convenience only and MUST be removed again on commit
    /// via [`strip_editor_hints`](Self::strip_editor_hints) so the
    /// persisted file never contains it. Default returns the buffer
    /// unchanged — adapters with no editor hints don't override.
    ///
    /// Use case: the Postgres DB-script editor appends a trailing SQL
    /// comment listing the database's tables as copy-paste tokens, which
    /// previously required a concrete-type downcast from the TUI. Routing
    /// it through the trait keeps the TUI free of any adapter-specific
    /// type knowledge.
    async fn augment_editor_buffer(&self, _node: &NodeRef, buffer: String) -> String {
        buffer
    }

    /// Inverse of [`augment_editor_buffer`](Self::augment_editor_buffer):
    /// strip any editor-only hint the adapter previously injected so the
    /// committed/persisted text is clean user content. Called on save.
    /// Default returns the text unchanged.
    fn strip_editor_hints(&self, text: &str) -> String {
        text.to_string()
    }

    /// Capabilities of this adapter (for UI feature gating). The default is
    /// all-`false` ([`AdapterCapabilities::default`]) — a minimal read-only
    /// adapter. Adapters opt into features (create/delete/search, eager
    /// subtree, adapter-side grouping, …) by overriding.
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::default()
    }

    /// Whether this adapter currently has any *running* activity worth a
    /// global action-bar highlight — the host ORs this across all open
    /// adapters to light up the toggle-tracking hint while a tracking is
    /// live. Default `false`; the task/tracking adapters override to report
    /// their in-memory active set. Must be cheap and non-blocking (read a
    /// snapshot best-effort) — it is polled on the render path.
    fn has_active_tracking(&self) -> bool {
        false
    }

    /// Serve a named, adapter-defined list of selectable values — the
    /// generic backing for frontend menus that need "the set of things the
    /// user may choose from" without the adapter knowing what widget renders
    /// it. `source` is an opaque adapter-defined key (e.g. `"tags"`); an
    /// unknown source returns an empty list. The chosen value travels back
    /// into a value-accepting action via [`ActionContext::value`].
    ///
    /// Default returns no values, so adapters that expose no such lists need
    /// not override it.
    async fn list_values(&self, source: &str) -> Result<Vec<ValueOption>> {
        let _ = source;
        Ok(Vec::new())
    }

    /// Subscribe to live connection-status updates. Default returns a
    /// receiver that is permanently `Ready` — adapters that don't perform
    /// async auth (or that block synchronously inside `create`) don't
    /// need to override this.
    ///
    /// The sender backing the default channel lives for the duration of
    /// the process so consumers can `borrow()` the current value or
    /// `await` forever on `changed()` without spurious sender-dropped
    /// errors.
    fn subscribe_status(&self) -> tokio::sync::watch::Receiver<AdapterStatus> {
        static READY_TX: std::sync::OnceLock<tokio::sync::watch::Sender<AdapterStatus>> =
            std::sync::OnceLock::new();
        let tx = READY_TX.get_or_init(|| tokio::sync::watch::channel(AdapterStatus::Ready).0);
        tx.subscribe()
    }

    /// Subscribe to out-of-band [`Invalidation`] signals. A `broadcast`
    /// receiver (not `watch`) because invalidations are discrete *events*,
    /// not a latest-value state — coalescing them would drop intermediate
    /// changes — and because one adapter instance can back several views,
    /// each of which subscribes independently.
    ///
    /// Default returns a receiver whose sender lives for the duration of
    /// the process and never sends — pull-only adapters (Jira, Taiga,
    /// Postgres, …) don't need to override and their forwarder simply
    /// parks forever. Mirrors the [`subscribe_status`](Self::subscribe_status)
    /// default's static-`OnceLock` keepalive.
    fn subscribe_invalidations(&self) -> tokio::sync::broadcast::Receiver<Invalidation> {
        static SINK_TX: std::sync::OnceLock<tokio::sync::broadcast::Sender<Invalidation>> =
            std::sync::OnceLock::new();
        let tx = SINK_TX.get_or_init(|| tokio::sync::broadcast::channel(1).0);
        tx.subscribe()
    }

    /// Hard refresh: the user explicitly asked for a full reload (the `reload`
    /// action, e.g. `r`). Adapters that cache and/or fetch in the background
    /// should treat this as "abort everything in flight and start over":
    /// cancel any running fetches, drop the cache, and let the ensuing
    /// [`list`](Node::list) repopulate from scratch. Called by the frontend
    /// *before* the reload's `list()`, so a cleared cache forces a cold fetch.
    ///
    /// Default is a no-op: a stateless pull adapter (Jira, Taiga, Postgres, …)
    /// has nothing in flight and no cache to drop — its `list()` already hits
    /// the backend every time — so the reload's `list()` alone suffices.
    async fn refresh(&self) -> Result<()> {
        Ok(())
    }

    /// Subscribe to time-anchored [`Reminder`] events the adapter fires ahead
    /// of scheduled items. Same broadcast-channel story as
    /// [`subscribe_invalidations`](Self::subscribe_invalidations) — discrete
    /// events (not a latest-value state), and one adapter instance can back
    /// several views that each subscribe independently.
    ///
    /// This is purely a *contract*: the adapter offers a stream, owns *when*
    /// a reminder fires (and, later, a filter for *which* items get one), and
    /// the frontend owns *what happens* — it runs the user-configured command.
    /// Scheduling policy stays in the adapter; side-effecting I/O stays in the
    /// frontend.
    ///
    /// Default returns a receiver whose sender lives for the process and never
    /// sends — adapters that model no schedule (Jira, Taiga, Postgres, …)
    /// don't override, and their forwarder simply parks forever. Mirrors the
    /// [`subscribe_invalidations`](Self::subscribe_invalidations) default's
    /// static-`OnceLock` keepalive.
    fn subscribe_reminders(&self) -> tokio::sync::broadcast::Receiver<Reminder> {
        static SINK_TX: std::sync::OnceLock<tokio::sync::broadcast::Sender<Reminder>> =
            std::sync::OnceLock::new();
        let tx = SINK_TX.get_or_init(|| tokio::sync::broadcast::channel(1).0);
        tx.subscribe()
    }

    /// Take the stream of backend-initiated [`PromptRequest`]s this adapter
    /// raises mid-operation (e.g. an MFA prompt during an interactive login).
    /// **Single-consumer** — an `mpsc` receiver, not a broadcast one — because
    /// exactly one frontend services user input, and each request carries its
    /// own one-shot responder that cannot be cloned. Callable once; further
    /// calls return `None`.
    ///
    /// Returning `Some` is also what *installs the sending half* inside the
    /// adapter: an adapter that raises prompts creates its sink lazily here, so
    /// a non-interactive frontend (which never calls this) leaves the adapter
    /// with no sink, and any prompt it would raise fails loudly instead of
    /// hanging. This is the seam that turns "no prompt consumer configured"
    /// into a clean error rather than a deadlock.
    ///
    /// Default returns `None` — adapters that never need mid-operation input
    /// (Jira, Taiga, Postgres, …) don't override.
    fn take_prompt_requests(&self) -> Option<tokio::sync::mpsc::Receiver<PromptRequest>> {
        None
    }

    /// Recompute the adapter's currently-live rows (M9). Called by the
    /// frontend's per-view refresh timer — whose cadence the adapter sets
    /// via [`Invalidation::RefreshInterval`] — once per tick; each returned
    /// [`NodeSummary`] is patched into the matching visible row in place
    /// (see [`Invalidation::Row`]). The adapter returns *only* the rows
    /// whose rendering actually changes between ticks (e.g. the entries
    /// for running trackings, with a freshly-computed elapsed duration),
    /// not its whole list — a row not returned keeps its current cells.
    ///
    /// Pure recompute from already-loaded state: no backend round-trip, so
    /// it stays cheap at 1 Hz. Default returns an empty list — pull-only
    /// adapters with no time-derived rows never need to override and their
    /// timer (if any) patches nothing.
    async fn live_rows(&self) -> Vec<NodeSummary> {
        Vec::new()
    }

    /// Resolve the **id of the bucket the current instant falls into** for a
    /// view grouped by `group_by`. The frontend calls this when it receives
    /// [`Invalidation::NowAnchored`]: each grouped pane hands in its own
    /// [`GroupSpec`] (per-pane frontend state the adapter can't know) and gets
    /// back the single bucket node id whose subtree it should reload in place,
    /// leaving every other bucket untouched.
    ///
    /// Returns the bucket the *most recent* item falls into — e.g. the
    /// tracking adapter returns the group node of the youngest tracking, which
    /// is the one a start/stop just shifted (a start mints the newest interval;
    /// a stop freezes it). Computed with the **same** bucketing the grouped
    /// list uses, so the returned id always matches a real bucket row when one
    /// exists. `None` when nothing is grouped this way (no items, or the
    /// adapter has no now-anchored data) — the frontend then leaves the pane
    /// as-is. Default returns `None`: adapters without now-anchored grouping
    /// never need to override.
    async fn bucket_for_now(&self, _group_by: &GroupSpec) -> Option<String> {
        None
    }

    /// The refreshed **rows of the now-bucket**, folded against the *live*
    /// current instant — the live-tick counterpart of
    /// [`bucket_for_now`](Self::bucket_for_now) for a grouped tree whose
    /// durations count up. While [`live_rows`](Self::live_rows) ticks the flat
    /// list (its rows are keyed independently of any grouping), a grouped
    /// tree's rows carry grouping-dependent ids the spec-less `live_rows`
    /// can't produce — so the frontend hands in the pane's [`GroupSpec`] and
    /// saved-query `query` and gets back the bucket header (its total re-summed
    /// to *now*) plus the tree rows on the running chain (their own/cumulated
    /// re-folded to *now*), each keyed exactly as the rendered tree row so the
    /// frontend's in-place row patch swaps the ticking cells without a reload.
    ///
    /// Returns only the rows that actually move (the running tasks and their
    /// ancestors, whose cumulated grows, plus the header) — untouched siblings
    /// keep their frozen value and aren't churned. Empty when nothing is
    /// running (no now-bucket) or the bucket is empty. Default returns empty:
    /// adapters without now-anchored grouping never tick a tree.
    async fn live_group_rows(
        &self,
        _group_by: &GroupSpec,
        _query: Option<&str>,
    ) -> Vec<NodeSummary> {
        Vec::new()
    }

    /// Cheap staleness probe, called by the frontend when the user switches
    /// to a tab backed by this adapter. An adapter whose backing store can
    /// change *outside* the process (e.g. the local task/tracking DB written
    /// by the CLI or a waybar module — no in-process domain event fires)
    /// should compare a low-cost fingerprint against its cache and, on
    /// drift, drop the cache and emit [`Invalidation::All`] through its
    /// invalidation stream so the visible panes reload. Must be cheap
    /// enough to run on every tab switch. Default: no-op — pull-only remote
    /// adapters revalidate via their normal reload/HTTP paths instead.
    async fn revalidate(&self) {}

    /// Submit credentials gathered from a [`AdapterStatus::NeedsCreds`]
    /// form. The adapter performs the login and updates its status
    /// channel — `Ok(())` means the login round-trip succeeded; the
    /// frontend should refresh views once the status flips to `Ready`.
    /// Default impl returns `NotSupported` for adapters that do not use
    /// interactive auth.
    async fn submit_credentials(
        &self,
        _fields: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        Err(ContentError::NotSupported(
            "this adapter does not support interactive credential submission".into(),
        ))
    }

    /// The user dismissed a [`AdapterStatus::NeedsCreds`] form without
    /// filling it in. The adapter aborts the login it is waiting on.
    ///
    /// Closing the form is not enough: the login blocks until it gets an
    /// answer, holding the auth lock, so every later attempt queues up
    /// behind a dialog that is no longer on screen. Frontends therefore
    /// call this whenever they take the form away for good.
    async fn cancel_credentials(&self) -> Result<()> {
        Err(ContentError::NotSupported(
            "this adapter does not support interactive credential submission".into(),
        ))
    }

    /// Attempt a silent session refresh (e.g. via OAuth refresh token,
    /// JWT refresh endpoint). Returns `Ok(())` if the live session is
    /// now valid again. Default impl reports `NotSupported`, signalling
    /// the orchestrator to fall back to a full re-authentication.
    async fn try_refresh_session(&self) -> Result<()> {
        Err(ContentError::NotSupported(
            "this adapter does not support session refresh".into(),
        ))
    }

    /// Drop the cached session (derived token / login cookie) and force
    /// the next request to re-authenticate using the configured
    /// credential providers. Primary credentials (keyring entry, file
    /// on disk, …) stay untouched. Default impl is a no-op for adapters
    /// without a session cache.
    async fn invalidate_session(&self) -> Result<()> {
        Ok(())
    }

    /// Drop both the session cache and any cached primary credentials
    /// (e.g. a keyring entry written on first login). The next request
    /// re-runs the providers from scratch, prompting the user when
    /// applicable. Default impl is a no-op.
    async fn invalidate_credentials(&self) -> Result<()> {
        Ok(())
    }

    /// Load the persisted sort spec for a view scope. `scope` is an
    /// opaque key chosen by the frontend (e.g. `"jira:items"`).
    /// Default impl returns an empty list — adapters that don't
    /// persist sort state are simply stateless.
    async fn load_view_sort(&self, _scope: &str) -> Result<Vec<SortKey>> {
        Ok(Vec::new())
    }

    /// Persist the sort spec for a view scope. Empty `sort` should
    /// remove any stored entry. Default impl is a no-op.
    async fn save_view_sort(&self, _scope: &str, _sort: &[SortKey]) -> Result<()> {
        Ok(())
    }

    /// Extract the variables referenced in a saved-query string using the
    /// adapter's own inline syntax (e.g. `${name:default}` for Taiga).
    ///
    /// Returned in source order (or whatever order the adapter chooses to
    /// present them in the input popup). Adapters without variable
    /// support return an empty vec — the frontend then treats the query
    /// as having no variables and skips the popup.
    fn query_variables(&self, _query: &str) -> Vec<QueryVariable> {
        Vec::new()
    }

    /// Substitute variable bindings into a raw saved-query string and
    /// return the final string passed to `list(...)`. Variables not
    /// present in `vars` should fall back to whatever default the
    /// adapter parses from the raw query; if no default exists, the
    /// adapter is free to leave the placeholder, error, or drop the
    /// filter — frontends prevent this case by validating presence
    /// before calling.
    ///
    /// Adapters without variable support keep the identity default
    /// (return `query` verbatim).
    fn render_query(
        &self,
        query: &str,
        _vars: &std::collections::HashMap<String, String>,
    ) -> String {
        query.to_string()
    }

    /// Run a free-form, adapter-native query (e.g. raw SQL for
    /// Postgres). `context` carries adapter-specific addressing data
    /// (for Postgres: the `database` to connect to). Multi-statement
    /// queries are allowed; only the last statement's output is
    /// returned in [`CustomQueryResult`]. Whether this is exposed to
    /// the user at a given drill level is decided by the YAML
    /// `shortcuts:` map binding a key to a [`NodeAction`] whose
    /// [`Node::invoke_action`] returns an
    /// [`ActionDispatch::OpenEditor`] for `session_kind == "query_editor"`.
    ///
    /// Default impl returns `NotSupported`.
    async fn execute_custom_query(
        &self,
        _query: &str,
        _context: &CustomQueryContext,
    ) -> Result<CustomQueryResult> {
        Err(ContentError::NotSupported(
            "this adapter does not support custom queries".into(),
        ))
    }

    /// The [`CustomQueryContext`] addressing data for queries issued from
    /// `node_id` — the routing information [`execute_custom_query`] needs
    /// but that only the adapter can derive.
    ///
    /// Concretely: a SQL adapter's `database` field. The host has to fill
    /// it in when it opens a query editor on some node, and it must not
    /// get that value by parsing the node id — id shapes are the
    /// adapter's own business and differ between backends (Postgres nests
    /// tables under a schema level, a SQLite file has none). So the host
    /// asks instead.
    ///
    /// Defaults to an empty context: adapters that need no addressing,
    /// and adapters with no custom queries at all, need not override.
    ///
    /// [`execute_custom_query`]: ContentAdapter::execute_custom_query
    fn custom_query_context(&self, _node_id: &str) -> CustomQueryContext {
        CustomQueryContext::new()
    }

    /// Adapter-managed persistence for named saved queries.
    ///
    /// `Some(store)` when the adapter owns a flat `(name → query body)`
    /// namespace it persists itself (Jira, Taiga, …). `None` when the
    /// adapter has a different query story (Postgres: per-table SQL
    /// scripts via its own API) or stores nothing at all. Frontends
    /// fall back to the inline `default:` query from view-YAML when
    /// this returns `None`.
    fn saved_query_store(&self) -> Option<&dyn SavedQueryStore> {
        None
    }

    /// Adapter-managed persistence for named *extended* query documents —
    /// Markdown files under `<instance_data_dir>/extended_queries/`.
    ///
    /// Unlike [`saved_query_store`](Self::saved_query_store) this needs no
    /// adapter to do anything: the store is stateless (a root path and a
    /// suffix), so the default builds one on demand from
    /// [`instance_data_dir`](Self::instance_data_dir). An adapter gets
    /// extended queries by having a normal query story at all, which is what
    /// the `saved_query_store().is_some()` gate expresses — Postgres, whose
    /// queries are per-table SQL scripts in its own namespace, keeps
    /// returning `None` here without saying so twice.
    ///
    /// Returns an owned box rather than a borrow for the same reason: there
    /// is no field to borrow from. Decorators need not forward this as long
    /// as they forward `saved_query_store` and `instance_data_dir`, which
    /// all of them do.
    fn extended_query_store(&self) -> Option<Box<dyn ExtendedQueryStore>> {
        self.saved_query_store()?;
        Some(Box::new(FsQueryStore::new(
            self.instance_data_dir().join(EXTENDED_QUERY_DIR),
            EXTENDED_QUERY_SUFFIX,
        )))
    }

    /// File-name suffix (with dot) for a query body when it is opened in the
    /// external editor, so the editor picks the right syntax highlighting.
    ///
    /// Defaults to `.yaml` — the query DSL most adapters use is a YAML
    /// `FilterExpr` document. Adapters whose query is a different language
    /// override this (Jira: `.jql`, Confluence: `.cql`).
    fn query_body_suffix(&self) -> &str {
        ".yaml"
    }

    /// Name of the query language this adapter speaks, as an extended query
    /// document writes it in a fence info-string (```` ```jql mentioned_in ````).
    ///
    /// The default derives it from
    /// [`query_body_suffix`](Self::query_body_suffix) (`.jql` → `jql`), which
    /// is the same information seen from the editor's side — so an adapter
    /// that already names its language for syntax highlighting does not have
    /// to name it twice. Override only where the two genuinely differ.
    fn query_language(&self) -> &str {
        self.query_body_suffix().trim_start_matches('.')
    }

    /// Adapter-managed persistence for editable *scripts* attached to
    /// the content tree.
    ///
    /// `Some(store)` when the adapter lets the user author scripts that
    /// it persists itself (Postgres: SQL scripts, both database-level
    /// and per-table). `None` when the adapter has no such concept —
    /// the default. See [`ScriptStore`] for the two coordinate spaces
    /// (hierarchical database-level paths vs. flat node-scoped names)
    /// and why the contract keeps them backend-opaque.
    fn script_store(&self) -> Option<&dyn ScriptStore> {
        None
    }

    /// Adapter-side full-tree search.
    ///
    /// `Some(results)` when the adapter can return hits with full
    /// ancestor paths (so the TUI can lazy-expand the tree to each
    /// hit). `None` signals "not supported" — the frontend then falls
    /// back to local in-memory filtering over the already-loaded rows.
    ///
    /// `query` is the raw user-typed string; the adapter is responsible
    /// for translating it into its native query language (CQL, JQL,
    /// SQL `ILIKE`, …) and for applying any per-instance scoping
    /// (e.g. space whitelist). `limit` caps the result set; the
    /// adapter signals truncation via [`TreeSearchResults::truncated`].
    ///
    /// The default impl returns `Ok(None)` — adapters opt in by
    /// overriding.
    async fn search_in_tree(&self, _query: &str, _limit: u32) -> Result<Option<TreeSearchResults>> {
        Ok(None)
    }

    /// Resolve a single node id to its ancestor path, for jumping to a
    /// node that may not be loaded yet.
    ///
    /// Same shape as [`TreeFindHit::path`]: the chain of node ids from
    /// the tree root down to and including `node_id`, ready to feed a
    /// lazy-expand driver. This is the addressing counterpart to
    /// [`Self::search_in_tree`] — no query language, no ranking, just
    /// "where does this id live".
    ///
    /// The host calls it when following a stored link (see `NodeRef`)
    /// whose target isn't among the currently loaded rows. `Ok(None)`
    /// means the adapter cannot locate the node — either because it
    /// doesn't offer the feature (the default) or because the id is
    /// gone. Either way the front-end must cope: it can still focus a
    /// node that happens to be loaded already, and otherwise reports
    /// that the link target isn't reachable. Not implementing this
    /// therefore costs deep-link support, nothing else.
    ///
    /// Adapters with a flat root level can return
    /// `Ok(Some(vec![node_id.to_string()]))` once they confirm the node
    /// exists; adapters with [`AdapterCapabilities::unstable_node_ids`]
    /// should leave the default in place, since their ids don't survive
    /// long enough to be linked in the first place.
    async fn locate_node_path(&self, _node_id: &str) -> Result<Option<Vec<String>>> {
        Ok(None)
    }

    /// The lifecycle hook ids this adapter can fire (e.g. `["connected"]`).
    ///
    /// A *hook* is a named point in an adapter's lifetime that a front-end
    /// turns into an action invocation via host configuration (`hooks:` in the
    /// instance's view file): each configured hook binds a `run: <action-id>`
    /// triple that the host invokes — throttled — whenever the hook fires. This
    /// method only *declares* which hook ids are meaningful for the adapter; it
    /// carries no behaviour. The host validates configured hook names against
    /// this list (warning on unknown ones) so typos surface instead of silently
    /// never firing.
    ///
    /// The first hook is `connected` — fired right after the adapter is
    /// successfully constructed (the factory's `create` returned `Ok`). For the
    /// in-process local adapter that is every program start, which is how the
    /// generalised auto-backup works: bind `backup` to `connected` with a 24h
    /// throttle and the database is backed up once a day on first use.
    ///
    /// The default impl returns an empty list (adapter fires no hooks).
    fn hooks(&self) -> Vec<&str> {
        Vec::new()
    }

    /// The [`Anonymizer`](anonymize::Anonymizer) this adapter's user-visible
    /// output is replaced with when anonymization is requested (the host sets
    /// [`HostContext::anonymize`] and wraps the adapter — see the
    /// [`anonymize`](crate::anonymize) module).
    ///
    /// Anonymization is a **contract obligation**, not an opt-in: every adapter
    /// is anonymized when requested. The default returns the domain-agnostic
    /// [`StandardAnonymizer`](anonymize::StandardAnonymizer) — the mandatory,
    /// always-safe fallback (it replaces free text with neutral tokens and
    /// keeps numbers/durations/timestamps verbatim). Adapters that can produce
    /// *plausible* fakes (stable pseudo-names for Tasks/Trackings/Projects,
    /// format-preserving keys for Jira/Taiga) override this to return their own
    /// strategy.
    fn anonymizer(&self) -> std::sync::Arc<dyn anonymize::Anonymizer> {
        std::sync::Arc::new(anonymize::StandardAnonymizer::new())
    }

    /// The **dynamically** described columns for one node type, keyed by its
    /// [`NodeType::type_id`] — columns that aren't part of the adapter's
    /// static declaration because they have to be read from somewhere first.
    ///
    /// This is the second of the two channels that deliver a
    /// [`ColumnSchema`]; the first is [`children::Child::columns`], which an
    /// adapter fills in synchronously for its own list columns.
    /// [`children::columns_for`] unions both, so a front-end never has to know
    /// which channel a column came from.
    ///
    /// The default returns nothing. The custom-columns decorator overrides
    /// this to expose the user's locally stored columns and their types.
    async fn describe_columns(&self, _node_type: &str) -> Vec<ColumnSchema> {
        Vec::new()
    }
}

/// A single item in the content tree.
#[async_trait]
pub trait Node: Send + Sync {
    fn id(&self) -> &str;
    fn label(&self) -> &str;
    fn node_type(&self) -> &NodeType;
    fn metadata(&self) -> &Metadata;

    /// Populate the display fields (`label`, `metadata`) that a lazily
    /// constructed node leaves as placeholders.
    ///
    /// Several adapters build a node from just an id/key without a network
    /// round-trip — [`ContentAdapter::get_by_id`] does this so child-only
    /// operations (`list`, `invoke_action`) don't pay for a detail fetch they
    /// don't need, and so a user who can see an issue in search but lacks
    /// read permission on it still gets a usable node. Such a stub reports its
    /// **id** as `label()` and a sparse `metadata()` until its detail is first
    /// awaited.
    ///
    /// Consumers that read those display fields *directly* off a re-resolved
    /// node — the post-edit row patch (`patch_content_row`) is the only one —
    /// must call `hydrate` first. Without it the patched row shows the id
    /// instead of the title and drops every other column until a full reload.
    /// Centralising the call here (rather than eagerly hydrating inside each
    /// adapter's `get_by_id`) keeps `get_by_id` uniformly cheap and stops the
    /// fix from being re-derived per adapter.
    ///
    /// Default = no-op: adapters whose nodes are display-ready on construction
    /// (eager fetchers like Taiga, in-memory local Tasks/Trackings, the mock)
    /// need not override it. A failed hydration must degrade gracefully —
    /// leave the stub, never panic.
    async fn hydrate(&mut self) {}

    /// The **list-row** projection of this node — the [`NodeSummary`] it would
    /// occupy if it appeared in its parent's [`Node::list`] result.
    ///
    /// The post-edit row patch (`patch_content_row`, the only consumer) refreshes
    /// a row in place by re-fetching the node and copying its fresh values back
    /// over the visible row. It needs the *row*-shaped projection, **not**
    /// [`Node::metadata`]: for several adapters `metadata()` is a *detail*
    /// projection with a different key set (Jira carries `summary` and editable
    /// flags but no `updated`/`attachments`) or is intentionally empty (Taiga's
    /// by-id nodes render no metadata table). Copying that over a list row drops
    /// or reshapes columns until a full reload — exactly the "edit doesn't
    /// refresh the row" symptom.
    ///
    /// Default = assemble from `label`/`node_type`/`metadata`, reproducing the
    /// long-standing patch behaviour. Adapters whose `metadata()` diverges from
    /// their `list()` row **must** override this to rebuild the `list()` shape
    /// from their detail. Fields the detail can't supply (e.g. an attachment
    /// count not carried in a detail fetch) may be omitted: the patch merges by
    /// key via [`Metadata::set_field`] and keeps the row's last-known value for
    /// any key this projection omits.
    fn row_summary(&self) -> NodeSummary {
        NodeSummary {
            id: self.id().to_string(),
            label: self.label().to_string(),
            node_type: self.node_type().clone(),
            metadata: self.metadata().clone(),
            has_children: None,
        }
    }

    /// Navigate to a specific child by ID.
    ///
    /// The single-child navigation primitive an adapter's own
    /// [`ContentAdapter::get_by_id`] walks segment-by-segment. Distinct from
    /// the *listing* surface ([`ContentAdapter::childs`]), which is the single
    /// source of truth about which child *types* exist, how they sort, and how
    /// they are fetched. Default = not found (leaf nodes never navigate).
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        Err(ContentError::NotFound(id.to_string()))
    }

    /// Read-only content body, if any.
    fn content(&self) -> Option<&dyn Content> {
        None
    }

    /// Dispatch a shortcut-bound action by its `id`. Distinct from
    /// [`Node::execute`] (which completes a menu-driven action with a
    /// user input). `invoke_action` decides what UI flow the TUI should
    /// start — open an editor, execute a query, create a child, …
    ///
    /// Default impl returns [`ActionDispatch::Noop`] so adapters can
    /// adopt the new dispatch path incrementally. Adapters that don't
    /// implement this method simply have non-functional shortcuts;
    /// existing menu/picker dispatch through [`Node::execute`] is
    /// unaffected.
    async fn invoke_action(&self, _name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
        Ok(ActionDispatch::Noop)
    }

    /// Render the initial buffer for an `InputSpec::Editor` action.
    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        let _ = action_id;
        Err(ContentError::NotSupported("prepare not supported".into()))
    }

    /// Fetch the option list for an `InputSpec::Picker` action.
    async fn picker_options(&self, action_id: &str) -> Result<Vec<ActionOption>> {
        let _ = action_id;
        Ok(Vec::new())
    }

    /// Prefill values for an `InputSpec::Form` action, keyed by
    /// [`FormFieldSpec::key`]. Used by edit flows to seed the form with the
    /// node's current values; keys without an entry fall back to the
    /// field's [`FormFieldSpec::default`]. The default impl returns no
    /// prefills (suitable for a create flow).
    async fn form_prep(
        &self,
        action_id: &str,
    ) -> Result<std::collections::HashMap<String, String>> {
        let _ = action_id;
        Ok(std::collections::HashMap::new())
    }

    /// Execute an action with the user's input.
    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        let _ = (action_id, input);
        Err(ContentError::NotSupported("execute not supported".into()))
    }
}

/// Read-only content access.
#[async_trait]
pub trait Content: Send + Sync {
    fn node_type(&self) -> &NodeType;

    /// Version identifier for conflict detection.
    fn version(&self) -> Option<&str>;

    /// Download the content body.
    async fn read(&self) -> Result<Vec<u8>>;

    /// Read as text (convenience, fails for binary).
    async fn read_text(&self) -> Result<String> {
        let bytes = self.read().await?;
        String::from_utf8(bytes).map_err(|e| ContentError::Other(Box::new(e)))
    }
}

// ---------------------------------------------------------------------------
// SavedQueryStore / ExtendedQueryStore
// ---------------------------------------------------------------------------

/// Adapter-owned persistence layer for named saved queries.
///
/// Storage and file format are the adapter's concern — frontends only
/// see a flat namespace of `(name → raw query body)` pairs. The body is
/// adapter-specific text (a JQL string for Jira, a YAML block for
/// Taiga, …) parsed by the same code path that handles the inline
/// `default:` query in view-YAML. Adapters typically write under
/// [`ContentAdapter::instance_data_dir`]`/queries/<name>.yaml`, but
/// implementations are free to pick any layout (a single database
/// row, a remote KV store, …) as long as the trait contract holds.
#[async_trait]
pub trait SavedQueryStore: Send + Sync {
    /// All saved-query names known to this adapter instance, sorted
    /// for stable UI listing. Empty when nothing has been saved yet.
    async fn list(&self) -> Result<Vec<String>>;

    /// Raw query body for `name`. Returns
    /// [`ContentError::NotFound`] when the entry doesn't exist.
    async fn load(&self, name: &str) -> Result<String>;

    /// Persist `body` under `name`. Creates the entry if missing,
    /// overwrites if present. Implementations are responsible for
    /// creating any parent directories.
    async fn save(&self, name: &str, body: &str) -> Result<()>;

    /// Remove the entry. Missing entries are not an error (idempotent
    /// delete).
    async fn delete(&self, name: &str) -> Result<()>;

    /// Optional: the on-disk path of the entry, for adapters that
    /// expose one. Used by `:query edit` to launch a `FileEditSession`
    /// against the file. Adapters without a filesystem-backed store
    /// return `None`; the frontend falls back to a text-buffer edit
    /// via `load`/`save`.
    fn path(&self, _name: &str) -> Option<std::path::PathBuf> {
        None
    }
}

/// Adapter-owned persistence layer for named *extended* query documents.
///
/// Same five methods as [`SavedQueryStore`], deliberately a distinct type:
/// the two namespaces hold bodies in different languages (an adapter-native
/// query vs. a Markdown document combining several of them), and a value
/// that could stand for either would defeat the whole point of recording a
/// [`QueryKind`] alongside a stored name. `list()` is rendered 1:1 into the
/// query menu, so a consumer holding a `dyn ExtendedQueryStore` knows,
/// without probing, how to interpret what comes back from `load`.
#[async_trait]
pub trait ExtendedQueryStore: Send + Sync {
    /// All extended-query names known to this adapter instance, sorted
    /// for stable UI listing. Empty when nothing has been saved yet.
    async fn list(&self) -> Result<Vec<String>>;

    /// Raw document text for `name` (Markdown container). Returns
    /// [`ContentError::NotFound`] when the entry doesn't exist.
    async fn load(&self, name: &str) -> Result<String>;

    /// Persist `body` under `name`. Creates the entry if missing,
    /// overwrites if present. Implementations are responsible for
    /// creating any parent directories.
    async fn save(&self, name: &str, body: &str) -> Result<()>;

    /// Remove the entry. Missing entries are not an error (idempotent
    /// delete).
    async fn delete(&self, name: &str) -> Result<()>;

    /// Optional: the on-disk path of the entry, so the frontend can edit
    /// the document as a file (with Markdown highlighting) instead of a
    /// text buffer. See [`SavedQueryStore::path`].
    fn path(&self, _name: &str) -> Option<std::path::PathBuf> {
        None
    }
}

/// Directory name (under [`ContentAdapter::instance_data_dir`]) holding
/// extended-query documents. Sibling of `queries/`, not a subdirectory of
/// it — one flat namespace per kind (plan section 4).
pub const EXTENDED_QUERY_DIR: &str = "extended_queries";

/// File-name suffix for extended-query documents. Markdown, and the same
/// for every adapter: the container is the framework's format, not the
/// adapter's — only the fences inside it speak the adapter's language.
pub const EXTENDED_QUERY_SUFFIX: &str = ".md";

/// Filesystem-backed query store. One file per query under
/// `<root>/<name><suffix>`; it implements [`SavedQueryStore`] *and*
/// [`ExtendedQueryStore`], since both are the same file layout under a
/// different root and suffix. Which trait a given instance serves is decided
/// by how it was constructed, not by a flag it carries.
///
/// For saved queries, `suffix` is the adapter's
/// [`ContentAdapter::query_body_suffix`] — a Jira query is a `.jql` file, a
/// Confluence one `.cql`, the FilterExpr-based adapters keep `.yaml`. Pass
/// the *same* constant to both so the stored file and the name the external
/// editor sees can't drift apart. For extended queries it is
/// [`EXTENDED_QUERY_SUFFIX`].
///
/// Names are passed through unchanged (no escaping) — adapters that need
/// exotic characters in query names should layer their own validation on
/// top. The root directory is created lazily on first `save`.
///
/// The suffix is exact in both directions: only files carrying it are listed,
/// loaded and deleted. Bodies stored under any other extension are invisible
/// to this store — a breaking change against the versions that hard-coded
/// `.yaml` for every adapter, which requires renaming those files once.
pub struct FsQueryStore {
    root: std::path::PathBuf,
    /// File-name suffix *with* the leading dot, e.g. `".jql"`.
    suffix: String,
}

impl FsQueryStore {
    /// Use `<instance_data_dir>/queries/` as the storage root and `suffix`
    /// (with the leading dot) as the file extension.
    pub fn new(root: std::path::PathBuf, suffix: &str) -> Self {
        Self {
            root,
            suffix: suffix.to_string(),
        }
    }

    fn file_path(&self, name: &str) -> std::path::PathBuf {
        self.root.join(format!("{name}{}", self.suffix))
    }

    /// Whether a directory entry belongs to this store. Only the configured
    /// suffix counts — a body written under a different extension is not this
    /// store's business, even if the store used to write `.yaml` for everyone.
    fn is_query_file(&self, path: &std::path::Path) -> bool {
        path.file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|name| name.ends_with(&self.suffix))
    }

    /// Shared body of both trait impls — see [`SavedQueryStore::list`].
    async fn list_names(&self) -> Result<Vec<String>> {
        let mut rd = match tokio::fs::read_dir(&self.root).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ContentError::Other(Box::new(e))),
        };
        let mut names = Vec::new();
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?
        {
            let path = entry.path();
            if !self.is_query_file(&path) {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.push(stem.to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    /// Shared body of both trait impls — see [`SavedQueryStore::load`].
    async fn read(&self, name: &str) -> Result<String> {
        match tokio::fs::read_to_string(self.file_path(name)).await {
            Ok(s) => Ok(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(ContentError::NotFound(name.to_string()))
            }
            Err(e) => Err(ContentError::Other(Box::new(e))),
        }
    }

    /// Shared body of both trait impls — see [`SavedQueryStore::save`].
    async fn write(&self, name: &str, body: &str) -> Result<()> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;
        tokio::fs::write(self.file_path(name), body)
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;
        Ok(())
    }

    /// Shared body of both trait impls — see [`SavedQueryStore::delete`].
    async fn remove(&self, name: &str) -> Result<()> {
        match tokio::fs::remove_file(self.file_path(name)).await {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(ContentError::Other(Box::new(e))),
        }
    }
}

#[async_trait]
impl SavedQueryStore for FsQueryStore {
    async fn list(&self) -> Result<Vec<String>> {
        self.list_names().await
    }

    async fn load(&self, name: &str) -> Result<String> {
        self.read(name).await
    }

    async fn save(&self, name: &str, body: &str) -> Result<()> {
        self.write(name, body).await
    }

    async fn delete(&self, name: &str) -> Result<()> {
        self.remove(name).await
    }

    fn path(&self, name: &str) -> Option<std::path::PathBuf> {
        Some(self.file_path(name))
    }
}

#[async_trait]
impl ExtendedQueryStore for FsQueryStore {
    async fn list(&self) -> Result<Vec<String>> {
        self.list_names().await
    }

    async fn load(&self, name: &str) -> Result<String> {
        self.read(name).await
    }

    async fn save(&self, name: &str, body: &str) -> Result<()> {
        self.write(name, body).await
    }

    async fn delete(&self, name: &str) -> Result<()> {
        self.remove(name).await
    }

    fn path(&self, name: &str) -> Option<std::path::PathBuf> {
        Some(self.file_path(name))
    }
}

// ---------------------------------------------------------------------------
// Query kinds
// ---------------------------------------------------------------------------

/// Which of the two stores owns a query body.
///
/// The distinction is invisible in the query menu — a name is a name, and
/// that one of them fans out to three backend calls is the framework's
/// business. It matters only where a *stored* reference has to find its body
/// again: a `query_shortcut` row and the `default_query:{scope}` setting both
/// record the kind so resolution goes straight to the owning store instead of
/// probing both (plan section 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum QueryKind {
    /// A single adapter-native query body, in [`SavedQueryStore`].
    #[default]
    Saved,
    /// A Markdown document combining several of them, in
    /// [`ExtendedQueryStore`].
    Extended,
}

impl QueryKind {
    /// Stable wire form used in DB columns and setting values.
    pub fn as_str(self) -> &'static str {
        match self {
            QueryKind::Saved => "saved",
            QueryKind::Extended => "extended",
        }
    }

    /// Parse the wire form. Anything else is `None` — callers decide
    /// whether that means "legacy value" or "corrupt row".
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "saved" => Some(QueryKind::Saved),
            "extended" => Some(QueryKind::Extended),
            _ => None,
        }
    }
}

impl std::fmt::Display for QueryKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A named query plus the store that owns it — what the
/// `default_query:{scope}` setting holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultQuery {
    pub kind: QueryKind,
    pub name: String,
}

impl DefaultQuery {
    pub fn saved(name: impl Into<String>) -> Self {
        Self {
            kind: QueryKind::Saved,
            name: name.into(),
        }
    }

    pub fn extended(name: impl Into<String>) -> Self {
        Self {
            kind: QueryKind::Extended,
            name: name.into(),
        }
    }

    /// Encode for the settings row: `saved:Name` / `extended:Name`.
    pub fn to_setting(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.name)
    }

    /// Decode a settings value.
    ///
    /// Splits at the *first* colon and accepts the prefix only when it is
    /// exactly one of the two kinds; anything else is a value written before
    /// kinds existed and is taken whole as a saved-query name. That is what
    /// keeps a name containing a colon readable in both directions — the
    /// alternative (split at the last colon, or reject unknown prefixes)
    /// would silently lose such a default.
    pub fn from_setting(value: &str) -> Self {
        match value.split_once(':') {
            Some((prefix, rest)) => match QueryKind::from_str(prefix) {
                Some(kind) => Self {
                    kind,
                    name: rest.to_string(),
                },
                None => Self::saved(value),
            },
            None => Self::saved(value),
        }
    }
}

/// Which store, if any, already holds `name` for this adapter.
///
/// Names are unique across *both* stores per scope (plan section 4): two menu
/// entries called `foo` would be meaningless to a user who cannot see the
/// difference in the first place. Creation calls this and refuses a
/// collision; the returned kind is what lets the frontend offer to open the
/// existing entry instead.
///
/// `Ok(None)` means the name is free. A store that fails to list is an error,
/// not a free name — reporting "free" on an unreadable directory would
/// overwrite a body that is merely unreachable right now.
pub async fn existing_query_kind(
    adapter: &dyn ContentAdapter,
    name: &str,
) -> Result<Option<QueryKind>> {
    if let Some(store) = adapter.saved_query_store()
        && store.list().await?.iter().any(|n| n == name)
    {
        return Ok(Some(QueryKind::Saved));
    }
    if let Some(store) = adapter.extended_query_store()
        && store.list().await?.iter().any(|n| n == name)
    {
        return Ok(Some(QueryKind::Extended));
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// BookmarkStore
// ---------------------------------------------------------------------------

/// A single bookmarked entry: an adapter-opaque `id` plus the time it was
/// bookmarked (RFC3339, UTC). Adapters decide what `id` means — for the
/// Jira adapter it is the issue key, for others it could be any stable
/// node identifier.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Bookmark {
    pub id: String,
    /// RFC3339 timestamp of when the bookmark was added.
    pub bookmarked_at: String,
}

/// Adapter-owned persistence for bookmarked nodes.
///
/// The store is intentionally adapter-agnostic: it knows only opaque `id`
/// strings and a timestamp, so any adapter can reuse it to let users pin
/// nodes and revisit them in a dedicated view. Bookmarking is a toggle —
/// adding an existing id removes it.
#[async_trait]
pub trait BookmarkStore: Send + Sync {
    /// All bookmarks for this adapter instance. Order is the on-disk
    /// order (insertion order); callers that want a particular ordering
    /// sort themselves. Empty when nothing has been bookmarked yet.
    async fn list(&self) -> Result<Vec<Bookmark>>;

    /// Whether `id` is currently bookmarked.
    async fn contains(&self, id: &str) -> Result<bool>;

    /// Toggle the bookmark for `id`. Adds it (stamping the current UTC
    /// time) when missing, removes it when present. Returns `true` when
    /// the id is now bookmarked, `false` when it was just removed.
    async fn toggle(&self, id: &str) -> Result<bool>;
}

/// Filesystem-backed [`BookmarkStore`]. Persists the whole set as a single
/// `bookmarks.yaml` (a YAML sequence of [`Bookmark`]) under `<root>`. The
/// directory is created lazily on first write; a missing file reads as an
/// empty set.
pub struct FsBookmarkStore {
    root: std::path::PathBuf,
}

impl FsBookmarkStore {
    /// Use `<instance_data_dir>` as the storage root; the set lives in
    /// `<root>/bookmarks.yaml`.
    pub fn new(root: std::path::PathBuf) -> Self {
        Self { root }
    }

    fn file_path(&self) -> std::path::PathBuf {
        self.root.join("bookmarks.yaml")
    }

    async fn read_all(&self) -> Result<Vec<Bookmark>> {
        match tokio::fs::read_to_string(self.file_path()).await {
            Ok(s) if s.trim().is_empty() => Ok(Vec::new()),
            Ok(s) => serde_yaml::from_str(&s).map_err(|e| ContentError::Other(Box::new(e))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(ContentError::Other(Box::new(e))),
        }
    }

    async fn write_all(&self, bookmarks: &[Bookmark]) -> Result<()> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;
        let body =
            serde_yaml::to_string(bookmarks).map_err(|e| ContentError::Other(Box::new(e)))?;
        tokio::fs::write(self.file_path(), body)
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;
        Ok(())
    }
}

#[async_trait]
impl BookmarkStore for FsBookmarkStore {
    async fn list(&self) -> Result<Vec<Bookmark>> {
        self.read_all().await
    }

    async fn contains(&self, id: &str) -> Result<bool> {
        Ok(self.read_all().await?.iter().any(|b| b.id == id))
    }

    async fn toggle(&self, id: &str) -> Result<bool> {
        let mut bookmarks = self.read_all().await?;
        if let Some(pos) = bookmarks.iter().position(|b| b.id == id) {
            bookmarks.remove(pos);
            self.write_all(&bookmarks).await?;
            Ok(false)
        } else {
            bookmarks.push(Bookmark {
                id: id.to_string(),
                bookmarked_at: chrono::Utc::now().to_rfc3339(),
            });
            self.write_all(&bookmarks).await?;
            Ok(true)
        }
    }
}

// ---------------------------------------------------------------------------
// ScriptStore
// ---------------------------------------------------------------------------

/// Adapter-owned persistence for editable scripts attached to the
/// content tree.
///
/// This decouples the frontend from any one adapter's storage: the TUI
/// used to call `not_yet_done_postgres_adapter::query::*` directly for
/// every script CRUD operation, which baked Postgres' on-disk layout
/// into the host. The store hides that layout behind two coordinate
/// spaces, both **backend-opaque** — the contract carries no notion of
/// `schema`/`table`/file-extension, so a future non-SQL adapter can
/// back the same operations with whatever storage it likes:
///
/// - **Database-level** scripts live in a *hierarchical* namespace
///   keyed by `(database, rel_path)`. `rel_path` is a forward-slash
///   relative path the adapter interprets — directories and nested
///   scripts are allowed. These are the `db_scripts/<db>/…` entries.
/// - **Node-scoped** scripts live in a *flat* per-node namespace keyed
///   by `(node_id, name)`. `node_id` is the canonical node path string
///   the adapter already understands; the adapter parses it back into
///   whatever internal coordinates it needs. These are the per-table
///   query scripts.
///
/// Most methods return [`crate::Result`]; filesystem errors surface as
/// [`ContentError::Other`] so their `Display` (e.g. "directory not
/// empty (3 entries)") reaches the user unchanged. The path/template
/// accessors are sync and infallible — they only compute, never touch
/// storage.
#[async_trait]
pub trait ScriptStore: Send + Sync {
    // --- Addressing and templates -------------------------------------
    //
    // The host needs these to open a script in an external editor: it
    // has to know which file to hand the editor (so an LSP and the
    // adapter agree on one path) and what to seed a brand-new script
    // with. Both are layout decisions, so both belong to the adapter —
    // the host asks and does not compute.

    /// On-disk path of the database-level script at `rel_path` under
    /// `database`.
    fn db_script_path(&self, database: &str, rel_path: &str) -> std::path::PathBuf;

    /// Contents to seed a brand-new database-level script with. Should
    /// go through [`crate::script_buffer::default_buffer`] so the host's
    /// parser finds the usual marker.
    fn default_db_script_body(&self, database: &str, rel_path: &str) -> String;

    /// On-disk path of the node-scoped script `name` attached to
    /// `node_id`, or `None` when `node_id` names no script namespace
    /// (e.g. a level that owns no scripts, or an id shape this adapter
    /// does not recognise).
    fn node_script_path(&self, node_id: &str, name: &str) -> Option<std::path::PathBuf>;

    /// Contents to seed a brand-new node-scoped script for `node_id`
    /// with — typically a starter query against whatever that node
    /// addresses.
    fn default_node_script_body(&self, node_id: &str) -> String;

    /// Name the host opens when the user asks for "the" script of a node
    /// without naming one (the `Q` editor's implicit script).
    fn default_node_script_name(&self) -> &str {
        "default"
    }

    // --- Database-level (hierarchical) --------------------------------

    /// Whether `rel_path` under `database` is a directory. A missing
    /// entry (or any probe error) reports `false` — callers use this
    /// only to choose between file- and directory-semantics.
    async fn db_entry_is_dir(&self, database: &str, rel_path: &str) -> bool;

    /// Create a script at `rel_path` under `database`, seeding it with
    /// the adapter's default template and creating any parent
    /// directories. Returns `Ok(true)` when created, `Ok(false)` when a
    /// file already existed there (left untouched).
    async fn create_db_script(&self, database: &str, rel_path: &str) -> Result<bool>;

    /// Create an (empty) directory at `rel_path` under `database`,
    /// including parents. Idempotent if it already exists.
    async fn create_db_dir(&self, database: &str, rel_path: &str) -> Result<()>;

    /// Rename the entry at `rel_path` (file or directory) to
    /// `new_name`, keeping it in the same parent directory.
    async fn rename_db_entry(&self, database: &str, rel_path: &str, new_name: &str) -> Result<()>;

    /// Move the entry at `src` to `dst` (both `rel_path`s under
    /// `database`). Parent directories of `dst` are created as needed.
    async fn move_db_entry(&self, database: &str, src: &str, dst: &str) -> Result<()>;

    /// Delete the script at `rel_path`. Missing files are not an error
    /// (idempotent delete).
    async fn delete_db_script(&self, database: &str, rel_path: &str) -> Result<()>;

    /// Delete the directory at `rel_path`. Errors if it is non-empty —
    /// the error `Display` names the entry count.
    async fn delete_db_dir(&self, database: &str, rel_path: &str) -> Result<()>;

    // --- Node-scoped (flat) -------------------------------------------

    /// Names of all scripts attached to `node_id`, sorted for stable
    /// listing. Empty when none exist (or the node has no script
    /// namespace).
    async fn list_node_scripts(&self, node_id: &str) -> Result<Vec<String>>;

    /// Delete the script `name` attached to `node_id`. Missing entries
    /// are not an error (idempotent delete).
    async fn delete_node_script(&self, node_id: &str, name: &str) -> Result<()>;
}

// ---------------------------------------------------------------------------
// Host event bus (cross-adapter coordination)
// ---------------------------------------------------------------------------

/// An opaque cross-adapter event. The host delivers it verbatim to every
/// subscriber of the same channel **without inspecting it** — the payload's
/// meaning is a private contract between the adapters that share a channel.
///
/// The type erasure is what keeps the host (and this contract crate) free of
/// any adapter's domain types: the host is a dumb broker. Adapters that agree
/// on a channel downcast the `Arc<dyn Any>` back to their shared concrete
/// event type (e.g. the local Tasks/Trackings adapters exchange a task-domain
/// event); an adapter that doesn't recognise a payload simply ignores it.
/// Essential for out-of-process plugins, which cannot share a global
/// singleton and must receive their coordination channel from the host.
pub type HostEvent = std::sync::Arc<dyn std::any::Any + Send + Sync>;

/// A host-provided publish/subscribe bus, keyed by an opaque channel string.
///
/// The **host** (the App that composes the adapters) owns one implementation
/// and hands it to every adapter via [`HostContext`] at construction.
/// Adapters use it to coordinate across instances the host wired into the
/// same session — e.g. a tracking toggle in the Tasks view repainting the
/// Trackings view — without any adapter referencing another, and without this
/// contract crate knowing the payload type.
///
/// Channels scope delivery: a `publish` reaches only the `subscribe`rs of the
/// same channel string. Adapters backed by the same data source pick a
/// shared, stable channel (the local adapters use their database DSN) so two
/// instances over one store coordinate while unrelated adapters stay silent.
pub trait HostEventBus: Send + Sync {
    /// Publish `event` to `channel`; delivered to all current subscribers of
    /// that channel. Lossy by design: with no subscriber it is dropped.
    fn publish(&self, channel: &str, event: HostEvent);

    /// Subscribe to `channel`, receiving every event published to it from now
    /// on. A `broadcast` receiver because events are discrete and one channel
    /// may have several independent subscribers.
    fn subscribe(&self, channel: &str) -> tokio::sync::broadcast::Receiver<HostEvent>;

    /// How many live subscribers `channel` currently has.
    ///
    /// Lets an emitter that *waits for a reply* (e.g. the office365-web login
    /// waiting for a typed MFA code) abort cleanly when nobody is listening,
    /// instead of blocking forever on an answer that can never arrive — the
    /// "no consumer → clean cancel" contract. Fire-and-forget publishers ignore
    /// it. Zero is a valid transient answer (a subscriber may arm a moment
    /// later), so treat it as advisory, not a guarantee.
    fn receiver_count(&self, channel: &str) -> usize;
}

/// The single well-known [`HostEventBus`] channel carrying [`BusEvent`]s.
///
/// The DSN-keyed channels the local adapters use for [`DomainEvent`]-style
/// coordination are a *different* payload type and a different channel space;
/// this one is reserved for the topic-routed [`BusEvent`] traffic the TUI's
/// rule engine consumes. Keeping it a single global channel means a subscriber
/// arms exactly one receiver and routes purely by [`BusEvent::topic`] — no
/// channel-name contract has to be shared between an emitter and a consumer,
/// which is what preserves the loose coupling (an adapter that emits an event
/// need not know which view, if any, reacts to it).
pub const EVENT_CHANNEL: &str = "events";

/// A topic-routed, self-describing event exchanged over the [`EVENT_CHANNEL`].
///
/// Unlike the opaque [`DomainEvent`] payloads the local adapters downcast, a
/// `BusEvent` is a *shared* concrete type: any emitter (an adapter backend, or
/// the host itself) fills it in, and any consumer (chiefly the TUI rule
/// engine) matches on [`topic`](Self::topic) and reads
/// [`payload`](Self::payload) as JSON. This is what lets an adapter drive a
/// UI reaction — e.g. an MFA number-match prompt — without a Cargo dependency
/// in either direction: the contract is the string topic plus a JSON shape,
/// both of which live only in configuration and this struct.
///
/// Published as an [`Arc<dyn Any>`](HostEvent); consumers downcast with
/// [`from_host_event`](BusEvent::from_host_event).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BusEvent {
    /// Namespaced routing key, e.g. `"office365-web:mfa:number-match"`. The
    /// rule engine matches an `event_actions` binding's `on:` against this.
    pub topic: String,
    /// Who emitted it — typically the emitting adapter instance's id. Lets a
    /// consumer and any response scope to the *right* origin when several
    /// instances of the same adapter run at once (e.g. two calendar
    /// connections both mid-login).
    pub source: String,
    /// Event-specific data as JSON (e.g. `{"number": 42}`). Actions template
    /// fields out of it; a request/response pair carries the answer back the
    /// same way.
    #[serde(default)]
    pub payload: serde_json::Value,
    /// Ties a response event to the request that prompted it. An emitter that
    /// expects a reply sets it; the rule engine copies it onto whatever the
    /// bound action emits, so the emitter can match the answer to its request
    /// even with several in flight.
    #[serde(default)]
    pub correlation_id: Option<String>,
}

impl BusEvent {
    /// Construct a `BusEvent` with no correlation id (a fire-and-forget
    /// notification or a request that expects no reply).
    pub fn new(
        topic: impl Into<String>,
        source: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            topic: topic.into(),
            source: source.into(),
            payload,
            correlation_id: None,
        }
    }

    /// Builder-style: attach a correlation id (marks this as a request whose
    /// reply must carry the same id back).
    pub fn with_correlation(mut self, correlation_id: impl Into<String>) -> Self {
        self.correlation_id = Some(correlation_id.into());
        self
    }

    /// Downcast an opaque [`HostEvent`] back to a `BusEvent`. `None` for any
    /// other payload type sharing the channel (keeps consumers total).
    pub fn from_host_event(event: &HostEvent) -> Option<Self> {
        event.downcast_ref::<BusEvent>().cloned()
    }
}

/// Publish a [`BusEvent`] on the well-known [`EVENT_CHANNEL`]. Convenience
/// wrapper over [`HostEventBus::publish`] so emitters need not import
/// [`Arc`](std::sync::Arc) or know the channel name.
pub fn publish_event(bus: &dyn HostEventBus, event: BusEvent) {
    bus.publish(EVENT_CHANNEL, std::sync::Arc::new(event));
}

/// Subscribe to the well-known [`EVENT_CHANNEL`]. The receiver yields opaque
/// [`HostEvent`]s; use [`BusEvent::from_host_event`] to recover the typed
/// event (and ignore anything that is not a `BusEvent`).
pub fn subscribe_events(bus: &dyn HostEventBus) -> tokio::sync::broadcast::Receiver<HostEvent> {
    bus.subscribe(EVENT_CHANNEL)
}

/// Capabilities the host injects into every adapter at construction
/// ([`AdapterFactory::create`]). A struct (not a bare bus) so future
/// host-provided handles can be added without churning every factory
/// signature again.
#[derive(Clone)]
pub struct HostContext {
    /// The cross-adapter coordination bus (see [`HostEventBus`]).
    pub event_bus: std::sync::Arc<dyn HostEventBus>,
    /// When set, the host wraps every adapter it builds so all user-visible
    /// output is replaced with plausible fake data — for screenshots/screencasts
    /// against a live productive instance. Off by default; the host turns it on
    /// from configuration/environment. See the [`anonymize`](crate::anonymize)
    /// module for the mechanism and what is / isn't scrubbed.
    pub anonymize: bool,
}

/// A ready-made in-process [`HostEventBus`] the host can instantiate directly
/// (`Arc::new(InMemoryHostBus::default())`) instead of writing its own broker.
/// Lazily creates one `broadcast` sender per channel on first use and keeps it
/// alive for the process, so a late subscriber to an already-used channel
/// still works.
pub struct InMemoryHostBus {
    channels: std::sync::Mutex<
        std::collections::HashMap<String, tokio::sync::broadcast::Sender<HostEvent>>,
    >,
    capacity: usize,
}

impl InMemoryHostBus {
    /// Create a bus whose per-channel broadcast buffers hold `capacity`
    /// events. A subscriber that lags past `capacity` observes a
    /// `RecvError::Lagged`; adapters resync conservatively on it.
    pub fn new(capacity: usize) -> Self {
        Self {
            channels: std::sync::Mutex::new(std::collections::HashMap::new()),
            capacity,
        }
    }

    fn sender(&self, channel: &str) -> tokio::sync::broadcast::Sender<HostEvent> {
        self.channels
            .lock()
            .unwrap()
            .entry(channel.to_string())
            .or_insert_with(|| tokio::sync::broadcast::channel(self.capacity).0)
            .clone()
    }
}

impl Default for InMemoryHostBus {
    fn default() -> Self {
        Self::new(256)
    }
}

impl HostEventBus for InMemoryHostBus {
    fn publish(&self, channel: &str, event: HostEvent) {
        let _ = self.sender(channel).send(event);
    }

    fn subscribe(&self, channel: &str) -> tokio::sync::broadcast::Receiver<HostEvent> {
        self.sender(channel).subscribe()
    }

    fn receiver_count(&self, channel: &str) -> usize {
        self.channels
            .lock()
            .unwrap()
            .get(channel)
            .map(|tx| tx.receiver_count())
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// AdapterFactory (registry pattern)
// ---------------------------------------------------------------------------

/// Object-safe factory stored in the adapter registry.
///
/// Every concrete leaf factory is written as a [`TypedAdapterFactory`] and
/// lifted into this facade via [`typed`]; the transparent decorators
/// (anonymizing / custom-columns) implement it directly and forward
/// [`config_schema`](AdapterFactory::config_schema) to their inner factory.
pub trait AdapterFactory: Send + Sync {
    /// Adapter type name (e.g. "jira", "confluence").
    fn adapter_type(&self) -> &str;

    /// Create an adapter from an opaque config string (YAML/JSON).
    /// `instance_id` comes from the YAML `adapter.id:` field, with the
    /// adapter type as fallback default. The factory must thread it
    /// into the produced adapter so [`ContentAdapter::instance_id`]
    /// returns it.
    ///
    /// `ctx` carries the host-provided capabilities (currently the
    /// cross-adapter [`HostEventBus`]). Remote adapters ignore it; the local
    /// in-process adapters use the bus to coordinate (see the local-adapter
    /// crate). Passing it here — rather than capturing it in the factory —
    /// keeps factories stateless and is the seam a plugin host injects through.
    fn create(
        &self,
        instance_id: &str,
        config: &str,
        ctx: &HostContext,
    ) -> Result<Box<dyn ContentAdapter>>;

    /// Reflect this adapter type's configuration into a runtime schema.
    ///
    /// Frontends (the CLI config-template command, the TUI form) consume the
    /// schema to render or validate a config *without* a live instance. For a
    /// factory built from a [`TypedAdapterFactory`] this is derived
    /// automatically from the associated `Config` type, so the schema can
    /// never drift from what [`create`](AdapterFactory::create) deserializes.
    fn config_schema(&self) -> fieldsmith::TypeSchema;

    /// The authentication mechanisms this adapter type implements, with
    /// the input fields each one needs.
    ///
    /// The empty default means "no authentication" — true for the local
    /// adapters (tasks, trackings, projects, sqlite). Like
    /// [`config_schema`](AdapterFactory::config_schema) this is readable
    /// *without* a live instance, which is what lets the config wizard
    /// offer the choices and `nyd adapter <type> help` list them from the
    /// same source the validation uses.
    fn auth_mechanisms(&self) -> &'static [MechanismSpec] {
        &[]
    }
}

/// Typed adapter factory — what every concrete adapter implements.
///
/// The associated [`Config`](Self::Config) type is the single source of
/// truth: it is deserialized from the YAML config *and* reflected into the
/// [`config_schema`](AdapterFactory::config_schema). Its
/// [`Buildable`](fieldsmith::Buildable) bound is what makes "an adapter config
/// always has a schema" a compile-time guarantee — a config type that does
/// not derive `Buildable` cannot be used here. Lift an implementor into the
/// object-safe [`AdapterFactory`] with [`typed`].
pub trait TypedAdapterFactory: Send + Sync {
    /// The adapter's configuration struct — `#[derive(Deserialize, Buildable)]`.
    type Config: fieldsmith::Buildable + serde::de::DeserializeOwned;

    /// Adapter type name (e.g. "jira").
    fn adapter_type(&self) -> &str;

    /// Build an adapter instance from the already-parsed config. The generic
    /// YAML → `Config` deserialization is handled once by [`TypedFactory`], so
    /// every factory shares identical parsing and error reporting.
    fn build(
        &self,
        instance_id: &str,
        config: Self::Config,
        ctx: &HostContext,
    ) -> Result<Box<dyn ContentAdapter>>;

    /// The authentication mechanisms this adapter implements — its own
    /// table, so a new mechanism never touches this crate. `build` is
    /// expected to check the config against it with
    /// [`AuthSpec::validate_against`]. Defaults to none, which is what
    /// the local adapters want.
    fn auth_mechanisms(&self) -> &'static [MechanismSpec] {
        &[]
    }
}

/// Object-safe adapter over a [`TypedAdapterFactory`]. A distinct concrete
/// type (rather than a blanket impl) so it never collides with the decorators'
/// hand-written [`AdapterFactory`] impls. Construct via [`typed`].
pub struct TypedFactory<F: TypedAdapterFactory>(pub F);

impl<F: TypedAdapterFactory> AdapterFactory for TypedFactory<F> {
    fn adapter_type(&self) -> &str {
        self.0.adapter_type()
    }

    fn create(
        &self,
        instance_id: &str,
        config: &str,
        ctx: &HostContext,
    ) -> Result<Box<dyn ContentAdapter>> {
        let cfg: F::Config = serde_yaml::from_str(config).map_err(|e| {
            ContentError::Other(format!("Invalid {} config: {e}", self.0.adapter_type()).into())
        })?;
        self.0.build(instance_id, cfg, ctx)
    }

    fn config_schema(&self) -> fieldsmith::TypeSchema {
        <F::Config as fieldsmith::Buildable>::schema()
    }

    fn auth_mechanisms(&self) -> &'static [MechanismSpec] {
        self.0.auth_mechanisms()
    }
}

/// Lift a [`TypedAdapterFactory`] into a boxed object-safe [`AdapterFactory`]
/// for the registry — the single sanctioned path from a concrete factory to
/// the registry, and where the `Config: Buildable` bound is discharged.
pub fn typed<F: TypedAdapterFactory + 'static>(factory: F) -> Box<dyn AdapterFactory> {
    Box::new(TypedFactory(factory))
}

// ---------------------------------------------------------------------------
// Tests — BusEvent over the host bus
// ---------------------------------------------------------------------------

#[cfg(test)]
mod adapter_status_tests {
    use super::*;

    #[test]
    fn resting_states_have_no_banner() {
        assert!(AdapterStatus::Idle.banner_text().is_none());
        assert!(AdapterStatus::Ready.banner_text().is_none());
    }

    #[test]
    fn connecting_reports_retry_and_timeout_only_when_they_say_something() {
        let bounded = AdapterStatus::Connecting {
            retry: 2,
            max_retries: 5,
            timeout_secs: 30,
        }
        .banner_text()
        .unwrap();
        assert_eq!(bounded, "Connecting… (2/5) Timeout: 30s");

        // A single open-ended attempt: no retry budget, no deadline to name.
        let open = AdapterStatus::Connecting {
            retry: 1,
            max_retries: 1,
            timeout_secs: 0,
        }
        .banner_text()
        .unwrap();
        assert_eq!(open, "Connecting…");
    }

    #[test]
    fn failure_reason_reaches_the_banner() {
        let text = AdapterStatus::Failed {
            reason: "no route to host".into(),
        }
        .banner_text()
        .unwrap();
        assert!(text.contains("no route to host"), "got: {text}");
    }
}

#[cfg(test)]
mod bus_event_tests {
    use super::*;

    #[tokio::test]
    async fn publish_event_delivers_to_subscriber_as_bus_event() {
        let bus = InMemoryHostBus::default();
        let mut rx = subscribe_events(&bus);
        publish_event(
            &bus,
            BusEvent::new(
                "office365-web:mfa:number-match",
                "conn-a",
                serde_json::json!({ "number": 42 }),
            )
            .with_correlation("req-1"),
        );
        let got = rx.recv().await.expect("event delivered");
        let ev = BusEvent::from_host_event(&got).expect("downcasts to BusEvent");
        assert_eq!(ev.topic, "office365-web:mfa:number-match");
        assert_eq!(ev.source, "conn-a");
        assert_eq!(ev.correlation_id.as_deref(), Some("req-1"));
        assert_eq!(ev.payload["number"], 42);
    }

    #[tokio::test]
    async fn foreign_payload_on_channel_downcasts_to_none() {
        // A non-BusEvent published on the same channel must be ignored, not
        // panic — keeps consumers total if the channel is ever shared.
        let bus = InMemoryHostBus::default();
        let mut rx = subscribe_events(&bus);
        bus.publish(
            EVENT_CHANNEL,
            std::sync::Arc::new(String::from("not a bus event")),
        );
        let got = rx.recv().await.expect("something delivered");
        assert!(BusEvent::from_host_event(&got).is_none());
    }
}

// ---------------------------------------------------------------------------
// Tests — InputSpec::Form contract round-trip (M6/E5)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod form_contract_tests {
    use super::*;
    use std::collections::HashMap;

    /// The Form action `FormNode`'s type would declare via
    /// `ContentAdapter::actions_for_type`. Kept as a free helper so the spec
    /// test can assert on it without a full adapter.
    fn form_node_action() -> NodeAction {
        NodeAction::new(
            "edit",
            "Edit",
            InputSpec::Form {
                fields: vec![
                    FormFieldSpec::text("title", "Title"),
                    FormFieldSpec::select("status", "Status", vec!["todo".into(), "done".into()]),
                    FormFieldSpec::toggle("urgent", "Urgent"),
                ],
            },
        )
    }

    /// A node that exposes a single `InputSpec::Form` action, prefills it,
    /// and records the values delivered back through `execute`.
    struct FormNode {
        node_type: NodeType,
        metadata: Metadata,
        last_form: std::sync::Mutex<Option<HashMap<String, String>>>,
    }

    impl FormNode {
        fn new() -> Self {
            Self {
                node_type: NodeType {
                    type_id: "test:item".into(),
                    mime_type: "text/plain".into(),
                    syntax: None,
                    file_extension: ".txt".into(),
                    display_name: "Item".into(),
                },
                metadata: Metadata::default(),
                last_form: std::sync::Mutex::new(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl Node for FormNode {
        fn id(&self) -> &str {
            "n1"
        }
        fn label(&self) -> &str {
            "Item"
        }
        fn node_type(&self) -> &NodeType {
            &self.node_type
        }
        fn metadata(&self) -> &Metadata {
            &self.metadata
        }

        async fn form_prep(&self, action_id: &str) -> Result<HashMap<String, String>> {
            assert_eq!(action_id, "edit");
            let mut m = HashMap::new();
            m.insert("title".to_string(), "current".to_string());
            m.insert("status".to_string(), "done".to_string());
            Ok(m)
        }

        async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
            assert_eq!(action_id, "edit");
            match input {
                ActionInput::Form(values) => {
                    *self.last_form.lock().unwrap() = Some(values);
                    Ok(ActionOutcome::Done {
                        message: Some("saved".into()),
                    })
                }
                _ => panic!("expected ActionInput::Form"),
            }
        }
    }

    #[test]
    fn action_advertises_form_input_spec() {
        let action = form_node_action();
        match action.input {
            InputSpec::Form { fields } => {
                assert_eq!(fields.len(), 3);
                assert_eq!(fields[0].key, "title");
                assert!(matches!(fields[0].kind, FormFieldKind::Text));
                assert!(matches!(fields[2].kind, FormFieldKind::Toggle));
            }
            _ => panic!("expected Form input spec"),
        }
    }

    #[test]
    fn column_schema_maps_to_form_field() {
        // A non-`text` free value becomes a text field whose label hints the
        // expected type, and is always optional (empty clears the cell).
        let dur = ColumnSchema::new("est", "Estimate").typed("duration");
        let f = dur.to_form_field();
        assert_eq!(f.key, "est");
        assert_eq!(f.label, "Estimate (duration)");
        assert!(matches!(f.kind, FormFieldKind::Text));
        assert!(!f.required);

        // A `text` column keeps its plain label (no type hint).
        let txt = ColumnSchema {
            label: None,
            ..ColumnSchema::new("note", "")
        };
        assert_eq!(txt.to_form_field().label, "note");

        // A closed option set drives a select.
        let sel = ColumnSchema {
            label: None,
            ..ColumnSchema::new("prio", "").with_options(vec!["hi".into(), "lo".into()])
        };
        assert!(matches!(
            sel.to_form_field().kind,
            FormFieldKind::Select { .. }
        ));
    }

    #[tokio::test]
    async fn form_prep_supplies_edit_prefill() {
        let node = FormNode::new();
        let prefill = node.form_prep("edit").await.unwrap();
        assert_eq!(prefill.get("title").unwrap(), "current");
        assert_eq!(prefill.get("status").unwrap(), "done");
    }

    #[tokio::test]
    async fn execute_receives_form_values() {
        let mut node = FormNode::new();
        let mut values = HashMap::new();
        values.insert("title".to_string(), "new title".to_string());
        values.insert("status".to_string(), "todo".to_string());
        values.insert("urgent".to_string(), "true".to_string());

        let outcome = node
            .execute("edit", ActionInput::Form(values))
            .await
            .unwrap();
        assert!(matches!(outcome, ActionOutcome::Done { .. }));

        let recorded = node.last_form.lock().unwrap().clone().unwrap();
        assert_eq!(recorded.get("title").unwrap(), "new title");
        assert_eq!(recorded.get("status").unwrap(), "todo");
        assert_eq!(recorded.get("urgent").unwrap(), "true");
    }
}

// ---------------------------------------------------------------------------
// Tests — mark/paste-move contract round-trip (M7/E6)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod mark_move_contract_tests {
    use super::*;

    /// A node that records the [`MarkedNode`] handed to it via
    /// [`ActionContext`] when a `paste-move` action is invoked.
    struct MoveNode {
        node_type: NodeType,
        metadata: Metadata,
        last_marked: std::sync::Mutex<Option<MarkedNode>>,
    }

    impl MoveNode {
        fn new() -> Self {
            Self {
                node_type: NodeType {
                    type_id: "test:item".into(),
                    mime_type: "text/plain".into(),
                    syntax: None,
                    file_extension: ".txt".into(),
                    display_name: "Item".into(),
                },
                metadata: Metadata::default(),
                last_marked: std::sync::Mutex::new(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl Node for MoveNode {
        fn id(&self) -> &str {
            "target"
        }
        fn label(&self) -> &str {
            "Target"
        }
        fn node_type(&self) -> &NodeType {
            &self.node_type
        }
        fn metadata(&self) -> &Metadata {
            &self.metadata
        }

        async fn invoke_action(&self, name: &str, ctx: &ActionContext) -> Result<ActionDispatch> {
            match name {
                // `mark-move` is frontend-owned (records the marked node in
                // session state); the adapter has nothing to do → Noop.
                "mark-move" => Ok(ActionDispatch::Noop),
                // `paste-move` reads the marked node out of the context and
                // performs the move (here: just records what it received).
                "paste-move" => {
                    *self.last_marked.lock().unwrap() = ctx.marked.clone();
                    if ctx.marked.is_some() {
                        Ok(ActionDispatch::Reload)
                    } else {
                        Ok(ActionDispatch::Error("nothing marked".into()))
                    }
                }
                other => Ok(ActionDispatch::Error(format!("unknown action {other}"))),
            }
        }
    }

    #[test]
    fn action_context_default_has_no_mark() {
        let ctx = ActionContext::default();
        assert!(ctx.marked.is_none());
    }

    #[tokio::test]
    async fn paste_move_receives_marked_node_from_context() {
        let node = MoveNode::new();
        let marked = MarkedNode {
            node_id: "src-1".into(),
            node_type: node.node_type().clone(),
            label: "Source task".into(),
        };
        let ctx = ActionContext {
            marked: Some(marked.clone()),
            confirmed: false,
            query: None,
            value: None,
            text: None,
        };

        let dispatch = node.invoke_action("paste-move", &ctx).await.unwrap();
        assert!(matches!(dispatch, ActionDispatch::Reload));

        let recorded = node.last_marked.lock().unwrap().clone().unwrap();
        assert_eq!(recorded, marked);
        assert_eq!(recorded.node_id, "src-1");
    }

    #[tokio::test]
    async fn paste_move_without_mark_is_rejected() {
        let node = MoveNode::new();
        let dispatch = node
            .invoke_action("paste-move", &ActionContext::default())
            .await
            .unwrap();
        assert!(matches!(dispatch, ActionDispatch::Error(_)));
        assert!(node.last_marked.lock().unwrap().is_none());
    }
}

#[cfg(test)]
mod list_subtree_default_tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    fn nt(type_id: &str) -> NodeType {
        NodeType {
            type_id: type_id.into(),
            mime_type: "text/plain".into(),
            syntax: None,
            file_extension: ".txt".into(),
            display_name: type_id.into(),
        }
    }

    fn params() -> ListParams {
        ListParams {
            node_type: nt("mock:item"),
            query: None,
            sort: Vec::new(),
            page: None,
            download: false,
            group_by: None,
        }
    }

    /// Shared adjacency for the mock forest: `parent id -> [(child id, child
    /// type)]`. `liars` forces `has_children = Some(false)` on a node that
    /// actually *does* have edges, so we can prove the depth walk honours the
    /// leaf hint without calling `get_child`.
    struct Tree {
        edges: HashMap<String, Vec<(String, String)>>,
        liars: HashSet<String>,
    }

    fn tree(edges: &[(&str, &str, &str)]) -> Arc<Tree> {
        let mut map: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (p, c, t) in edges {
            map.entry((*p).into())
                .or_default()
                .push(((*c).into(), (*t).into()));
        }
        Arc::new(Tree {
            edges: map,
            liars: HashSet::new(),
        })
    }

    /// A passive node addressed by id. All knowledge about children lives on
    /// [`TreeAdapter`] (`childs` + `get_by_id`), exercising the generic
    /// [`children::list_subtree`] recursion the way a real adapter would.
    struct MockNode {
        id: String,
        node_type: NodeType,
        metadata: Metadata,
    }

    fn node(id: &str) -> MockNode {
        MockNode {
            id: id.into(),
            node_type: nt("mock:node"),
            metadata: Metadata::default(),
        }
    }

    #[async_trait::async_trait]
    impl Node for MockNode {
        fn id(&self) -> &str {
            &self.id
        }
        fn label(&self) -> &str {
            &self.id
        }
        fn node_type(&self) -> &NodeType {
            &self.node_type
        }
        fn metadata(&self) -> &Metadata {
            &self.metadata
        }
    }

    /// Adapter over the shared [`Tree`]: `childs` reports the distinct child
    /// types in insertion order, each with a fetcher over the matching edges;
    /// `get_by_id` re-roots a fresh node at the requested id.
    struct TreeAdapter {
        tree: Arc<Tree>,
    }

    /// The edges under `parent` whose child type is `want`, as list rows. A
    /// `liar` (or a genuinely childless node) reports `has_children = false`.
    fn list_edges(tree: &Tree, parent: &str, want: &str) -> Result<ListResult> {
        let items = tree
            .edges
            .get(parent)
            .map(|edges| {
                edges
                    .iter()
                    .filter(|(_c, t)| t == want)
                    .map(|(c, _t)| {
                        let has = !tree.liars.contains(c)
                            && tree.edges.get(c).map(|e| !e.is_empty()).unwrap_or(false);
                        NodeSummary {
                            id: c.clone(),
                            label: c.clone(),
                            node_type: nt(want),
                            metadata: Metadata::default(),
                            has_children: Some(has),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(ListResult {
            items,
            applied_sort: Vec::new(),
            page: None,
            batch_download_available: false,
            downloaded: Vec::new(),
        })
    }

    #[async_trait::async_trait]
    impl ContentAdapter for TreeAdapter {
        fn adapter_type(&self) -> &str {
            "mock"
        }
        fn instance_id(&self) -> &str {
            "mock"
        }
        async fn root(&self) -> Result<Box<dyn Node>> {
            Ok(Box::new(node("root")))
        }
        async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
            Ok(Box::new(node(id)))
        }
        fn childs<'a>(&'a self, n: &'a dyn Node) -> Vec<children::Child<'a>> {
            let parent = n.id().to_string();
            let mut out = Vec::new();
            let mut seen: Vec<String> = Vec::new();
            if let Some(edges) = self.tree.edges.get(&parent) {
                for (_c, t) in edges {
                    if seen.contains(t) {
                        continue;
                    }
                    seen.push(t.clone());
                    let tree = self.tree.clone();
                    let parent = parent.clone();
                    let want = t.clone();
                    out.push(children::Child {
                        node_type: nt(t),
                        columns: Vec::new(),
                        list: Box::new(move |_params| {
                            Box::pin(async move { list_edges(&tree, &parent, &want) })
                        }),
                    });
                }
            }
            out
        }
    }

    fn child_ids(st: &Subtree) -> Vec<String> {
        st.items.iter().map(|n| n.summary.id.clone()).collect()
    }

    fn find<'a>(st: &'a Subtree, id: &str) -> &'a SubtreeNode {
        st.items
            .iter()
            .find(|n| n.summary.id == id)
            .unwrap_or_else(|| panic!("node {id} not found"))
    }

    // root → a, b ; a → a1, a2 ; a1 → a1x ; (b, a2, a1x are leaves)
    fn sample() -> Arc<Tree> {
        tree(&[
            ("root", "a", "mock:item"),
            ("root", "b", "mock:item"),
            ("a", "a1", "mock:item"),
            ("a", "a2", "mock:item"),
            ("a1", "a1x", "mock:item"),
        ])
    }

    async fn subtree(tree: Arc<Tree>, depth: u32) -> Subtree {
        let adapter = TreeAdapter { tree };
        let root = adapter.root().await.unwrap();
        children::list_subtree(&adapter, root.as_ref(), params(), depth)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn depth_zero_is_single_level() {
        let st = subtree(sample(), 0).await;
        assert_eq!(child_ids(&st), vec!["a", "b"]);
        // depth 0 ⇔ list(): no node is expanded.
        assert!(st.items.iter().all(|n| n.children.items.is_empty()));
    }

    #[tokio::test]
    async fn depth_one_expands_exactly_one_level() {
        let st = subtree(sample(), 1).await;

        let a = find(&st, "a");
        assert_eq!(child_ids(&a.children), vec!["a1", "a2"]);
        // one level only: a's children are not themselves expanded.
        assert!(a.children.items.iter().all(|n| n.children.items.is_empty()));

        // b is a genuine leaf (has_children == Some(false)) → not descended.
        let b = find(&st, "b");
        assert!(b.children.items.is_empty());
    }

    #[tokio::test]
    async fn depth_all_reaches_deepest_leaf() {
        let st = subtree(sample(), u32::MAX).await;
        let a = find(&st, "a");
        let a1 = find(&a.children, "a1");
        assert_eq!(child_ids(&a1.children), vec!["a1x"]);
        // recursion stops naturally at the leaf, no depth limit needed.
        assert!(a1.children.items[0].children.items.is_empty());
        // sibling leaf a2 stays empty.
        assert!(find(&a.children, "a2").children.items.is_empty());
    }

    #[tokio::test]
    async fn has_children_false_short_circuits_descent() {
        // `trap` has real edges but is flagged a liar → summary says leaf.
        let mut map: HashMap<String, Vec<(String, String)>> = HashMap::new();
        map.insert("root".into(), vec![("trap".into(), "mock:item".into())]);
        map.insert("trap".into(), vec![("hidden".into(), "mock:item".into())]);
        let t = Arc::new(Tree {
            edges: map,
            liars: HashSet::from(["trap".to_string()]),
        });
        let st = subtree(t, u32::MAX).await;
        let trap = find(&st, "trap");
        assert_eq!(trap.summary.has_children, Some(false));
        // Even at depth all, the leaf hint prevents descent.
        assert!(trap.children.items.is_empty());
    }

    #[tokio::test]
    async fn multi_child_type_merges_typed_lists_in_order() {
        // root → p (mock:item); p has two child types x then y.
        let t = tree(&[
            ("root", "p", "mock:item"),
            ("p", "x1", "mock:x"),
            ("p", "y1", "mock:y"),
        ]);
        let st = subtree(t, 1).await;
        let p = find(&st, "p");
        // Both typed child lists merged, child-type (insertion) order kept.
        assert_eq!(child_ids(&p.children), vec!["x1", "y1"]);
    }
}

#[cfg(test)]
mod bookmark_store_tests {
    use super::*;

    #[tokio::test]
    async fn empty_store_lists_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBookmarkStore::new(dir.path().to_path_buf());
        assert!(store.list().await.unwrap().is_empty());
        assert!(!store.contains("PROJ-1").await.unwrap());
    }

    #[tokio::test]
    async fn toggle_adds_then_removes() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsBookmarkStore::new(dir.path().to_path_buf());

        // Add.
        assert!(store.toggle("PROJ-1").await.unwrap());
        assert!(store.contains("PROJ-1").await.unwrap());
        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "PROJ-1");
        assert!(!list[0].bookmarked_at.is_empty());

        // Remove.
        assert!(!store.toggle("PROJ-1").await.unwrap());
        assert!(!store.contains("PROJ-1").await.unwrap());
        assert!(store.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn keeps_insertion_order_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = FsBookmarkStore::new(dir.path().to_path_buf());
            store.toggle("PROJ-2").await.unwrap();
            store.toggle("PROJ-1").await.unwrap();
            store.toggle("PROJ-3").await.unwrap();
        }
        // A fresh store over the same dir reads the persisted set.
        let store = FsBookmarkStore::new(dir.path().to_path_buf());
        let ids: Vec<String> = store
            .list()
            .await
            .unwrap()
            .into_iter()
            .map(|b| b.id)
            .collect();
        assert_eq!(ids, vec!["PROJ-2", "PROJ-1", "PROJ-3"]);
    }
}

#[cfg(test)]
mod query_kind_tests {
    use super::*;

    #[test]
    fn setting_round_trips_both_kinds() {
        for d in [
            DefaultQuery::saved("My Tickets"),
            DefaultQuery::extended("Sprint"),
        ] {
            assert_eq!(DefaultQuery::from_setting(&d.to_setting()), d);
        }
    }

    /// A value written before kinds existed is a bare name, and must keep
    /// working as a saved-query default.
    #[test]
    fn legacy_value_reads_as_a_saved_name() {
        assert_eq!(
            DefaultQuery::from_setting("My Tickets"),
            DefaultQuery::saved("My Tickets")
        );
    }

    /// A legacy name may itself contain a colon. Only the two known prefixes
    /// count as a kind — everything else is taken whole, so such a default
    /// survives instead of decaying into a name nobody has.
    #[test]
    fn unknown_prefix_is_part_of_the_name() {
        assert_eq!(
            DefaultQuery::from_setting("urgent: mine"),
            DefaultQuery::saved("urgent: mine")
        );
    }

    /// Encoding splits at the *first* colon, so a name containing one comes
    /// back intact rather than being cut at the last separator.
    #[test]
    fn a_colon_in_the_name_survives_encoding() {
        let d = DefaultQuery::extended("urgent: mine");
        assert_eq!(d.to_setting(), "extended:urgent: mine");
        assert_eq!(DefaultQuery::from_setting(&d.to_setting()), d);
    }
}

#[cfg(test)]
mod fs_query_store_tests {
    use super::*;

    /// One type, two namespaces: the same names must not leak from the saved
    /// store into the extended one, or the kind on a stored reference would
    /// point at a body that is only accidentally there.
    #[tokio::test]
    async fn the_two_stores_keep_separate_namespaces() {
        let dir = tempfile::tempdir().unwrap();
        let saved = FsQueryStore::new(dir.path().join("queries"), ".jql");
        let extended =
            FsQueryStore::new(dir.path().join(EXTENDED_QUERY_DIR), EXTENDED_QUERY_SUFFIX);

        SavedQueryStore::save(&saved, "Mine", "assignee = currentUser()")
            .await
            .unwrap();
        ExtendedQueryStore::save(&extended, "Mine", "```yaml\nquery: x\n```")
            .await
            .unwrap();

        assert_eq!(SavedQueryStore::list(&saved).await.unwrap(), vec!["Mine"]);
        assert_eq!(
            ExtendedQueryStore::list(&extended).await.unwrap(),
            vec!["Mine"]
        );
        assert_eq!(
            SavedQueryStore::load(&saved, "Mine").await.unwrap(),
            "assignee = currentUser()"
        );
        assert!(
            ExtendedQueryStore::load(&extended, "Mine")
                .await
                .unwrap()
                .starts_with("```yaml")
        );

        ExtendedQueryStore::delete(&extended, "Mine").await.unwrap();
        assert_eq!(SavedQueryStore::list(&saved).await.unwrap(), vec!["Mine"]);
        assert!(
            ExtendedQueryStore::list(&extended)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
