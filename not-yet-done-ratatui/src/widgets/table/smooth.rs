//! Smooth (line-wise) scrolling for the [`Table`] widget.
//!
//! In the default *discrete* mode the table scrolls one whole row at a time
//! and the first visible row always sits flush against the top edge. Smooth
//! mode decouples scrolling from selection: the viewport moves one *physical
//! line* at a time, so a tall multi-line row may be partially clipped at the
//! top and the content appears to glide continuously over the screen. It is
//! meant for long, multi-line lists such as the chat view.
//!
//! ## Scroll position
//!
//! The position is stored as the existing row index [`Table::scroll_offset`]
//! plus [`Table::scroll_sub_line`] — the number of leading physical lines of
//! that row hidden above the viewport. Together they encode a single global
//! physical-line offset; the helpers here convert between that offset and the
//! `(row, sub-line)` pair, and clamp it to the valid range.
//!
//! ## Selection (the cursor rides the leading edge)
//!
//! The selection (the highlight, and the row that node-actions operate on) is
//! attached to a *specific row*, not to a screen position. Scrolling only pans
//! the viewport, but every pan also hands the focus **one row onward in the
//! direction of travel as soon as that row can be seen**: scrolling down moves
//! it to the next selectable row the moment any highlightable line of that row
//! enters the viewport, scrolling up to the previous one. Only when the
//! neighbour is still off-screen (a row taller than what is left of the
//! viewport) does the focus stay put and the pan is pure scrolling.
//!
//! The rule is deliberately about the **neighbour**, not about the current
//! row: waiting for the focused row to leave the view is what made `j` feel
//! dead in a chat, where a long message keeps the highlight for a dozen
//! keypresses while the next message already sits fully on screen. One pan
//! hands off at most one row, so `j`/`k` walk the messages one by one and the
//! cursor ends up riding the edge the content scrolls toward.
//!
//! "Can be seen" means a line that actually *shows* the highlight: the trailing
//! spacer line of a chat row opts out of it, so a row whose spacer alone peeks
//! in is not focused yet — the highlight would be invisible.
//!
//! A pan large enough to leave the focus completely behind (the page keys)
//! falls back to the direction-agnostic re-attach below, so the cursor never
//! drops off-screen.
//!
//! Frame rebuilds and data/viewport changes use a gentler, direction-agnostic
//! re-attach ([`reattach_selection_if_offscreen`](Table::reattach_selection_if_offscreen))
//! that only moves the focus when its row is *completely* off-screen — so
//! re-selecting the same row every frame never makes the focus drift on its
//! own; only an actual scroll hands it off.
//!
//! `Home`/`End` are explicit jumps and place the selection on the first / last
//! selectable row; programmatic selection ([`Table::set_selected`]) scrolls
//! the target *minimally* into view rather than forcing it to the top. The one
//! placement that does anchor the target at the top edge is
//! [`Table::set_selected_at_top`] — used when what follows the target is the
//! point of the jump (e.g. the first unread chat message).
//!
//! ## Cursor step when nothing can scroll
//!
//! Selection is normally driven *by* scrolling, so when there is nothing to
//! scroll — the whole list fits on screen, or the viewport already sits at an
//! edge — pressing `j`/`k` would otherwise do nothing. To keep a visible,
//! wandering cursor in that case, the `j`/`k` handlers fall back to stepping
//! the selection to the next/previous selectable row
//! ([`step_selection_down`](Table::step_selection_down) /
//! [`step_selection_up`](Table::step_selection_up)). This is also what keeps
//! the very first/last message reachable once scrolling has bottomed out.

use super::component::Table;

impl Table {
    /// Total number of physical lines across all data rows.
    fn total_data_lines(&self) -> usize {
        self.rows.iter().map(|r| r.height()).sum()
    }

    /// Largest valid scroll offset (in physical lines) for `line_budget`
    /// visible lines: scrolling stops once the last physical line rests on
    /// the bottom edge. Zero when all content fits.
    fn max_scroll_line(&self, line_budget: usize) -> usize {
        self.total_data_lines().saturating_sub(line_budget)
    }

