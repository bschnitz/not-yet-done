use ratatui::{Frame, layout::Rect};
use tuirealm::command::{Cmd, CmdResult, Direction};
use tuirealm::component::{AppComponent, Component};
use tuirealm::event::{Event, Key, KeyEvent, KeyModifiers, NoUserEvent};
use tuirealm::props::{AttrValue, Attribute, QueryResult};
use tuirealm::state::{State, StateValue};

use super::{
    TextArea,
    render::{TextAreaViewData, render},
    state::TextAreaEvent,
};

/// Splits the line at the cursor (what `enter` does).
pub const CMD_NEWLINE: &str = "newline";
/// Deletes the character under the cursor, joining the next line when the
/// cursor sits at the end of one.
pub const CMD_DELETE_FWD: &str = "delete_fwd";
/// Deletes from the cursor back to the start of the line.
pub const CMD_KILL_TO_START: &str = "kill_to_start";
/// Jump the cursor to the start / end of the current line.
pub const CMD_HOME: &str = "home";
pub const CMD_END: &str = "end";

impl Component for TextArea {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        // The configured row count is what the widget *asks* for; what it gets
        // decides how far the text has to scroll, so the window is trimmed to
        // the area before anything is drawn.
        self.ensure_visible(area.height.saturating_sub(1) as usize);
        let style = if self.focused {
            &self.active_style
        } else {
            &self.inactive_style
        };
        let data = TextAreaViewData {
            title: &self.title,
            lines: &self.lines,
            placeholder: &self.placeholder,
            row: self.row,
            col: self.col,
            scroll: self.scroll,
            focused: self.focused,
            style,
        };
        render(frame, area, &data);
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        match attr {
            Attribute::Focus => Some(QueryResult::Owned(AttrValue::Flag(self.focused))),
            Attribute::Value => Some(QueryResult::Owned(AttrValue::String(self.value()))),
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
                    self.set_value(&s);
                }
            }
            _ => {}
        }
    }

    fn state(&self) -> State {
        State::Single(StateValue::String(self.value()))
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        let moved = |a: &Self| {
            CmdResult::Changed(State::Pair((
                StateValue::Usize(a.row),
                StateValue::Usize(a.col),
            )))
        };
        match cmd {
            Cmd::Move(Direction::Left) => {
                self.move_left();
                moved(self)
            }
            Cmd::Move(Direction::Right) => {
                self.move_right();
                moved(self)
            }
            Cmd::Move(Direction::Up) => {
                self.move_up();
                moved(self)
            }
            Cmd::Move(Direction::Down) => {
                self.move_down();
                moved(self)
            }
            Cmd::Custom(CMD_HOME) => {
                self.move_home();
                moved(self)
            }
            Cmd::Custom(CMD_END) => {
                self.move_end();
                moved(self)
            }
            Cmd::Delete => {
                self.delete_back();
                CmdResult::Changed(State::Single(StateValue::String(self.value())))
            }
            Cmd::Custom(CMD_DELETE_FWD) => {
                self.delete_forward();
                CmdResult::Changed(State::Single(StateValue::String(self.value())))
            }
            Cmd::Custom(CMD_KILL_TO_START) => {
                self.kill_to_start();
                CmdResult::Changed(State::Single(StateValue::String(self.value())))
            }
            Cmd::Custom(CMD_NEWLINE) => {
                self.insert_newline();
                CmdResult::Changed(State::Single(StateValue::String(self.value())))
            }
            Cmd::Type(c) => {
                self.insert_char(c);
                CmdResult::Changed(State::Single(StateValue::String(self.value())))
            }
            _ => CmdResult::NoChange,
        }
    }
}

impl AppComponent<TextAreaEvent, NoUserEvent> for TextArea {
    fn on(&mut self, ev: &Event<NoUserEvent>) -> Option<TextAreaEvent> {
        let Event::Keyboard(key_ev) = ev else {
            return None;
        };
        let key_ev = *key_ev;

        let cmd = if key_ev == self.keymap.move_left {
            Cmd::Move(Direction::Left)
        } else if key_ev == self.keymap.move_right {
            Cmd::Move(Direction::Right)
        } else if key_ev == self.keymap.move_up {
            Cmd::Move(Direction::Up)
        } else if key_ev == self.keymap.move_down {
            Cmd::Move(Direction::Down)
        } else if key_ev == self.keymap.move_home {
            Cmd::Custom(CMD_HOME)
        } else if key_ev == self.keymap.move_end {
            Cmd::Custom(CMD_END)
        } else if key_ev == self.keymap.delete_back {
            Cmd::Delete
        } else if key_ev == self.keymap.delete_fwd {
            Cmd::Custom(CMD_DELETE_FWD)
        } else if key_ev == self.keymap.kill_to_start {
            Cmd::Custom(CMD_KILL_TO_START)
        } else if key_ev == self.keymap.newline {
            Cmd::Custom(CMD_NEWLINE)
        } else {
            match key_ev {
                KeyEvent {
                    code: Key::Char(c),
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char(c),
                    modifiers: KeyModifiers::SHIFT,
                } => Cmd::Type(c),
                _ => return None,
            }
        };

        match self.perform(cmd) {
            CmdResult::Changed(State::Single(StateValue::String(s))) => {
                Some(TextAreaEvent::Changed(s))
            }
            CmdResult::Changed(State::Pair((StateValue::Usize(row), StateValue::Usize(col)))) => {
                Some(TextAreaEvent::CursorMoved(row, col))
            }
            _ => None,
        }
    }
}
