//! Table rendering logic.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use super::{ColumnStyles, StyleMap, TableWidgetCell, TableWidgetRow};
use super::style::{TableStyle, TableStyleType as ST};

pub(super) struct RenderData<'a> {
    pub fixed_header_rows: &'a [TableWidgetRow],
    pub rows: &'a [TableWidgetRow],
    pub fixed_footer_rows: &'a [TableWidgetRow],
    pub selected_row: usize,
    /// Optional column cursor index. `None` = column-cursor feature
    /// disabled, no per-column highlight is applied.
    pub selected_column: Option<usize>,
    pub scroll_offset: usize,
    /// Smooth scrolling: leading physical lines of the row at `scroll_offset`
    /// that are clipped above the viewport top. 0 in discrete mode (the top
    /// row renders flush). See [`super::smooth`].
    pub scroll_sub_line: usize,
    /// Number of leading logical columns hidden by horizontal scroll.
    /// Cells whose entire range falls within `0..scroll_col_offset` are
    /// skipped at render time.
    pub scroll_col_offset: usize,
    /// Total characters hidden left of the viewport (sum of hidden cell
    /// widths + separators). Used to shift jump-mode label positions.
    pub scrolled_chars: usize,
    /// Whether at least one column extends past the visible right edge.
    /// Drives the `›` indicator in the header row.
    pub has_more_right: bool,
    pub separator: &'a str,
    pub col_styles: &'a ColumnStyles,
    pub style_map: &'a StyleMap,
    pub style: &'a TableStyle,
    pub focused: bool,
    /// Hop-style jump: (visible_index, all_match_char_positions, label) for matching rows.
    pub jump_matches: &'a [(usize, Vec<usize>, String)],
    /// Whether we're in label-input phase (dim non-matching rows).
    pub jump_showing_labels: bool,
    /// Current partial label input (to highlight matching labels).
    pub jump_input: &'a str,
}

pub(super) fn render(buf: &mut Buffer, area: Rect, data: &RenderData) {
    if area.height == 0 || area.width == 0 { return; }

    let fixed_top = data.fixed_header_rows.len() as u16;
    let fixed_bottom = data.fixed_footer_rows.len() as u16;
    let data_height = area.height.saturating_sub(fixed_top).saturating_sub(fixed_bottom);

    // Fixed header rows at top.
    let mut y = area.top();
    for row in data.fixed_header_rows {
        if y >= area.bottom() { break; }
        let row_area = Rect { y, height: 1, ..area };
        render_fixed_row(buf, row_area, row, data);
        y += 1;
    }

    // Scrollable data rows in the middle. Each row spans `row.height()`
    // physical lines (1 for the classic single-line case); the inner loop
    // paints them top-to-bottom and advances `y` by the row's height.
    let data_bottom = area.bottom().saturating_sub(fixed_bottom);
    let _ = data_height;
    for (vi, row) in data.rows.iter()
        .skip(data.scroll_offset)
        .enumerate()
    {
        if y >= data_bottom { break; }
        let row_idx = data.scroll_offset + vi;
        let is_selected = data.focused && row_idx == data.selected_row;

        // In jump label phase: find if this row has a match. Jump anchors
        // on the row's primary (first) line.
        let jump_match = if data.jump_showing_labels {
            data.jump_matches.iter().find(|(idx, _, _)| *idx == vi)
        } else {
            None
        };
        let dim_row = data.jump_showing_labels && jump_match.is_none() && row.selectable;

        // Smooth scrolling clips the first visible row's leading lines so it
        // can be partially scrolled off the top. Later rows always start at
        // their first line.
        let skip_lines = if vi == 0 {
            data.scroll_sub_line.min(row.lines.len())
        } else {
            0
        };

        for (li, line) in row.lines.iter().enumerate().skip(skip_lines) {
            if y >= data_bottom { break; }
            let row_area = Rect { y, height: 1, ..area };
            // A line opts out of selection styling via `highlight_on_select`
            // (e.g. a spacer line stays "outside" the selection block).
            let line_selected = is_selected && line.highlight_on_select && !dim_row;
            render_data_row(buf, row_area, &line.cells, row_idx, line_selected, data);

            // Dim non-matching rows during jump (all physical lines).
            if dim_row {
                for x in area.left()..area.right() {
                    if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                        let base = data.style.resolved_style(ST::Row);
                        cell.set_style(Style::default()
                            .fg(base.fg.unwrap_or_default())
                            .bg(base.bg.unwrap_or_default()));
                    }
                }
            }

            // Overlay jump labels on the primary line only. With horizontal
            // scroll, match_pos is computed in the unscrolled row, so
            // subtract scrolled_chars to get the rendered x — and skip
            // labels whose anchor lies left of the viewport.
            if li == 0 {
                if let Some((_, positions, label)) = jump_match {
                    let label_style = Style::default()
                        .fg(ratatui::style::Color::Black)
                        .bg(ratatui::style::Color::Yellow)
                        .add_modifier(Modifier::BOLD);
                    for match_pos in positions {
                        let Some(visible_pos) = match_pos.checked_sub(data.scrolled_chars) else {
                            continue;
                        };
                        let label_x = area.left() + visible_pos as u16 + 1; // +1 = after the match char
                        for (i, ch) in label.chars().enumerate() {
                            let x = label_x + i as u16;
                            if x < area.right() {
                                if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                                    cell.set_char(ch);
                                    cell.set_style(label_style);
                                }
                            }
                        }
                    }
                }
            }

            y += 1;
        }
    }

    // Fixed footer rows — directly below the last data row, but pinned
    // to the bottom if the data would push them off-screen.
    let max_footer_y = area.bottom().saturating_sub(fixed_bottom);
    let mut fy = y.min(max_footer_y);
    for row in data.fixed_footer_rows {
        if fy >= area.bottom() { break; }
        let row_area = Rect { y: fy, height: 1, ..area };
        render_fixed_row(buf, row_area, row, data);
        fy += 1;
    }

    render_scroll_indicators(buf, area, data);
}

