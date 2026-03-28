pub mod keymap;
pub mod state;
pub mod style;

pub use keymap::MultiChoiceKeymap;
pub use state::{MultiChoiceEvent, MultiChoiceState};
pub use style::{MultiChoiceStyle, MultiChoiceStyleType};

use crate::widgets::common::{render_prefixed_line, PREFIX_LEN};

use ratatui::{buffer::Buffer, layout::Rect, style::Style, widgets::Widget};

/// A multiple-choice widget with a drop-down-style expanded list.
///
/// # Collapsed (`state.open == false`)
/// ```text
/// ▍ Title
/// ▍ Option 1, Option 2
/// ```
///
/// # Expanded (`state.open == true`)
/// ```text
/// ▍ Title
/// ▍▶ Option 1      ← cursor row
/// ▍  Option 2
/// ▍  Option 3
///                  ← blank closing line
/// ```
///
/// When expanded the widget **overwrites** buffer cells below its nominal
/// `area`, allowing the drop-down to overlap widgets that were rendered
/// earlier.  The caller is responsible for rendering this widget **last**
/// among potentially overlapped siblings; the `Form` widget handles this
/// automatically by rendering the active widget after all inactive ones.
#[derive(Debug, Clone)]
pub struct MultiChoice<'a> {
    pub title: &'a str,
    pub choices: &'a [&'a str],
    pub placeholder: &'a str,
    pub width: Option<u16>,
    pub style: MultiChoiceStyle,
    pub keymap: MultiChoiceKeymap,
}

impl<'a> MultiChoice<'a> {
    pub fn new(title: &'a str, choices: &'a [&'a str]) -> Self {
        Self {
            title,
            choices,
            placeholder: "",
            width: None,
            style: MultiChoiceStyle::default(),
            keymap: MultiChoiceKeymap::default(),
        }
    }

    pub fn placeholder(mut self, text: &'a str) -> Self {
        self.placeholder = text;
        self
    }

    pub fn width(mut self, w: u16) -> Self {
        self.width = Some(w);
        self
    }

    pub fn style(mut self, s: MultiChoiceStyle) -> Self {
        self.style = s;
        self
    }

    pub fn keymap(mut self, km: MultiChoiceKeymap) -> Self {
        self.keymap = km;
        self
    }

    /// Renders the widget into `buf` at the position given by `area`.
    ///
    /// **Closed state** writes two rows: title + summary line.
    ///
    /// **Open state** writes `1 + choices.len() + 1` rows starting at
    /// `area.y`.  When expanded the widget may write *below* `area.bottom()`
    /// to overlap adjacent widgets — this is intentional (drop-down overlay).
    /// The width used is `self.width` if set, otherwise `area.width`.
    pub fn render_with_state(self, area: Rect, buf: &mut Buffer, state: &MultiChoiceState) {
        let total_width = self.width.unwrap_or(area.width);
        let text_width = total_width.saturating_sub(PREFIX_LEN) as usize;
        let x = area.x;
        let y = area.y;

        // Row 0: title
        let title_style = self.style.resolved_style(MultiChoiceStyleType::Title);
        render_prefixed_line(
            buf,
            x,
            y,
            total_width,
            self.title,
            text_width,
            &self.style.prefix_color,
            &title_style,
            false,
        );

        if state.open {
            // Expanded: one row per choice.
            for (i, &choice) in self.choices.iter().enumerate() {
                let row = y + 1 + i as u16;
                let is_selected = state.selected.get(i).copied().unwrap_or(false);
                let is_cursor = i == state.cursor;

                let style_type = match (is_selected, is_cursor) {
                    (false, false) => MultiChoiceStyleType::Normal,
                    (false, true) => MultiChoiceStyleType::Active,
                    (true, false) => MultiChoiceStyleType::Selected,
                    (true, true) => MultiChoiceStyleType::SelectedActive,
                };

                let row_style = self.style.resolved_style(style_type);
                render_prefixed_line(
                    buf,
                    x,
                    row,
                    total_width,
                    choice,
                    text_width,
                    &self.style.prefix_color,
                    &row_style,
                    is_cursor,
                );
            }

            // Blank closing line.
            let closing_row = y + 1 + self.choices.len() as u16;
            let closing_style = self.style.resolved_style(MultiChoiceStyleType::LastLine);
            render_empty_line(buf, x, closing_row, total_width, closing_style);
        } else {
            // Collapsed: show a summary of selected items.
            let summary = self.build_summary(state);
            let (summary_text, summary_style) = if summary.is_empty() {
                (
                    self.placeholder.to_string(),
                    self.style.resolved_style(MultiChoiceStyleType::Normal),
                )
            } else {
                (
                    summary,
                    self.style.resolved_style(MultiChoiceStyleType::SelectedActive),
                )
            };

            render_prefixed_line(
                buf,
                x,
                y + 1,
                total_width,
                &summary_text,
                text_width,
                &self.style.prefix_color,
                &summary_style,
                false,
            );
        }
    }

    /// Returns the number of buffer rows the widget occupies in its current state.
    pub fn required_height(&self, state: &MultiChoiceState) -> u16 {
        if state.open {
            1 + self.choices.len() as u16 + 1
        } else {
            2
        }
    }

    fn build_summary(&self, state: &MultiChoiceState) -> String {
        self.choices
            .iter()
            .enumerate()
            .filter_map(|(i, &c)| {
                if state.selected.get(i).copied().unwrap_or(false) {
                    Some(c)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl Widget for MultiChoice<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let state = MultiChoiceState::new(self.choices.len());
        self.render_with_state(area, buf, &state);
    }
}

/// Renders a blank line with a background colour — the closing line in expanded mode.
fn render_empty_line(buf: &mut Buffer, x: u16, y: u16, total_width: u16, style: Style) {
    for dx in 0..total_width {
        if let Some(cell) = buf.cell_mut((x + dx, y)) {
            cell.set_char(' ');
            cell.set_style(style);
        }
    }
}
