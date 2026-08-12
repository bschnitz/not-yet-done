use ratatui::{buffer::Buffer, layout::Rect, style::Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{
    LeaderEntry,
    style::{LeaderListStyle, LeaderListStyleType},
};

/// Display width of `s` in terminal columns (CJK wide chars count as 2).
pub(crate) fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Builds a filler run of exactly `gap` display columns by repeating `filler`.
///
/// The last repetition is padded with spaces if a wide filler glyph would not
/// fit the remaining columns exactly, so the returned string is always `gap`
/// columns wide. Zero-width glyphs in `filler` are ignored; an all-zero-width
/// (or empty) filler degrades to spaces.
pub(crate) fn build_filler(filler: &str, gap: usize) -> String {
    if gap == 0 {
        return String::new();
    }
    let mut glyphs: Vec<char> = filler
        .chars()
        .filter(|c| c.width().unwrap_or(0) > 0)
        .collect();
    if glyphs.is_empty() {
        glyphs.push(' ');
    }

    let mut out = String::with_capacity(gap);
    let mut w = 0usize;
    let mut i = 0usize;
    while w < gap {
        let ch = glyphs[i % glyphs.len()];
        let cw = ch.width().unwrap_or(0);
        if w + cw > gap {
            // A wide glyph does not fit the last column(s); pad with spaces.
            out.push(' ');
            w += 1;
        } else {
            out.push(ch);
            w += cw;
        }
        i += 1;
    }
    out
}

/// All data required to render one frame of a [`super::LeaderList`].
pub(super) struct RenderData<'a> {
    pub entries: &'a [LeaderEntry],
    /// Indices into `entries` that are visible, in display order (the fuzzy
    /// filter's result, or `0..entries.len()` when no filter is active).
    pub visible: &'a [usize],
    /// Entry indices that are marked (rendered with `marker` appended).
    pub marked: &'a std::collections::BTreeSet<usize>,
    /// Glyph appended to marked entries' right column; empty disables marking.
    pub marker: &'a str,
    /// Optional header line; empty means no title is drawn.
    pub title: &'a str,
    pub post: &'a str,
    pub pre: &'a str,
    pub filler: &'a str,
    pub cursor: usize,
    pub scroll_offset: usize,
    pub selectable: bool,
    pub focused: bool,
    pub style: &'a LeaderListStyle,
    /// Effective line width in columns (already clamped to the area width).
    pub line_width: u16,
    /// Number of entry rows to draw (the visible window height).
    pub entry_rows: usize,
    /// Whether to draw the status line below the entries.
    pub show_status: bool,
    /// Whether the fuzzy-search prompt row is drawn above the entries.
    pub show_search: bool,
    /// The current fuzzy query (rendered on the prompt row).
    pub query: &'a str,
    /// Hint shown dimmed on the prompt row while `query` is empty.
    pub search_placeholder: &'a str,
}

