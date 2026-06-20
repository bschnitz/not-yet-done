// not-yet-done-core/src/filter/mod.rs

//! Generic filter DSL for building `WHERE` clauses from YAML/JSON.
//!
//! # Architecture note
//!
//! This module is intentionally **not** a Shaku service.  It translates a
//! [`FilterExpr`] directly into `sea_orm::Condition` and may therefore only
//! be used inside the `repository` layer, never in services or CLI code.
//!
//! # Quick-start
//!
//! ```rust,ignore
//! // 1. Parse a YAML string into a FilterExpr
//! let expr: FilterExpr = serde_yaml::from_str(yaml_str)?;
//!
//! // 2. Build a Condition using your entity's ColumnRegistry
//! let condition = FilterBuilder::new(&TaskColumnRegistry).build(&expr)?;
//!
//! // 3. Apply to any SeaORM query
//! Task::find().filter(condition).all(&db).await?
//! ```

mod builder;
pub mod tree_ops;

// The filter *language* (expr / date_range / query_filter) now lives in
// the host-agnostic `not-yet-done-filter` crate. These re-exports keep
// the historic `crate::filter::…` and `not_yet_done_core::filter::…`
// paths valid for every existing caller (C1 of the DB-split: move the
// language out without churning consumers). The *binding* half
// (FilterBuilder/ColumnRegistry, tree_ops) stays here because it is tied
// to SeaORM and the task entity.
pub use not_yet_done_filter::query_filter;
pub use not_yet_done_filter::{extract_date_bounds, DateBounds};
pub use not_yet_done_filter::{ColRef, FilterExpr, FilterLeaf, Literal, Operator, Rhs};

pub use builder::{ColumnRegistry, FilterBuilder};
