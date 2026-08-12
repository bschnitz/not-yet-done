mod component;
mod fuzzy;
mod keymap;
mod render;
mod state;
pub mod style;

pub use keymap::LeaderListKeymap;
pub use state::LeaderListEvent;
pub use style::{LeaderListStyle, LeaderListStyleType};

pub(crate) use render::display_width;

/// How wide each rendered line should be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaderWidth {
    /// Use the full width of the area handed to `view` (default).
    Fill,
    /// Use the widget's own [`LeaderList::min_width`] — the tightest layout in
    /// which every `left + right` still fits without truncation.
    Min,
    /// Use an explicit column count. Should be `>= min_width()`; a smaller
    /// value truncates the longest lines. Clamped to the area width.
    Fixed(u16),
}

impl Default for LeaderWidth {
    fn default() -> Self {
        LeaderWidth::Fill
    }
}

/// A pair rendered on one line: `left = a + post`, `right = pre + b`.
#[derive(Debug, Clone)]
pub struct LeaderEntry {
    /// The `a` element (left side).
    pub a: String,
    /// The `b` element (right side).
    pub b: String,
}

/// A list of two-column lines joined by a repeating filler ("leader"), e.g. a
/// table of contents (`Chapter One …………… 12`).
///
/// Each entry maps an `a` to a `b`. A line is laid out as
/// `a + post + n·f + pre + b`, where `post` is a fixed postfix appended to
/// every `a`, `pre` a fixed prefix prepended to every `b`, and `f` the filler
/// repeated `n` times so the line reaches the target width. `b` is thus flush
/// to the right edge and the fillers align into vertical columns.
///
/// The widget can compute its own minimal width via [`min_width`]:
/// `max(width(a + post + pre + b))` over all entries — the width at which the
/// longest line has zero filler. Render at that width with
/// [`LeaderWidth::Min`], at the area width with [`LeaderWidth::Fill`], or at any
/// `b >= min_width()` with [`LeaderWidth::Fixed`].
///
/// By default the widget is a passive display. Enable [`with_selectable`] to get
/// a movable cursor and `Enter`/`Esc` events for the table-of-contents use case
/// (jump to the selected entry).
///
/// ```rust
/// use not_yet_done_ratatui::LeaderList;
///
/// let toc = LeaderList::default()
///     .with_entries(vec![
///         ("Introduction", "1"),
///         ("Getting Started", "7"),
///         ("Advanced Topics", "42"),
///     ])
///     .with_affixes("", "  ", " ."); // post, filler, pre
/// let min = toc.min_width();
/// ```
///
/// [`min_width`]: LeaderList::min_width
/// [`with_selectable`]: LeaderList::with_selectable
pub struct LeaderList {
    // --- framework state ---
    pub(crate) focused: bool,

    // --- interactive state ---
    pub(crate) cursor: usize,
    pub(crate) scroll_offset: usize,
    /// Number of entry rows shown in the last `view` — used by page up/down,
    /// which run without knowing the render area.
    pub(crate) page_rows: usize,

    // --- data ---
    pub(crate) entries: Vec<LeaderEntry>,
    /// Indices into `entries` that are currently visible, in display order.
    /// Identical to `0..entries.len()` unless a fuzzy filter is active, in
    /// which case it holds only the matching entries, best match first. All
    /// cursor/scroll/render arithmetic runs over this list, not `entries`.
    pub(crate) matches: Vec<usize>,
    /// Entry indices that are "marked" (a multi-select). Marked entries render
    /// with the [`marker`](Self::marker) glyph appended and stay visible even
    /// when a fuzzy filter would otherwise hide them. Empty by default.
    pub(crate) marked: std::collections::BTreeSet<usize>,
    /// Glyph appended to marked entries' right column (e.g. `*`). Empty (the
    /// default) disables marking entirely — no glyph, no reserved width.
    pub(crate) marker: String,

    // --- fuzzy search ---
    /// Whether the optional fuzzy filter (typed against the left `a` column)
    /// is enabled. Off by default.
    pub(crate) search_enabled: bool,
    /// The current filter query (matched case-insensitively against `a`).
    pub(crate) query: String,
    /// Hint shown dimmed on the empty search prompt (e.g. how to search keys).
    /// Empty by default; disappears as soon as the user types.
    pub(crate) search_placeholder: String,