/// Renders the leader list into `frame` at `area`.
///
/// One entry per row starting at `scroll_offset`. Each line is laid out as
/// `left + filler + right`, right-aligned so `right` ends at `line_width`.
pub(super) fn render(buf: &mut Buffer, area: Rect, data: &RenderData<'_>) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let line_width = data.line_width.min(area.width);

    // Optional fuzzy-search prompt on the very first row.
    let search_rows: u16 = if data.show_search { 1 } else { 0 };
    if search_rows == 1 {
        let search_style = data.style.resolved_style(LeaderListStyleType::Search);
        if let Some(bg) = search_style.bg {
            for dx in 0..line_width {
                if let Some(cell) = buf.cell_mut((area.x + dx, area.y)) {
                    cell.set_char(' ');
                    cell.set_style(Style::default().bg(bg));
                }
            }
        }
        let prompt = format!("/{}", data.query);
        put_str(
            buf,
            area.x,
            area.y,
            area.x + line_width,
            &prompt,
            search_style,
        );
        // On an empty query, trail a dimmed hint after the `/` cursor telling
        // the user how to search by keybinding.
        if data.query.is_empty() && !data.search_placeholder.is_empty() {
            let hint_style = data.style.resolved_style(LeaderListStyleType::Filler);
            put_str(
                buf,
                area.x + 1,
                area.y,
                area.x + line_width,
                data.search_placeholder,
                hint_style,
            );
        }
    }

    // Optional title line, below the search prompt; entries follow it.
    let title_rows: u16 = if data.title.is_empty() { 0 } else { 1 };
    if title_rows == 1 {
        let title_style = data.style.resolved_style(LeaderListStyleType::Title);
        let ty = area.y + search_rows;
        if let Some(bg) = title_style.bg {
            for dx in 0..line_width {
                if let Some(cell) = buf.cell_mut((area.x + dx, ty)) {
                    cell.set_char(' ');
                    cell.set_style(Style::default().bg(bg));
                }
            }
        }
        put_str(
            buf,
            area.x,
            ty,
            area.x + line_width,
            data.title,
            title_style,
        );
    }

    // The rows above the entries (search + title) and below (status).
    let head_rows = search_rows + title_rows;
    // How many entry rows actually fit between the header rows and the status
    // line (if shown).
    let status_rows: u16 = if data.show_status { 1 } else { 0 };
    let avail = area.height.saturating_sub(head_rows + status_rows) as usize;
    let rows = data.entry_rows.min(avail);

    for row in 0..rows {
        let vis_idx = data.scroll_offset + row;
        let Some(&entry_idx) = data.visible.get(vis_idx) else {
            break;
        };
        let Some(entry) = data.entries.get(entry_idx) else {
            break;
        };
        let y = area.y + head_rows + row as u16;

        let is_cursor = data.selectable && data.focused && vis_idx == data.cursor;
        let cursor_overlay = if is_cursor {
            Some(data.style.resolved_style(LeaderListStyleType::Cursor))
        } else {
            None
        };
        let patch = |base: Style| match cursor_overlay {
            Some(c) => base.patch(c),
            None => base,
        };

        // Marked rows recolour their label (and marker glyph) via the `Marked`
        // overlay, patched under the cursor overlay so a selected marked row
        // still shows the cursor background.
        let is_marked = data.marked.contains(&entry_idx);
        let left_base = data.style.resolved_style(LeaderListStyleType::Left);
        let left_base = if is_marked {
            left_base.patch(data.style.resolved_style(LeaderListStyleType::Marked))
        } else {
            left_base
        };
        let left_style = patch(left_base);
        let filler_style = patch(data.style.resolved_style(LeaderListStyleType::Filler));
        let right_style = patch(data.style.resolved_style(LeaderListStyleType::Right));

        // Fill the row background first so cursor highlighting spans the line.
        if let Some(bg) = cursor_overlay.and_then(|c| c.bg).or(left_style.bg) {
            for dx in 0..line_width {
                if let Some(cell) = buf.cell_mut((area.x + dx, y)) {
                    cell.set_char(' ');
                    cell.set_style(Style::default().bg(bg));
                }
            }
        }

        let max_x = area.x + line_width;
        let px = area.x;

        // A configured marker sits right after the label (the functionality),
        // not after the keys: the glyph on marked rows, an equal-width blank on
        // the rest, so the label column stays aligned regardless of marks.
        let marker_w = display_width(data.marker);
        let mark = if marker_w == 0 {
            String::new()
        } else if is_marked {
            data.marker.to_string()
        } else {
            " ".repeat(marker_w)
        };

        // An entry with no mapping (`b` empty) is just a bare label — no
        // post, no filler, no prefix; render `a` (plus its marker) and move on.
        if entry.b.is_empty() {
            let label = if marker_w == 0 {
                entry.a.clone()
            } else {
                format!("{}{}", entry.a, mark)
            };
            put_str(buf, px, y, max_x, &label, left_style);
            continue;
        }

        let left = format!("{}{}{}", entry.a, mark, data.post);
        let right = format!("{}{}", data.pre, entry.b);
        let avail = line_width as usize;
        let left_w = display_width(&left);
        let right_w = display_width(&right);

        let mut px = px;

        if left_w >= avail {
            // No room for anything past the (truncated) left segment.
            put_str(buf, px, y, max_x, &left, left_style);
            continue;
        }

        // Left segment.
        px = put_str(buf, px, y, max_x, &left, left_style);

        // Filler + right, right-aligned to end at `max_x`.
        let right_start = max_x.saturating_sub(right_w.min(avail - left_w) as u16);
        let gap = right_start.saturating_sub(px) as usize;
        if gap > 0 {
            let mid = build_filler(data.filler, gap);
            px = put_str(buf, px, y, right_start, &mid, filler_style);
        }
        put_str(buf, px.max(right_start), y, max_x, &right, right_style);
    }

    // Status line below the entries: "N entries · Page x/y". Counts the
    // filtered (visible) set, not the underlying entries.
    if data.show_status && avail >= 1 {
        let total = data.visible.len();
        let vis = rows.max(1);
        let pages = total.div_ceil(vis).max(1);
        // The current page is the one holding the "active" row: the cursor in
        // selectable mode, otherwise the top of the scrolled window.
        let anchor = if data.selectable {
            data.cursor
        } else {
            data.scroll_offset
        };
        let page = (anchor / vis + 1).min(pages);
        let drawn = rows.min(total.saturating_sub(data.scroll_offset));
        let y = area.y + head_rows + drawn as u16;

        let status_style = data.style.resolved_style(LeaderListStyleType::Status);
        if let Some(bg) = status_style.bg {
            for dx in 0..line_width {
                if let Some(cell) = buf.cell_mut((area.x + dx, y)) {
                    cell.set_char(' ');
                    cell.set_style(Style::default().bg(bg));
                }
            }
        }
        let noun = if total == 1 { "entry" } else { "entries" };
        let mut text = format!("{total} {noun} · Page {page}/{pages}");
        // Marking is color-only (no glyph), so surface the tagged count here
        // to make a batch selection legible at a glance.
        let tagged = data.marked.len();
        if tagged > 0 {
            text.push_str(&format!(" · {tagged} tagged"));
        }
        put_str(buf, area.x, y, area.x + line_width, &text, status_style);
    }
}

