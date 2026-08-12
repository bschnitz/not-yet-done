//! Entity modules. Each entity lives in its own submodule so the schema
//! registry glob `not_yet_done_custom_columns::entity::*` (one level below
//! `entity`) matches it — the same layout every other adapter crate uses. A
//! `Model` placed directly in `entity` would not match that glob and its table
//! would never be created.
//!
//! Two tables:
//!   * [`custom_cell`] — one row per stored `(scope, row_id, column_key)` value.
//!   * [`custom_column`] — the per-`(scope, node_type, column_key)` *schema*:
//!     the authoritative value type (type-on-first-write) against which later
//!     writes are validated.
//!
//! The submodules are addressed explicitly (`entity::custom_cell::Entity`,
//! `entity::custom_column::Entity`) rather than glob-re-exported, since both
//! define the same sea-orm item names (`Entity`, `Column`, `Model`, …).

pub mod custom_cell;
pub mod custom_column;