    // --- configuration (set once at construction) ---
    pub(crate) title: String,
    pub(crate) post: String,
    pub(crate) pre: String,
    pub(crate) filler: String,
    pub(crate) width: LeaderWidth,
    pub(crate) selectable: bool,
    /// Max number of entry rows to show. `None` uses the full area height.
    /// When there are more entries than visible rows, the list scrolls.
    pub(crate) max_rows: Option<u16>,
    pub(crate) status_line: bool,
    pub(crate) style: LeaderListStyle,
    pub(crate) keymap: LeaderListKeymap,
}

impl Default for LeaderList {
    fn default() -> Self {
        Self {
            focused: false,
            cursor: 0,
            scroll_offset: 0,
            page_rows: 0,
            entries: Vec::new(),
            matches: Vec::new(),
            marked: std::collections::BTreeSet::new(),
            marker: String::new(),
            search_enabled: false,
            query: String::new(),
            search_placeholder: String::new(),
            title: String::new(),
            post: String::new(),
            pre: String::new(),
            filler: " ".into(),
            width: LeaderWidth::default(),
            selectable: false,
            max_rows: None,
            status_line: false,
            style: LeaderListStyle::default(),
            keymap: LeaderListKeymap::default(),
        }
    }
}

impl LeaderList {
    /// Sets the entries from `(a, b)` pairs.
    pub fn with_entries(mut self, pairs: Vec<(impl Into<String>, impl Into<String>)>) -> Self {
        self.entries = pairs
            .into_iter()
            .map(|(a, b)| LeaderEntry {
                a: a.into(),
                b: b.into(),
            })
            .collect();
        self.recompute_matches();
        self.clamp_cursor();
        self
    }

    /// Replaces the entries in place, resetting the cursor, scroll and filter.
    pub fn set_entries(&mut self, pairs: Vec<(String, String)>) {
        self.entries = pairs
            .into_iter()
            .map(|(a, b)| LeaderEntry { a, b })
            .collect();
        self.cursor = 0;
        self.scroll_offset = 0;
        self.query.clear();
        self.recompute_matches();
    }

    /// Sets an optional title/header line rendered above the entries.
    ///
    /// An empty title (the default) means no header line is drawn, and the
    /// entries occupy the full area. The title is styled via
    /// [`LeaderListStyleType::Title`] and counted into [`min_width`].
    ///
    /// [`LeaderListStyleType::Title`]: style::LeaderListStyleType::Title
    /// [`min_width`]: LeaderList::min_width
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Sets the postfix (appended to every `a`), the filler (repeated between
    /// the two segments), and the prefix (prepended to every `b`).
    pub fn with_affixes(
        mut self,
        post: impl Into<String>,
        filler: impl Into<String>,
        pre: impl Into<String>,
    ) -> Self {
        self.post = post.into();
        self.filler = filler.into();
        self.pre = pre.into();
        self
    }

    /// Sets only the filler string (repeated to pad each line to width).
    pub fn with_filler(mut self, filler: impl Into<String>) -> Self {
        self.filler = filler.into();
        self
    }

    /// Chooses the rendered line width (default [`LeaderWidth::Fill`]).
    pub fn with_width(mut self, width: LeaderWidth) -> Self {
        self.width = width;
        self
    }

    /// Enables cursor navigation and `Enter`/`Esc` events (default `false`).
    pub fn with_selectable(mut self, selectable: bool) -> Self {
        self.selectable = selectable;
        self
    }

    /// Enables an incremental fuzzy filter over the left (`a`) column
    /// (default `false`). While on, printable keys typed into the widget
    /// build a query string, the list shows only matching entries (best match
    /// first), and a prompt row is drawn at the top. `Backspace` edits the
    /// query. Navigation stays on the arrow keys / `Ctrl-j`/`Ctrl-k`, so the
    /// plain letter keys are free for typing.
    pub fn with_search(mut self, enabled: bool) -> Self {
        self.search_enabled = enabled;
        self.recompute_matches();
        self
    }

    /// Sets the hint shown dimmed on the empty search prompt (e.g.
    /// `Type ". " to search by keybinding`). Only visible while the query is
    /// empty; typing anything replaces it with the live query.
    pub fn with_search_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.search_placeholder = placeholder.into();
        self
    }