/// Writes `s` starting at `x0`, advancing by each glyph's display width, and
/// stops before `max_x`. Returns the x-position after the last written glyph.
fn put_str(buf: &mut Buffer, x0: u16, y: u16, max_x: u16, s: &str, style: Style) -> u16 {
    let mut px = x0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0) as u16;
        if px + cw > max_x {
            break;
        }
        if let Some(cell) = buf.cell_mut((px, y)) {
            cell.set_char(ch);
            cell.set_style(style);
        }
        px += cw;
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    static NO_MARKS: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();

    #[test]
    fn filler_exact_single_char() {
        assert_eq!(build_filler(".", 5), ".....");
    }

    #[test]
    fn filler_multi_char_repeats_and_truncates() {
        // Each of '-' and '=' is one column, so the pattern continues glyph by
        // glyph and stops exactly at the gap: gap 5 → "-=-=-".
        assert_eq!(build_filler("-=", 5), "-=-=-");
    }

    #[test]
    fn filler_zero_gap_is_empty() {
        assert_eq!(build_filler(".", 0), "");
    }

    #[test]
    fn filler_empty_degrades_to_spaces() {
        assert_eq!(build_filler("", 3), "   ");
    }

    #[test]
    fn wide_glyph_pads_when_it_would_overflow() {
        // '★' is width 1, but a CJK wide glyph is width 2.
        assert_eq!(build_filler("你", 3), "你 ");
    }

    fn row_text(buf: &Buffer, y: u16, width: u16) -> String {
        (0..width)
            .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
            .collect()
    }

    #[test]
    fn line_fills_to_width_with_right_flush() {
        let entries = vec![LeaderEntry {
            a: "Intro".into(),
            b: "12".into(),
        }];
        let style = LeaderListStyle::new();
        let width = 20u16;
        let visible: Vec<usize> = (0..entries.len()).collect();
        let data = RenderData {
            entries: &entries,
            visible: &visible,
            marked: &NO_MARKS,
            marker: "",
            title: "",
            post: "",
            pre: " ",
            filler: ".",
            cursor: 0,
            scroll_offset: 0,
            selectable: false,
            focused: false,
            style: &style,
            line_width: width,
            entry_rows: 1,
            show_status: false,
            show_search: false,
            query: "",
            search_placeholder: "",
        };
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, &data);

        // "Intro" + dot leader + " 12", exactly 20 columns, value flush right.
        let expected = format!("Intro{} 12", ".".repeat(20 - 5 - 3));
        assert_eq!(expected.len(), 20);
        assert_eq!(row_text(&buf, 0, width), expected);
    }

    #[test]
    fn entry_without_mapping_shows_only_label() {
        // An empty `b` suppresses post, filler and pre — the row is the bare
        // label followed by blank padding, no dot leader.
        let entries = vec![LeaderEntry {
            a: "Archive".into(),
            b: String::new(),
        }];
        let style = LeaderListStyle::new();
        let width = 20u16;
        let visible: Vec<usize> = (0..entries.len()).collect();
        let data = RenderData {
            entries: &entries,
            visible: &visible,
            marked: &NO_MARKS,
            marker: "",
            title: "",
            post: ":",
            pre: " ",
            filler: ".",
            cursor: 0,
            scroll_offset: 0,
            selectable: false,
            focused: false,
            style: &style,
            line_width: width,
            entry_rows: 1,
            show_status: false,
            show_search: false,
            query: "",
            search_placeholder: "",
        };
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, &data);

        // No post (`:`), no dot filler — just "Archive" and spaces.
        assert_eq!(
            row_text(&buf, 0, width),
            format!("Archive{}", " ".repeat(13))
        );
    }

    #[test]
    fn status_page_follows_cursor_when_selectable() {
        // 30 entries, 4 visible → 8 pages. The cursor on entry #9 (0-based)
        // sits on page 9/4 + 1 = 3, regardless of the scroll window.
        let entries: Vec<LeaderEntry> = (0..30)
            .map(|i| LeaderEntry {
                a: format!("Item {i}"),
                b: i.to_string(),
            })
            .collect();
        let style = LeaderListStyle::new();
        let width = 24u16;
        let visible: Vec<usize> = (0..entries.len()).collect();
        let data = RenderData {
            entries: &entries,
            visible: &visible,
            marked: &NO_MARKS,
            marker: "",
            title: "",
            post: "",
            pre: " ",
            filler: ".",
            cursor: 9,
            scroll_offset: 8,
            selectable: true,
            focused: true,
            style: &style,
            line_width: width,
            entry_rows: 4,
            show_status: true,
            show_search: false,
            query: "",
            search_placeholder: "",
        };
        // 4 entry rows + 1 status row.
        let area = Rect::new(0, 0, width, 5);
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, &data);

        let status = row_text(&buf, 4, width);
        assert!(
            status.starts_with("30 entries · Page 3/8"),
            "got: {status:?}"
        );
    }

    /// The status line reports how many entries are tagged (marked), so a
    /// color-only batch selection stays legible.
    #[test]
    fn status_reports_tagged_count() {
        let entries: Vec<LeaderEntry> = (0..5)
            .map(|i| LeaderEntry {
                a: format!("Item {i}"),
                b: i.to_string(),
            })
            .collect();
        let style = LeaderListStyle::new();
        let width = 40u16;
        let visible: Vec<usize> = (0..entries.len()).collect();
        let marks: std::collections::BTreeSet<usize> = [1usize, 3].into_iter().collect();
        let data = RenderData {
            entries: &entries,
            visible: &visible,
            marked: &marks,
            marker: "",
            title: "",
            post: "",
            pre: " ",
            filler: ".",
            cursor: 0,
            scroll_offset: 0,
            selectable: true,
            focused: true,
            style: &style,
            line_width: width,
            entry_rows: 5,
            show_status: true,
            show_search: false,
            query: "",
            search_placeholder: "",
        };
        let area = Rect::new(0, 0, width, 6);
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, &data);

        let status = row_text(&buf, 5, width);
        assert!(status.contains("· 2 tagged"), "got: {status:?}");
    }

    /// The mark glyph sits right after the label (the functionality), not after
    /// the keys, and unmarked rows reserve the same width so the label column
    /// keeps a stable right edge.
    #[test]
    fn marker_renders_after_label_not_keys() {
        let entries = vec![
            LeaderEntry {
                a: "Alpha".into(),
                b: "12".into(),
            },
            LeaderEntry {
                a: "Bravo".into(),
                b: "34".into(),
            },
        ];
        let style = LeaderListStyle::new();
        let width = 20u16;
        let visible: Vec<usize> = (0..entries.len()).collect();
        let mut marks = std::collections::BTreeSet::new();
        marks.insert(0usize);
        let data = RenderData {
            entries: &entries,
            visible: &visible,
            marked: &marks,
            marker: "*",
            title: "",
            post: " ",
            pre: " ",
            filler: ".",
            cursor: 0,
            scroll_offset: 0,
            selectable: false,
            focused: false,
            style: &style,
            line_width: width,
            entry_rows: 2,
            show_status: false,
            show_search: false,
            query: "",
            search_placeholder: "",
        };
        let area = Rect::new(0, 0, width, 2);
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, &data);

        let marked = row_text(&buf, 0, width);
        let unmarked = row_text(&buf, 1, width);
        // Glyph immediately behind the label, keys still flush right.
        assert!(marked.starts_with("Alpha* "), "got: {marked:?}");
        assert!(marked.trim_end().ends_with("12"), "got: {marked:?}");
        // Unmarked row keeps the reserved blank, so the label column aligns.
        assert!(unmarked.starts_with("Bravo  "), "got: {unmarked:?}");
        assert!(!unmarked.contains('*'), "got: {unmarked:?}");
    }

    /// A configured `Marked` style recolours the label (and marker) of marked
    /// rows only; unmarked rows keep the plain `Left` foreground.
    #[test]
    fn marked_rows_recolour_their_label() {
        use ratatui::style::Color;
        let entries = vec![
            LeaderEntry {
                a: "Alpha".into(),
                b: "12".into(),
            },
            LeaderEntry {
                a: "Bravo".into(),
                b: "34".into(),
            },
        ];
        let style = LeaderListStyle::new()
            .set_style(LeaderListStyleType::Left, Style::default().fg(Color::White))
            .set_style(
                LeaderListStyleType::Marked,
                Style::default().fg(Color::Yellow),
            );
        let width = 20u16;
        let visible: Vec<usize> = (0..entries.len()).collect();
        let mut marks = std::collections::BTreeSet::new();
        marks.insert(0usize);
        let data = RenderData {
            entries: &entries,
            visible: &visible,
            marked: &marks,
            marker: "*",
            title: "",
            post: " ",
            pre: " ",
            filler: ".",
            cursor: 0,
            scroll_offset: 0,
            selectable: false,
            focused: false,
            style: &style,
            line_width: width,
            entry_rows: 2,
            show_status: false,
            show_search: false,
            query: "",
            search_placeholder: "",
        };
        let area = Rect::new(0, 0, width, 2);
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, &data);

        // Marked row's label glyph carries the Marked foreground; unmarked
        // row keeps the plain Left foreground.
        assert_eq!(buf.cell((0, 0)).unwrap().fg, Color::Yellow);
        assert_eq!(buf.cell((0, 1)).unwrap().fg, Color::White);
    }
}
