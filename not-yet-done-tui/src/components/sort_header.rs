//! Header decoration: applied-sort arrow + sort-hint mode overlay.
//!
//! All sort-aware tables (Tasks, Content) feed their header cells through
//! these helpers so the sort UI stays consistent across views.

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use not_yet_done_content::{SortDirection, SortKey};
use not_yet_done_ratatui::TableWidgetCell;

/// StyleMap slot reserved for the "dimmed" header cell style. Views that
/// surface sort-hint overlay must register a dim color at this index.
pub const DIM_STYLE_ID: usize = 0;

/// Visual state layered onto column headers while sort mode is active.
#[derive(Debug, Clone)]
pub enum HeaderOverlay {
    /// No overlay; only the applied-sort arrow is added.
    None,
    /// Phase 1: column picker. Each candidate column shows its label
    /// woven into the header text. Other columns are dimmed.
    PickColumn {
        /// Map column key → label string (e.g. "a"). Only entries here
        /// are treated as candidates.
        labels: HashMap<String, String>,
        /// Number of label chars already consumed by user input.
        input_len: usize,
    },
    /// Phase 2: a column was picked. The picker `(d)esc/(a)sc/(c)lear`
    /// is rendered as an overlay (no layout impact) by
    /// [`render_direction_picker_overlay`]. Other columns are dimmed.
    PickDirection { column_key: String },
}

impl Default for HeaderOverlay {
    fn default() -> Self {
        HeaderOverlay::None
    }
}

impl HeaderOverlay {
    pub fn is_active(&self) -> bool {
        !matches!(self, HeaderOverlay::None)
    }
}

/// String shown in the direction-picker phase. Public so tests can pin it.
pub const DIRECTION_PICKER_LABEL: &str = "(d)esc/(a)sc/(c)lear";

/// Compute the header text for a single column under the given overlay.
/// `original` is the column's natural header label (e.g. "Status").
///
/// Width-stable across all phases: candidates always start from
/// `with_sort_arrow(original, …)`, so the column's width is dictated by
/// its plain header (plus any applied-sort arrow) and never expands or
/// shrinks while the user is in sort-hint mode.
pub fn header_text(
    original: &str,
    column_key: &str,
    applied: &[SortKey],
    overlay: &HeaderOverlay,
) -> String {
    let with_arrow = with_sort_arrow(original, column_key, applied);
    match overlay {
        HeaderOverlay::None => with_arrow,
        HeaderOverlay::PickColumn { labels, .. } => match labels.get(column_key) {
            Some(label_str) => weave_label(&with_arrow, label_str),
            None => with_arrow,
        },
        HeaderOverlay::PickDirection { .. } => with_arrow,
    }
}

/// Build the final `TableWidgetCell` for a header column given the
/// already-laid-out text (post-sizing) and overlay state. The fitted
/// text may differ from `header_text(original, ...)` due to padding /
/// truncation by the layout engine.
pub fn header_cell(fitted: &str, column_key: &str, overlay: &HeaderOverlay) -> TableWidgetCell {
    match overlay {
        HeaderOverlay::None => TableWidgetCell::plain(fitted.to_string()),
        HeaderOverlay::PickColumn { labels, input_len } => match labels.get(column_key) {
            Some(label_str) => {
                let label_len = label_str.chars().count();
                let lo = (*input_len).min(label_len);
                let fitted_len = fitted.chars().count();
                let hi = label_len.min(fitted_len);
                if lo < hi {
                    TableWidgetCell::with_highlights(fitted.to_string(), vec![lo..hi])
                } else {
                    TableWidgetCell::plain(fitted.to_string())
                }
            }
            None => TableWidgetCell::plain(fitted.to_string()).with_style(DIM_STYLE_ID),
        },
        HeaderOverlay::PickDirection { column_key: chosen } => {
            if column_key == chosen {
                // Underlying cell stays as the original header (with
                // arrow, if any). The picker is painted on top via
                // [`render_direction_picker_overlay`].
                TableWidgetCell::plain(fitted.to_string())
            } else {
                TableWidgetCell::plain(fitted.to_string()).with_style(DIM_STYLE_ID)
            }
        }
    }
}

/// Paint the direction picker on top of the header row of the chosen
/// column. Does nothing if `overlay` is not `PickDirection` or the
/// column is not present.
///
/// `table_area` is the rect the table widget was drawn into (header row
/// is the first line). `column_keys` and `col_widths` describe the
/// laid-out columns in display order. `separator_width` is the visual
/// width of the column separator (`"  "` is 2).
pub fn render_direction_picker_overlay(
    frame: &mut Frame,
    table_area: Rect,
    column_keys: &[&str],
    col_widths: &[usize],
    separator_width: u16,
    overlay: &HeaderOverlay,
    style: Style,
) {
    let HeaderOverlay::PickDirection { column_key: chosen } = overlay else {
        return;
    };
    if table_area.height == 0 || table_area.width == 0 {
        return;
    }
    let Some(idx) = column_keys.iter().position(|k| *k == chosen) else {
        return;
    };

    let mut x_offset: u16 = 0;
    for i in 0..idx {
        let w = col_widths.get(i).copied().unwrap_or(0) as u16;
        x_offset = x_offset.saturating_add(w);
        x_offset = x_offset.saturating_add(separator_width);
    }
    let x = table_area.x.saturating_add(x_offset);
    if x >= table_area.right() {
        return;
    }
    let width = table_area.right().saturating_sub(x);
    let overlay_area = Rect {
        x,
        y: table_area.y,
        width,
        height: 1,
    };
    let line = Line::from(Span::styled(DIRECTION_PICKER_LABEL.to_string(), style));
    frame.render_widget(Paragraph::new(line), overlay_area);
}

