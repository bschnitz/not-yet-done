//! Cell content, alignment, and text fitting.

use std::ops::Range;

/// Text alignment within a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CellAlignment {
    #[default]
    Left,
    Right,
    Center,
}

/// A range within a cell's text that carries a style identifier.
///
/// The actual visual style (colors, bold, etc.) is resolved by the rendering
/// layer — this struct only carries a numeric ID so the core stays
/// framework-agnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledSpan {
    /// Char-index range within the cell text.
    pub range: Range<usize>,
    /// Application-defined style identifier.
    pub style_id: usize,
}

/// Content of a single table cell.
#[derive(Debug, Clone)]
pub struct CellContent {
    pub text: String,
    pub alignment: CellAlignment,
    /// Optional style spans within the text.
    pub spans: Vec<StyledSpan>,
}

impl CellContent {
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            text: s.into(),
            alignment: CellAlignment::Left,
            spans: Vec::new(),
        }
    }

    pub fn aligned(s: impl Into<String>, alignment: CellAlignment) -> Self {
        Self {
            text: s.into(),
            alignment,
            spans: Vec::new(),
        }
    }

    pub fn with_spans(mut self, spans: Vec<StyledSpan>) -> Self {
        self.spans = spans;
        self
    }
}

impl From<String> for CellContent {
    fn from(s: String) -> Self {
        Self::text(s)
    }
}

impl From<&str> for CellContent {
    fn from(s: &str) -> Self {
        Self::text(s)
    }
}

/// Truncate or pad a string to exactly `width` display columns.
pub fn fit_to_width(s: &str, width: usize) -> String {
    let (fitted, _) = fit_to_width_with_highlights(s, width, &[]);
    fitted
}

/// Truncate or pad `s` to exactly `width` display columns, and project
/// char-index `highlight_ranges` (relative to `s`) onto the resulting string.
///
/// Returns `(fitted_string, projected_char_ranges)`.
///
/// When the string is truncated an ellipsis (`…`) is appended. Ranges that
/// fall entirely after the cut-off point are dropped; partially overlapping
/// ranges are clamped.
pub fn fit_to_width_with_highlights(
    s: &str,
    width: usize,
    highlight_ranges: &[Range<usize>],
) -> (String, Vec<Range<usize>>) {
    use unicode_width::UnicodeWidthChar;
    use unicode_width::UnicodeWidthStr;

    let display_width = s.width();

    if display_width <= width {
        let padding = width - display_width;
        let padded = format!("{}{}", s, " ".repeat(padding));
        return (padded, highlight_ranges.to_vec());
    }

    // Need to truncate. Reserve one display column for the ellipsis.
    let target = width.saturating_sub(1);

    let mut kept_chars: Vec<char> = Vec::new();
    let mut used_cols = 0usize;

    for ch in s.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if used_cols + ch_width > target {
            break;
        }
        kept_chars.push(ch);
        used_cols += ch_width;
    }

    let kept_len = kept_chars.len();

    let mut result = String::with_capacity(kept_len + 3);
    for ch in &kept_chars {
        result.push(*ch);
    }
    result.push('…');

    let projected: Vec<Range<usize>> = highlight_ranges
        .iter()
        .filter_map(|r| {
            let start = r.start.min(kept_len);
            let end = r.end.min(kept_len);
            if start < end { Some(start..end) } else { None }
        })
        .collect();

    (result, projected)
}

/// Fit text to `width` with alignment applied.
///
/// Left alignment pads on the right, right on the left, center on both sides.
/// Truncation always cuts from the right with an ellipsis.
pub fn fit_aligned(s: &str, width: usize, alignment: CellAlignment) -> String {
    use unicode_width::UnicodeWidthStr;

    let display_width = s.width();

    if display_width > width {
        // Truncate (always from right, regardless of alignment).
        return fit_to_width(s, width);
    }

    let padding = width - display_width;
    match alignment {
        CellAlignment::Left => format!("{}{}", s, " ".repeat(padding)),
        CellAlignment::Right => format!("{}{}", " ".repeat(padding), s),
        CellAlignment::Center => {
            let left_pad = padding / 2;
            let right_pad = padding - left_pad;
            format!("{}{}{}", " ".repeat(left_pad), s, " ".repeat(right_pad))
        }
    }
}

