//! Host-agnostic filter DSL: the *language* half of the old
//! `not-yet-done-core::filter` module.
//!
//! This crate holds the parts of the filter system that have **no
//! dependency on any concrete entity or database**:
//!
//! - [`FilterExpr`] and friends — the parsed filter AST ([`expr`]).
//! - [`query_filter`] — parsing a saved-query YAML document into a
//!   [`FilterExpr`] plus options, with relative-date resolution.
//! - [`extract_date_bounds`] / [`DateBounds`] — pulling concrete date
//!   ranges out of a [`FilterExpr`] ([`date_range`]).
//!
//! The *binding* half — translating a [`FilterExpr`] into a
//! `sea_orm::Condition` ([`FilterBuilder`]/`ColumnRegistry`) and the
//! task-tree operators (`tree_ops`) — stays in `not-yet-done-core`
//! (and later `not-yet-done-task-core`) because it is tied to SeaORM
//! and the task entity. Splitting the language out lets the TUI and
//! adapters parse/inspect filters without pulling in the whole task
//! database stack.
//!
//! # Quick-start
//!
//! ```rust,ignore
//! // Parse a YAML string into a FilterExpr
//! let expr: FilterExpr = serde_yaml::from_str(yaml_str)?;
//! ```

mod date_range;
mod expr;
pub mod query_filter;

pub use date_range::{extract_date_bounds, DateBounds};
pub use expr::{ColRef, FilterExpr, FilterLeaf, Literal, Operator, Rhs};
