use ratatui::{Frame, layout::Rect};
use tuirealm::command::{Cmd, CmdResult};
use tuirealm::component::{AppComponent, Component};
use tuirealm::event::{Event, KeyEvent, NoUserEvent};
use tuirealm::props::{AttrValue, Attribute, QueryResult};
use tuirealm::state::State;

use super::{Grid, GridChild, GridEvent, render::render};

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

impl Component for Grid {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        render(frame, area, self);
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        match attr {
            Attribute::Focus => Some(QueryResult::Owned(AttrValue::Flag(self.focused))),
            _ => None,
        }
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        if let Attribute::Focus = attr {
            if let AttrValue::Flag(f) = value {
                self.focused = f;
                self.update_child_focus();
            }
        }
    }

    fn state(&self) -> State {
        // Grid doesn't expose a meaningful State value; callers use focused_cell() directly.
        State::None
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        // Forward the command to the focused child widget.
        let focused = self.focus_cell;
        let idx = focused.0 * self.cols + focused.1;
        if let Some(Some(child)) = self.children.get_mut(idx) {
            child.perform(cmd)
        } else {
            CmdResult::NoChange
        }
    }
}

// ---------------------------------------------------------------------------
// AppComponent<GridEvent, NoUserEvent>
// ---------------------------------------------------------------------------

impl AppComponent<GridEvent, NoUserEvent> for Grid {
    fn on(&mut self, ev: &Event<NoUserEvent>) -> Option<GridEvent> {
        let Event::Keyboard(key) = ev else {
            return None;
        };
        let key = *key;
        let old_focus = self.focus_cell;
        self.handle_key(key);
        if self.focus_cell != old_focus {
            let (row, col) = self.focus_cell;
            Some(GridEvent::FocusChanged { row, col })
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// GridChild for Grid (enables nesting)
// ---------------------------------------------------------------------------

impl GridChild for Grid {
    fn on_key(&mut self, key: KeyEvent) -> bool {
        self.handle_key(key)
    }
}

// ---------------------------------------------------------------------------
// GridChild implementations for existing widgets
// ---------------------------------------------------------------------------

impl GridChild for crate::widgets::text_input::TextInput {
    fn on_key(&mut self, key: KeyEvent) -> bool {
        use tuirealm::component::AppComponent;
        self.on(&Event::Keyboard(key)).is_some()
    }
}

impl GridChild for crate::widgets::multi_choice::MultiChoice {
    fn on_key(&mut self, key: KeyEvent) -> bool {
        use tuirealm::component::AppComponent;
        self.on(&Event::Keyboard(key)).is_some()
    }
}

// ---------------------------------------------------------------------------
// Internal: key dispatch
// ---------------------------------------------------------------------------

impl Grid {
    /// Forwards `key` to the focused child, then checks the grid's own keymap.
    ///
    /// Returns `true` when the key was consumed by any party (child or navigation).
    pub(super) fn handle_key(&mut self, key: KeyEvent) -> bool {
        // 1. Forward to focused child; if it consumed the key, we're done.
        let focused = self.focus_cell;
        let idx = focused.0 * self.cols + focused.1;
        if let Some(Some(child)) = self.children.get_mut(idx) {
            if child.on_key(key) {
                return true; // consumed by child
            }
        }

        // 2. Check own keymap.
        let km = self.keymap.clone();

        if km.next_cell.as_ref() == Some(&key) {
            self.focus_next();
            return true;
        }
        if km.prev_cell.as_ref() == Some(&key) {
            self.focus_prev();
            return true;
        }
        if km.next_in_row.as_ref() == Some(&key) {
            self.focus_next_in_row();
            return true;
        }
        if km.prev_in_row.as_ref() == Some(&key) {
            self.focus_prev_in_row();
            return true;
        }
        if km.next_in_col.as_ref() == Some(&key) {
            self.focus_next_in_col();
            return true;
        }
        if km.prev_in_col.as_ref() == Some(&key) {
            self.focus_prev_in_col();
            return true;
        }

        false
    }
}
