use ratatui::{Frame, layout::Rect};
use tuirealm::command::{Cmd, CmdResult, Direction};
use tuirealm::component::{AppComponent, Component};
use tuirealm::event::{Event, Key, KeyEvent, KeyModifiers, NoUserEvent};
use tuirealm::props::{AttrValue, Attribute, QueryResult};
use tuirealm::state::{State, StateValue};

use super::{
    TextInput,
    render::{TextInputViewData, render},
    state::TextInputEvent,
};

/// [`Attribute::Custom`] key for the error message slot.
///
/// ```rust
/// use not_yet_done_ratatui::widgets::text_input::{TextInput, ATTR_ERROR};
/// use tuirealm::component::Component;
/// use tuirealm::props::{AttrValue, Attribute};
///
/// let mut component = TextInput::default();
/// // Set an error:
/// component.attr(Attribute::Custom(ATTR_ERROR), AttrValue::String("Required".into()));
/// // Clear the error:
/// component.attr(Attribute::Custom(ATTR_ERROR), AttrValue::Flag(false));
/// ```
pub const ATTR_ERROR: &str = "error";

const CMD_DELETE_FWD: &str = "delete_fwd";
const CMD_CLEAR: &str = "clear";

impl Component for TextInput {
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

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        match attr {
            Attribute::Focus => Some(QueryResult::Owned(AttrValue::Flag(self.focused))),
            Attribute::Value => Some(QueryResult::Owned(AttrValue::String(self.value.clone()))),
            Attribute::Custom(key) if key == ATTR_ERROR => Some(QueryResult::Owned(match &self.error {
                Some(e) => AttrValue::String(e.clone()),
                None    => AttrValue::Flag(false),
            })),
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
        State::Single(StateValue::String(self.value.clone()))
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        match cmd {
            Cmd::Move(Direction::Left) => {
                self.move_cursor_left();
                CmdResult::Changed(State::Single(StateValue::Usize(self.cursor)))
            }
            Cmd::Move(Direction::Right) => {
                self.move_cursor_right();
                CmdResult::Changed(State::Single(StateValue::Usize(self.cursor)))
            }
            Cmd::Delete => {
                self.pop_char();
                CmdResult::Changed(State::Single(StateValue::String(self.value.clone())))
            }
            Cmd::Custom(CMD_DELETE_FWD) => {
                self.delete_forward();
                CmdResult::Changed(State::Single(StateValue::String(self.value.clone())))
            }
            Cmd::Custom(CMD_CLEAR) => {
                self.clear_value();
                CmdResult::Changed(State::Single(StateValue::String(self.value.clone())))
            }
            Cmd::Submit => CmdResult::Submit(self.state()),
            Cmd::Type(c) => {
                self.push_char(c);
                CmdResult::Changed(State::Single(StateValue::String(self.value.clone())))
            }
            _ => CmdResult::NoChange,
        }
    }
}

impl AppComponent<TextInputEvent, NoUserEvent> for TextInput {
    fn on(&mut self, ev: &Event<NoUserEvent>) -> Option<TextInputEvent> {
        let Event::Keyboard(key_ev) = ev else {
            return None;
        };
        let key_ev = *key_ev;

        let cmd = if key_ev == self.keymap.move_left {
            Cmd::Move(Direction::Left)
        } else if key_ev == self.keymap.move_right {
            Cmd::Move(Direction::Right)
        } else if key_ev == self.keymap.delete_back {
            Cmd::Delete
        } else if key_ev == self.keymap.delete_fwd {
            Cmd::Custom(CMD_DELETE_FWD)
        } else if key_ev == self.keymap.clear {
            Cmd::Custom(CMD_CLEAR)
        } else if key_ev == self.keymap.submit {
            Cmd::Submit
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
            CmdResult::Changed(State::Single(StateValue::String(s))) => {
                Some(TextInputEvent::Changed(s))
            }
            CmdResult::Changed(State::Single(StateValue::Usize(pos))) => {
                Some(TextInputEvent::CursorMoved(pos))
            }
            CmdResult::Submit(State::Single(StateValue::String(s))) => {
                Some(TextInputEvent::Submitted(s))
            }
            _ => None,
        }
    }
}
