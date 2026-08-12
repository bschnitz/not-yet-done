//! The editor itself: mode, pending input, and the key → action state machine.
//!
//! This module is free of any framework: it takes a [`KeyEvent`] and mutates
//! state. The tuirealm plumbing lives in [`crate::component`], the drawing in
//! [`crate::render`] — so the whole grammar is testable by feeding it keys.
//!
//! The state machine is split by mode, one submodule each, because each mode
//! reads differently: [`normal`] is vim's `{count}{operator}{motion}` grammar,
//! [`insert`] is "type a character, unless it is a key", [`command`] is a line
//! editor for `:` and `/`, [`visual`] moves one end of a selection.
//! [`pending`] holds the half-typed command they share.

mod command;
mod insert;
mod normal;
mod pending;
mod repeat;
mod search;
mod visual;

use tuirealm::event::KeyEvent;

use crate::buffer::{Buffer, Position};
use crate::keymap::Keymap;
use crate::mode::Mode;
use crate::motion::{self, Motion};
use crate::register::Registers;
use crate::state::VimEvent;
use crate::style::VimStyle;
use pending::Pending;

/// A modal, vim-like multi-line text editor.
///
/// Construct once, keep it, feed it keys. Mounted as a tuirealm component it
/// implements [`tuirealm::component::Component`] and
/// [`tuirealm::component::AppComponent<VimEvent, NoUserEvent>`].
///
/// ```rust
/// use tuirealm::event::Key;
/// use vimrealm::{VimEditor, VimEvent};
///
/// let mut editor = VimEditor::default().with_text("hello world");
/// editor.on_key(Key::Char('d').into());
/// editor.on_key(Key::Char('w').into());
/// assert_eq!(editor.text(), "world");
///
/// for key in [Key::Char(':'), Key::Char('w'), Key::Char('q'), Key::Enter] {
///     if let Some(event) = editor.on_key(key.into()) {
///         assert_eq!(event, VimEvent::SaveAndClose);
///     }
/// }
/// ```
pub struct VimEditor {
    // --- editing state ---
    buffer: Buffer,
    mode: Mode,
    registers: Registers,
    pending: Pending,
    /// The end of a visual selection the cursor left behind. Only meaningful
    /// while [`Self::mode`] is visual.
    visual_anchor: Position,
    /// Text typed after the [`Self::prompt`] character, without the prompt.
    command: String,
    /// The character that opened the command line: `:`, `/` or `?`.
    prompt: char,
    /// A one-line message for the status area (`E37: …`), cleared by the next
    /// key press, like vim's.
    message: Option<String>,
    /// The pattern of the last `/` or `?`, kept so `n` has something to repeat.
    last_search: Option<String>,
    /// Which way that search went — `n` follows it, `N` turns it around.
    search_forward: bool,
    /// Keys of the command being typed, kept until it turns out to be a change.
    recording: Vec<KeyEvent>,
    /// Keys of the last completed change — what `.` replays.
    last_change: Vec<KeyEvent>,
    /// Whether keys are coming from [`Self::repeat_change`] rather than a user.
    replaying: bool,

    // --- framework state ---
    pub(crate) focused: bool,
    /// First visible display row; maintained by the renderer, which is the only
    /// place that knows how the text wraps.
    pub(crate) scroll: usize,

    // --- configuration ---
    pub(crate) keymap: Keymap,
    pub(crate) style: VimStyle,
    pub(crate) title: String,
    pub(crate) line_numbers: bool,
    pub(crate) show_status: bool,
}