fn weave_label(original: &str, label: &str) -> String {
    let label_chars: Vec<char> = label.chars().collect();
    let header_chars: Vec<char> = original.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(header_chars.len().max(label_chars.len()));
    out.extend_from_slice(&label_chars);
    if header_chars.len() > label_chars.len() {
        out.extend_from_slice(&header_chars[label_chars.len()..]);
    }
    out.iter().collect()
}

fn with_sort_arrow(original: &str, column_key: &str, applied: &[SortKey]) -> String {
    let Some(idx) = applied.iter().position(|s| s.column == column_key) else {
        return original.to_string();
    };
    let arrow = match applied[idx].direction {
        SortDirection::Asc => '\u{25B2}',
        SortDirection::Desc => '\u{25BC}',
    };
    if applied.len() > 1 {
        format!("{original} {arrow}{}", subscript_digit(idx + 1))
    } else {
        format!("{original} {arrow}")
    }
}

fn subscript_digit(n: usize) -> String {
    const DIGITS: [char; 10] = ['₀', '₁', '₂', '₃', '₄', '₅', '₆', '₇', '₈', '₉'];
    n.to_string()
        .chars()
        .filter_map(|c| c.to_digit(10).map(|d| DIGITS[d as usize]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn applied(col: &str, dir: SortDirection) -> Vec<SortKey> {
        vec![SortKey {
            column: col.into(),
            direction: dir,
        }]
    }

    #[test]
    fn sort_arrow_appends_for_sorted_column() {
        let a = applied("modified", SortDirection::Desc);
        assert_eq!(
            header_text("Modified", "modified", &a, &HeaderOverlay::None),
            "Modified ▼"
        );
        assert_eq!(
            header_text("Status", "status", &a, &HeaderOverlay::None),
            "Status"
        );
    }

    #[test]
    fn pick_column_weaves_label_into_header() {
        let mut labels = HashMap::new();
        labels.insert("status".to_string(), "a".to_string());
        let overlay = HeaderOverlay::PickColumn {
            labels,
            input_len: 0,
        };
        assert_eq!(header_text("Status", "status", &[], &overlay), "atatus");
        assert_eq!(header_text("Pri", "priority", &[], &overlay), "Pri");
    }

    #[test]
    fn pick_column_keeps_sort_arrow_on_candidate() {
        // A candidate that's also currently sorted must keep its arrow
        // so the column's display width does not change between Off and
        // PickColumn phases.
        let a = applied("status", SortDirection::Asc);
        let mut labels = HashMap::new();
        labels.insert("status".to_string(), "a".to_string());
        let overlay = HeaderOverlay::PickColumn {
            labels,
            input_len: 0,
        };
        assert_eq!(header_text("Status", "status", &a, &overlay), "atatus ▲");
    }

    #[test]
    fn pick_column_short_header_keeps_label() {
        let mut labels = HashMap::new();
        labels.insert("notes".to_string(), "a".to_string());
        let overlay = HeaderOverlay::PickColumn {
            labels,
            input_len: 0,
        };
        assert_eq!(header_text("N", "notes", &[], &overlay), "a");
    }

    #[test]
    fn pick_direction_keeps_original_header() {
        // Picker is drawn as overlay; the underlying header text must
        // stay at original width to not push other columns.
        let overlay = HeaderOverlay::PickDirection {
            column_key: "status".to_string(),
        };
        assert_eq!(header_text("Status", "status", &[], &overlay), "Status");
        assert_eq!(header_text("Pri", "priority", &[], &overlay), "Pri");
    }

    #[test]
    fn pick_direction_keeps_arrow_under_overlay() {
        let a = applied("status", SortDirection::Desc);
        let overlay = HeaderOverlay::PickDirection {
            column_key: "status".to_string(),
        };
        assert_eq!(header_text("Status", "status", &a, &overlay), "Status ▼");
    }

    #[test]
    fn header_cell_dims_non_candidates_in_pick_column() {
        let mut labels = HashMap::new();
        labels.insert("status".to_string(), "a".to_string());
        let overlay = HeaderOverlay::PickColumn {
            labels,
            input_len: 0,
        };
        let candidate = header_cell("atatus", "status", &overlay);
        assert!(
            candidate.style_id.is_none(),
            "candidate should not be dimmed"
        );
        assert_eq!(candidate.highlights, vec![0..1]);
        let non_candidate = header_cell("Pri", "priority", &overlay);
        assert_eq!(non_candidate.style_id, Some(DIM_STYLE_ID));
    }

    #[test]
    fn header_cell_dims_other_columns_in_pick_direction() {
        let overlay = HeaderOverlay::PickDirection {
            column_key: "status".to_string(),
        };
        // Chosen column: cell is plain (no dim, no special highlights);
        // the picker is drawn on top by render_direction_picker_overlay.
        let chosen = header_cell("Status", "status", &overlay);
        assert!(chosen.style_id.is_none());
        assert_eq!(chosen.highlights, Vec::<std::ops::Range<usize>>::new());
        assert_eq!(
            header_cell("Pri", "priority", &overlay).style_id,
            Some(DIM_STYLE_ID)
        );
    }
}
