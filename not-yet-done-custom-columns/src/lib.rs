//! Custom user columns — an adapter-agnostic facility that lets a user add
//! extra, locally-stored columns to *any* adapter's tables.
//!
//! # The idea
//!
//! A user declares an extra column in a view's YAML (`source: custom`, keyed by
//! some `key`) and stores per-row values for it. The **definitions** live in
//! the view config like any other column; only the **values** live in a
//! database — one row per `(scope, row_id, column_key)` cell — matched onto a
//! content row by its node id. Because the view column resolves its cell from
//! the row's metadata field of the same key, and this layer injects stored
//! cells as exactly such fields, the column renders through the ordinary path
//! with no change to column resolution.
//!
//! # Why it needs no adapter code
//!
//! The data is a **local annotation on top of** an adapter's rows, not adapter
//! content, so it isn't coupled to any adapter's own storage. There is a single
//! lib-owned SQLite ([`store::default_sqlite_url`]) shared across every adapter
//! instance; a decorator ([`custom_columns_factory`]) wired in at the host's
//! factory chokepoint injects stored cells on read and handles the `set-cell` /
//! `clear-cell` write actions. Adapters implement nothing, the content trait is
//! unchanged, and there is no feature gate — the opt-in is simply declaring a
//! `source: custom` column and storing a value.
//!
//! See [`decorator`] for the read/inject + write mechanics and the ordering
//! relative to anonymization.

pub mod decorator;
pub mod entity;
pub mod store;

pub use decorator::{
    CLEAR_CELL_ACTION_ID, CustomColumnsAdapter, CustomColumnsNode, EDIT_CELLS_ACTION_ID,
    SET_CELL_ACTION_ID, custom_columns_factory,
};
pub use store::{Cell, LocalColumnStore, VALUE_TYPES, default_sqlite_url, shared_store};
