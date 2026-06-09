//! Horizontal cell grouping — merge multiple columns into one cell within a row.

use crate::cell::{CellAlignment, CellContent, fit_aligned};

/// A cell that spans multiple columns.
#[derive(Debug, Clone)]
pub struct GroupedCell {
    /// Content to render across the spanned columns.
    pub content: CellContent,
    /// Number of columns this cell spans (including the starting column).
    pub span: usize,
}

impl GroupedCell {
    pub fn new(content: impl Into<CellContent>, span: usize) -> Self {
        Self { content: content.into(), span: span.max(1) }
    }

    pub fn aligned(text: impl Into<String>, alignment: CellAlignment, span: usize) -> Self {
        Self {
            content: CellContent::aligned(text, alignment),
            span: span.max(1),
        }
    }

    /// Fit the grouped cell text to the combined width of the spanned columns
    /// plus the separators between them.
    pub fn fit(&self, col_widths: &[usize], separator_width: usize) -> String {
        let total_width: usize = col_widths.iter().sum::<usize>()
            + separator_width * col_widths.len().saturating_sub(1);
        fit_aligned(&self.content.text, total_width, self.content.alignment)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouped_cell_width() {
        let cell = GroupedCell::new("Summary", 3);
        // 3 columns of width 5, separator 2 chars → 5+2+5+2+5 = 19
        let fitted = cell.fit(&[5, 5, 5], 2);
        assert_eq!(fitted.len(), 19);
        assert!(fitted.starts_with("Summary"));
    }

    #[test]
    fn grouped_cell_centered() {
        let cell = GroupedCell::aligned("Title", CellAlignment::Center, 2);
        // 2 columns of width 10, separator 2 → 10+2+10 = 22
        let fitted = cell.fit(&[10, 10], 2);
        assert_eq!(fitted.len(), 22);
        // "Title" is 5 chars, centered in 22 → 8 left pad, 9 right pad
        assert_eq!(&fitted[8..13], "Title");
    }
}
