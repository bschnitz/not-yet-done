use ratatui::{Frame, layout::Rect};
use tuirealm::{
    AttrValue, Attribute, MockComponent, State, StateValue,
    command::{Cmd, CmdResult, Direction},
    event::{Event, NoUserEvent},
    props::{PropPayload, PropValue},
};

use super::{
    MultiChoiceKeymap,
    render::{MultiChoiceViewData, render},
    state::MultiChoiceEvent,
    style::MultiChoiceStyle,
};

/// [`Attribute::Custom`] key for the selected indices slot.
///
/// ```rust
/// // Get selected indices:
/// if let Some(AttrValue::Payload(PropPayload::Vec(values))) =
///     component.query(Attribute::Custom(ATTR_SELECTED))
/// {
///     let indices: Vec<usize> = values
///         .iter()
///         .filter_map(|v| if let PropValue::Usize(i) = v { Some(*i) } else { None })
///         .collect();
/// }
/// // Set selected indices:
/// component.attr(
///     Attribute::Custom(ATTR_SELECTED),
///     AttrValue::Payload(PropPayload::Vec(vec![PropValue::Usize(0), PropValue::Usize(2)])),
/// );
/// ```
pub const ATTR_SELECTED: &str = "selected";

/// A multiple‑choice dropdown widget implementing tuirealm's [`MockComponent`] and
/// [`tuirealm::Component<MultiChoiceEvent, NoUserEvent>`].
///
/// All state is owned by the component.  Construct once and mount into a
/// tuirealm [`Application`](tuirealm::Application); do not rebuild per frame.
///
/// ```rust
/// let choices = vec!["Option 1", "Option 2", "Option 3"];
/// let multi = MultiChoice::default()
///     .with_title("Tags")
///     .with_choices(choices)
///     .with_placeholder("none selected")
///     .with_style(style)
///     .with_keymap(keymap);
///
/// app.mount(Id::Tags, Box::new(multi), vec![])?;
/// ```
pub struct MultiChoice {
    // --- framework state ---
    focused: bool,

    // --- internal state ---
    open: bool,
    cursor: usize,
    selected: Vec<bool>, // length == choices.len()

    // --- configuration (set once at construction) ---
    title: String,
    choices: Vec<String>,
    placeholder: String,
    width: Option<u16>,
    inactive_style: MultiChoiceStyle,
    active_style: MultiChoiceStyle,
    keymap: MultiChoiceKeymap,
}

impl Default for MultiChoice {
    fn default() -> Self {
        Self {
            focused: false,
            open: false,
            cursor: 0,
            selected: Vec::new(),
            title: String::new(),
            choices: Vec::new(),
            placeholder: String::new(),
            width: None,
            keymap: MultiChoiceKeymap::default(),
            inactive_style: MultiChoiceStyle::default(),
            active_style: MultiChoiceStyle::default(),
        }
    }
}

impl MultiChoice {
    /// Sets the title displayed above the dropdown.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Sets the list of choices (must be non‑empty for meaningful interaction).
    pub fn with_choices(mut self, choices: Vec<impl Into<String>>) -> Self {
        self.choices = choices.into_iter().map(|c| c.into()).collect();
        self.selected = vec![false; self.choices.len()];
        self.cursor = 0;
        self
    }

    /// Sets the placeholder shown when no items are selected (collapsed state).
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Overrides the width of the widget (defaults to the area width).
    pub fn with_width(mut self, width: u16) -> Self {
        self.width = Some(width);
        self
    }

    /// Overrides the default key bindings.
    pub fn with_keymap(mut self, keymap: MultiChoiceKeymap) -> Self {
        self.keymap = keymap;
        self
    }

    /// Applies the style used when the component is not focused.
    pub fn with_inactive_style(mut self, style: MultiChoiceStyle) -> Self {
        self.inactive_style = style;
        self
    }

    /// Applies the style used when the component is focused.
    pub fn with_active_style(mut self, style: MultiChoiceStyle) -> Self {
        self.active_style = style;
        self
    }