/// Overlay `‹` / `›` glyphs at the pane edges in the top row whenever
/// horizontal scroll has hidden columns left/right of the viewport.
fn render_scroll_indicators(buf: &mut Buffer, area: Rect, data: &RenderData) {
    let has_left = data.scroll_col_offset > 0;
    let has_right = data.has_more_right;
    if !has_left && !has_right { return; }
    if area.width == 0 { return; }

    let style = data.style.resolved_style(ST::ScrollIndicator);
    let header_bg = data.style.resolved_style(ST::Header).bg.unwrap_or_default();
    let glyph_style = Style::default()
        .fg(style.fg.unwrap_or_default())
        .bg(style.bg.unwrap_or(header_bg))
        .add_modifier(style.add_modifier);

    let y = area.top();
    if has_left {
        if let Some(cell) = buf.cell_mut(Position::new(area.left(), y)) {
            cell.set_char('‹');
            cell.set_style(glyph_style);
        }
    }
    if has_right {
        if let Some(cell) = buf.cell_mut(Position::new(area.right().saturating_sub(1), y)) {
            cell.set_char('›');
            cell.set_style(glyph_style);
        }
    }
}

/// Render a fixed (header/footer) row — uses Header style, no selection.
fn render_fixed_row(
    buf: &mut Buffer,
    area: Rect,
    row: &TableWidgetRow,
    data: &RenderData,
) {
    let header_style = data.style.resolved_style(ST::Header);
    let bg = header_style.bg.unwrap_or_default();
    let hl_style = data.style.resolved_style(ST::Highlight);

    for x in area.left()..area.right() {
        if let Some(cell) = buf.cell_mut(Position::new(x, area.y)) {
            cell.set_char(' ');
            cell.set_style(Style::default().bg(bg));
        }
    }

    let mut spans: Vec<Span> = Vec::new();
    let mut col_idx = 0;
    let mut first_rendered = true;

    for cell in row.primary_line() {
        let cell_col = col_idx;
        let span = cell.col_span.max(1);
        col_idx += span;
        if cell_col + span <= data.scroll_col_offset {
            continue;
        }
        if !first_rendered {
            spans.push(Span::styled(data.separator.to_string(), Style::default().bg(bg)));
        }
        first_rendered = false;

        let col_fg = resolve_cell_fg(cell, cell_col, data);
        let normal = Style::default().fg(col_fg).bg(bg).add_modifier(Modifier::BOLD);
        let highlight = Style::default()
            .fg(hl_style.fg.unwrap_or(col_fg))
            .bg(bg)
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::UNDERLINED);
        if cell.highlights.is_empty() {
            spans.push(Span::styled(cell.text.clone(), normal));
        } else {
            spans.extend(spans_with_highlights(&cell.text, &cell.highlights, normal, highlight));
        }
    }
    Paragraph::new(Line::from(spans)).render(area, buf);
}

