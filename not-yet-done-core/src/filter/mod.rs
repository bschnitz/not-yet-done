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
mod date_range;
mod expr;
pub mod query_filter;
pub mod tree_ops;

pub use builder::{ColumnRegistry, FilterBuilder};
pub use date_range::{extract_date_bounds, DateBounds};
pub use expr::{ColRef, FilterExpr, FilterLeaf, Literal, Operator, Rhs};