    // --- internal helpers ---

    fn selected_indices(&self) -> Vec<usize> {
        self.selected
            .iter()
            .enumerate()
            .filter_map(|(i, &s)| if s { Some(i) } else { None })
            .collect()
    }

    fn toggle_selection(&mut self) {
        if self.cursor < self.selected.len() {
            self.selected[self.cursor] = !self.selected[self.cursor];
        }
    }

    fn move_cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    fn move_cursor_down(&mut self) {
        if self.cursor + 1 < self.selected.len() {
            self.cursor += 1;
        }
    }
}

impl MockComponent for MultiChoice {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let style = if self.focused { &self.active_style } else { &self.inactive_style };
        let data = MultiChoiceViewData {
            title: &self.title,
            choices: &self.choices,
            selected: &self.selected,
            cursor: self.cursor,
            open: self.open,
            placeholder: &self.placeholder,
            width: self.width,
            style,
        };
        render(frame, area, &data);
    }

    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        match attr {
            Attribute::Focus => Some(AttrValue::Flag(self.focused)),
            Attribute::Custom(key) if key == ATTR_SELECTED => {
                Some(AttrValue::Payload(PropPayload::Vec(
                    self.selected_indices()
                        .into_iter()
                        .map(PropValue::Usize)
                        .collect(),
                )))
            }
            _ => None,
        }
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Focus => {
                if let AttrValue::Flag(f) = value {
                    self.focused = f;
                    self.open = f;
                }
            }
            Attribute::Custom(key) if key == ATTR_SELECTED => {
                if let AttrValue::Payload(PropPayload::Vec(values)) = value {
                    self.selected.fill(false);
                    for v in values {
                        if let PropValue::Usize(i) = v {
                            if i < self.selected.len() {
                                self.selected[i] = true;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn state(&self) -> State {
        State::Vec(
            self.selected_indices()
                .into_iter()
                .map(StateValue::Usize)
                .collect(),
        )
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        match cmd {
            Cmd::Move(Direction::Up) => {
                self.move_cursor_up();
                CmdResult::Changed(State::One(StateValue::Usize(self.cursor)))
            }
            Cmd::Move(Direction::Down) => {
                self.move_cursor_down();
                CmdResult::Changed(State::One(StateValue::Usize(self.cursor)))
            }
            Cmd::Toggle => {
                self.toggle_selection();
                CmdResult::Changed(self.state())
            }
            Cmd::Submit => {
                self.open = true;
                CmdResult::None
            }
            Cmd::Cancel => {
                self.open = false;
                CmdResult::None
            }
            _ => CmdResult::None,
        }
    }
}

impl tuirealm::Component<MultiChoiceEvent, NoUserEvent> for MultiChoice {
    fn on(&mut self, ev: Event<NoUserEvent>) -> Option<MultiChoiceEvent> {
        let Event::Keyboard(key_ev) = ev else {
            return None;
        };

        if !self.open {
            return None;
        }

        // Close key is checked before the general dispatch so it always works.
        if key_ev == self.keymap.close {
            self.open = false;
            return Some(MultiChoiceEvent::Closed);
        }

        let cmd = if key_ev == self.keymap.move_up {
            Cmd::Move(Direction::Up)
        } else if key_ev == self.keymap.move_down {
            Cmd::Move(Direction::Down)
        } else if key_ev == self.keymap.toggle {
            Cmd::Toggle
        } else {
            return None;
        };

        match self.perform(cmd) {
            CmdResult::Changed(State::Vec(values)) => {
                let indices = values
                    .into_iter()
                    .filter_map(|v| if let StateValue::Usize(i) = v { Some(i) } else { None })
                    .collect();
                Some(MultiChoiceEvent::SelectionChanged(indices))
            }
            CmdResult::Changed(State::One(StateValue::Usize(idx))) => {
                Some(MultiChoiceEvent::HighlightChanged(idx))
            }
            _ => None,
        }
    }
}
