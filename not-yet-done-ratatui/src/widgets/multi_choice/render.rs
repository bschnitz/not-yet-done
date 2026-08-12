use crate::widgets::common::types::SelectionMarker;
use crate::widgets::common::{PREFIX_LEN, render_empty_line, render_prefixed_line};
use ratatui::buffer::Buffer;
use ratatui::layout::Position;
use ratatui::{Frame, layout::Rect};

use super::style::{MultiChoiceStyle, MultiChoiceStyleType as ST};

/// All data required to render a single frame of a [`super::MultiChoice`].
pub(super) struct MultiChoiceViewData<'a> {
    pub title: &'a str,
    pub choices: &'a [String],
    pub selected: &'a [bool],
    pub filtered_indices: &'a [usize],
    pub cursor: usize,
    pub scroll_offset: usize,
    pub open: bool,
    pub placeholder: &'a str,
    pub width: Option<u16>,
    pub max_height: Option<u16>,
    pub marker: &'a SelectionMarker,
    pub show_filter: bool,
    pub show_footer: bool,
    pub show_order: bool,
    /// Full item order (indices into choices). Used for order number display.
    pub order: &'a [usize],
    pub cursor_on_empty: bool,
    pub filter_query: &'a str,
    pub filter_cursor: usize,
    pub style: &'a MultiChoiceStyle,
    pub shortcuts: &'a [Option<char>],
}

