use ratatui::{Frame, layout::Rect};
use tuirealm::command::{Cmd, CmdResult};
use tuirealm::component::Component;
use tuirealm::props::{AttrValue, Attribute, QueryResult};
use tuirealm::state::{State, StateValue};

use super::{
    Toggle,
    render::{ToggleViewData, render},
};

impl Component for Toggle {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let style = if self.focused {
            &self.active_style
        } else {
            &self.inactive_style
        };
        let data = ToggleViewData {
            title: &self.title,
            on: self.on,
            on_label: &self.on_label,
            off_label: &self.off_label,
            style,
        };
        render(frame, area, &data);
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        match attr {
            Attribute::Focus => Some(QueryResult::Owned(AttrValue::Flag(self.focused))),
            Attribute::Value => Some(QueryResult::Owned(AttrValue::Flag(self.on))),
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
                if let AttrValue::Flag(b) = value {
                    self.on = b;
                }
            }
            _ => {}
        }
    }

    fn state(&self) -> State {
        State::Single(StateValue::Bool(self.on))
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        match cmd {
            Cmd::Toggle => {
                self.on = !self.on;
                CmdResult::Changed(self.state())
            }
            Cmd::Submit => CmdResult::Submit(self.state()),
            _ => CmdResult::NoChange,
        }
    }
}
