//! Extended queries: combine several adapter-native queries with set
//! operations, filter each result locally, and impose an explicit order —
//! without any adapter having to know that it happened.
//!
//! An extended query is a Markdown document. The first unnamed `yaml` fence is
//! the specification; every named fence (```` ```jql mentioned_in ````) is a
//! library entry addressable via `query-ref:`. The Markdown container is what
//! keeps the format unambiguous, since a bare YAML file could not be told apart
//! from the YAML-`FilterExpr` bodies several adapters already store as their
//! native query.
//!
//! The crate sits *above* the adapter, not as a decorator around it: a
//! decorator would see `ListParams::query` as an opaque string and would have
//! to guess from its content whether an extended document is in play. Both the
//! TUI and the CLI drive it from the point where they already call
//! `render_query` + `list`.
//!
//! Design and rationale: `docs/plan-extended-queries.md`.

pub mod adapter;
pub mod ast;
pub mod executor;
pub mod markdown;
pub mod parse;
pub mod rows;

pub use adapter::{AdapterBackend, RunError, document_variables, prepare, run};
pub use ast::{Direction, ExtendedQuery, Fetch, FetchSource, Node, NodeKind, OrderKey};
pub use executor::{Backend, ExecError, Execution, Run, Warning, execute, variables};
pub use markdown::{Document, Fence, MarkdownError};
pub use parse::{ParseError, check_languages, default_template, parse};
pub use rows::{ColumnTypes, SummaryRow};
