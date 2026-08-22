//! Window-local drag selection.
//!
//! A terminal's own selection knows nothing about the UI drawn inside it: it
//! sees a grid of cells and takes whole screen rows, straight through a
//! popup's edge and out the other side. This one is anchored to the region
//! the drag started in and is clipped to that rectangle for its whole life,
//! so dragging past a popup's border selects the popup's last column, not the
//! table behind it.
//!
//! Two shapes:
//!
//! * **flow** (default) — like a normal terminal selection, except that it
//!   wraps at the region's left and right edge instead of the terminal's.
//! * **block** (Alt held) — a plain rectangle, for pulling one column out of
//!   a table.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;

use crate::ui::theme::Theme;

/// A selection in progress, or the last finished one.
#[derive(Clone, Copy, Debug)]
pub struct Selection {
    /// The region the drag started in. Both ends stay inside it.
    pub bounds: Rect,
    anchor: (u16, u16),
    cursor: (u16, u16),
    block: bool,
    /// `false` once the button came back up. The selection stays visible
    /// after that so it is obvious what was copied.
    pub dragging: bool,
}

impl Selection {
    pub fn new(bounds: Rect, x: u16, y: u16, block: bool) -> Self {
        let at = clamp_into(bounds, x, y);
        Self {
            bounds,
            anchor: at,
            cursor: at,
            block,
            dragging: true,
        }
    }

    /// Nothing was ever dragged: the press and the release sit on the same
    /// cell. That is a click, and a click means what the surface under it
    /// says it means rather than "select one character".
    pub fn is_click(&self) -> bool {
        self.anchor == self.cursor
    }

    /// Move the loose end. The pointer may be anywhere on the terminal —
    /// including over a different popup — but the selection is not.
    pub fn extend_to(&mut self, x: u16, y: u16) {
        self.cursor = clamp_into(self.bounds, x, y);
    }

    /// Anchor and cursor in reading order, whichever way the drag went.
    fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        let (a, c) = (self.anchor, self.cursor);
        if (a.1, a.0) <= (c.1, c.0) {
            (a, c)
        } else {
            (c, a)
        }
    }

    /// The inclusive column range selected on row `y`, or `None` when the row
    /// is outside the selection.
    fn row_span(&self, y: u16) -> Option<(u16, u16)> {
        let (start, end) = self.ordered();
        if y < start.1 || y > end.1 {
            return None;
        }
        if self.block {
            let (x0, x1) = (
                self.anchor.0.min(self.cursor.0),
                self.anchor.0.max(self.cursor.0),
            );
            return Some((x0, x1));
        }
        let left = if y == start.1 {
            start.0
        } else {
            self.bounds.left()
        };
        let right = if y == end.1 {
            end.0
        } else {
            self.bounds.right().saturating_sub(1)
        };
        Some((left, right))
    }

    /// Tint the selected cells. Colours come from the theme like every other
    /// colour in the app — there is no hardcoded highlight here.
    pub fn paint(&self, buf: &mut Buffer, theme: &Theme) {
        let style = Style::default()
            .fg(theme.selection())
            .bg(theme.selection_bg());
        for y in self.bounds.top()..self.bounds.bottom() {
            let Some((x0, x1)) = self.row_span(y) else {
                continue;
            };
            for x in x0..=x1 {
                if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                    cell.set_style(style);
                }
            }
        }
    }

    /// The selected text, read back from what was actually drawn.
    ///
    /// Trailing blanks are dropped per line: a panel pads its rows out to the
    /// panel width, and nobody wants that padding in their paste buffer.
    pub fn text(&self, snap: &Snapshot) -> String {
        let mut lines: Vec<String> = Vec::new();
        for y in self.bounds.top()..self.bounds.bottom() {
            let Some((x0, x1)) = self.row_span(y) else {
                continue;
            };
            let mut line = String::new();
            for x in x0..=x1 {
                // The trailing half of a wide character carries an empty
                // symbol; skipping it keeps one glyph one glyph.
                line.push_str(snap.get(x, y));
            }
            lines.push(line.trim_end().to_string());
        }
        // A selection dragged past the last line of content ends in blank
        // rows; they carry no information, so they do not travel.
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }
}

/// The characters under a region, taken from the frame buffer at the end of a
/// render pass.
///
/// The buffer that was drawn is not reachable afterwards — ratatui resets it
/// on swap — so the copy has to happen while the frame is still in hand. Only
/// the selection's own region is captured, and only while a selection exists.
pub struct Snapshot {
    area: Rect,
    cells: Vec<String>,
}

impl Snapshot {
    pub fn capture(buf: &Buffer, area: Rect) -> Self {
        let mut cells = Vec::with_capacity(area.area() as usize);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                let symbol = buf
                    .cell(Position::new(x, y))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_default();
                cells.push(symbol);
            }
        }
        Self { area, cells }
    }

    fn get(&self, x: u16, y: u16) -> &str {
        if !self.area.contains(Position::new(x, y)) {
            return "";
        }
        let col = (x - self.area.x) as usize;
        let row = (y - self.area.y) as usize;
        self.cells
            .get(row * self.area.width as usize + col)
            .map(String::as_str)
            .unwrap_or("")
    }
}

