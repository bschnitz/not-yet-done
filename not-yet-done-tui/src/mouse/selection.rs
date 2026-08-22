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

/// How much of the text a drag takes at a time.
///
/// A terminal escalates with the click count — the second click picks a word,
/// the third a line — and dragging out of either keeps that unit instead of
/// falling back to single cells. The count is known at press time, so the
/// grain is fixed before the first drag event arrives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grain {
    Cell,
    Word,
    Line,
}

impl Grain {
    /// What a run of `clicks` presses selects by.
    pub fn of(clicks: u8) -> Self {
        match clicks {
            2 => Grain::Word,
            n if n >= 3 => Grain::Line,
            _ => Grain::Cell,
        }
    }
}

/// A selection in progress, or the last finished one.
#[derive(Clone, Copy, Debug)]
pub struct Selection {
    /// The region the drag started in. Both ends stay inside it.
    pub bounds: Rect,
    /// The cell the button went down on, and the cell the pointer is on now.
    /// The visible ends are derived from these two on every move, so a drag
    /// that turns around gives back exactly what it took.
    origin: (u16, u16),
    tip: (u16, u16),
    anchor: (u16, u16),
    cursor: (u16, u16),
    grain: Grain,
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
            origin: at,
            tip: at,
            anchor: at,
            cursor: at,
            grain: Grain::Cell,
            block,
            dragging: true,
        }
    }

    /// Select by whole words or whole lines from here on.
    ///
    /// Deliberately does *not* snap the anchor to that unit yet: until the
    /// pointer moves this is still a click, and a double click on a table row
    /// has to keep opening the row rather than highlighting its text.
    pub fn with_grain(mut self, grain: Grain) -> Self {
        self.grain = grain;
        self
    }

    /// Nothing was ever dragged: the press and the release sit on the same
    /// cell. That is a click, and a click means what the surface under it
    /// says it means rather than "select one character".
    pub fn is_click(&self) -> bool {
        self.origin == self.tip
    }

    /// Move the loose end. The pointer may be anywhere on the terminal —
    /// including over a different popup — but the selection is not.
    ///
    /// The snapshot is what word boundaries are read from; without one (the
    /// first drag of a frame that has not been painted yet) a word drag stays
    /// cell-wise for that one event and catches up on the next, because both
    /// ends are recomputed from scratch every time.
    pub fn extend_to(&mut self, x: u16, y: u16, snap: Option<&Snapshot>) {
        self.tip = clamp_into(self.bounds, x, y);
        self.widen(snap);
    }

    /// Derive the visible ends from press and pointer, grown to the grain.
    ///
    /// Reading order decides which end is which: the leading end grows
    /// backwards to the start of its word or line, the trailing end forwards
    /// to the end of its own. Both units come out whole, which is what makes
    /// a drag that reverses direction symmetrical.
    fn widen(&mut self, snap: Option<&Snapshot>) {
        let (o, t) = (self.origin, self.tip);
        let forward = (o.1, o.0) <= (t.1, t.0);
        let (anchor, cursor) = match (self.grain, snap) {
            (Grain::Word, Some(snap)) => {
                let edge = |at: (u16, u16), leading: bool| {
                    word_run(snap, self.bounds, at.0, at.1)
                        .map_or(at, |(l, r)| (if leading { l } else { r }, at.1))
                };
                (edge(o, forward), edge(t, !forward))
            }
            (Grain::Line, _) => {
                let (l, r) = (self.bounds.left(), self.bounds.right().saturating_sub(1));
                if forward {
                    ((l, o.1), (r, t.1))
                } else {
                    ((r, o.1), (l, t.1))
                }
            }
            _ => (o, t),
        };
        self.anchor = anchor;
        self.cursor = cursor;
    }

    /// Reshape into the word under `(x, y)`, as a double click does in a
    /// terminal. Reports whether there was a word there at all — on a blank
    /// cell or a tree glyph the selection is left alone.
    ///
    /// The word is read back from what was drawn, so it is the word the user
    /// sees, and it stops at the region's edge like every other selection
    /// here: a value clipped by its column ends where the column does.
    pub fn select_word(&mut self, snap: &Snapshot, x: u16, y: u16) -> bool {
        let (x, y) = clamp_into(self.bounds, x, y);
        let Some((left, right)) = word_run(snap, self.bounds, x, y) else {
            return false;
        };
        self.reshape((left, y), (right, y));
        true
    }

    /// Reshape into the whole line at `y`, blanks at either end left out — the
    /// third click of a run. Reports whether the line held anything.
    pub fn select_line(&mut self, snap: &Snapshot, y: u16) -> bool {
        let (_, y) = clamp_into(self.bounds, self.bounds.left(), y);
        let filled = |x: &u16| !snap.get(*x, y).trim().is_empty();
        let range = self.bounds.left()..self.bounds.right();
        let (Some(first), Some(last)) = (range.clone().find(filled), range.rev().find(filled))
        else {
            return false;
        };
        self.reshape((first, y), (last, y));
        true
    }

    /// Put both ends somewhere else. Always a flow selection: `Alt` shapes a
    /// drag, not the run a click picks out.
    fn reshape(&mut self, anchor: (u16, u16), cursor: (u16, u16)) {
        self.anchor = anchor;
        self.cursor = cursor;
        self.block = false;
        self.dragging = false;
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

/// The inclusive column range of the word covering `(x, y)`, or `None` when
/// there is no word there — a blank, a tree glyph, a border.
///
/// Clipped to `bounds` like every other selection here: a value cut short by
/// its column ends where the column does, and the cells beyond belong to
/// another widget.
fn word_run(snap: &Snapshot, bounds: Rect, x: u16, y: u16) -> Option<(u16, u16)> {
    if !is_word_cell(snap, x, y) {
        return None;
    }
    let mut left = x;
    while left > bounds.left() && is_word_cell(snap, left - 1, y) {
        left -= 1;
    }
    let mut right = x;
    while right + 1 < bounds.right() && is_word_cell(snap, right + 1, y) {
        right += 1;
    }
    Some((left, right))
}

/// Whether a cell belongs to a word.
///
/// Letters and digits, plus the punctuation that holds an identifier together
/// — a ticket key, a path, a URL, a `snake_case` name should each come out in
/// one click. Everything else separates: spaces, the tree connectors, quotes
/// and brackets, so double-clicking a value inside them does not drag them
/// along.
fn is_word_cell(snap: &Snapshot, x: u16, y: u16) -> bool {
    snap.get(x, y)
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || "_-./:#@+~=?&%".contains(c))
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
        sel.extend_to(29, 9, None);
        let snap = Snapshot::capture(&buf, bounds);
        // Nothing from the `L` or `R` columns outside the region comes along.
        assert_eq!(sel.text(&snap), "aaaaaaaaaa\nbbbbbbbbbb\ncccccccccc");
    }

    #[test]
    fn flow_selection_wraps_at_the_region_edge_not_the_terminal_edge() {
        let (buf, bounds) = scene();
        let mut sel = Selection::new(bounds, 12, 5, false);
        sel.extend_to(7, 6, None);
        let snap = Snapshot::capture(&buf, bounds);
        // From column 12 to the region's right edge, then from the region's
        // left edge up to column 7 — not through the `R`s in between.
        assert_eq!(sel.text(&snap), "aaa\nbbb");
    }

    #[test]
    fn block_selection_takes_a_column() {
        let (buf, bounds) = scene();
        let mut sel = Selection::new(bounds, 6, 5, true);
        sel.extend_to(7, 7, None);
        let snap = Snapshot::capture(&buf, bounds);
        assert_eq!(sel.text(&snap), "aa\nbb\ncc");
    }

    #[test]
    fn dragging_backwards_selects_the_same_text() {
        let (buf, bounds) = scene();
        let snap = Snapshot::capture(&buf, bounds);
        let mut forward = Selection::new(bounds, 6, 5, false);
        forward.extend_to(8, 6, None);
        let mut backward = Selection::new(bounds, 8, 6, false);
        backward.extend_to(6, 5, None);
        assert_eq!(forward.text(&snap), backward.text(&snap));
    }

    #[test]
    fn panel_padding_is_trimmed_off_each_line() {
        let bounds = Rect::new(0, 0, 12, 2);
        let mut buf = Buffer::empty(bounds);
        buf.set_string(0, 0, "hello       ", Style::default());
        buf.set_string(0, 1, "world       ", Style::default());
        let mut sel = Selection::new(bounds, 0, 0, false);
        sel.extend_to(11, 1, None);
        let snap = Snapshot::capture(&buf, bounds);
        assert_eq!(sel.text(&snap), "hello\nworld");
    }

    #[test]
    fn trailing_blank_rows_are_dropped() {
        let bounds = Rect::new(0, 0, 6, 3);
        let mut buf = Buffer::empty(bounds);
        buf.set_string(0, 0, "text  ", Style::default());
        let mut sel = Selection::new(bounds, 0, 0, false);
        sel.extend_to(5, 2, None);
        let snap = Snapshot::capture(&buf, bounds);
        assert_eq!(sel.text(&snap), "text");
    }

    #[test]
    fn only_selected_cells_are_tinted() {
        let (mut buf, bounds) = scene();
        let theme = Theme::new(crate::config::ThemeConfig::default());
        let mut sel = Selection::new(bounds, 5, 5, true);
        sel.extend_to(6, 5, None);
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
        sel.extend_to(8, 6, None);
        assert!(!sel.is_click());
    }

    #[test]
    fn a_drag_that_left_the_region_is_not_a_click() {
        // Clamping pulls the cursor back to the region's edge, not to the
        // anchor, so leaving the window can never read as a press.
        let (_, bounds) = scene();
        let mut sel = Selection::new(bounds, 7, 6, false);
        sel.extend_to(29, 9, None);
        assert!(!sel.is_click());
    }

    /// One line of realistic table text inside a region that starts at
    /// column 5, so nothing here can pass by accident on `x == index`.
    fn line(text: &str) -> (Snapshot, Selection) {
        let bounds = Rect::new(5, 5, text.chars().count() as u16, 1);
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 10));
        buf.set_string(5, 5, text, Style::default());
        let snap = Snapshot::capture(&buf, bounds);
        (snap, Selection::new(bounds, 5, 5, false))
    }

    /// What a click on the character at `offset` of that line picks out.
    fn word_at(text: &str, offset: u16) -> Option<String> {
        let (snap, mut sel) = line(text);
        sel.select_word(&snap, 5 + offset, 5)
            .then(|| sel.text(&snap))
    }

    #[test]
    fn a_double_click_takes_the_word_under_it() {
        assert_eq!(word_at("hello world", 7).as_deref(), Some("world"));
        assert_eq!(word_at("hello world", 0).as_deref(), Some("hello"));
    }

    #[test]
    fn an_identifier_comes_out_in_one_piece() {
        // The punctuation that holds a key, a path or a URL together is part
        // of the word — that is the whole point of clicking one.
        assert_eq!(word_at("PROJ-1234 open", 3).as_deref(), Some("PROJ-1234"));
        assert_eq!(
            word_at("see https://example.test/a/b now", 12).as_deref(),
            Some("https://example.test/a/b"),
        );
        assert_eq!(
            word_at("a_snake_case value", 2).as_deref(),
            Some("a_snake_case")
        );
    }

    #[test]
    fn brackets_and_quotes_stay_behind() {
        assert_eq!(word_at("(inner)", 3).as_deref(), Some("inner"));
        assert_eq!(word_at("say \"word\" here", 6).as_deref(), Some("word"));
    }

    #[test]
    fn a_blank_or_a_tree_glyph_is_not_a_word() {
        assert_eq!(word_at("a  b", 1), None, "the gap");
        assert_eq!(word_at("├── Child", 1), None, "a connector cell");
    }

    #[test]
    fn a_word_stops_at_the_region_edge() {
        // The region cuts the text short; the click must not read on into the
        // cells beyond it, which belong to another widget.
        let bounds = Rect::new(5, 5, 4, 1);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 10));
        buf.set_string(5, 5, "abcdefgh", Style::default());
        let snap = Snapshot::capture(&buf, bounds);
        let mut sel = Selection::new(bounds, 6, 5, false);
        assert!(sel.select_word(&snap, 6, 5));
        assert_eq!(sel.text(&snap), "abcd");
    }

    #[test]
    fn a_triple_click_takes_the_line_without_its_padding() {
        let (snap, mut sel) = line("   spaced out   ");
        assert!(sel.select_line(&snap, 5));
        assert_eq!(sel.text(&snap), "spaced out");
    }

    #[test]
    fn a_blank_line_has_nothing_to_take() {
        let (snap, mut sel) = line("      ");
        assert!(!sel.select_line(&snap, 5));
    }

    #[test]
    fn picking_a_run_drops_the_block_shape() {
        // `Alt` shapes a drag; a click that picks out a word is flowing text
        // either way, so a leftover block flag cannot narrow it.
        let (snap, _) = line("one two");
        let bounds = Rect::new(5, 5, 7, 1);
        let mut sel = Selection::new(bounds, 5, 5, true);
        assert!(sel.select_word(&snap, 9, 5));
        assert_eq!(sel.text(&snap), "two");
    }

    /// Press at `from` and drag to `to` — both offsets into that one line —
    /// with the grain a run of clicks would have set.
    fn dragged(text: &str, grain: Grain, from: u16, to: u16) -> String {
        let (snap, sel) = line(text);
        let mut sel = Selection::new(sel.bounds, 5 + from, 5, false).with_grain(grain);
        sel.extend_to(5 + to, 5, Some(&snap));
        sel.text(&snap)
    }

    #[test]
    fn a_run_of_clicks_maps_onto_a_grain() {
        assert_eq!(Grain::of(1), Grain::Cell);
        assert_eq!(Grain::of(2), Grain::Word);
        assert_eq!(Grain::of(3), Grain::Line);
    }

    #[test]
    fn a_double_click_that_never_moves_is_still_a_click() {
        // The grain is set on the press but nothing snaps to it yet —
        // otherwise a double click on a table row would highlight its text
        // instead of opening it.
        let (_, bounds) = scene();
        let sel = Selection::new(bounds, 7, 6, false).with_grain(Grain::Word);
        assert!(sel.is_click());
    }

    #[test]
    fn dragging_out_of_a_double_click_takes_whole_words() {
        // Both ends come out whole: the word the press landed in and the word
        // the pointer is over, however little of either was crossed.
        assert_eq!(dragged("alpha beta gamma", Grain::Word, 2, 7), "alpha beta");
    }

    #[test]
    fn a_word_drag_that_turns_around_is_symmetrical() {
        let forward = dragged("alpha beta gamma", Grain::Word, 2, 13);
        let backward = dragged("alpha beta gamma", Grain::Word, 13, 2);
        assert_eq!(forward, "alpha beta gamma");
        assert_eq!(forward, backward);
    }

    #[test]
    fn a_word_drag_ending_on_a_blank_stops_at_the_last_word() {
        // Nothing to widen out there, so that end stays where the pointer is
        // — and the trailing blank does not travel.
        assert_eq!(dragged("alpha beta", Grain::Word, 2, 5), "alpha");
    }

    #[test]
    fn dragging_out_of_a_triple_click_takes_whole_lines() {
        let (buf, bounds) = scene();
        let snap = Snapshot::capture(&buf, bounds);
        // Press in the middle of the first row, pointer in the middle of the
        // second: both rows come out entire, edge to edge of the region.
        let mut sel = Selection::new(bounds, 8, 5, false).with_grain(Grain::Line);
        sel.extend_to(7, 6, Some(&snap));
        assert_eq!(sel.text(&snap), "aaaaaaaaaa\nbbbbbbbbbb");
    }

    #[test]
    fn a_line_drag_that_turns_around_is_symmetrical() {
        let (buf, bounds) = scene();
        let snap = Snapshot::capture(&buf, bounds);
        let mut up = Selection::new(bounds, 7, 7, false).with_grain(Grain::Line);
        up.extend_to(8, 6, Some(&snap));
        assert_eq!(up.text(&snap), "bbbbbbbbbb\ncccccccccc");
    }

    #[test]
    fn a_line_drag_needs_no_snapshot() {
        // Line boundaries are the region's edges, not something read back off
        // the screen, so the first drag of a frame is already whole-line.
        let (buf, bounds) = scene();
        let mut sel = Selection::new(bounds, 8, 5, false).with_grain(Grain::Line);
        sel.extend_to(7, 5, None);
        let snap = Snapshot::capture(&buf, bounds);
        assert_eq!(sel.text(&snap), "aaaaaaaaaa");
    }

    #[test]
    fn wide_characters_survive_the_round_trip() {
        let bounds = Rect::new(0, 0, 8, 1);
        let mut buf = Buffer::empty(bounds);
        buf.set_string(0, 0, "├─ ✓ ok", Style::default());
        let mut sel = Selection::new(bounds, 0, 0, false);
        sel.extend_to(7, 0, None);
        let snap = Snapshot::capture(&buf, bounds);
        assert_eq!(sel.text(&snap), "├─ ✓ ok");
    }
}