impl Default for VimEditor {
    fn default() -> Self {
        Self {
            buffer: Buffer::default(),
            mode: Mode::Normal,
            registers: Registers::new(),
            pending: Pending::default(),
            visual_anchor: Position::new(0, 0),
            command: String::new(),
            prompt: ':',
            message: None,
            last_search: None,
            search_forward: true,
            recording: Vec::new(),
            last_change: Vec::new(),
            replaying: false,
            focused: true,
            scroll: 0,
            keymap: Keymap::default(),
            style: VimStyle::default(),
            title: String::new(),
            line_numbers: false,
            show_status: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Construction and host API
// ---------------------------------------------------------------------------

impl VimEditor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_text(mut self, text: &str) -> Self {
        self.set_text(text);
        self
    }

    pub fn with_keymap(mut self, keymap: Keymap) -> Self {
        self.keymap = keymap;
        self
    }

    pub fn with_style(mut self, style: VimStyle) -> Self {
        self.style = style;
        self
    }

    /// Title for the surrounding block; empty means no block title.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn with_line_numbers(mut self, on: bool) -> Self {
        self.line_numbers = on;
        self
    }

    /// Whether to draw the mode/command line at the bottom of the area.
    pub fn with_status_line(mut self, on: bool) -> Self {
        self.show_status = on;
        self
    }

    /// Replace the content; resets cursor, undo history and mode. Registers
    /// survive, as they do in vim when you open another buffer.
    pub fn set_text(&mut self, text: &str) {
        self.buffer.set_text(text);
        self.mode = Mode::Normal;
        self.pending = Pending::default();
        self.command.clear();
        self.message = None;
        self.scroll = 0;
        self.recording.clear();
    }

    pub fn text(&self) -> String {
        self.buffer.text()
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The register file, for a host that wants to show or seed it.
    pub fn registers(&self) -> &Registers {
        &self.registers
    }

    pub fn registers_mut(&mut self) -> &mut Registers {
        &mut self.registers
    }

    pub fn is_dirty(&self) -> bool {
        self.buffer.is_dirty()
    }

    /// Tell the editor the host has persisted the text. `:w` already does this
    /// optimistically; call it again if your write was asynchronous.
    pub fn mark_clean(&mut self) {
        self.buffer.mark_clean();
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn set_message(&mut self, msg: impl Into<String>) {
        self.message = Some(msg.into());
    }

    /// The command line as it should be displayed, including its prompt
    /// character, or `None` outside command mode.
    pub fn command_line(&self) -> Option<String> {
        (self.mode == Mode::Command).then(|| format!("{}{}", self.prompt, self.command))
    }

    /// The incomplete command shown in the corner (`2d`), empty when idle.
    pub fn pending_label(&self) -> String {
        self.pending.label()
    }

    /// Move the cursor by one motion, bypassing the modal grammar. For hosts
    /// that drive the widget by command rather than by key press.
    pub fn move_cursor(&mut self, m: Motion) {
        match self.mode {
            Mode::Insert => {
                let target =
                    motion::resolve_bounded(&self.buffer, m, 1, false, motion::Bound::PastEnd);
                self.buffer.set_cursor_insert(target);
            }
            _ => {
                let target = motion::resolve(&self.buffer, m, 1, false);
                self.buffer.set_cursor(target);
            }
        }
    }

    /// Insert `text` at the cursor as a *single* undo step — the path a
    /// bracketed paste takes, where one step per character would be useless.
    pub fn insert_text(&mut self, text: &str) -> Option<VimEvent> {
        if text.is_empty() {
            return None;
        }
        self.buffer.snapshot();
        let pos = self.buffer.insert_str(self.buffer.cursor(), text);
        self.buffer.set_cursor_insert(pos);
        Some(VimEvent::Changed)
    }

    /// Whether input is mid-command — a host may want to hold its own global
    /// keys back while a count or operator is pending.
    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// The visual selection as an ordered, *inclusive* position pair, or `None`
    /// outside visual mode. In [`Mode::VisualLine`] the columns are the ones of
    /// the two ends; the whole lines are selected.
    pub fn selection(&self) -> Option<(Position, Position)> {
        self.mode.is_visual().then(|| {
            let cursor = self.buffer.cursor();
            match self.visual_anchor <= cursor {
                true => (self.visual_anchor, cursor),
                false => (cursor, self.visual_anchor),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------------

impl VimEditor {
    /// Feed one key press. Returns an event when it means something to the host.
    pub fn on_key(&mut self, key: KeyEvent) -> Option<VimEvent> {
        self.message = None;
        let before = self.mode;
        self.record(key);
        let event = match before {
            Mode::Normal => self.on_normal(key),
            Mode::Insert => self.on_insert(key),
            Mode::Command => self.on_command(key),
            Mode::Visual | Mode::VisualLine => self.on_visual(key),
        };
        self.settle_recording(before, event);
        event
    }
}

#[cfg(test)]
mod tests {
    use tuirealm::event::{Key, KeyModifiers};

    use super::*;
    use crate::buffer::Position;

    /// Feed a string of plain character keys, one at a time.
    pub(super) fn keys(editor: &mut VimEditor, s: &str) -> Vec<VimEvent> {
        s.chars()
            .filter_map(|c| editor.on_key(Key::Char(c).into()))
            .collect()
    }

    pub(super) fn editor(text: &str) -> VimEditor {
        VimEditor::default().with_text(text)
    }

    #[test]
    fn motions_move_the_cursor_without_reporting_a_change() {
        let mut e = editor("foo bar");
        assert!(keys(&mut e, "w").is_empty());
        assert_eq!(e.buffer().cursor(), Position::new(0, 4));
        assert!(!e.is_dirty());
    }

    #[test]
    fn a_count_prefix_is_collected_across_key_presses() {
        let mut e = editor("a b c d e");
        keys(&mut e, "3w");
        assert_eq!(e.buffer().cursor(), Position::new(0, 6));
        assert!(
            !e.is_pending(),
            "a completed command clears the pending state"
        );
    }

    #[test]
    fn zero_is_a_motion_but_a_digit_once_a_count_is_started() {
        let mut e = editor("abcdefghijkl");
        keys(&mut e, "$");
        keys(&mut e, "0");
        assert_eq!(
            e.buffer().cursor(),
            Position::new(0, 0),
            "bare 0 is LineStart"
        );
        keys(&mut e, "10l");
        assert_eq!(
            e.buffer().cursor(),
            Position::new(0, 10),
            "0 after 1 is a digit"
        );
    }

    #[test]
    fn gg_needs_both_keys() {
        let mut e = editor("a\nb\nc");
        keys(&mut e, "G");
        assert_eq!(e.buffer().cursor().line, 2);
        keys(&mut e, "g");
        assert!(e.is_pending(), "a lone g waits for the second key");
        keys(&mut e, "g");
        assert_eq!(e.buffer().cursor().line, 0);
    }

    #[test]
    fn operator_and_motion_combine_into_one_change() {
        let mut e = editor("foo bar baz");
        let events = keys(&mut e, "dw");
        assert_eq!(events, vec![VimEvent::Changed]);
        assert_eq!(e.text(), "bar baz");
    }

    #[test]
    fn the_two_counts_multiply() {
        let mut e = editor("a b c d e f g");
        keys(&mut e, "2d3w");
        assert_eq!(e.text(), "g", "2d3w deletes six words");
    }

    #[test]
    fn a_doubled_operator_is_linewise() {
        let mut e = editor("one\ntwo\nthree");
        keys(&mut e, "dd");
        assert_eq!(e.text(), "two\nthree");
        keys(&mut e, "yy");
        keys(&mut e, "p");
        assert_eq!(e.text(), "two\ntwo\nthree");
    }

    #[test]
    fn c_switches_to_insert_mode_and_typing_replaces_the_word() {
        let mut e = editor("foo bar");
        keys(&mut e, "cw");
        assert_eq!(e.mode(), Mode::Insert);
        keys(&mut e, "baz");
        assert_eq!(e.text(), "baz bar");
        e.on_key(Key::Esc.into());
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn an_insert_session_is_a_single_undo_step() {
        let mut e = editor("x");
        keys(&mut e, "A");
        keys(&mut e, "abc");
        e.on_key(Key::Esc.into());
        assert_eq!(e.text(), "xabc");
        keys(&mut e, "u");
        assert_eq!(e.text(), "x", "u must undo the whole typed run");
    }

    #[test]
    fn o_opens_a_line_below_and_undo_removes_it_again() {
        let mut e = editor("one");
        let events = keys(&mut e, "o");
        assert_eq!(events, vec![VimEvent::Changed]);
        keys(&mut e, "two");
        e.on_key(Key::Esc.into());
        assert_eq!(e.text(), "one\ntwo");
        keys(&mut e, "u");
        assert_eq!(e.text(), "one");
    }

    #[test]
    fn capital_o_opens_a_line_above() {
        let mut e = editor("two");
        keys(&mut e, "O");
        keys(&mut e, "one");
        e.on_key(Key::Esc.into());
        assert_eq!(e.text(), "one\ntwo");
    }

    #[test]
    fn escape_from_insert_steps_one_left() {
        let mut e = editor("abc");
        keys(&mut e, "A");
        assert_eq!(
            e.buffer().cursor(),
            Position::new(0, 3),
            "insert may sit past the end"
        );
        e.on_key(Key::Esc.into());
        assert_eq!(e.buffer().cursor(), Position::new(0, 2));
    }

    #[test]
    fn arrows_in_insert_mode_reach_the_column_past_the_last_character() {
        for key in [Key::Right, Key::End] {
            let mut e = editor("ab");
            keys(&mut e, "i");
            e.on_key(key.into());
            e.on_key(key.into());
            assert_eq!(e.buffer().cursor(), Position::new(0, 2), "{key:?}");
            // And typing there appends rather than overwriting.
            keys(&mut e, "c");
            assert_eq!(e.text(), "abc", "{key:?}");
        }
    }

    #[test]
    fn arrows_in_normal_mode_still_stop_on_the_last_character() {
        let mut e = editor("ab");
        e.on_key(Key::Right.into());
        e.on_key(Key::Right.into());
        assert_eq!(e.buffer().cursor(), Position::new(0, 1));
        e.on_key(Key::End.into());
        assert_eq!(e.buffer().cursor(), Position::new(0, 1));
    }

    #[test]
    fn enter_splits_the_line_and_backspace_joins_it_again() {
        let mut e = editor("ab");
        keys(&mut e, "i");
        e.on_key(Key::Right.into());
        e.on_key(Key::Enter.into());
        assert_eq!(e.text(), "a\nb");
        e.on_key(Key::Backspace.into());
        assert_eq!(e.text(), "ab");
        assert_eq!(e.buffer().cursor(), Position::new(0, 1));
    }

    #[test]
    fn redo_is_bound_to_ctrl_r() {
        let mut e = editor("abc");
        keys(&mut e, "x");
        assert_eq!(e.text(), "bc");
        keys(&mut e, "u");
        assert_eq!(e.text(), "abc");
        e.on_key(KeyEvent::new(Key::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(e.text(), "bc");
    }

    #[test]
    fn undo_at_the_oldest_change_reports_a_message() {
        let mut e = editor("abc");
        assert!(keys(&mut e, "u").is_empty());
        assert_eq!(e.message(), Some("Already at oldest change"));
    }

    #[test]
    fn a_message_is_cleared_by_the_next_key() {
        let mut e = editor("abc");
        keys(&mut e, "u");
        assert!(e.message().is_some());
        keys(&mut e, "l");
        assert_eq!(e.message(), None);
    }

    #[test]
    fn escape_cancels_a_pending_operator() {
        let mut e = editor("foo bar");
        keys(&mut e, "2d");
        assert!(e.is_pending());
        e.on_key(Key::Esc.into());
        assert!(!e.is_pending());
        keys(&mut e, "w");
        assert_eq!(e.text(), "foo bar", "the cancelled operator must not fire");
        assert_eq!(e.buffer().cursor(), Position::new(0, 4));
    }

    #[test]
    fn a_named_register_survives_the_next_delete() {
        let mut e = editor("keep\nlose");
        keys(&mut e, "\"ayy");
        keys(&mut e, "jdd");
        assert_eq!(e.text(), "keep");
        keys(&mut e, "\"ap");
        assert_eq!(
            e.text(),
            "keep\nkeep",
            "\"ap must paste the yank, not the delete"
        );
    }

    #[test]
    fn an_uppercase_register_appends() {
        let mut e = editor("one\ntwo\nthree");
        keys(&mut e, "\"ayy");
        keys(&mut e, "j\"Ayy");
        keys(&mut e, "G\"ap");
        assert_eq!(e.text(), "one\ntwo\nthree\none\ntwo");
    }

    #[test]
    fn the_pending_label_shows_the_register_being_typed() {
        let mut e = editor("abc");
        keys(&mut e, "\"");
        assert_eq!(e.pending_label(), "\"");
        keys(&mut e, "a");
        assert_eq!(e.pending_label(), "\"a");
        assert!(e.is_pending());
    }

    #[test]
    fn ciw_replaces_the_word_the_cursor_sits_in() {
        let mut e = editor("foo bar baz");
        keys(&mut e, "w");
        keys(&mut e, "ciw");
        assert_eq!(e.mode(), Mode::Insert);
        keys(&mut e, "qux");
        assert_eq!(e.text(), "foo qux baz");
    }

    #[test]
    fn daw_takes_the_trailing_blank_too() {
        let mut e = editor("foo bar baz");
        keys(&mut e, "w");
        keys(&mut e, "daw");
        assert_eq!(e.text(), "foo baz");
    }

    #[test]
    fn a_text_object_can_be_yanked_into_a_named_register() {
        let mut e = editor("say \"hello there\" now");
        keys(&mut e, "5l");
        keys(&mut e, "\"ayi\"");
        assert_eq!(e.registers().get(Some('a')).text, "hello there");
        assert!(!e.is_dirty(), "y must not touch the text");
    }

    #[test]
    fn a_text_object_needs_the_operator_first() {
        let mut e = editor("foo bar");
        keys(&mut e, "iw");
        assert_eq!(e.mode(), Mode::Insert, "bare i is still insert mode");
        assert_eq!(e.text(), "wfoo bar");
    }

    #[test]
    fn an_unclosed_block_leaves_the_text_alone() {
        let mut e = editor("f(a, b");
        keys(&mut e, "3l");
        assert!(keys(&mut e, "di(").is_empty());
        assert_eq!(e.text(), "f(a, b");
        assert!(!e.is_pending(), "the failed command must not stay armed");
    }

    #[test]
    fn a_count_before_the_text_object_widens_it() {
        let mut e = editor("one two three four");
        keys(&mut e, "d3aw");
        assert_eq!(e.text(), "four");
    }

    #[test]
    fn the_pending_label_shows_the_half_typed_text_object() {
        let mut e = editor("foo");
        keys(&mut e, "ci");
        assert_eq!(e.pending_label(), "ci");
        e.on_key(Key::Esc.into());
        assert!(!e.is_pending());
    }

    #[test]
    fn dot_repeats_an_operator_at_the_new_cursor_position() {
        let mut e = editor("one two three");
        keys(&mut e, "dw");
        assert_eq!(e.text(), "two three");
        keys(&mut e, ".");
        assert_eq!(e.text(), "three");
    }

    #[test]
    fn dot_repeats_a_whole_insert_session() {
        let mut e = editor("foo bar");
        keys(&mut e, "ciwX");
        e.on_key(Key::Esc.into());
        assert_eq!(e.text(), "X bar");
        keys(&mut e, "w");
        keys(&mut e, ".");
        assert_eq!(e.text(), "X X", "the typed text is part of the change");
        assert_eq!(
            e.mode(),
            Mode::Normal,
            "a repeat never leaves insert mode behind"
        );
    }

    #[test]
    fn a_motion_is_not_a_change_and_does_not_become_the_repeat() {
        let mut e = editor("one two three");
        keys(&mut e, "x");
        keys(&mut e, "ww");
        keys(&mut e, ".");
        assert_eq!(e.text(), "ne two hree", "x was repeated, not the motions");
    }

    #[test]
    fn undo_and_redo_are_not_themselves_repeatable() {
        let mut e = editor("abcdef");
        keys(&mut e, "x");
        keys(&mut e, "u");
        assert_eq!(e.text(), "abcdef");
        keys(&mut e, ".");
        assert_eq!(
            e.text(),
            "bcdef",
            "the repeat is still the delete, not the undo"
        );
    }

    #[test]
    fn dot_repeats_a_visual_operation() {
        let mut e = editor("abcdef");
        keys(&mut e, "vld");
        assert_eq!(e.text(), "cdef");
        keys(&mut e, ".");
        assert_eq!(e.text(), "ef");
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn a_count_repeats_the_replay() {
        let mut e = editor("one two three four");
        keys(&mut e, "dw");
        keys(&mut e, "2.");
        assert_eq!(e.text(), "four");
    }

    #[test]
    fn dot_without_a_change_says_so() {
        let mut e = editor("abc");
        keys(&mut e, ".");
        assert_eq!(e.message(), Some("Nothing to repeat"));
        assert_eq!(e.text(), "abc");
    }

    #[test]
    fn a_search_jumps_to_the_next_match_and_n_keeps_going() {
        let mut e = editor("one two\nthree two");
        keys(&mut e, "/two");
        assert_eq!(e.command_line().as_deref(), Some("/two"));
        e.on_key(Key::Enter.into());
        assert_eq!(e.buffer().cursor(), Position::new(0, 4));
        keys(&mut e, "n");
        assert_eq!(e.buffer().cursor(), Position::new(1, 6));
        keys(&mut e, "N");
        assert_eq!(
            e.buffer().cursor(),
            Position::new(0, 4),
            "N turns the search around"
        );
        assert!(!e.is_dirty(), "a search is not a change");
    }

    #[test]
    fn a_backward_search_starts_the_other_way_and_n_follows_it() {
        let mut e = editor("hit one hit two hit");
        keys(&mut e, "$");
        keys(&mut e, "?hit");
        e.on_key(Key::Enter.into());
        assert_eq!(e.buffer().cursor(), Position::new(0, 8));
        keys(&mut e, "n");
        assert_eq!(
            e.buffer().cursor(),
            Position::new(0, 0),
            "n keeps going backwards"
        );
    }

    #[test]
    fn wrapping_around_the_end_is_reported() {
        let mut e = editor("target\nother");
        keys(&mut e, "j");
        keys(&mut e, "/target");
        e.on_key(Key::Enter.into());
        assert_eq!(e.buffer().cursor(), Position::new(0, 0));
        assert_eq!(e.message(), Some("search hit BOTTOM, continuing at TOP"));
    }

    #[test]
    fn a_missing_pattern_reports_vims_error_and_leaves_the_cursor() {
        let mut e = editor("one two");
        keys(&mut e, "/nope");
        e.on_key(Key::Enter.into());
        assert_eq!(e.message(), Some("E486: Pattern not found: nope"));
        assert_eq!(e.buffer().cursor(), Position::new(0, 0));
    }

    #[test]
    fn an_empty_pattern_repeats_the_last_one() {
        let mut e = editor("a hit b hit");
        keys(&mut e, "/hit");
        e.on_key(Key::Enter.into());
        keys(&mut e, "/");
        e.on_key(Key::Enter.into());
        assert_eq!(e.buffer().cursor(), Position::new(0, 8));
    }

    #[test]
    fn repeating_without_a_previous_search_reports_it() {
        let mut e = editor("abc");
        keys(&mut e, "n");
        assert_eq!(e.message(), Some("E35: No previous regular expression"));
    }

    #[test]
    fn a_count_before_n_skips_matches() {
        let mut e = editor("x x x x");
        keys(&mut e, "/x");
        e.on_key(Key::Enter.into());
        assert_eq!(e.buffer().cursor(), Position::new(0, 2));
        keys(&mut e, "2n");
        assert_eq!(e.buffer().cursor(), Position::new(0, 6));
    }

    #[test]
    fn a_visual_selection_includes_the_character_under_the_cursor() {
        let mut e = editor("abcdef");
        keys(&mut e, "vll");
        assert_eq!(e.mode(), Mode::Visual);
        assert_eq!(
            e.selection(),
            Some((Position::new(0, 0), Position::new(0, 2)))
        );
        keys(&mut e, "d");
        assert_eq!(e.text(), "def", "the third character is part of the span");
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn a_selection_may_grow_backwards() {
        let mut e = editor("abcdef");
        keys(&mut e, "3l");
        keys(&mut e, "vhh");
        assert_eq!(
            e.selection(),
            Some((Position::new(0, 1), Position::new(0, 3)))
        );
        keys(&mut e, "y");
        assert_eq!(e.registers().get(None).text, "bcd");
        assert!(!e.is_dirty());
    }

    #[test]
    fn visual_line_takes_whole_lines() {
        let mut e = editor("one\ntwo\nthree");
        keys(&mut e, "Vj");
        assert_eq!(e.mode(), Mode::VisualLine);
        keys(&mut e, "d");
        assert_eq!(e.text(), "three");
        keys(&mut e, "P");
        assert_eq!(e.text(), "one\ntwo\nthree", "the yank was linewise");
    }

    #[test]
    fn o_jumps_to_the_other_end_of_the_selection() {
        let mut e = editor("abcdef");
        keys(&mut e, "3l");
        keys(&mut e, "v");
        keys(&mut e, "o");
        assert_eq!(
            e.buffer().cursor(),
            Position::new(0, 3),
            "a one-cell selection cannot flip"
        );
        keys(&mut e, "ll");
        keys(&mut e, "o");
        assert_eq!(e.buffer().cursor(), Position::new(0, 3));
        keys(&mut e, "h");
        assert_eq!(
            e.selection(),
            Some((Position::new(0, 2), Position::new(0, 5)))
        );
    }

    #[test]
    fn c_on_a_selection_leaves_insert_mode_behind() {
        let mut e = editor("foo bar");
        keys(&mut e, "vll");
        keys(&mut e, "c");
        assert_eq!(e.mode(), Mode::Insert);
        keys(&mut e, "baz");
        assert_eq!(e.text(), "baz bar");
    }

    #[test]
    fn the_same_visual_key_leaves_the_mode_and_the_other_switches_it() {
        let mut e = editor("one\ntwo");
        keys(&mut e, "v");
        keys(&mut e, "V");
        assert_eq!(e.mode(), Mode::VisualLine, "V switches, keeping the anchor");
        keys(&mut e, "V");
        assert_eq!(e.mode(), Mode::Normal);
        keys(&mut e, "v");
        e.on_key(Key::Esc.into());
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.selection(), None);
    }

    #[test]
    fn viw_selects_the_word_without_an_operator() {
        let mut e = editor("foo bar baz");
        keys(&mut e, "w");
        keys(&mut e, "viw");
        assert_eq!(
            e.selection(),
            Some((Position::new(0, 4), Position::new(0, 6)))
        );
        keys(&mut e, "d");
        assert_eq!(e.text(), "foo  baz");
    }

    #[test]
    fn a_count_repeats_a_motion_inside_a_selection() {
        let mut e = editor("a b c d e");
        keys(&mut e, "v3w");
        keys(&mut e, "d");
        assert_eq!(e.text(), " e");
    }

    #[test]
    fn a_visual_delete_can_target_a_named_register() {
        let mut e = editor("abcdef");
        keys(&mut e, "vl\"ad");
        assert_eq!(e.registers().get(Some('a')).text, "ab");
    }

    #[test]
    fn keys_without_a_meaning_for_a_selection_are_ignored() {
        let mut e = editor("abc");
        keys(&mut e, "vl");
        keys(&mut e, "p");
        assert_eq!(
            e.text(),
            "abc",
            "p has no visual meaning yet and must not paste"
        );
        assert_eq!(e.mode(), Mode::Visual, "and must not drop the selection");
    }

    #[test]
    fn write_emits_save_and_clears_the_dirty_flag() {
        let mut e = editor("a");
        keys(&mut e, "x");
        assert!(e.is_dirty());
        keys(&mut e, ":w");
        assert_eq!(e.command_line().as_deref(), Some(":w"));
        assert_eq!(e.on_key(Key::Enter.into()), Some(VimEvent::Save));
        assert!(!e.is_dirty());
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn wq_and_x_both_save_and_close() {
        for cmd in [":wq", ":x"] {
            let mut e = editor("a");
            keys(&mut e, cmd);
            assert_eq!(
                e.on_key(Key::Enter.into()),
                Some(VimEvent::SaveAndClose),
                "{cmd} must save and close"
            );
        }
    }

    #[test]
    fn quit_refuses_to_discard_unwritten_changes() {
        let mut e = editor("abc");
        keys(&mut e, "x");
        keys(&mut e, ":q");
        assert_eq!(e.on_key(Key::Enter.into()), None);
        assert!(e.message().is_some_and(|m| m.starts_with("E37")));

        keys(&mut e, ":q!");
        assert_eq!(e.on_key(Key::Enter.into()), Some(VimEvent::Cancel));
    }

    #[test]
    fn quit_on_a_clean_buffer_just_cancels() {
        let mut e = editor("abc");
        keys(&mut e, ":q");
        assert_eq!(e.on_key(Key::Enter.into()), Some(VimEvent::Cancel));
    }

    #[test]
    fn an_unknown_ex_command_reports_vims_error() {
        let mut e = editor("abc");
        keys(&mut e, ":nope");
        e.on_key(Key::Enter.into());
        assert_eq!(e.message(), Some("E492: Not an editor command: nope"));
        assert_eq!(e.mode(), Mode::Normal);
    }

    #[test]
    fn escape_leaves_the_command_line_without_running_it() {
        let mut e = editor("abc");
        keys(&mut e, ":q!");
        e.on_key(Key::Esc.into());
        assert_eq!(e.mode(), Mode::Normal);
        assert_eq!(e.command_line(), None);
    }

    #[test]
    fn the_pending_label_shows_what_has_been_typed() {
        let mut e = editor("abc");
        keys(&mut e, "2d3");
        assert_eq!(e.pending_label(), "2d3");
    }

    #[test]
    fn control_combinations_are_not_typed_into_the_buffer() {
        let mut e = editor("");
        keys(&mut e, "i");
        assert_eq!(
            e.on_key(KeyEvent::new(Key::Char('s'), KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(e.text(), "", "Ctrl+S stays available to the host");
    }

    #[test]
    fn set_text_resets_mode_and_pending_input() {
        let mut e = editor("abc");
        keys(&mut e, "2d");
        e.set_text("fresh");
        assert_eq!(e.mode(), Mode::Normal);
        assert!(!e.is_pending());
        assert!(!e.is_dirty());
        assert_eq!(e.text(), "fresh");
    }
}