/// Pull a coordinate inside `rect`.
fn clamp_into(rect: Rect, x: u16, y: u16) -> (u16, u16) {
    (
        x.clamp(rect.left(), rect.right().saturating_sub(1)),
        y.clamp(rect.top(), rect.bottom().saturating_sub(1)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 10x3 region at (5,5) filled with recognisable rows, inside a wider
    /// terminal whose remaining cells must never be selected.
    fn scene() -> (Buffer, Rect) {
        let bounds = Rect::new(5, 5, 10, 3);
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 10));
        buf.set_string(0, 5, "LLLLLaaaaaaaaaaRRRRR", Style::default());
        buf.set_string(0, 6, "LLLLLbbbbbbbbbbRRRRR", Style::default());
        buf.set_string(0, 7, "LLLLLccccccccccRRRRR", Style::default());
        (buf, bounds)
    }

    #[test]
    fn a_drag_leaving_the_region_stays_inside_it() {
        let (buf, bounds) = scene();
        let mut sel = Selection::new(bounds, 5, 5, false);
        // Pointer runs far off to the right and below the region.
        sel.extend_to(29, 9);
        let snap = Snapshot::capture(&buf, bounds);
        // Nothing from the `L` or `R` columns outside the region comes along.
        assert_eq!(sel.text(&snap), "aaaaaaaaaa\nbbbbbbbbbb\ncccccccccc");
    }

    #[test]
    fn flow_selection_wraps_at_the_region_edge_not_the_terminal_edge() {
        let (buf, bounds) = scene();
        let mut sel = Selection::new(bounds, 12, 5, false);
        sel.extend_to(7, 6);
        let snap = Snapshot::capture(&buf, bounds);
        // From column 12 to the region's right edge, then from the region's
        // left edge up to column 7 — not through the `R`s in between.
        assert_eq!(sel.text(&snap), "aaa\nbbb");
    }

    #[test]
    fn block_selection_takes_a_column() {
        let (buf, bounds) = scene();
        let mut sel = Selection::new(bounds, 6, 5, true);
        sel.extend_to(7, 7);
        let snap = Snapshot::capture(&buf, bounds);
        assert_eq!(sel.text(&snap), "aa\nbb\ncc");
    }

    #[test]
    fn dragging_backwards_selects_the_same_text() {
        let (buf, bounds) = scene();
        let snap = Snapshot::capture(&buf, bounds);
        let mut forward = Selection::new(bounds, 6, 5, false);
        forward.extend_to(8, 6);
        let mut backward = Selection::new(bounds, 8, 6, false);
        backward.extend_to(6, 5);
        assert_eq!(forward.text(&snap), backward.text(&snap));
    }

    #[test]
    fn panel_padding_is_trimmed_off_each_line() {
        let bounds = Rect::new(0, 0, 12, 2);
        let mut buf = Buffer::empty(bounds);
        buf.set_string(0, 0, "hello       ", Style::default());
        buf.set_string(0, 1, "world       ", Style::default());
        let mut sel = Selection::new(bounds, 0, 0, false);
        sel.extend_to(11, 1);
        let snap = Snapshot::capture(&buf, bounds);
        assert_eq!(sel.text(&snap), "hello\nworld");
    }

    #[test]
    fn trailing_blank_rows_are_dropped() {
        let bounds = Rect::new(0, 0, 6, 3);
        let mut buf = Buffer::empty(bounds);
        buf.set_string(0, 0, "text  ", Style::default());
        let mut sel = Selection::new(bounds, 0, 0, false);
        sel.extend_to(5, 2);
        let snap = Snapshot::capture(&buf, bounds);
        assert_eq!(sel.text(&snap), "text");
    }

    #[test]
    fn only_selected_cells_are_tinted() {
        let (mut buf, bounds) = scene();
        let theme = Theme::new(crate::config::ThemeConfig::default());
        let mut sel = Selection::new(bounds, 5, 5, true);
        sel.extend_to(6, 5);
        sel.paint(&mut buf, &theme);
        let bg = theme.selection_bg();
        assert_eq!(buf[(5, 5)].bg, bg);
        assert_eq!(buf[(6, 5)].bg, bg);
        // The cell just outside the block keeps its own style.
        assert_ne!(buf[(7, 5)].bg, bg);
        // …and so does the row below, which the block never reached.
        assert_ne!(buf[(5, 6)].bg, bg);
    }

    #[test]
    fn a_press_and_release_without_movement_is_a_click() {
        let (_, bounds) = scene();
        let sel = Selection::new(bounds, 7, 6, false);
        assert!(sel.is_click());
    }

    #[test]
    fn one_cell_of_movement_already_makes_it_a_drag() {
        // The threshold is deliberately a single cell: anything wider would
        // swallow a short selection, and the user asked for text either way.
        let (_, bounds) = scene();
        let mut sel = Selection::new(bounds, 7, 6, false);
        sel.extend_to(8, 6);
        assert!(!sel.is_click());
    }

    #[test]
    fn a_drag_that_left_the_region_is_not_a_click() {
        // Clamping pulls the cursor back to the region's edge, not to the
        // anchor, so leaving the window can never read as a press.
        let (_, bounds) = scene();
        let mut sel = Selection::new(bounds, 7, 6, false);
        sel.extend_to(29, 9);
        assert!(!sel.is_click());
    }

    #[test]
    fn wide_characters_survive_the_round_trip() {
        let bounds = Rect::new(0, 0, 8, 1);
        let mut buf = Buffer::empty(bounds);
        buf.set_string(0, 0, "├─ ✓ ok", Style::default());
        let mut sel = Selection::new(bounds, 0, 0, false);
        sel.extend_to(7, 0);
        let snap = Snapshot::capture(&buf, bounds);
        assert_eq!(sel.text(&snap), "├─ ✓ ok");
    }
}