    /// Current scroll position as a global physical-line offset from the top
    /// of the data region.
    fn scroll_line_offset(&self) -> usize {
        let lines_above: usize = self
            .rows
            .iter()
            .take(self.scroll_offset)
            .map(|r| r.height())
            .sum();
        lines_above + self.scroll_sub_line
    }

    /// Global physical-line offset of `row`'s first line.
    fn row_line_start(&self, row: usize) -> usize {
        self.rows.iter().take(row).map(|r| r.height()).sum()
    }

    /// Decompose a global physical-line offset back into
    /// `(scroll_offset, scroll_sub_line)`, clamped to `[0, max_scroll_line]`.
    /// Pans only — never touches the selection.
    fn set_scroll_line_offset(&mut self, line: usize, line_budget: usize) {
        if self.rows.is_empty() {
            self.scroll_offset = 0;
            self.scroll_sub_line = 0;
            return;
        }
        let line = line.min(self.max_scroll_line(line_budget));
        let mut acc = 0;
        for (i, row) in self.rows.iter().enumerate() {
            let h = row.height();
            if acc + h > line {
                self.scroll_offset = i;
                self.scroll_sub_line = line - acc;
                return;
            }
            acc += h;
        }
        // Reached only when the whole content fits (`line == 0`) or in the
        // degenerate zero-height-viewport case: pin to the last row's top.
        self.scroll_offset = self.rows.len() - 1;
        self.scroll_sub_line = 0;
    }

    fn first_selectable(&self) -> Option<usize> {
        (0..self.rows.len()).find(|&i| self.rows[i].selectable)
    }

    fn last_selectable(&self) -> Option<usize> {
        (0..self.rows.len())
            .rev()
            .find(|&i| self.rows[i].selectable)
    }

    /// The topmost and bottommost *selectable* rows that are at least
    /// partially inside the viewport `[view_top, view_bottom)` (both in
    /// global physical-line coordinates). `None` if no selectable row is
    /// visible (e.g. a zero-height viewport).
    fn visible_selectable_range(
        &self,
        view_top: usize,
        view_bottom: usize,
    ) -> Option<(usize, usize)> {
        let mut acc = 0;
        let mut first = None;
        let mut last = None;
        for (i, row) in self.rows.iter().enumerate() {
            let start = acc;
            let end = acc + row.height();
            acc = end;
            let visible = start < view_bottom && end > view_top;
            if visible && row.selectable {
                if first.is_none() {
                    first = Some(i);
                }
                last = Some(i);
            }
        }
        first.zip(last)
    }

    /// Global physical-line span of the part of `row` that can actually *show*
    /// the highlight, i.e. from its first to its last `highlight_on_select`
    /// line. `None` for a row that has no such line at all (a pure spacer):
    /// focusing it would leave no visible cursor.
    fn row_highlight_span(&self, row: usize) -> Option<(usize, usize)> {
        let lines = &self.rows.get(row)?.lines;
        let first = lines.iter().position(|l| l.highlight_on_select)?;
        let last = lines.iter().rposition(|l| l.highlight_on_select)?;
        let base = self.row_line_start(row);
        Some((base + first, base + last + 1))
    }

