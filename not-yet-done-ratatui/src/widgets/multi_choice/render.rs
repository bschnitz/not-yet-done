use crate::widgets::common::{render_empty_line, render_prefixed_line, PREFIX_LEN};
use ratatui::{Frame, layout::Rect};

use super::style::{MultiChoiceStyle, MultiChoiceStyleType};

/// All data required to render a single frame of a [`super::MultiChoice`].
pub(super) struct MultiChoiceViewData<'a> {
    pub title: &'a str,
    pub choices: &'a [String],
    pub selected: &'a [bool],
    pub cursor: usize,
    pub open: bool,
    pub placeholder: &'a str,
    pub width: Option<u16>,
    pub style: &'a MultiChoiceStyle,
}

/// Renders the multi-choice widget into `frame` at `area`.
///
/// Layout:
/// - Row 0: title
/// - If `open` is true:
///   - Rows 1..=choices.len(): each choice, with prefix and style according to selection/cursor
///   - One blank closing line
/// - If `open` is false:
///   - Row 1: summary of selected items (or placeholder if none)
pub(super) fn render(frame: &mut Frame, area: Rect, data: &MultiChoiceViewData<'_>) {
    let total_width = data.width.unwrap_or(area.width);
    let text_width = total_width.saturating_sub(PREFIX_LEN) as usize;
    let x = area.x;
    let y = area.y;

    // Row 0: title
    let title_style = data.style.resolved_style(MultiChoiceStyleType::Title);
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

    if data.open {
        // Expanded: one row per choice
        for (i, choice) in data.choices.iter().enumerate() {
            let row = y + 1 + i as u16;
            let is_selected = data.selected.get(i).copied().unwrap_or(false);
            let is_cursor = i == data.cursor;

            let style_type = match (is_selected, is_cursor) {
                (false, false) => MultiChoiceStyleType::Normal,
                (false, true) => MultiChoiceStyleType::Active,
                (true, false) => MultiChoiceStyleType::Selected,
                (true, true) => MultiChoiceStyleType::SelectedActive,
            };
            let row_style = data.style.resolved_style(style_type);
            render_prefixed_line(
                frame.buffer_mut(),
                x,
                row,
                total_width,
                choice,
                text_width,
                &data.style.prefix_color,
                &row_style,
                is_cursor,
            );
        }

        // Blank closing line
        let closing_row = y + 1 + data.choices.len() as u16;
        let closing_style = data.style.resolved_style(MultiChoiceStyleType::LastLine);
        render_empty_line(frame.buffer_mut(), x, closing_row, total_width, closing_style);
    } else {
        // Collapsed: show summary of selected items
        let summary = build_summary(data.choices, data.selected);
        let (summary_text, summary_style) = if summary.is_empty() {
            (
                data.placeholder.to_string(),
                data.style.resolved_style(MultiChoiceStyleType::Normal),
            )
        } else {
            (
                summary,
                data.style.resolved_style(MultiChoiceStyleType::SelectedActive),
            )
        };
        render_prefixed_line(
            frame.buffer_mut(),
            x,
            y + 1,
            total_width,
            &summary_text,
            text_width,
            &data.style.prefix_color,
            &summary_style,
            false,
        );
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