    /// Caps the number of entry rows shown at `max` (excluding the optional
    /// title and status lines). When there are more entries than visible rows
    /// the list becomes scrollable: page with the `page_up`/`page_down` keys,
    /// or move the cursor (in [`with_selectable`] mode) to scroll it along.
    ///
    /// `None` (the default) uses the full height of the render area.
    ///
    /// [`with_selectable`]: LeaderList::with_selectable
    pub fn with_max_rows(mut self, max: u16) -> Self {
        self.max_rows = Some(max);
        self
    }

    /// Shows a status line below the entries (`N entries · Page x/y`), styled
    /// via [`LeaderListStyleType::Status`]. Default `false`.
    ///
    /// [`LeaderListStyleType::Status`]: style::LeaderListStyleType::Status
    pub fn with_status_line(mut self, show: bool) -> Self {
        self.status_line = show;
        self
    }

    /// Sets the visual style.
    pub fn with_style(mut self, style: LeaderListStyle) -> Self {
        self.style = style;
        self
    }

    /// Sets the keymap (only relevant when [`with_selectable`] is on).
    ///
    /// [`with_selectable`]: LeaderList::with_selectable
    pub fn with_keymap(mut self, keymap: LeaderListKeymap) -> Self {
        self.keymap = keymap;
        self
    }

    /// Sets the glyph appended to marked entries (e.g. `*`). An empty glyph
    /// (the default) disables marking. Enabling a marker reserves its width in
    /// [`min_width`](Self::min_width) so marked and unmarked rows stay aligned.
    pub fn with_marker(mut self, marker: impl Into<String>) -> Self {
        self.marker = marker.into();
        self
    }

    /// Replaces the marked set with `indices` (entry indices), re-pinning the
    /// visible list so marked rows survive an active filter.
    pub fn set_marked(&mut self, indices: impl IntoIterator<Item = usize>) {
        let n = self.entries.len();
        self.marked = indices.into_iter().filter(|&i| i < n).collect();
        self.recompute_matches();
    }

    /// Toggles the mark on entry `idx`. Re-pins the visible list.
    pub fn toggle_marked(&mut self, idx: usize) {
        if idx >= self.entries.len() {
            return;
        }
        if !self.marked.insert(idx) {
            self.marked.remove(&idx);
        }
        self.recompute_matches();
    }

    /// Marks every entry currently visible (the filtered set). Re-pins so the
    /// newly-marked rows stay visible if the filter later narrows.
    pub fn mark_visible(&mut self) {
        for &i in &self.matches.clone() {
            self.marked.insert(i);
        }
        self.recompute_matches();
    }

    /// Clears all marks.
    pub fn clear_marked(&mut self) {
        if self.marked.is_empty() {
            return;
        }
        self.marked.clear();
        self.recompute_matches();
    }

    /// The marked entry indices, ascending.
    pub fn marked(&self) -> Vec<usize> {
        self.marked.iter().copied().collect()
    }

    /// Whether any entry is marked.
    pub fn has_marks(&self) -> bool {
        !self.marked.is_empty()
    }

    /// The entry indices currently visible (the filtered set), in display
    /// order — the same list the cursor and rendering run over.
    pub fn visible_indices(&self) -> Vec<usize> {
        self.matches.clone()
    }

    /// The minimal width in which no line is truncated: the maximum of the
    /// title width and `max(width(a + post + pre + b))` over all entries.
    /// `0` when there are no entries and no title.
    pub fn min_width(&self) -> u16 {
        // A configured marker reserves its width on every entry so marked and
        // unmarked rows keep the same right edge (see `render`).
        let marker_w = display_width(&self.marker);
        self.entries
            .iter()
            .map(|e| {
                display_width(&e.a)
                    + display_width(&self.post)
                    + display_width(&self.pre)
                    + display_width(&e.b)
                    + marker_w
            })
            .chain(std::iter::once(display_width(&self.title)))
            .max()
            .unwrap_or(0)
            .min(u16::MAX as usize) as u16
    }

