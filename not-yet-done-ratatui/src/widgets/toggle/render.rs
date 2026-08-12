use super::style::{ToggleStyle, ToggleStyleType};
use crate::widgets::common::{PREFIX_LEN, render_prefixed_line};
use ratatui::{Frame, layout::Rect};

/// All data required to render a single frame of a [`super::Toggle`].
///
/// Constructed in the component's `view` implementation and passed to
/// [`render`]. Keeping render logic as a free function makes it independently
/// testable without a full tuirealm component context.
pub(super) struct ToggleViewData<'a> {
    pub title: &'a str,
    pub on: bool,
    pub on_label: &'a str,
    pub off_label: &'a str,
    /// Already-selected style — caller picks active vs inactive based on focus.
    pub style: &'a ToggleStyle,
}

/// Renders the toggle widget into `frame` at `area`.
///
/// Layout (two rows):
/// - Row 0: title
/// - Row 1: value — `[x] <on_label>` when on, `[ ] <off_label>` when off
pub(super) fn render(frame: &mut Frame, area: Rect, data: &ToggleViewData<'_>) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let total_width = area.width;
    let text_width = total_width.saturating_sub(PREFIX_LEN) as usize;
    let buf = frame.buffer_mut();

    // Row 0: title
    let title_style = data.style.resolved_style(ToggleStyleType::Title);
    render_prefixed_line(
        buf,
        area.x,
        area.y,
        total_width,
        data.title,
        text_width,
        &data.style.prefix_color,
        &title_style,
        false,
    );

    // Row 1: value
    if area.height > 1 {
        let marker = if data.on { "[x] " } else { "[ ] " };
        let label = if data.on {
            data.on_label
        } else {
            data.off_label
        };
        let value_text = format!("{marker}{label}");
        let value_style = data.style.resolved_style(ToggleStyleType::Value);
        render_prefixed_line(
            buf,
            area.x,
            area.y + 1,
            total_width,
            &value_text,
            text_width,
            &data.style.prefix_color,
            &value_style,
            false,
        );
    }
}
