pub mod field;
pub mod keymap;
pub mod state;
pub mod style;

pub use field::{FormField, FormFieldWidget};
pub use keymap::FormKeymap;
pub use state::{FieldEvent, FormEvent, FormFieldState, FormState};
pub use style::{FormStyle, FormWidgetStyle};

use crate::widgets::common::KeyBinding;
use crate::widgets::form::style::{merge_multi_choice, merge_text_input};
use crate::widgets::multi_choice::MultiChoiceEvent;
use crossterm::event::{Event, KeyEvent, KeyEventKind};
use ratatui::{buffer::Buffer, layout::Rect};

// ── Form widget ───────────────────────────────────────────────────────────────

/// A form widget that manages layout, focus, style inheritance, and render order
/// for a list of named fields.
///
/// # Render order
///
/// All inactive fields are rendered first, then the active field is rendered
/// last — even though it occupies its original position in the layout.  This
/// guarantees that an expanded `MultiChoice` drop-down always appears on top of
/// any fields below it.
///
/// # Style inheritance
///
/// `FormStyle` defines default active / inactive styles.  These are merged into
/// each widget's own `*Style` struct before rendering, but only for slots that
/// the widget has not configured itself (`None` slots).  Per-widget styles
/// always win.
#[derive(Debug, Clone)]
pub struct Form<'a> {
    pub fields: Vec<FormField<'a>>,
    pub style: FormStyle,
    /// Blank rows inserted between each pair of adjacent fields.
    pub spacing: u16,
    pub keymap: FormKeymap,
}

impl<'a> Form<'a> {
    pub fn new() -> Self {
        Self {
            fields: Vec::new(),
            style: FormStyle::default(),
            spacing: 1,
            keymap: FormKeymap::default(),
        }
    }

    pub fn style(mut self, s: FormStyle) -> Self {
        self.style = s;
        self
    }

    /// Blank rows between fields (default: 1).
    pub fn spacing(mut self, s: u16) -> Self {
        self.spacing = s;
        self
    }

    pub fn keymap(mut self, k: FormKeymap) -> Self {
        self.keymap = k;
        self
    }

    pub fn field(mut self, f: FormField<'a>) -> Self {
        self.fields.push(f);
        self
    }
}