    /// The cursor position within the currently visible (filtered) list.
    pub fn selected(&self) -> usize {
        self.cursor
    }

    /// The index into `entries` of the currently selected row, mapping through
    /// the active fuzzy filter. `None` when the visible list is empty.
    pub fn selected_index(&self) -> Option<usize> {
        self.matches.get(self.cursor).copied()
    }

    /// The current fuzzy-filter query (empty when nothing is typed).
    pub fn search_query(&self) -> &str {
        &self.query
    }

    /// `true` when the fuzzy filter is enabled *and* narrowing the list.
    pub fn search_active(&self) -> bool {
        self.search_enabled && !self.query.is_empty()
    }

    /// Appends a character to the fuzzy query and re-filters (no-op unless
    /// search is enabled).
    pub fn push_search(&mut self, c: char) {
        if !self.search_enabled {
            return;
        }
        self.query.push(c);
        self.reset_after_query_change();
    }

    /// Removes the last character from the fuzzy query and re-filters.
    pub fn backspace_search(&mut self) {
        if !self.search_enabled {
            return;
        }
        self.query.pop();
        self.reset_after_query_change();
    }

    /// Clears the fuzzy query, restoring the full list.
    pub fn clear_search(&mut self) {
        if self.query.is_empty() {
            return;
        }
        self.query.clear();
        self.reset_after_query_change();
    }

    /// Replaces the fuzzy query wholesale and re-filters. Used to carry a
    /// live filter across a list rebuild (no-op unless search is enabled).
    pub fn set_search_query(&mut self, query: impl Into<String>) {
        if !self.search_enabled {
            return;
        }
        self.query = query.into();
        self.reset_after_query_change();
    }

    // --- internal helpers ---

    fn reset_after_query_change(&mut self) {
        self.cursor = 0;
        self.scroll_offset = 0;
        self.recompute_matches();
    }

    /// Rebuilds [`matches`](Self::matches) from `entries` and the current query.
    /// Without an active filter this is the identity `0..entries.len()`.
    pub(crate) fn recompute_matches(&mut self) {
        let n = self.entries.len();
        if !self.search_enabled || self.query.is_empty() {
            self.matches = (0..n).collect();
        } else {
            // A leading `.` switches the filter from the label (left column) to
            // the keys (right column) — e.g. `. ctrl+k` finds every row bound
            // to `ctrl+k`. The dot and any following space are stripped; the
            // rest is the needle (empty → all rows in key mode).
            let (in_keys, needle) = match self.query.strip_prefix('.') {
                Some(rest) => (true, rest.trim_start()),
                None => (false, self.query.as_str()),
            };
            let matcher = fuzzy_matcher::skim::SkimMatcherV2::default();
            let mut scored: Vec<(i32, usize)> = self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(i, e)| {
                    let haystack = if in_keys { &e.b } else { &e.a };
                    fuzzy::fuzzy_score(&matcher, haystack, needle).map(|s| (s, i))
                })
                .collect();
            // Best score first, stable by original order on ties.
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            self.matches = scored.into_iter().map(|(_, i)| i).collect();
            // Marked entries stay visible even when they don't match the
            // filter: append any that the fuzzy pass dropped, in entry order.
            for &i in &self.marked {
                if !self.matches.contains(&i) {
                    self.matches.push(i);
                }
            }
        }
        self.clamp_cursor();
    }

    pub(crate) fn clamp_cursor(&mut self) {
        if self.cursor >= self.matches.len() {
            self.cursor = self.matches.len().saturating_sub(1);
        }
    }

    pub(crate) fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        }
    }

    pub(crate) fn move_down(&mut self) {
        if self.cursor + 1 < self.matches.len() {
            self.cursor += 1;
        }
    }

    /// Effective page size for page up/down: the entry-row count from the last
    /// `view`, falling back to `max_rows` (or 1) before the first render.
    pub(crate) fn page_size(&self) -> usize {
        if self.page_rows > 0 {
            self.page_rows
        } else {
            self.max_rows.map(|m| m as usize).unwrap_or(1).max(1)
        }
    }

    /// The largest valid `scroll_offset` given the current page size.
    pub(crate) fn max_scroll_offset(&self) -> usize {
        self.matches.len().saturating_sub(self.page_size())
    }

    /// `true` when there are more visible entries than fit in the visible rows.
    pub fn is_scrollable(&self) -> bool {
        self.matches.len() > self.page_size()
    }

    /// Moves one page down (`down = true`) or up. In selectable mode this jumps
    /// the cursor by a page; otherwise it scrolls the window directly. Returns
    /// `true` if the position changed.
    pub(crate) fn page_move(&mut self, down: bool) -> bool {
        let page = self.page_size();
        if self.selectable {
            let before = self.cursor;
            if down {
                self.cursor = (self.cursor + page).min(self.matches.len().saturating_sub(1));
            } else {
                self.cursor = self.cursor.saturating_sub(page);
            }
            self.cursor != before
        } else {
            let before = self.scroll_offset;
            if down {
                self.scroll_offset = (self.scroll_offset + page).min(self.max_scroll_offset());
            } else {
                self.scroll_offset = self.scroll_offset.saturating_sub(page);
            }
            self.scroll_offset != before
        }
    }
}