/// Render a single physical line of a data row with selection highlighting.
///
/// `cells` are the cells of one physical line (a single-line row has just
/// one such line). `row_idx` is the row's data index; combined with
/// `data.selected_column` it lets the renderer pick `RowSelected` /
/// `ColumnSelected` / `CellSelected` / `Row` per cell. The whole-line
/// pre-fill uses the row-level base (Row or RowSelected) so separators and
/// trailing whitespace inherit it; per-cell spans then override their own
/// bg/fg when they sit on the column cursor.
fn render_data_row(
    buf: &mut Buffer,
    area: Rect,
    cells: &[TableWidgetCell],
    row_idx: usize,
    selected: bool,
    data: &RenderData,
) {
    let row_base = if selected {
        data.style.resolved_style(ST::RowSelected)
    } else {
        data.style.resolved_style(ST::Row)
    };
    let row_bg = row_base.bg.unwrap_or_default();
    let hl_style = data.style.resolved_style(ST::Highlight);
    let prefix_style = data.style.resolved_style(ST::Prefix);
    let is_selected_row = data.focused && row_idx == data.selected_row;

    for x in area.left()..area.right() {
        if let Some(cell) = buf.cell_mut(Position::new(x, area.y)) {
            cell.set_char(' ');
            cell.set_bg(row_bg);
        }
    }

    let mut spans: Vec<Span> = Vec::new();
    let mut col_idx = 0;
    let mut first_rendered = true;

    for cell in cells {
        let cell_col = col_idx;
        let span = cell.col_span.max(1);
        col_idx += span;
        if cell_col + span <= data.scroll_col_offset {
            continue;
        }
        if !first_rendered {
            spans.push(Span::styled(data.separator.to_string(), Style::default().bg(row_bg)));
        }
        first_rendered = false;

        // Pick the cell's base style with column-cursor precedence:
        // CellSelected (intersection) > RowSelected > ColumnSelected > Row.
        let is_col_match = data.focused && data.selected_column == Some(cell_col);
        let cell_base = match (is_selected_row, is_col_match) {
            (true, true) => data.style.resolved_style(ST::CellSelected),
            (true, false) => row_base,
            (false, true) => data.style.resolved_style(ST::ColumnSelected),
            (false, false) => row_base,
        };
        let cell_bg = cell_base.bg.unwrap_or(row_bg);
        let col_fg = resolve_cell_fg(cell, cell_col, data);
        // Per-column / per-cell color (`col_fg`, from a column `style:` or a
        // cell `style_id`) owns the foreground. Selecting a row changes only
        // the *background* (RowSelected bg), keeping each column's color — a
        // row highlight should not flatten the palette. The `Row` base fg is
        // only the fallback when a column declares no color.
        //
        // The exception is the column cursor (ColumnSelected / CellSelected,
        // `is_col_match`): that is a deliberate, high-contrast pointer at a
        // single cell, so there the base style's fg (e.g. on_primary) wins.
        let cell_fg = if is_col_match {
            cell_base.fg.unwrap_or(col_fg)
        } else {
            col_fg
        };
        let normal_style = Style::default()
            .fg(cell_fg)
            .bg(cell_bg)
            .add_modifier(cell_base.add_modifier);
        let cell_hl_style = Style::default()
            .fg(hl_style.fg.unwrap_or(cell_fg))
            .bg(cell_bg)
            .add_modifier(Modifier::BOLD);
        let cell_prefix_style = Style::default()
            .fg(prefix_style.fg.unwrap_or(cell_fg))
            .bg(cell_bg);

        if !cell.segments.is_empty() {
            for (seg_text, seg_style_id) in &cell.segments {
                let seg_style = match seg_style_id {
                    Some(id) => match data.style_map.get(*id) {
                        Some(s) => Style::default()
                            .fg(s.fg.unwrap_or(cell_fg))
                            .bg(cell_bg)
                            .add_modifier(s.add_modifier),
                        None => normal_style,
                    },
                    None => normal_style,
                };
                spans.push(Span::styled(seg_text.clone(), seg_style));
            }
        } else if cell.prefix_len > 0 {
            let chars: Vec<char> = cell.text.chars().collect();
            let prefix: String = chars.iter().take(cell.prefix_len).collect();
            let rest: String = chars.iter().skip(cell.prefix_len).collect();

            spans.push(Span::styled(prefix, cell_prefix_style));

            let shifted: Vec<std::ops::Range<usize>> = cell.highlights.iter()
                .filter_map(|r| {
                    let start = r.start.checked_sub(cell.prefix_len)?;
                    let end = r.end.saturating_sub(cell.prefix_len);
                    if start < end { Some(start..end) } else { None }
                })
                .collect();

            spans.extend(spans_with_highlights(&rest, &shifted, normal_style, cell_hl_style));
        } else if !cell.highlights.is_empty() {
            spans.extend(spans_with_highlights(&cell.text, &cell.highlights, normal_style, cell_hl_style));
        } else {
            spans.push(Span::styled(cell.text.clone(), normal_style));
        }
    }
    Paragraph::new(Line::from(spans)).render(area, buf);
}

