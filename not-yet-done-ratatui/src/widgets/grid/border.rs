// All border types and constants are now defined in `not-yet-done-grid-core`.
// This module re-exports them so that the rest of `not-yet-done-ratatui` and
// its consumers continue to use the same import paths as before.
pub use not_yet_done_grid_core::{
    BORDER_DASHED, BORDER_DASHED_EXTENDED, BORDER_DOTTED, BORDER_DOTTED_EXTENDED,
    BORDER_DOUBLE_EXTENDED, BORDER_ROUNDED, BORDER_ROUNDED_EXTENDED, BORDER_SIMPLE,
    BORDER_SIMPLE_EXTENDED, BORDER_THICK_EXTENDED, BorderChars, BorderPos, BorderText, CellGroup,
    GapPos, GapSlot, GridConfig, SpannedBorder, TextAnchor,
};

// ── NormalisedPos (kept local — only used inside the grid widget) ─────────────

/// Internal canonical representation of a border/gap address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum NormalisedPos {
    Outer,
    VGap(usize),
    HGap(usize),
    VGapSpanned {
        gap_idx: usize,
        start: usize,
        end: usize,
    },
    HGapSpanned {
        gap_idx: usize,
        start: usize,
        end: usize,
    },
}

pub(super) trait BorderPosExt {
    fn normalise(&self, rows: usize, cols: usize) -> Option<NormalisedPos>;
}

impl BorderPosExt for BorderPos {
    /// Normalise to a canonical form so `AfterCol(i)` and `BeforeCol(i+1)` are equal.
    ///
    /// Returns `None` for out-of-range positions (e.g. `AfterCol` with index ≥ cols−1).
    fn normalise(&self, rows: usize, cols: usize) -> Option<NormalisedPos> {
        match *self {
            BorderPos::Grid => Some(NormalisedPos::Outer),
            BorderPos::AfterCol(i) => {
                if i + 1 < cols {
                    Some(NormalisedPos::VGap(i))
                } else {
                    None
                }
            }
            BorderPos::BeforeCol(i) => {
                if i > 0 && i < cols {
                    Some(NormalisedPos::VGap(i - 1))
                } else {
                    None
                }
            }
            BorderPos::AfterRow(i) => {
                if i + 1 < rows {
                    Some(NormalisedPos::HGap(i))
                } else {
                    None
                }
            }
            BorderPos::BeforeRow(i) => {
                if i > 0 && i < rows {
                    Some(NormalisedPos::HGap(i - 1))
                } else {
                    None
                }
            }
            BorderPos::AfterColSpanned {
                col,
                row_start,
                row_end,
            } => {
                if col + 1 < cols && row_start <= row_end && row_end < rows {
                    Some(NormalisedPos::VGapSpanned {
                        gap_idx: col,
                        start: row_start,
                        end: row_end,
                    })
                } else {
                    None
                }
            }
            BorderPos::BeforeColSpanned {
                col,
                row_start,
                row_end,
            } => {
                if col > 0 && col < cols && row_start <= row_end && row_end < rows {
                    Some(NormalisedPos::VGapSpanned {
                        gap_idx: col - 1,
                        start: row_start,
                        end: row_end,
                    })
                } else {
                    None
                }
            }
            BorderPos::AfterRowSpanned {
                row,
                col_start,
                col_end,
            } => {
                if row + 1 < rows && col_start <= col_end && col_end < cols {
                    Some(NormalisedPos::HGapSpanned {
                        gap_idx: row,
                        start: col_start,
                        end: col_end,
                    })
                } else {
                    None
                }
            }
            BorderPos::BeforeRowSpanned {
                row,
                col_start,
                col_end,
            } => {
                if row > 0 && row < rows && col_start <= col_end && col_end < cols {
                    Some(NormalisedPos::HGapSpanned {
                        gap_idx: row - 1,
                        start: col_start,
                        end: col_end,
                    })
                } else {
                    None
                }
            }
        }
    }
}
