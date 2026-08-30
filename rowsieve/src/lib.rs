//! A small filter language over rows, written in YAML.
//!
//! `rowsieve` is the *language* half of a filter system: an AST, a parser, and
//! an evaluator that runs an expression against rows already in memory. It
//! knows nothing about where rows come from — no entity, no database, no
//! schema. A host teaches it about its own data by implementing one method:
//!
//! ```
//! use rowsieve::{Field, FilterExpr, RowFields, matches};
//!
//! struct Task { title: String, priority: i64 }
//!
//! impl RowFields for Task {
//!     fn field(&self, name: &str) -> Field<'_> {
//!         match name {
//!             "title" => Field::Text(self.title.as_str().into()),
//!             "priority" => Field::Number(self.priority as f64),
//!             _ => Field::Null,
//!         }
//!     }
//! }
//!
//! let expr: FilterExpr = serde_yaml::from_str(
//!     "and:\n  - [title, has, deploy]\n  - [priority, '>=', 3]\n",
//! )
//! .unwrap();
//!
//! let row = Task { title: "Deploy the bar".into(), priority: 5 };
//! assert!(matches(&expr, &row));
//! ```
//!
//! # The language
//!
//! An expression is either a combinator or a leaf:
//!
//! ```yaml
//! and: # every branch matches; `or` and `not` likewise
//!   - [status, '=', open] # [field, operator, value]
//!   - [assignee, is_not_null] # two elements when the operator takes no value
//!   - not: [title, matches, '^WIP'] # regex, case-sensitive
//! ```
//!
//! See [`Operator`] for the full set and [`expr`](mod@expr) for the exact
//! parsing rules, and [`eval`] for what each operator means against a row.
//!
//! # Extending it
//!
//! A host predicate that this crate cannot evaluate — "is a descendant of X",
//! "is assigned to me" — is written `[name, argument]` and parses to
//! [`FilterExpr::Custom`]. `rowsieve` carries it through the tree untouched and
//! evaluates it to `false`; the host recognises it by name and resolves it
//! however it must. Call [`FilterExpr::validate_custom`] once at load time with
//! the names you do support, so a typo is a configuration error rather than a
//! branch that silently matches nothing.
//!
//! # What is deliberately *not* here
//!
//! Translating an expression into SQL. That needs a schema, a dialect and a
//! query builder, and every host has different ones. The AST is public
//! precisely so a host can walk it and build its own `WHERE` clause — see the
//! `not-yet-done-filter` crate for one that does.

mod date_range;
pub mod eval;
mod expr;
pub mod regex_cache;

#[cfg(feature = "natural-dates")]
pub mod dates;

pub use date_range::{DateBounds, extract_date_bounds};
pub use eval::{Field, RowFields, matches};
pub use expr::{
    ColRef, FilterExpr, FilterLeaf, Literal, Operator, Rhs, UnknownCustom, UnknownOperator,
};
pub use regex_cache::precompile;
