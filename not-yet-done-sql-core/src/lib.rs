//! Backend-agnostic building blocks for SQL `ContentAdapter`s.
//!
//! Everything here is what two SQL adapters would otherwise implement
//! twice: identifier quoting, the pure-text SQL sniffers the script
//! editor needs, the on-disk layout of editable scripts, the
//! [`ScriptStore`](not_yet_done_content::ScriptStore) implementation on
//! top of that layout, the editor's table-name completions
//! ([`script_completions`]), the checks an edited view definition has to
//! pass before it may be run ([`view_ddl`]), the buffer protocol and
//! `UPDATE` builder behind editing a single data row ([`row_edit`]), and
//! — in [`db_script_nodes`] — the container-level script branch as
//! ready-made [`Node`](not_yet_done_content::Node)s.
//!
//! What is *not* here: connecting, dialect-specific catalogue queries,
//! and the *catalogue* part of the node tree an adapter exposes. Those
//! differ enough between backends (Postgres has a server, credentials
//! and a schema level; a single-file database has none of the three)
//! that sharing them would cost more than it saves.

pub mod db_script_nodes;
pub mod ident;
pub mod row_edit;
pub mod script_completions;
pub mod script_files;
pub mod script_store;
pub mod sql_shape;
pub mod view_ddl;

pub use db_script_nodes::{DB_SCRIPTS_GROUP_ID, DbScriptNodeTypes, DbScriptTree};
pub use ident::quote_ident;
pub use row_edit::{RowCell, RowKeySource, RowKeySpec, RowRead, RowSnapshot};
pub use script_completions::{Completion, qualified_table};
pub use script_store::{NodeScriptLayout, SqlScriptStore};
pub use view_ddl::{ParsedCreateView, parse_create_view};