#[cfg(test)]
mod marking_tests {
    use super::*;

    fn list() -> LeaderList {
        LeaderList::default()
            .with_entries(vec![("Alpha", "a"), ("Beta", "b"), ("Gamma", "g")])
            .with_search(true)
            .with_marker("*")
    }

    #[test]
    fn toggle_marks_and_unmarks() {
        let mut l = list();
        assert!(!l.has_marks());
        l.toggle_marked(1);
        assert_eq!(l.marked(), vec![1]);
        assert!(l.has_marks());
        l.toggle_marked(1);
        assert!(l.marked().is_empty());
        assert!(!l.has_marks());
    }

    #[test]
    fn toggle_out_of_range_is_noop() {
        let mut l = list();
        l.toggle_marked(99);
        assert!(l.marked().is_empty());
    }

    #[test]
    fn set_marked_filters_out_of_range_indices() {
        let mut l = list();
        l.set_marked([0, 2, 99]);
        assert_eq!(l.marked(), vec![0, 2]);
    }

    #[test]
    fn mark_visible_marks_the_filtered_set_only() {
        let mut l = LeaderList::default()
            .with_entries(vec![("Open", "o"), ("Close", "c"), ("Reopen", "r")])
            .with_search(true)
            .with_marker("*");
        // "open" matches Open (0) and Reopen (2) but not Close (1).
        for ch in "open".chars() {
            l.push_search(ch);
        }
        let visible = l.visible_indices();
        assert!(visible.contains(&0) && visible.contains(&2));
        assert!(!visible.contains(&1), "Close should be filtered out");
        l.mark_visible();
        assert_eq!(l.marked(), vec![0, 2], "only visible rows get marked");
    }

    #[test]
    fn clear_marked_removes_all() {
        let mut l = list();
        l.set_marked([0, 1]);
        l.clear_marked();
        assert!(l.marked().is_empty());
    }

    #[test]
    fn marked_entry_stays_visible_under_filter() {
        let mut l = list();
        // Beta (1) does not match the query "g", but marking pins it.
        l.toggle_marked(1);
        l.push_search('g');
        assert!(
            !l.visible_indices().is_empty() && l.visible_indices().contains(&1),
            "a marked row survives a filter it would not otherwise match"
        );
    }

    #[test]
    fn marker_reserves_width_for_alignment() {
        // The marker glyph is reserved on every row so marked and unmarked
        // rows keep the same right edge.
        let unmarked = LeaderList::default().with_entries(vec![("Alpha", "a")]);
        let marked = LeaderList::default()
            .with_entries(vec![("Alpha", "a")])
            .with_marker("*");
        assert_eq!(
            marked.min_width(),
            unmarked.min_width() + 1,
            "a one-column marker widens every row by one"
        );
    }
}
