//! not-yet-done's own layer over the [`rowsieve`] filter language.
//!
//! The language — the AST, the YAML forms, the in-memory evaluator — is
//! host-agnostic and lives in [`rowsieve`]. What is left here is the part that
//! is about *this* application:
//!
//! - [`query_filter`] — the saved-query document (`name:`, `query:`,
//!   `options:`) and its `include_ancestors` option.
//! - [`HAS_ANCESTOR`] / [`IN_TREE`] — the two task-tree predicates. They are
//!   [`FilterExpr::Custom`] leaves to `rowsieve`, which carries them through
//!   untouched; `not-yet-done-task-core` resolves them against the task
//!   hierarchy before any SQL is built.
//! - [`extract_date_bounds`] — the date window, over the task entity's own
//!   date columns.
//!
//! Everything else is re-exported unchanged, so a caller does not need to know
//! where the line runs.
//!
//! The *binding* half — translating a [`FilterExpr`] into a `sea_orm::Condition`
//! (`FilterBuilder`/`ColumnRegistry`) and the tree operators (`tree_ops`) —
//! stays in `not-yet-done-task-core`, because it is tied to SeaORM and the task
//! entity.

pub mod query_filter;

pub use rowsieve::dates::{resolve_dates, try_resolve_date};
pub use rowsieve::eval;
pub use rowsieve::regex_cache;
pub use rowsieve::{
    ColRef, DateBounds, Field, FilterExpr, FilterLeaf, Literal, Operator, Rhs, RowFields,
    UnknownCustom, matches, precompile,
};

/// `[has_ancestor, <description>]` — tasks *below* the named one.
pub const HAS_ANCESTOR: &str = "has_ancestor";

/// `[in_tree, <description>]` — the named task and everything below it.
pub const IN_TREE: &str = "in_tree";

/// Every host predicate this application understands.
///
/// Pass it to [`FilterExpr::validate_custom`] wherever a filter is loaded: a
/// misspelt `in_tre` is otherwise a predicate nobody implements, which
/// evaluates to `false` and looks like a query that simply found nothing.
pub const TREE_PREDICATES: &[&str] = &[HAS_ANCESTOR, IN_TREE];

/// The task columns that hold a date.
const DATE_COLUMNS: &[&str] = &["started_at", "ended_at", "created_at"];

/// The tightest date window a filter can match, over the task entity's dates.
///
/// A thin wrapper over [`rowsieve::extract_date_bounds`], which has no schema
/// to ask and therefore takes the column names as an argument.
pub fn extract_date_bounds(expr: &FilterExpr) -> DateBounds {
    rowsieve::extract_date_bounds(expr, DATE_COLUMNS)
}