    /// Hand the focus one row onward after a scroll: the **adjacent selectable
    /// row in the direction of travel** takes the focus as soon as any of its
    /// highlightable lines is inside the viewport — the next row down when
    /// scrolling down, the previous row up when scrolling up. The row being
    /// left need not have moved out of view; what matters is that the user can
    /// see the row the cursor moves onto (see the module docs for why).
    ///
    /// At most one row per pan, so `j`/`k` step message by message instead of
    /// jumping to the far edge of the viewport. When the neighbour is still
    /// off-screen the focus stays put and the pan is pure scrolling. A pan big
    /// enough to leave the focus entirely behind (the page keys) is caught by
    /// [`reattach_selection_if_offscreen`](Self::reattach_selection_if_offscreen).
    /// Never moves the viewport.
    fn handoff_after_scroll(&mut self, down: bool, line_budget: usize) {
        if self.rows.is_empty() {
            return;
        }
        let view_top = self.scroll_line_offset();
        let view_bottom = view_top + line_budget;
        let neighbour = if down {
            (self.selected_row + 1..self.rows.len()).find(|&i| self.rows[i].selectable)
        } else {
            (0..self.selected_row)
                .rev()
                .find(|&i| self.rows[i].selectable)
        };
        if let Some(i) = neighbour {
            if let Some((start, end)) = self.row_highlight_span(i) {
                if start < view_bottom && end > view_top {
                    self.selected_row = i;
                }
            }
        }
        self.reattach_selection_if_offscreen(line_budget);
    }

    /// Gentle re-attach for frame rebuilds and data/viewport changes: keep the
    /// selection wherever it is as long as **any** part of its row is visible;
    /// only when the row is *completely* off an edge, hand off to the nearest
    /// partially-visible selectable row. Unlike [`handoff_after_scroll`] this
    /// is direction-agnostic and idempotent, so calling it every frame (via
    /// `restore_selected` / `resync_smooth`) never makes the focus drift on
    /// its own — only an actual scroll moves the focus.
    pub(crate) fn reattach_selection_if_offscreen(&mut self, line_budget: usize) {
        if self.rows.is_empty() {
            return;
        }
        let view_top = self.scroll_line_offset();
        let view_bottom = view_top + line_budget;
        let sel_start = self.row_line_start(self.selected_row);
        let sel_end = sel_start + self.rows[self.selected_row].height();
        // Still (at least partially) visible → keep the selection put.
        if sel_end > view_top && sel_start < view_bottom {
            return;
        }
        if let Some((first, last)) = self.visible_selectable_range(view_top, view_bottom) {
            self.selected_row = if sel_end <= view_top { first } else { last };
        }
    }

    /// Re-clamp the continuous scroll position to the current data set and
    /// viewport, then make sure the selection is still on a valid, visible
    /// row. Called from `view()` and `set_rows` while smooth mode is on so a
    /// changed data set or viewport size never leaves a stale position.
    pub(crate) fn resync_smooth(&mut self, line_budget: usize) {
        let cur = self.scroll_line_offset();
        self.set_scroll_line_offset(cur, line_budget);
        self.clamp_selection_smooth();
        self.reattach_selection_if_offscreen(line_budget);
    }

    /// Clamp `selected_row` to a valid, selectable index after a data change,
    /// without touching the scroll position. Mirrors the discrete
    /// `clamp_selection` but lives here so smooth mode does not depend on the
    /// private discrete helper.
    fn clamp_selection_smooth(&mut self) {
        if self.rows.is_empty() {
            self.selected_row = 0;
            return;
        }
        if self.selected_row >= self.rows.len() {
            self.selected_row = self.rows.len() - 1;
        }
        if !self.rows[self.selected_row].selectable {
            let fwd = (self.selected_row..self.rows.len()).find(|&i| self.rows[i].selectable);
            self.selected_row = fwd
                .or_else(|| {
                    (0..self.selected_row)
                        .rev()
                        .find(|&i| self.rows[i].selectable)
                })
                .unwrap_or(self.selected_row);
        }
    }

    /// Scroll by `delta` physical lines (positive = content moves up, i.e.
    /// toward later rows; negative = toward earlier rows). After a pan that
    /// actually moves the viewport, the focus hands off in the scroll direction
    /// the moment its row is no longer fully visible (see
    /// [`handoff_after_scroll`](Self::handoff_after_scroll)). Returns `true` if
    /// the viewport moved — `false` means the position was already clamped
    /// (everything fits, or at an edge), which the `j`/`k` handlers use to fall
    /// back to a cursor step.
    pub(crate) fn scroll_lines(&mut self, delta: isize) -> bool {
        if self.rows.is_empty() {
            return false;
        }
        let before = self.scroll_line_offset();
        let next = (before as isize).saturating_add(delta).max(0) as usize;
        self.set_scroll_line_offset(next, self.last_line_budget);
        let moved = self.scroll_line_offset() != before;
        if moved {
            self.handoff_after_scroll(delta > 0, self.last_line_budget);
        }
        moved
    }

