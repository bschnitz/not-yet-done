//! Framework-agnostic table layout and computation.
//!
//! This crate provides the core data types and algorithms for table rendering
//! without any dependency on a specific terminal UI framework:
//!
//! - [`ColumnId`], [`ColStrategy`], [`ColSizer`] — column identification and sizing
//! - [`CellContent`], [`CellAlignment`], [`StyledSpan`] — cell data with alignment and styles
//! - [`Row`], [`ComputedRow`] — input and output row types
//! - [`compute_table`] — layout computation (column widths, cell fitting, alignment)
//! - [`fit_to_width`], [`fit_aligned`] — unicode-aware text truncation/padding
//! - [`RenderTarget`], [`CharBuf`] — abstraction for character-based rendering and testing

pub mod cell;
pub mod column;
pub mod grouping;
pub mod layout;
pub mod row;
pub mod target;

pub use cell::{CellAlignment, CellContent, StyledSpan, fit_to_width, fit_aligned, fit_to_width_with_highlights, fit_aligned_with_highlights};
pub use column::{ColSizer, ColStrategy, ColumnId, FixedColSizer, MixedColSizer};
pub use layout::{
    ComputedMultiTable, ComputedTable, LineTemplate, RowTemplate, TableConfig, compute_multiline_table,
    compute_table,
};
pub use row::{ComputedLine, ComputedMultiRow, ComputedRow, Row};
pub use grouping::GroupedCell;
pub use target::{CharBuf, RenderTarget, render_to_target};
