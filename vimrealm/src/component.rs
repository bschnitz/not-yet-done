//! tuirealm plumbing.
//!
//! The interesting half of a modal editor is that it must see *raw keys*: a
//! keymap that pre-translated `d` into `Cmd::Delete` would lose the pending
//! operator. So [`AppComponent::on`] feeds keys straight into
//! [`VimEditor::on_key`], and [`Component::perform`] stays a thin courtesy for
//! hosts that drive components by command — it can move the cursor and submit,
//! nothing modal.

use ratatui::Frame;
use ratatui::layout::Rect;
use tuirealm::command::{Cmd, CmdResult, Direction};
use tuirealm::component::{AppComponent, Component};
use tuirealm::event::{Event, NoUserEvent};
use tuirealm::props::{AttrValue, Attribute, QueryResult};
use tuirealm::state::{State, StateValue};

use crate::editor::VimEditor;
use crate::mode::Mode;
use crate::motion::Motion;
use crate::render;
use crate::state::VimEvent;

/// [`Attribute::Custom`] key for toggling the line-number gutter.
///
/// ```rust
/// use tuirealm::component::Component;
/// use tuirealm::props::{AttrValue, Attribute};
/// use vimrealm::{VimEditor, component::ATTR_LINE_NUMBERS};
///
/// let mut editor = VimEditor::default();
/// editor.attr(Attribute::Custom(ATTR_LINE_NUMBERS), AttrValue::Flag(true));
/// ```
pub const ATTR_LINE_NUMBERS: &str = "line_numbers";

/// [`Attribute::Custom`] key for the mode name, queried as a string.
pub const ATTR_MODE: &str = "mode";

impl Component for VimEditor {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        render::render(self, frame, area);
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        match attr {
            Attribute::Focus => Some(QueryResult::Owned(AttrValue::Flag(self.focused))),
            Attribute::Value | Attribute::Content => {
                Some(QueryResult::Owned(AttrValue::String(self.text())))
            }
            Attribute::Title => Some(QueryResult::Owned(AttrValue::String(self.title.clone()))),
            Attribute::Custom(key) if key == ATTR_LINE_NUMBERS => {
                Some(QueryResult::Owned(AttrValue::Flag(self.line_numbers)))
            }
            Attribute::Custom(key) if key == ATTR_MODE => Some(QueryResult::Owned(
                AttrValue::String(self.mode().label().to_string()),
            )),
            _ => None,
        }
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match (attr, value) {
            (Attribute::Focus, AttrValue::Flag(f)) => self.focused = f,
            (Attribute::Value | Attribute::Content, AttrValue::String(s)) => self.set_text(&s),
            (Attribute::Title, AttrValue::String(s)) => self.title = s,
            (Attribute::Custom(ATTR_LINE_NUMBERS), AttrValue::Flag(f)) => self.line_numbers = f,
            _ => {}
        }
    }

    fn state(&self) -> State {
        State::Single(StateValue::String(self.text()))
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        let motion = match cmd {
            Cmd::Move(Direction::Left) => Motion::Left,
            Cmd::Move(Direction::Right) => Motion::Right,
            Cmd::Move(Direction::Up) => Motion::Up,
            Cmd::Move(Direction::Down) => Motion::Down,
            Cmd::Submit => return CmdResult::Submit(self.state()),
            other => return CmdResult::Invalid(other),
        };
        self.move_cursor(motion);
        CmdResult::Visual
    }
}