/// Fit text to `width` with alignment and highlight projection.
///
/// Returns `(fitted_string, projected_char_ranges)`.
/// For Right/Center alignment, highlight ranges are shifted by the left padding.
pub fn fit_aligned_with_highlights(
    s: &str,
    width: usize,
    alignment: CellAlignment,
    highlight_ranges: &[Range<usize>],
) -> (String, Vec<Range<usize>>) {
    use unicode_width::UnicodeWidthStr;

    let display_width = s.width();

    if display_width > width {
        // Truncation always from right, regardless of alignment.
        return fit_to_width_with_highlights(s, width, highlight_ranges);
    }

    let padding = width - display_width;
    let (left_pad, right_pad) = match alignment {
        CellAlignment::Left => (0, padding),
        CellAlignment::Right => (padding, 0),
        CellAlignment::Center => {
            let l = padding / 2;
            (l, padding - l)
        }
    };

    let fitted = format!("{}{}{}", " ".repeat(left_pad), s, " ".repeat(right_pad));

    // Shift highlight ranges by the left padding (in char count).
    let shift = left_pad; // spaces are 1 char each
    let shifted: Vec<Range<usize>> = highlight_ranges
        .iter()
        .map(|r| (r.start + shift)..(r.end + shift))
        .collect();

    (fitted, shifted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_left_pad() {
        assert_eq!(fit_to_width("abc", 6), "abc   ");
    }

    #[test]
    fn fit_truncate() {
        assert_eq!(fit_to_width("abcdefgh", 5), "abcd…");
    }

    #[test]
    fn fit_exact() {
        assert_eq!(fit_to_width("abc", 3), "abc");
    }

    #[test]
    fn fit_aligned_right() {
        assert_eq!(fit_aligned("42", 6, CellAlignment::Right), "    42");
    }

    #[test]
    fn fit_aligned_center() {
        assert_eq!(fit_aligned("hi", 6, CellAlignment::Center), "  hi  ");
    }

    #[test]
    fn fit_aligned_center_odd() {
        assert_eq!(fit_aligned("hi", 7, CellAlignment::Center), "  hi   ");
    }

    #[test]
    fn fit_aligned_truncate() {
        assert_eq!(fit_aligned("abcdefgh", 5, CellAlignment::Right), "abcd…");
    }

    #[test]
    fn highlight_projection() {
        let (fitted, ranges) = fit_to_width_with_highlights("abcdefgh", 5, &[2..6]);
        assert_eq!(fitted, "abcd…");
        assert_eq!(ranges, vec![2..4]);
    }

    #[test]
    fn highlight_no_truncation() {
        let (fitted, ranges) = fit_to_width_with_highlights("abc", 6, &[1..3]);
        assert_eq!(fitted, "abc   ");
        assert_eq!(ranges, vec![1..3]);
    }

    #[test]
    fn aligned_highlights_right() {
        let (fitted, ranges) = fit_aligned_with_highlights("42", 6, CellAlignment::Right, &[0..2]);
        assert_eq!(fitted, "    42");
        // Ranges shifted by 4 (left padding).
        assert_eq!(ranges, vec![4..6]);
    }

    #[test]
    fn aligned_highlights_center() {
        let (fitted, ranges) = fit_aligned_with_highlights("hi", 6, CellAlignment::Center, &[0..2]);
        assert_eq!(fitted, "  hi  ");
        assert_eq!(ranges, vec![2..4]);
    }

    #[test]
    fn aligned_highlights_truncate() {
        let (fitted, ranges) =
            fit_aligned_with_highlights("abcdefgh", 5, CellAlignment::Right, &[1..4]);
        assert_eq!(fitted, "abcd…");
        assert_eq!(ranges, vec![1..4]);
    }
}