impl Default for Form<'_> {
    fn default() -> Self {
        Self::new()
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

impl<'a> Form<'a> {
    /// Renders all fields into `buf` within `area`.
    ///
    /// Inactive fields are drawn first; the active field is drawn last so that
    /// any overlay (e.g. an open `MultiChoice` drop-down) covers adjacent rows.
    pub fn render_with_state(&self, area: Rect, buf: &mut Buffer, state: &FormState) {
        if let Some(bg) = self.style.background {
            for row in area.y..area.y.saturating_add(area.height) {
                for col in area.x..area.x.saturating_add(area.width) {
                    if let Some(cell) = buf.cell_mut((col, row)) {
                        cell.set_bg(bg);
                    }
                }
            }
        }

        let rects = self.compute_rects(area);

        // 1. All inactive fields first.
        for (i, (field, rect)) in self.fields.iter().zip(rects.iter()).enumerate() {
            if i == state.active {
                continue;
            }
            if let Some(field_state) = state.fields.get(i) {
                self.render_field(field, *rect, buf, field_state, false);
            }
        }

        // 2. Active field last — its overlay goes on top.
        if let (Some(field), Some(rect)) =
            (self.fields.get(state.active), rects.get(state.active))
        {
            if let Some(field_state) = state.fields.get(state.active) {
                self.render_field(field, *rect, buf, field_state, true);
            }
        }
    }

    /// Returns the terminal cursor position for the active field if it is a
    /// `TextInput` with a non-empty value.
    pub fn cursor_position(&self, area: Rect, state: &FormState) -> Option<(u16, u16)> {
        let rects = self.compute_rects(area);
        let field = self.fields.get(state.active)?;
        let rect = rects.get(state.active)?;
        let field_state = state.fields.get(state.active)?;

        match (&field.widget, field_state) {
            (FormFieldWidget::TextInput(ti), FormFieldState::TextInput(ts))
                if !ts.value().is_empty() =>
            {
                Some(ti.cursor_position(*rect, ts))
            }
            _ => None,
        }
    }

    fn compute_rects(&self, area: Rect) -> Vec<Rect> {
        let mut rects = Vec::with_capacity(self.fields.len());
        let mut y = area.y;
        for (i, field) in self.fields.iter().enumerate() {
            rects.push(Rect::new(area.x, y, area.width, field.height));
            y = y.saturating_add(field.height);
            if i + 1 < self.fields.len() {
                y = y.saturating_add(self.spacing);
            }
        }
        rects
    }

    fn render_field(
        &self,
        field: &FormField,
        rect: Rect,
        buf: &mut Buffer,
        field_state: &FormFieldState,
        is_active: bool,
    ) {
        let form_slot = if is_active {
            &self.style.active
        } else {
            &self.style.inactive
        };

        match (&field.widget, field_state) {
            (FormFieldWidget::TextInput(ti), FormFieldState::TextInput(ts)) => {
                let merged = crate::widgets::text_input::TextInput {
                    style: merge_text_input(ti.style.clone(), form_slot),
                    ..ti.clone()
                };
                merged.render_with_state(rect, buf, ts);
            }
            (FormFieldWidget::MultiChoice(mc), FormFieldState::MultiChoice(ms)) => {
                let merged = crate::widgets::multi_choice::MultiChoice {
                    style: merge_multi_choice(mc.style.clone(), form_slot),
                    ..mc.clone()
                };
                merged.render_with_state(rect, buf, ms);
            }
            _ => {} // widget / state type mismatch — skip silently
        }
    }
}

// ── Event handling ────────────────────────────────────────────────────────────

impl Form<'_> {
    /// Processes a terminal event at the form level.
    ///
    /// Handling priority:
    /// 1. `Tab` / `Shift+Tab` → advance / retreat focus → `FormEvent::FocusChanged`
    /// 2. `Enter` on a `TextInput` → `FormEvent::Submit`
    /// 3. `Enter` on a `MultiChoice` → toggle the drop-down open / closed
    /// 4. Everything else → delegated to the active widget's `handle_event`
    ///
    /// Only `KeyEventKind::Press` events are consumed.
    pub fn handle_event(&self, event: &Event, state: &mut FormState) -> FormEvent {
        let Event::Key(KeyEvent {
            code,
            modifiers,
            kind,
            ..
        }) = event
        else {
            return FormEvent::Ignored;
        };

        if *kind != KeyEventKind::Press {
            return FormEvent::Ignored;
        }

        let pressed = KeyBinding {
            code: *code,
            modifiers: *modifiers,
        };

        // Form-level navigation.
        if pressed == self.keymap.focus_next {
            let from = state.active;
            state.focus_next();
            return FormEvent::FocusChanged {
                from,
                to: state.active,
            };
        }
        if pressed == self.keymap.focus_prev {
            let from = state.active;
            state.focus_prev();
            return FormEvent::FocusChanged {
                from,
                to: state.active,
            };
        }

        // Delegate to active field.
        let active = state.active;
        let Some(field) = self.fields.get(active) else {
            return FormEvent::Ignored;
        };
        let Some(field_state) = state.fields.get_mut(active) else {
            return FormEvent::Ignored;
        };

        match (&field.widget, field_state) {
            (FormFieldWidget::TextInput(ti), FormFieldState::TextInput(ts)) => {
                if pressed == self.keymap.confirm {
                    return FormEvent::Submit;
                }
                let ev = ts.handle_event(event, &ti.keymap);
                FormEvent::FieldEvent {
                    index: active,
                    event: FieldEvent::TextInput(ev),
                }
            }

            (FormFieldWidget::MultiChoice(mc_widget), FormFieldState::MultiChoice(mc_state)) => {
                // Enter toggles the drop-down instead of submitting.
                if pressed == self.keymap.confirm {
                    if mc_state.open {
                        mc_state.close();
                        return FormEvent::FieldEvent {
                            index: active,
                            event: FieldEvent::MultiChoice(MultiChoiceEvent::Closed),
                        };
                    } else {
                        mc_state.open();
                        return FormEvent::FieldEvent {
                            index: active,
                            event: FieldEvent::MultiChoice(MultiChoiceEvent::Opened),
                        };
                    }
                }
                let ev = mc_state.handle_event(event, &mc_widget.keymap);
                FormEvent::FieldEvent {
                    index: active,
                    event: FieldEvent::MultiChoice(ev),
                }
            }

            _ => FormEvent::Ignored,
        }
    }
}