impl AppComponent<VimEvent, NoUserEvent> for VimEditor {
    fn on(&mut self, ev: &Event<NoUserEvent>) -> Option<VimEvent> {
        match ev {
            Event::Keyboard(key) => self.on_key(*key),
            // A bracketed paste arrives as one event; inserting it character by
            // character would be O(n) undo steps, so it goes in as one edit.
            Event::Paste(text) if self.mode() == Mode::Insert => self.insert_text(text),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tuirealm::event::Key;

    use super::*;

    fn draw(editor: &mut VimEditor, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| editor.view(frame, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_buffer_text_is_drawn() {
        let mut editor = VimEditor::default()
            .with_text("hello\nworld")
            .with_status_line(false);
        let out = draw(&mut editor, 12, 3);
        assert!(out.starts_with("hello"), "got:\n{out}");
        assert!(out.contains("world"), "got:\n{out}");
    }

    #[test]
    fn a_long_line_is_soft_wrapped_into_the_area() {
        let mut editor = VimEditor::default()
            .with_text("abcdefgh")
            .with_status_line(false);
        let out = draw(&mut editor, 4, 3);
        assert_eq!(out, "abcd\nefgh\n");
    }

    #[test]
    fn insert_mode_shows_up_on_the_status_line() {
        let mut editor = VimEditor::default().with_text("x");
        editor.on_key(Key::Char('i').into());
        let out = draw(&mut editor, 20, 3);
        assert!(out.contains("-- INSERT --"), "got:\n{out}");
    }

    #[test]
    fn the_command_line_replaces_the_mode_indicator() {
        let mut editor = VimEditor::default().with_text("x");
        editor.on_key(Key::Char(':').into());
        editor.on_key(Key::Char('w').into());
        let out = draw(&mut editor, 20, 3);
        assert!(out.contains(":w"), "got:\n{out}");
    }

    #[test]
    fn line_numbers_are_off_by_default_and_can_be_switched_on() {
        let mut editor = VimEditor::default()
            .with_text("a\nb")
            .with_status_line(false);
        assert!(!draw(&mut editor, 8, 2).contains('1'));
        editor.attr(Attribute::Custom(ATTR_LINE_NUMBERS), AttrValue::Flag(true));
        let out = draw(&mut editor, 8, 2);
        assert!(out.starts_with("1 a"), "got:\n{out}");
    }

    #[test]
    fn a_title_draws_a_block_around_the_text() {
        let mut editor = VimEditor::default()
            .with_text("hi")
            .with_title("Message")
            .with_status_line(false);
        let out = draw(&mut editor, 14, 3);
        assert!(out.contains("Message"), "got:\n{out}");
        assert!(out.contains("hi"), "got:\n{out}");
    }

    #[test]
    fn the_view_survives_an_area_too_small_to_draw_in() {
        let mut editor = VimEditor::default().with_text("hello");
        draw(&mut editor, 1, 1);
        draw(&mut editor, 0, 0);
    }

    #[test]
    fn keys_reach_the_editor_through_the_app_component() {
        let mut editor = VimEditor::default().with_text("foo bar");
        let event = Event::<NoUserEvent>::Keyboard(Key::Char('d').into());
        assert_eq!(editor.on(&event), None, "an operator alone is not an event");
        let event = Event::<NoUserEvent>::Keyboard(Key::Char('w').into());
        assert_eq!(editor.on(&event), Some(VimEvent::Changed));
        assert_eq!(editor.text(), "bar");
    }

    #[test]
    fn a_paste_is_inserted_in_insert_mode_only() {
        let mut editor = VimEditor::default().with_text("");
        let paste = Event::<NoUserEvent>::Paste("ab\ncd".into());
        assert_eq!(editor.on(&paste), None, "normal mode ignores a paste");
        editor.on_key(Key::Char('i').into());
        assert_eq!(editor.on(&paste), Some(VimEvent::Changed));
        assert_eq!(editor.text(), "ab\ncd");
        editor.on_key(Key::Esc.into());
        editor.on_key(Key::Char('u').into());
        assert_eq!(editor.text(), "", "the whole paste is one undo step");
    }

    #[test]
    fn the_state_and_value_attribute_carry_the_text() {
        let mut editor = VimEditor::default();
        editor.attr(Attribute::Value, AttrValue::String("typed".into()));
        assert_eq!(
            editor.state(),
            State::Single(StateValue::String("typed".into()))
        );
        let queried = editor.query(Attribute::Value).expect("value is queryable");
        assert_eq!(queried.as_ref().unwrap_string(), "typed");
    }

    #[test]
    fn perform_moves_the_cursor_but_stays_out_of_the_modal_grammar() {
        let mut editor = VimEditor::default().with_text("abc");
        assert_eq!(
            editor.perform(Cmd::Move(Direction::Right)),
            CmdResult::Visual
        );
        assert_eq!(editor.buffer().cursor().col, 1);
        assert!(matches!(editor.perform(Cmd::Delete), CmdResult::Invalid(_)));
    }
}
