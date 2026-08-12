//! Rendering logic for SelectList.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;

use super::component::SelectList;
use super::style::SelectListStyleType as ST;

/// Collected data for a single render pass.
#[allow(dead_code)]
pub struct RenderData<'a> {
    pub items: &'a [SelectListItem<'a>],
    pub cursor: usize,
    pub scroll_offset: usize,
    pub filter_query: &'a str,
    pub filter_cursor: usize,
    pub show_filter: bool,
    pub selected_count: usize,
    pub total_count: usize,
    pub show_footer: bool,
    pub focused: bool,
}

#[allow(dead_code)]
pub struct SelectListItem<'a> {
    pub label: &'a str,
    pub description: Option<&'a str>,
    pub icon: Option<&'a str>,
    pub group: Option<&'a str>,
    pub selected: bool,
    pub is_group_header: bool,
}

/// Renders the SelectList and returns the terminal cursor position (if the
/// filter input is visible and focused).
pub fn render(
    buf: &mut Buffer,
    area: Rect,
    data: &RenderData,
    list: &SelectList,
) -> Option<Position> {
    if area.height == 0 || area.width == 0 {
        return None;
    }

    let style = if data.focused {
        &list.active_style
    } else {
        &list.inactive_style
    };
    let marker = &list.marker;

    let mut y = area.top();
    let right = area.right();
    let left = area.left();

    let mut cursor_pos = None;

    // Filter row.
    if data.show_filter {
        if y >= area.bottom() {
            return None;
        }
        cursor_pos = render_filter_row(buf, left, y, right, data, style, list.cursor_on_empty);
        y += 1;
    }

    // Items.
    let items_height = if data.show_footer {
        area.bottom().saturating_sub(y).saturating_sub(1) as usize
    } else {
        area.bottom().saturating_sub(y) as usize
    };

    let visible_items: Vec<&SelectListItem> = data
        .items
        .iter()
        .skip(data.scroll_offset)
        .take(items_height)
        .collect();

    for (i, item) in visible_items.iter().enumerate() {
        if y >= area.bottom() {
            break;
        }
        let list_idx = data.scroll_offset + i;
        let is_cursor = list_idx == data.cursor;
        render_item_row(buf, left, y, right, item, is_cursor, marker, style);
        y += 1;
    }

    // Footer.
    if data.show_footer && y < area.bottom() {
        render_footer_row(buf, left, y, right, data, style);
    }

    if data.focused { cursor_pos } else { None }
}

/// Renders the filter row and returns the terminal cursor position.
fn render_filter_row(
    buf: &mut Buffer,
    left: u16,
    y: u16,
    right: u16,
    data: &RenderData,
    style: &super::style::SelectListStyle,
    cursor_on_empty: bool,
) -> Option<Position> {
    let input_style = style.resolved_style(ST::FilterInput);

    // Fill the entire row with the FilterInput bg first so the
    // highlight extends past the typed text. Cells will get a real
    // character/style assigned below as needed.
    if let Some(bg) = input_style.bg {
        for x in left..right {
            if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                cell.set_char(' ');
                cell.set_bg(bg);
            }
        }
    }

    // Icon.
    write_str(buf, left, y, "󰈲 ", input_style, right);
    let text_x = left + 3; // icon is 2 wide + space

    let chars: Vec<char> = data.filter_query.chars().collect();
    let max_w = right.saturating_sub(text_x) as usize;
    let view_start = if data.filter_cursor >= max_w {
        data.filter_cursor + 1 - max_w
    } else {
        0
    };

    if chars.is_empty() {
        // Placeholder text — uses placeholder_color over the input bg
        // when configured, so it reads as "dim hint text" rather than
        // real input.
        let placeholder = "type to filter…";
        let ph_style = match style.placeholder_color {
            Some(fg) => input_style.fg(fg),
            None => input_style,
        };
        let mut x = text_x;
        for ch in placeholder.chars() {
            if x >= right {
                break;
            }
            if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                cell.set_char(ch);
                cell.set_style(ph_style);
            }
            x += 1;
        }
        if cursor_on_empty {
            Some(Position::new(text_x, y))
        } else {
            None
        }
    } else {
        let mut x = text_x;
        for (screen_idx, char_idx) in (view_start..chars.len()).enumerate() {
            if screen_idx >= max_w || x >= right {
                break;
            }
            if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                cell.set_char(chars[char_idx]);
                cell.set_style(input_style);
            }
            x += 1;
        }

        let screen_pos = data.filter_cursor.saturating_sub(view_start);
        let cx = text_x + screen_pos as u16;
        if cx < right {
            Some(Position::new(cx, y))
        } else {
            None
        }
    }
}