pub(super) fn render(
    frame: &mut Frame,
    area: Rect,
    data: &MultiChoiceViewData<'_>,
) -> Option<Position> {
    let total_width = data.width.unwrap_or(area.width);
    let text_width = total_width.saturating_sub(PREFIX_LEN) as usize;
    let x = area.x;
    let mut y = area.y;
    let mut cursor_pos = None;

    // Row 0: title
    let title_style = data.style.resolved_style(ST::Title);
    render_prefixed_line(
        frame.buffer_mut(),
        x,
        y,
        total_width,
        data.title,
        text_width,
        &data.style.prefix_color,
        &title_style,
        false,
    );
    y += 1;

    if data.open {
        // Filter row (optional).
        if data.show_filter {
            if y < area.bottom() {
                cursor_pos = render_filter_row(
                    frame.buffer_mut(),
                    x,
                    y,
                    total_width,
                    text_width,
                    data.filter_query,
                    data.filter_cursor,
                    &data.style.prefix_color,
                    data.style,
                    data.cursor_on_empty,
                );
                y += 1;
            }
        }

        // Determine visible items.
        let visible_count = if let Some(max_h) = data.max_height {
            (max_h as usize).min(data.filtered_indices.len())
        } else {
            data.filtered_indices.len()
        };

        let visible_items = data
            .filtered_indices
            .iter()
            .skip(data.scroll_offset)
            .take(visible_count);

        // Calculate order number padding width based on total items.
        let order_width = if data.show_order {
            let total_items = data.order.len();
            let digits = if total_items == 0 {
                1
            } else {
                (total_items as f64).log10() as usize + 1
            };
            digits + 2 // "N. " with right-aligned number
        } else {
            0
        };

        for (vi, &real_idx) in visible_items.enumerate() {
            if y >= area.bottom() {
                break;
            }
            let choice = &data.choices[real_idx];
            let is_selected = data.selected.get(real_idx).copied().unwrap_or(false);
            let is_cursor = (data.scroll_offset + vi) == data.cursor;

            let style_type = match (is_selected, is_cursor) {
                (false, false) => ST::Normal,
                (false, true) => ST::Active,
                (true, false) => ST::Selected,
                (true, true) => ST::SelectedActive,
            };
            let row_style = data.style.resolved_style(style_type);

            let marker_text = data.marker.text(is_selected);

            let display = if data.show_order {
                // Look up position of this item in the full order vec.
                let order_pos = data
                    .order
                    .iter()
                    .position(|&x| x == real_idx)
                    .map(|p| p + 1)
                    .unwrap_or(0);
                let num_str = format!("{:>width$}. ", order_pos, width = order_width - 2);
                if marker_text.is_empty() {
                    format!("{num_str}{choice}")
                } else {
                    format!("{num_str}{marker_text}{choice}")
                }
            } else if marker_text.is_empty() {
                choice.clone()
            } else {
                format!("{marker_text}{choice}")
            };

            render_prefixed_line(
                frame.buffer_mut(),
                x,
                y,
                total_width,
                &display,
                text_width,
                &data.style.prefix_color,
                &row_style,
                is_cursor,
            );

            // Highlight shortcut character if present.
            if let Some(Some(sc)) = data.shortcuts.get(real_idx) {
                // Find the position of the shortcut char in the choice label.
                if let Some(char_offset) = choice
                    .chars()
                    .position(|c| c.to_lowercase().next() == Some(*sc))
                {
                    // Compute the screen position: PREFIX_LEN + marker + order + char_offset
                    let prefix_chars = marker_text.chars().count();
                    let screen_offset = if data.show_order {
                        order_width + prefix_chars + char_offset
                    } else {
                        prefix_chars + char_offset
                    };
                    let cx = x + PREFIX_LEN + screen_offset as u16;
                    if cx < x + total_width {
                        if let Some(cell) = frame.buffer_mut().cell_mut((cx, y)) {
                            use ratatui::style::Modifier;
                            let hl_style =
                                row_style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
                            cell.set_style(hl_style);
                        }
                    }
                }
            }

            y += 1;
        }

        // Footer row (optional).
        if data.show_footer && y < area.bottom() {
            let selected_count = data.selected.iter().filter(|&&s| s).count();
            let footer_text = format!("{} selected", selected_count);
            let footer_style = data.style.resolved_style(ST::Footer);
            render_prefixed_line(
                frame.buffer_mut(),
                x,
                y,
                total_width,
                &footer_text,
                text_width,
                &data.style.prefix_color,
                &footer_style,
                false,
            );
            y += 1;
        }

        // Blank closing line.
        if y < area.bottom() {
            let closing_style = data.style.resolved_style(ST::LastLine);
            render_empty_line(frame.buffer_mut(), x, y, total_width, closing_style);
        }
    } else {
        // Collapsed: show summary of selected items.
        let summary = build_summary(data.choices, data.selected);
        let (summary_text, summary_style) = if summary.is_empty() {
            (
                data.placeholder.to_string(),
                data.style.resolved_style(ST::Normal),
            )
        } else {
            (summary, data.style.resolved_style(ST::SelectedActive))
        };
        render_prefixed_line(
            frame.buffer_mut(),
            x,
            y,
            total_width,
            &summary_text,
            text_width,
            &data.style.prefix_color,
            &summary_style,
            false,
        );
    }

    cursor_pos
}

fn render_filter_row(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    total_width: u16,
    text_width: usize,
    query: &str,
    cursor_pos: usize,
    prefix_color: &Option<ratatui::style::Color>,
    style: &MultiChoiceStyle,
    cursor_on_empty: bool,
) -> Option<Position> {
    let input_style = style.resolved_style(ST::FilterInput);

    // Render prefix + text line.
    let display = if query.is_empty() {
        "type to filter…".to_string()
    } else {
        query.to_string()
    };
    render_prefixed_line(
        buf,
        x,
        y,
        total_width,
        &display,
        text_width,
        prefix_color,
        &input_style,
        false,
    );

    if query.is_empty() && !cursor_on_empty {
        return None;
    }

    // Compute terminal cursor position.
    let text_start = x + PREFIX_LEN;
    let max_w = total_width.saturating_sub(PREFIX_LEN) as usize;
    let view_start = if cursor_pos >= max_w {
        cursor_pos + 1 - max_w
    } else {
        0
    };
    let screen_pos = cursor_pos.saturating_sub(view_start);

    if screen_pos < max_w {
        let cx = text_start + screen_pos as u16;
        Some(Position::new(cx, y))
    } else {
        None
    }
}

fn build_summary(choices: &[String], selected: &[bool]) -> String {
    choices
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            if selected.get(i).copied().unwrap_or(false) {
                Some(c.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}