    /// Move the selection to the next selectable row below the current one and
    /// scroll it minimally into view. Returns `true` if it moved.
    ///
    /// This is the `j` fallback when the viewport cannot scroll any further:
    /// the virtual cursor then walks message-by-message instead of standing
    /// still. It is what makes `j`/`k` move the focus even when the whole list
    /// fits on screen (nothing to scroll) or when sitting at the bottom edge
    /// (so the very last message stays reachable).
    pub(crate) fn step_selection_down(&mut self) -> bool {
        if let Some(next) =
            (self.selected_row + 1..self.rows.len()).find(|&i| self.rows[i].selectable)
        {
            self.selected_row = next;
            self.scroll_selection_into_view();
            true
        } else {
            false
        }
    }

    /// Move the selection to the previous selectable row above the current one
    /// and scroll it minimally into view. The upward counterpart of
    /// [`step_selection_down`](Self::step_selection_down).
    pub(crate) fn step_selection_up(&mut self) -> bool {
        if let Some(prev) = (0..self.selected_row)
            .rev()
            .find(|&i| self.rows[i].selectable)
        {
            self.selected_row = prev;
            self.scroll_selection_into_view();
            true
        } else {
            false
        }
    }

    /// Scroll the *minimum* amount so the currently selected row becomes
    /// fully visible: nudge the top edge down to the row's top if it sits
    /// above the viewport, or the bottom edge up to the row's bottom if it
    /// sits below. A row already inside the viewport is left untouched (no
    /// snap-to-top). Used by programmatic selection (reload restore, jump /
    /// search resolution) in smooth mode.
    pub(crate) fn scroll_selection_into_view(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        let budget = self.last_line_budget;
        let start = self.row_line_start(self.selected_row);
        let end = start + self.rows[self.selected_row].height();
        let view_top = self.scroll_line_offset();
        let view_bottom = view_top + budget;
        if start < view_top {
            self.set_scroll_line_offset(start, budget);
        } else if end > view_bottom {
            self.set_scroll_line_offset(end.saturating_sub(budget), budget);
        }
    }

    /// Scroll to the very top and select the first selectable row.
    pub(crate) fn scroll_to_start(&mut self) {
        self.set_scroll_line_offset(0, self.last_line_budget);
        if let Some(i) = self.first_selectable() {
            self.selected_row = i;
        }
    }

    /// Select `row` and pull it to the **top** edge of the viewport.
    ///
    /// [`set_selected`](Table::set_selected) scrolls the *minimum* amount, so
    /// a target below the fold ends up parked at the **bottom** edge with
    /// everything that follows it off-screen. A "jump to the first unread
    /// row" wants the opposite: the run of unread rows has to read downward
    /// from the cursor, which means anchoring the target at the top.
    ///
    /// Lives here (rather than beside `set_selected`) because it needs the
    /// module-private line arithmetic; the discrete branch uses
    /// [`clamp_selection_smooth`](Self::clamp_selection_smooth) for the same
    /// reason — it clamps identically and, unlike the discrete
    /// `clamp_selection`, never touches the scroll position we just set.
    pub fn set_selected_at_top(&mut self, row: usize) {
        if self.rows.is_empty() {
            return;
        }
        self.selected_row = row;
        self.clamp_selection_smooth();
        if self.smooth_scroll {
            let start = self.row_line_start(self.selected_row);
            self.set_scroll_line_offset(start, self.last_line_budget);
        } else {
            // Discrete mode scrolls whole rows, so the selected row simply
            // becomes the first visible one. Overshoot near the end (blank
            // space below) is the same trade-off every other discrete jump
            // makes — `view()` never pulls the offset back on its own.
            self.scroll_offset = self.selected_row;
        }
    }

