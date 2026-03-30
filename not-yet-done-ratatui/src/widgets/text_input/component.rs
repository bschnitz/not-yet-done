use ratatui::{Frame, layout::Rect};
use tuirealm::{
    AttrValue, Attribute, CmdResult, MockComponent, State, StateValue,
    command::{Cmd, Direction},
    event::{Event, Key, KeyEvent, KeyModifiers, NoUserEvent},
};

use super::{
    TextInput,
    render::{TextInputViewData, render},
    state::TextInputEvent,
};

/// [`Attribute::Custom`] key for the error message slot.
///
/// ```rust
/// // Set an error:
/// component.attr(Attribute::Custom(ATTR_ERROR), AttrValue::String("Required".into()));
/// // Clear the error:
/// component.attr(Attribute::Custom(ATTR_ERROR), AttrValue::Flag(false));
/// ```
pub const ATTR_ERROR: &str = "error";

impl MockComponent for TextInput {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let style = if self.focused { &self.active_style } else { &self.inactive_style };
        let data = TextInputViewData {
            title:              &self.title,
            value:              &self.value,
            placeholder:        &self.placeholder,
            error:              self.error.as_deref(),
            cursor_byte_offset: self.cursor,
            focused:            self.focused,
            style,
        };
        render(frame, area, &data);
    }

    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        match attr {
            Attribute::Focus => Some(AttrValue::Flag(self.focused)),
            Attribute::Value => Some(AttrValue::String(self.value.clone())),
            Attribute::Custom(key) if key == ATTR_ERROR => Some(match &self.error {
                Some(e) => AttrValue::String(e.clone()),
                None    => AttrValue::Flag(false),
            }),
            _ => None,
        }
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Focus => {
                if let AttrValue::Flag(f) = value {
                    self.focused = f;
                }
            }
            Attribute::Value => {
                if let AttrValue::String(s) = value {
                    self.cursor = s.len();
                    self.value  = s;
                }
            }
            Attribute::Custom(key) if key == ATTR_ERROR => match value {
                AttrValue::String(msg)    => self.error = Some(msg),
                AttrValue::Flag(false)    => self.error = None,
                _                         => {}
            },
            _ => {}
        }
    }

    fn state(&self) -> State {
        State::One(StateValue::String(self.value.clone()))
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        match cmd {
            Cmd::Move(Direction::Left) => {
                self.move_cursor_left();
                CmdResult::None
            }
            Cmd::Move(Direction::Right) => {
                self.move_cursor_right();
                CmdResult::None
            }
            Cmd::Delete => {
                self.pop_char();
                CmdResult::Changed(State::One(StateValue::String(self.value.clone())))
            }
            Cmd::Custom("delete_fwd") => {
                self.delete_forward();
                CmdResult::Changed(State::One(StateValue::String(self.value.clone())))
            }
            Cmd::Custom("clear") => {
                self.clear_value();
                CmdResult::Changed(State::One(StateValue::String(self.value.clone())))
            }
            Cmd::Type(c) => {
                self.push_char(c);
                CmdResult::Changed(State::One(StateValue::String(self.value.clone())))
            }
            _ => CmdResult::None,
        }
    }
}

impl tuirealm::Component<TextInputEvent, NoUserEvent> for TextInput {
    fn on(&mut self, ev: Event<NoUserEvent>) -> Option<TextInputEvent> {
        let Event::Keyboard(key_ev) = ev else {
            return None;
        };

        let cmd = if key_ev == self.keymap.move_left {
            Cmd::Move(Direction::Left)
        } else if key_ev == self.keymap.move_right {
            Cmd::Move(Direction::Right)
        } else if key_ev == self.keymap.delete_back {
            Cmd::Delete
        } else if key_ev == self.keymap.delete_fwd {
            Cmd::Custom("delete_fwd")
        } else if key_ev == self.keymap.clear {
            Cmd::Custom("clear")
        } else {
            match key_ev {
                KeyEvent { code: Key::Char(c), modifiers: KeyModifiers::NONE }
                | KeyEvent { code: Key::Char(c), modifiers: KeyModifiers::SHIFT } => {
                    Cmd::Type(c)
                }
                _ => return None,
            }
        };

        match self.perform(cmd) {
            CmdResult::Changed(State::One(StateValue::String(s))) => {
                Some(TextInputEvent::Changed(s))
            }
            _ => None,
        }
    }
}