fn resolve_cell_fg(cell: &TableWidgetCell, col_idx: usize, data: &RenderData) -> ratatui::style::Color {
    if let Some(style_id) = cell.style_id {
        if let Some(style) = data.style_map.get(style_id) {
            if let Some(fg) = style.fg {
                return fg;
            }
        }
    }
    data.col_styles.get(col_idx).fg.unwrap_or_default()
}

fn spans_with_highlights<'a>(
    text: &str,
    highlights: &[std::ops::Range<usize>],
    normal: Style,
    highlight: Style,
) -> Vec<Span<'a>> {
    if highlights.is_empty() {
        return vec![Span::styled(text.to_string(), normal)];
    }

    let chars: Vec<char> = text.chars().collect();
    let mut result = Vec::new();
    let mut pos = 0;

    for range in highlights {
        let start = range.start.min(chars.len());
        let end = range.end.min(chars.len());
        if start > pos {
            result.push(Span::styled(
                chars[pos..start].iter().collect::<String>(),
                normal,
            ));
        }
        if start < end {
            result.push(Span::styled(
                chars[start..end].iter().collect::<String>(),
                highlight,
            ));
        }
        pos = end;
    }
    if pos < chars.len() {
        result.push(Span::styled(
            chars[pos..].iter().collect::<String>(),
            normal,
        ));
    }

    result
}

#[cfg(test)]
mod fg_precedence_tests {
    use super::super::{TableWidgetCell, TableWidgetLine, TableWidgetRow};
    use super::*;
    use ratatui::style::Color;