    /// Scroll to the bottom (last physical line on the bottom edge) and
    /// select the last selectable row.
    pub(crate) fn scroll_to_end(&mut self) {
        let max = self.max_scroll_line(self.last_line_budget);
        self.set_scroll_line_offset(max, self.last_line_budget);
        if let Some(i) = self.last_selectable() {
            self.selected_row = i;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::component::Table;
    use super::super::{TableWidgetCell, TableWidgetLine, TableWidgetRow};
    use tuirealm::command::{Cmd, Direction};
    use tuirealm::component::Component;

    /// A 3-line chat-style row: meta, body, spacer (spacer line opts out of
    /// the highlight; the row as a whole stays selectable).
    fn chat_row(text: &str) -> TableWidgetRow {
        TableWidgetRow::multiline(vec![
            TableWidgetLine::new(vec![TableWidgetCell::plain(format!("meta {text}"))]),
            TableWidgetLine::new(vec![TableWidgetCell::plain(text)]),
            TableWidgetLine::new(vec![]).with_highlight_on_select(false),
        ])
    }

    /// Build a smooth-scroll table of `n` 3-line rows with a known line
    /// budget, as if a render had already happened.
    fn smooth_table(n: usize, line_budget: usize) -> Table {
        let mut t = Table::default()
            .with_smooth_scroll(true)
            .with_rows((0..n).map(|i| chat_row(&format!("m{i}"))).collect());
        t.set_focused(true);
        t.last_line_budget = line_budget;
        t.last_visible_data_rows = line_budget / 3;
        t
    }

    #[test]
    fn down_scrolls_one_physical_line_not_a_whole_row() {
        // 5 rows × 3 lines = 15 lines; budget 9 lines (3 whole rows).
        let mut t = smooth_table(5, 9);
        assert_eq!(t.scroll_offset, 0);
        assert_eq!(t.scroll_sub_line, 0);
        assert_eq!(t.selected_row(), 0);

        t.scroll_lines(1);
        // Content scrolled one physical line: top row clipped, sub-line 1.
        assert_eq!(t.scroll_offset, 0);
        assert_eq!(t.scroll_sub_line, 1);
        // Row 1 is on screen, so one pan hands the focus onward to it.
        assert_eq!(t.selected_row(), 1);
    }

    #[test]
    fn three_lines_down_advances_one_full_row() {
        let mut t = smooth_table(5, 9);
        for _ in 0..3 {
            t.scroll_lines(1);
        }
        // Scrolled exactly one row's worth of lines.
        assert_eq!(t.scroll_offset, 1);
        assert_eq!(t.scroll_sub_line, 0);
        // One hand-off per pan: rows 1, 2 and 3 are visible in turn, so three
        // presses walk the focus three messages down.
        assert_eq!(t.selected_row(), 3);
    }

    #[test]
    fn the_focus_moves_on_as_soon_as_the_next_row_is_visible() {
        let mut t = smooth_table(5, 9);
        // Rows are 3 lines, the viewport 9 → rows 0,1,2 are on screen from the
        // start. Row 0 is still fully visible after one line of scroll, but
        // waiting for it to leave is exactly what made `j` feel dead in a chat:
        // the focus goes to row 1 because row 1 can be *seen*.
        t.scroll_lines(1);
        assert_eq!(t.selected_row(), 1, "next row on screen → focus moves");
        t.scroll_lines(1);
        assert_eq!(
            t.selected_row(),
            2,
            "one row per pan, never a jump to the edge"
        );
        // View [3,12) now shows rows 1,2,3 — row 3 has come in at the bottom.
        t.scroll_lines(1);
        assert_eq!(t.selected_row(), 3);
    }

    #[test]
    fn a_row_taller_than_the_rest_of_the_view_keeps_the_focus() {
        // One 12-line row followed by normal ones, in a 9-line viewport: while
        // the tall row is being scrolled through, its successor is nowhere on
        // screen, so the pan stays pure scrolling and the focus does not move.
        let mut t = Table::default().with_smooth_scroll(true).with_rows(vec![
            TableWidgetRow::multiline(
                (0..12)
                    .map(|i| {
                        TableWidgetLine::new(vec![TableWidgetCell::plain(format!("tall {i}"))])
                    })
                    .collect(),
            ),
            chat_row("after"),
        ]);
        t.set_focused(true);
        t.last_line_budget = 9;

        t.scroll_lines(1);
        assert_eq!(t.selected_row(), 0, "successor still off-screen");
        t.scroll_lines(1);
        assert_eq!(t.selected_row(), 0);
        // View [3,12): the tall row ends at 12, so row 1 is still out of sight.
        t.scroll_lines(1);
        assert_eq!(t.selected_row(), 0);
        // View [4,13) — the first line of row 1 appears at the bottom edge.
        t.scroll_lines(1);
        assert_eq!(t.selected_row(), 1, "focus follows once the next row shows");
    }

    #[test]
    fn a_bare_spacer_line_does_not_attract_the_focus() {
        // Scrolling up, the first line of the previous row to come back into
        // view is its trailing spacer — which opts out of the highlight. Moving
        // the focus there would leave no visible cursor, so the hand-off waits
        // for a line that can show it.
        let mut t = smooth_table(8, 9);
        t.scroll_lines(9); // view [9,18) → rows 3,4,5 visible, focus re-attached
        assert_eq!(t.selected_row(), 3);
        // View [8,17): line 8 is row 2's spacer, the only part of row 2 in
        // sight. Row 2's highlightable span is [6,8) — still above the edge.
        t.scroll_lines(-1);
        assert_eq!(t.selected_row(), 3, "only the invisible spacer is showing");
        // View [7,16): row 2's body line is on screen now.
        t.scroll_lines(-1);
        assert_eq!(t.selected_row(), 2);
    }

    #[test]
    fn hands_off_even_when_next_row_not_fully_visible() {
        // 3 rows × 3 = 9 lines, but a budget of only 4 lines: no two rows ever
        // fit together and a single 3-line row barely fits. The reported case:
        // the focused row clips while the *next* row is also not fully in view.
        let mut t = smooth_table(3, 4);
        assert_eq!(t.selected_row(), 0);

        // view_top 1 → row 0 [0,3) clipped at top; row 1 [3,6) starts inside
        // the [1,5) viewport but ends at 6 (clipped at the bottom). The focus
        // must still move to row 1 — the trigger is row 0 leaving, not whether
        // row 1 already fits.
        t.scroll_lines(1);
        assert_eq!(t.scroll_offset, 0);
        assert_eq!(t.scroll_sub_line, 1);
        assert_eq!(
            t.selected_row(),
            1,
            "next row focused though it is clipped at the bottom"
        );

        // Scrolling back up: row 1 now clips at the bottom of [0,4) → hand back
        // to the previous row (0), which also is not fully visible here.
        t.scroll_lines(-1);
        assert_eq!(t.selected_row(), 0, "previous row focused on the way up");
    }

    #[test]
    fn scrolling_back_up_hands_off_at_the_bottom() {
        let mut t = smooth_table(6, 9);
        // Deep enough that the selection has handed off a few times.
        // A page-sized pan leaves the focus far behind (row 0 is off-screen and
        // its neighbour too), so the re-attach places it on the first visible
        // row rather than the single-row hand-off.
        t.scroll_lines(9); // view_top = 9 → rows 3,4,5 fully visible
        assert_eq!(t.selected_row(), 3);
        // Scrolling back up hands the focus to the previous row as usual.
        t.scroll_lines(-9);
        assert_eq!(t.scroll_offset, 0);
        assert_eq!(t.scroll_sub_line, 0);
        assert_eq!(t.selected_row(), 2);
    }

    #[test]
    fn cannot_scroll_above_the_top() {
        let mut t = smooth_table(5, 9);
        t.scroll_lines(-5);
        assert_eq!(t.scroll_offset, 0);
        assert_eq!(t.scroll_sub_line, 0);
    }

    #[test]
    fn cannot_scroll_past_the_bottom() {
        // 5 rows × 3 = 15 lines, budget 9 → max scroll offset = 6 lines.
        let mut t = smooth_table(5, 9);
        t.scroll_lines(1000);
        // 6 lines = 2 whole rows → row 2, sub-line 0.
        assert_eq!(t.scroll_offset, 2);
        assert_eq!(t.scroll_sub_line, 0);
    }

    #[test]
    fn home_and_end_select_the_extremes() {
        let mut t = smooth_table(5, 9);
        t.scroll_to_end();
        assert_eq!(t.scroll_offset, 2);
        assert_eq!(t.scroll_sub_line, 0);
        assert_eq!(t.selected_row(), 4, "End selects the last message");
        t.scroll_to_start();
        assert_eq!(t.scroll_offset, 0);
        assert_eq!(t.scroll_sub_line, 0);
        assert_eq!(t.selected_row(), 0, "Home selects the first message");
    }

    #[test]
    fn everything_fits_keeps_top_anchored_and_selection_put() {
        // 2 rows × 3 = 6 lines, budget 20 → nothing to scroll. The low-level
        // `scroll_lines` pan is a no-op and never moves the selection on its
        // own — stepping the cursor is the `j`/`k` handler's job (see
        // `everything_fits_j_steps_cursor_message_by_message`).
        let mut t = smooth_table(2, 20);
        assert!(!t.scroll_lines(5), "nothing to scroll → no movement");
        assert_eq!(t.scroll_offset, 0);
        assert_eq!(t.scroll_sub_line, 0);
        assert_eq!(t.selected_row(), 0);
    }

    #[test]
    fn everything_fits_j_steps_cursor_message_by_message() {
        // 3 rows × 3 = 9 lines, budget 20 → the whole list fits, so j/k can
        // never scroll; the virtual cursor must still walk message by message.
        let mut t = smooth_table(3, 20);
        assert_eq!(t.selected_row(), 0);

        t.perform(Cmd::Move(Direction::Down));
        assert_eq!(t.scroll_offset, 0, "nothing scrolls");
        assert_eq!(t.scroll_sub_line, 0);
        assert_eq!(t.selected_row(), 1, "focus moved to the next message");

        t.perform(Cmd::Move(Direction::Down));
        assert_eq!(t.selected_row(), 2);

        // No message below the last → j is a no-op there.
        t.perform(Cmd::Move(Direction::Down));
        assert_eq!(t.selected_row(), 2, "stays on the last message");

        // k walks the cursor back up, still without scrolling.
        t.perform(Cmd::Move(Direction::Up));
        assert_eq!(t.selected_row(), 1);
        t.perform(Cmd::Move(Direction::Up));
        assert_eq!(t.selected_row(), 0);
        t.perform(Cmd::Move(Direction::Up));
        assert_eq!(t.selected_row(), 0, "stays on the first message");
    }

    #[test]
    fn after_bottoming_out_j_steps_through_the_last_visible_messages() {
        // 5 rows × 3 = 15 lines, budget 9 → max scroll offset = 6 lines.
        let mut t = smooth_table(5, 9);
        // Scroll all the way down; the focus rides along and ends on the last
        // message, which is what the pans kept handing it to.
        for _ in 0..20 {
            t.scroll_lines(1);
        }
        assert_eq!(t.scroll_offset, 2);
        assert_eq!(
            t.selected_row(),
            4,
            "focus rode the scroll to the last message"
        );

        // Put the cursor back on an earlier — still visible — message, the way
        // a jump or a search hit would. View is [6,15), so rows 2,3,4 show.
        t.set_selected(2);
        assert_eq!(t.scroll_offset, 2, "target already visible → no re-scroll");

        // Scroll is maxed, so j cannot pan further; the cursor must step onto
        // the still-visible rows so the very last message stays reachable.
        t.perform(Cmd::Move(Direction::Down));
        assert_eq!(t.scroll_offset, 2, "no further scroll");
        assert_eq!(t.selected_row(), 3);
        t.perform(Cmd::Move(Direction::Down));
        assert_eq!(t.selected_row(), 4, "last message reachable");
        t.perform(Cmd::Move(Direction::Down));
        assert_eq!(t.selected_row(), 4, "stays on the last message");
    }

    #[test]
    fn set_selected_scrolls_target_minimally_into_view() {
        let mut t = smooth_table(8, 9);
        // Row 5 spans [15,18); bottom edge into view → view_top = 18-9 = 9.
        t.set_selected(5);
        assert_eq!(t.selected_row(), 5);
        assert_eq!(t.scroll_offset, 3);
        assert_eq!(t.scroll_sub_line, 0);
        // A row already inside the viewport must NOT cause a re-scroll
        // (no snap-to-top). Row 4 spans [12,15) — fully visible already.
        let before = (t.scroll_offset, t.scroll_sub_line);
        t.set_selected(4);
        assert_eq!(t.selected_row(), 4);
        assert_eq!((t.scroll_offset, t.scroll_sub_line), before);
    }

    #[test]
    fn set_selected_at_top_anchors_the_target_at_the_top_edge() {
        let mut t = smooth_table(8, 9);
        // Row 5 spans [15,18). `set_selected` would park it at the bottom
        // edge (view_top 9, see the test above); the top-anchored placement
        // must instead start the viewport at the row itself, so rows 6 and 7
        // — the rest of the unread run — are what fills the screen below it.
        t.set_selected_at_top(5);
        assert_eq!(t.selected_row(), 5);
        assert_eq!(t.scroll_offset, 5);
        assert_eq!(t.scroll_sub_line, 0);

        // Near the end the position clamps: 8 rows × 3 = 24 lines, budget 9 →
        // max scroll = 15 lines = row 5. Asking for the last row therefore
        // leaves it on-screen instead of scrolling into empty space.
        t.set_selected_at_top(7);
        assert_eq!(t.selected_row(), 7);
        assert_eq!(t.scroll_offset, 5);
        assert_eq!(t.scroll_sub_line, 0);
    }

    #[test]
    fn rebuild_restore_preserves_line_scroll_but_explicit_select_scrolls_in() {
        let mut t = smooth_table(5, 9);
        t.scroll_lines(1); // sub-line 1; early hand-off moves focus to row 1
        assert_eq!(t.scroll_sub_line, 1);
        assert_eq!(t.selected_row(), 1);

        // Per-frame rebuild restores the current selection (row 1, fully
        // visible). It must NOT re-scroll — that was the "j/k does nothing"
        // bug, where set_data pulled the row back every frame and undid the
        // one-line scroll.
        t.restore_selected(1);
        assert_eq!(t.scroll_offset, 0);
        assert_eq!(t.scroll_sub_line, 1, "scroll preserved across rebuild");
        assert_eq!(t.selected_row(), 1);

        // Contrast: an *explicit* selection of the clipped row 0 (jump /
        // search) does scroll it fully back into view.
        t.set_selected(0);
        assert_eq!(t.scroll_sub_line, 0, "explicit select scrolls into view");
        assert_eq!(t.selected_row(), 0);
    }

    #[test]
    fn resync_after_shrink_clamps_position_and_selection() {
        let mut t = smooth_table(8, 9);
        t.scroll_to_end(); // deep scroll, selection on the last row
        assert!(t.scroll_offset > 0);
        // Data shrinks to 2 rows → previous offset + selection out of range.
        t.set_rows((0..2).map(|i| chat_row(&format!("s{i}"))).collect());
        // 2 rows × 3 = 6 lines ≤ budget 9 → clamped back to the top.
        assert_eq!(t.scroll_offset, 0);
        assert_eq!(t.scroll_sub_line, 0);
        assert_eq!(t.selected_row(), 1, "selection clamped to last valid row");
    }
}