fn render_item_row(
    buf: &mut Buffer,
    left: u16,
    y: u16,
    right: u16,
    item: &SelectListItem,
    is_cursor: bool,
    marker: &crate::widgets::common::types::SelectionMarker,
    style: &super::style::SelectListStyle,
) {
    let item_style = match (item.selected, is_cursor) {
        (true, true) => style.resolved_style(ST::ItemCursorSelected),
        (true, false) => style.resolved_style(ST::ItemSelected),
        (false, true) => style.resolved_style(ST::ItemCursor),
        (false, false) => style.resolved_style(ST::Item),
    };

    if item.is_group_header {
        let gs = style.resolved_style(ST::GroupHeader);
        write_str(buf, left, y, item.label, gs, right);
        return;
    }

    // Selected rows pull their brighter highlight back one cell so a
    // visible gutter shows between the bar and whatever frames the list
    // on the right. The trailing cell falls back to the base Item style
    // (or ItemCursor when the cursor is on it) so the row body still
    // reads as continuous.
    let highlight_right = if item.selected && right > left {
        right - 1
    } else {
        right
    };
    let trailing_style = if item.selected && highlight_right < right {
        let trail_slot = if is_cursor { ST::ItemCursor } else { ST::Item };
        Some(style.resolved_style(trail_slot))
    } else {
        None
    };

    // Fill the row background up to highlight_right with item_style …
    for x in left..highlight_right {
        if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
            cell.set_char(' ');
            cell.set_style(item_style);
        }
    }
    // … then fill the trailing gutter (if any) with the base style.
    if let Some(trail) = trailing_style {
        for x in highlight_right..right {
            if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                cell.set_char(' ');
                cell.set_style(trail);
            }
        }
    }

    let mut x = left;

    // Marker.
    write_str_styled(
        buf,
        &mut x,
        y,
        marker.text(item.selected),
        item_style,
        highlight_right,
    );

    // Icon.
    if let Some(icon) = item.icon {
        write_str_styled(buf, &mut x, y, icon, item_style, highlight_right);
        write_str_styled(buf, &mut x, y, " ", item_style, highlight_right);
    }

    // Label.
    write_str_styled(buf, &mut x, y, item.label, item_style, highlight_right);
}

fn render_footer_row(
    buf: &mut Buffer,
    left: u16,
    y: u16,
    right: u16,
    data: &RenderData,
    style: &super::style::SelectListStyle,
) {
    let fs = style.resolved_style(ST::Footer);
    // Fill the entire row with the Footer bg first so the highlight
    // extends past the text.
    if let Some(bg) = fs.bg {
        for x in left..right {
            if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                cell.set_char(' ');
                cell.set_bg(bg);
            }
        }
    }
    let text = format!("{} selected", data.selected_count);
    write_str(buf, left, y, &text, fs, right);
}

fn write_str(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style, max_x: u16) {
    let mut cx = x;
    for ch in text.chars() {
        if cx >= max_x {
            break;
        }
        if let Some(cell) = buf.cell_mut(Position::new(cx, y)) {
            cell.set_char(ch);
            cell.set_style(style);
        }
        cx += 1;
    }
}

fn write_str_styled(buf: &mut Buffer, x: &mut u16, y: u16, text: &str, style: Style, max_x: u16) {
    for ch in text.chars() {
        if *x >= max_x {
            break;
        }
        if let Some(cell) = buf.cell_mut(Position::new(*x, y)) {
            cell.set_char(ch);
            cell.set_style(style);
        }
        *x += 1;
    }
}
