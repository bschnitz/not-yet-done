use crate::widgets::common::impl_widget_style_base;
use ratatui::style::{Color, Style};

/// Identifies the visual part of a `Table` row to be styled.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableStyleType {
    /// Header row background/fg.
    Header = 0,
    /// Normal (unselected) row.
    Row = 1,
    /// Selected row.
    RowSelected = 2,
    /// Fuzzy-match highlight within a cell.
    Highlight = 3,
    /// Tree connector prefix (dim).
    Prefix = 4,
    /// Selected column (when the optional column cursor is active).
    /// Applied to all cells in the selected column except the
    /// row/column intersection.
    ColumnSelected = 5,
    /// Cell at the row/column cursor intersection — top precedence
    /// over `RowSelected` and `ColumnSelected`.
    CellSelected = 6,
    /// `‹` / `›` glyphs drawn at the pane edges in the header row when
    /// horizontal scroll has hidden columns left/right of the viewport.
    ScrollIndicator = 7,
}

#[derive(Debug, Clone)]
pub struct TableStyle {
    pub prefix_color: Option<Color>,
    pub styles: [Option<Style>; 8],
}

impl Default for TableStyle {
    fn default() -> Self {
        Self {
            prefix_color: None,
            styles: [None; 8],
        }
    }
}

impl TableStyle {
    pub fn new() -> Self {
        Self::default()
    }
}

impl_widget_style_base!(TableStyle, TableStyleType);
