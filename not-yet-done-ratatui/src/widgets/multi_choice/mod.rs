pub mod keymap;
pub mod state;
pub mod style;

pub use keymap::MultiChoiceKeymap;
pub use state::{MultiChoiceEvent, MultiChoiceState};
pub use style::MultiChoiceStyle;

use crate::widgets::common::{render_prefixed_line, LineStyle, PREFIX_LEN};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::Widget,
};

/// Ein Multiple-Choice-Widget mit aufklappbarem Menü.
///
/// # Collapsed (nicht aktiv / `state.open == false`)
/// ```text
/// ▍ Titel
/// ▍ Option 1, Option 2
/// ```
///
/// # Expanded (aktiv / `state.open == true`)
/// ```text
/// ▍ Titel
/// ▍ Option 1
/// ▍ Option 2
/// ▍ Option 3
///             ← Leerzeile
/// ```
///
/// Das Widget **überdeckt** darunterliegende Inhalte im Buffer, wenn es
/// aufgeklappt ist. Der Aufrufer muss dafür sorgen, dass das `area`-Rect
/// groß genug ist (mindestens `1 + item_count + 1` Zeilen im Open-Zustand).
#[derive(Debug, Clone)]
pub struct MultiChoice<'a> {
    pub title:      &'a str,
    pub choices:    &'a [&'a str],
    pub placeholder: &'a str,
    pub width:      Option<u16>,
    pub style:      MultiChoiceStyle,
    pub keymap:     MultiChoiceKeymap,
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

    // -----------------------------------------------------------------------
    // Render
    // -----------------------------------------------------------------------

    /// Rendert das Widget unter Berücksichtigung des aktuellen `state`.
    ///
    /// Im *closed*-Zustand werden Zeile 0 (Titel) + Zeile 1 (Zusammenfassung)
    /// beschrieben.
    ///
    /// Im *open*-Zustand werden Zeile 0 (Titel) + N Zeilen (je eine Choice) +
    /// eine leere Abschlusszeile beschrieben.
    pub fn render_with_state(self, area: Rect, buf: &mut Buffer, state: &MultiChoiceState) {
        let total_width = self.width.unwrap_or(area.width);
        let text_width = total_width.saturating_sub(PREFIX_LEN) as usize;

        // Zeile 0: Titel
        render_prefixed_line(
            buf,
            area.x, area.y,
            total_width,
            self.title,
            text_width,
            &self.style.prefix_color,
            &self.style.title_style,
            false,
        );

        if state.open {
            // --- Expanded: eine Zeile pro Choice ---
            for (i, &choice) in self.choices.iter().enumerate() {
                let row = area.y + 1 + i as u16;
                if row >= area.y + area.height {
                    break;
                }

                let is_selected  = state.selected.get(i).copied().unwrap_or(false);
                let is_cursor    = i == state.cursor;
                let line_style   = if is_selected {
                    &self.style.item_selected_style
                } else {
                    &self.style.item_style
                };

                render_prefixed_line(
                    buf,
                    area.x, row,
                    total_width,
                    choice,
                    text_width,
                    &self.style.prefix_color,
                    line_style,
                    is_cursor,
                );
            }

            // Abschluss-Leerzeile
            let empty_row = area.y + 1 + self.choices.len() as u16;
            if empty_row < area.y + area.height {
                render_empty_line(buf, area.x, empty_row, total_width, &self.style.item_style);
            }
        } else {
            // --- Collapsed: Zusammenfassung der gewählten Einträge ---
            let summary = self.build_summary(state);
            let (summary_text, summary_style) = if summary.is_empty() {
                (
                    self.placeholder.to_string(),
                    // Placeholder erhält gedimmte Farbe (fg der item_style, leicht gedimmt)
                    // – wir delegieren an item_style, der Aufrufer kann es überschreiben
                    &self.style.item_style,
                )
            } else {
                (summary, &self.style.item_selected_style)
            };

            render_prefixed_line(
                buf,
                area.x, area.y + 1,
                total_width,
                &summary_text,
                text_width,
                &self.style.prefix_color,
                summary_style,
                false,
            );
        }
    }

    /// Gibt an, wie viele Buffer-Zeilen das Widget im aktuellen Zustand belegt.
    ///
    /// Nützlich, um `area.height` korrekt vorzureserieren.
    pub fn required_height(&self, state: &MultiChoiceState) -> u16 {
        if state.open {
            1 /* Titel */ + self.choices.len() as u16 + 1 /* Leerzeile */
        } else {
            2 /* Titel + Zusammenfassung */
        }
    }

    // -----------------------------------------------------------------------
    // Hilfsmethoden
    // -----------------------------------------------------------------------

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

/// Rendert eine leere Zeile mit Hintergrundfarbe (die Abschlusszeile im
/// expanded-Modus).
fn render_empty_line(
    buf: &mut Buffer,
    x: u16, y: u16,
    total_width: u16,
    line_style: &LineStyle,
) {
    if let Some(bg) = line_style.bg {
        for dx in 0..total_width {
            if let Some(cell) = buf.cell_mut((x + dx, y)) {
                cell.set_bg(bg);
                cell.set_char(' ');
            }
        }
    }
}
