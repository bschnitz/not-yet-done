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
//! ## Selection (early hand-off in the scroll direction)
//!
//! The selection (the highlight, and the row that node-actions operate on) is
//! attached to a *specific row*, not to a screen position. Scrolling only pans
//! the viewport; the highlighted row stays put while it is **fully** visible.
//! The single trigger for a hand-off is "the focused row is no longer fully
//! visible": as soon as a pan clips even one of its physical lines at an edge,
//! the focus moves to the **adjacent selectable row in the scroll direction** —
//! the next row down when scrolling down, the previous row up when scrolling
//! up. The new row need **not** itself be fully visible: if a tall neighbour
//! still runs off the far edge, it is focused anyway (it becomes fully visible
//! as you keep scrolling). What matters is only whether the *current* focus has
//! started to leave the view, never whether the next one already fits. (When a
//! single row is taller than the whole viewport so nothing begins/ends inside
//! it, the focus stays put, so it is never lost.)
//!
//! Frame rebuilds and data/viewport changes use a gentler, direction-agnostic
//! re-attach ([`reattach_selection_if_offscreen`](Table::reattach_selection_if_offscreen))
//! that only moves the focus when its row is *completely* off-screen — so
//! re-selecting the same row every frame never makes the focus drift on its
//! own; only an actual scroll hands it off.
//!
//! `Home`/`End` are explicit jumps and place the selection on the first / last
//! selectable row; programmatic selection ([`Table::set_selected`]) scrolls
//! the target *minimally* into view rather than forcing it to the top.
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
        (0..self.rows.len()).rev().find(|&i| self.rows[i].selectable)
    }

    /// The topmost and bottommost *selectable* rows that are at least
    /// partially inside the viewport `[view_top, view_bottom)` (both in
    /// global physical-line coordinates). `None` if no selectable row is
    /// visible (e.g. a zero-height viewport).
    fn visible_selectable_range(&self, view_top: usize, view_bottom: usize) -> Option<(usize, usize)> {
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

    /// Early hand-off after a scroll, in the direction of travel: keep the
    /// focus on its row only while that row is **fully** visible; the moment a
    /// pan clips even one of its physical lines at an edge, hand the focus to
    /// the **adjacent selectable row in the scroll direction** — the next row
    /// that is no longer clipped at the *top* when scrolling down, the previous
    /// row no longer clipped at the *bottom* when scrolling up.
    ///
    /// The trigger is solely "the current selection is no longer fully
    /// visible"; the target need **not** itself be fully visible (a tall
    /// neighbour may still run off the far edge). This is what keeps `j`/`k`
    /// moving even when neither the old nor the new row fits in full. If no
    /// row begins/ends inside the viewport (a single row taller than the whole
    /// viewport), the focus stays put so it is never lost. Never moves the
    /// viewport.
    fn handoff_after_scroll(&mut self, down: bool, line_budget: usize) {
        if self.rows.is_empty() {
            return;
        }
        let view_top = self.scroll_line_offset();
        let view_bottom = view_top + line_budget;
        let sel_start = self.row_line_start(self.selected_row);
        let sel_end = sel_start + self.rows[self.selected_row].height();
        // Current focus still fully visible → nothing to hand off.
        if sel_start >= view_top && sel_end <= view_bottom {
            return;
        }
        let mut acc = 0;
        let mut target = None;
        for (i, row) in self.rows.iter().enumerate() {
            let start = acc;
            let end = acc + row.height();
            acc = end;
            if !row.selectable {
                continue;
            }
            if down {
                // First selectable row that begins inside the viewport (i.e.
                // is not clipped at the top) — the next message to focus.
                if start >= view_top && start < view_bottom {
                    target = Some(i);
                    break;
                }
            } else if end <= view_bottom && end > view_top {
                // Bottommost selectable row that ends inside the viewport
                // (not clipped at the bottom) — keep scanning for the last.
                target = Some(i);
            }
        }
        if let Some(i) = target {
            self.selected_row = i;
        }
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
                .or_else(|| (0..self.selected_row).rev().find(|&i| self.rows[i].selectable))
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
        if let Some(prev) = (0..self.selected_row).rev().find(|&i| self.rows[i].selectable) {
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
        // Early hand-off: row 0 is now clipped at the top, so the focus moves
        // to the next fully-visible message (row 1) — it is never shown cut.
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
        // Focus handed off to row 1 on the first line (row 0 clipped) and
        // stays there: row 1 spans [3,6) and is fully visible the whole way.
        assert_eq!(t.selected_row(), 1);
    }

    #[test]
    fn selection_hands_off_as_soon_as_its_row_is_clipped() {
        let mut t = smooth_table(5, 9);
        // Row 0 spans lines [0,3). A single line of scroll clips its top, so
        // the focus hands off immediately to the topmost fully-visible row.
        t.scroll_lines(1);
        assert_eq!(t.selected_row(), 1, "row 0 clipped at top → next full row");
        // Row 1 spans [3,6); it stays fully visible while view_top is 1..3,
        // so the focus sticks to it across those pans.
        t.scroll_lines(1);
        assert_eq!(t.selected_row(), 1, "row 1 still fully visible");
        t.scroll_lines(1);
        assert_eq!(t.selected_row(), 1, "row 1 still fully visible at view_top 3");
        // One more line clips row 1's top → hand off to row 2.
        t.scroll_lines(1);
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
        assert_eq!(t.selected_row(), 1, "next row focused though it is clipped at the bottom");

        // Scrolling back up: row 1 now clips at the bottom of [0,4) → hand back
        // to the previous row (0), which also is not fully visible here.
        t.scroll_lines(-1);
        assert_eq!(t.selected_row(), 0, "previous row focused on the way up");
    }

    #[test]
    fn scrolling_back_up_hands_off_at_the_bottom() {
        let mut t = smooth_table(6, 9);
        // Deep enough that the selection has handed off a few times.
        t.scroll_lines(9); // view_top = 9 → rows 3,4,5 fully visible; selection = 3
        assert_eq!(t.selected_row(), 3);
        // Scroll back to the top: row 3 spans [9,12), now clipped at the
        // bottom of the [0,9) viewport → hand off to the bottommost
        // fully-visible row (2).
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
        // Scroll all the way down; selection hands off to the topmost visible
        // row (2), leaving rows 3 and 4 visible but not yet selected.
        for _ in 0..20 {
            t.scroll_lines(1);
        }
        assert_eq!(t.scroll_offset, 2);
        assert_eq!(t.selected_row(), 2, "topmost visible after bottoming out");

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