    fn render_one(rows: &[TableWidgetRow], style: &TableStyle, col_styles: &ColumnStyles, style_map: &StyleMap, focused: bool, height: u16) -> Buffer {
        let data = RenderData {
            fixed_header_rows: &[],
            rows,
            fixed_footer_rows: &[],
            selected_row: 0,
            selected_column: None,
            scroll_offset: 0,
            scroll_sub_line: 0,
            scroll_col_offset: 0,
            scrolled_chars: 0,
            has_more_right: false,
            separator: "  ",
            col_styles,
            style_map,
            style,
            focused,
            jump_matches: &[],
            jump_showing_labels: false,
            jump_input: "",
        };
        let area = Rect { x: 0, y: 0, width: 10, height };
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, &data);
        buf
    }

    #[test]
    fn plain_cell_column_color_wins_over_row_base_fg() {
        // Row base fg = Red (the global text default); column 0 style fg =
        // Green (an explicit per-column color). A plain, unselected, unfocused
        // cell must render in the column color, not the row default.
        let style = TableStyle::new().set_style(ST::Row, Style::default().fg(Color::Red));
        let col_styles = ColumnStyles::new(vec![Style::default().fg(Color::Green)]);
        let style_map = StyleMap::new(vec![]);
        let rows = vec![TableWidgetRow::new(vec![TableWidgetCell::plain("Hi".to_string())])];
        let buf = render_one(&rows, &style, &col_styles, &style_map, false, 1);
        assert_eq!(buf.cell(Position::new(0, 0)).unwrap().fg, Color::Green);
    }

    #[test]
    fn multiline_cell_style_id_color_wins_over_row_base_fg() {
        // Multi-line path: cells carry their color via `style_id` into the
        // StyleMap, col_styles is empty. The per-column color must still win
        // over the Row base fg on an unselected row.
        let style = TableStyle::new().set_style(ST::Row, Style::default().fg(Color::Red));
        let col_styles = ColumnStyles::default();
        let style_map = StyleMap::new(vec![Style::default().fg(Color::Green)]);
        let line = TableWidgetLine::new(vec![
            TableWidgetCell::plain("author".to_string()).with_style(0),
        ]);
        let rows = vec![TableWidgetRow::multiline(vec![line])];
        let buf = render_one(&rows, &style, &col_styles, &style_map, false, 1);
        assert_eq!(buf.cell(Position::new(0, 0)).unwrap().fg, Color::Green);
    }

    #[test]
    fn selected_row_changes_only_bg_keeps_column_color() {
        // On the selected row only the background changes (RowSelected bg);
        // each column keeps its own fg — a row highlight must not flatten the
        // palette to the selection fg.
        let style = TableStyle::new()
            .set_style(ST::Row, Style::default().fg(Color::Red).bg(Color::Black))
            .set_style(
                ST::RowSelected,
                Style::default().fg(Color::Yellow).bg(Color::Blue),
            );
        let col_styles = ColumnStyles::new(vec![Style::default().fg(Color::Green)]);
        let style_map = StyleMap::new(vec![]);
        let rows = vec![TableWidgetRow::new(vec![TableWidgetCell::plain("Hi".to_string())])];
        let buf = render_one(&rows, &style, &col_styles, &style_map, true, 1);
        let cell = buf.cell(Position::new(0, 0)).unwrap();
        assert_eq!(cell.fg, Color::Green, "column color kept");
        assert_eq!(cell.bg, Color::Blue, "selection bg applied");
    }

    #[test]
    fn column_cursor_cell_recolors_fg() {
        // The column cursor (CellSelected) is a deliberate high-contrast
        // pointer: its base fg overrides the column color.
        let style = TableStyle::new()
            .set_style(ST::Row, Style::default().fg(Color::Red))
            .set_style(
                ST::CellSelected,
                Style::default().fg(Color::Yellow).bg(Color::Magenta),
            );
        let col_styles = ColumnStyles::new(vec![Style::default().fg(Color::Green)]);
        let style_map = StyleMap::new(vec![]);
        let rows = vec![TableWidgetRow::new(vec![TableWidgetCell::plain("Hi".to_string())])];
        let data = RenderData {
            fixed_header_rows: &[],
            rows: &rows,
            fixed_footer_rows: &[],
            selected_row: 0,
            selected_column: Some(0),
            scroll_offset: 0,
            scroll_sub_line: 0,
            scroll_col_offset: 0,
            scrolled_chars: 0,
            has_more_right: false,
            separator: "  ",
            col_styles: &col_styles,
            style_map: &style_map,
            style: &style,
            focused: true,
            jump_matches: &[],
            jump_showing_labels: false,
            jump_input: "",
        };
        let area = Rect { x: 0, y: 0, width: 10, height: 1 };
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, &data);
        assert_eq!(buf.cell(Position::new(0, 0)).unwrap().fg, Color::Yellow);
    }
}
