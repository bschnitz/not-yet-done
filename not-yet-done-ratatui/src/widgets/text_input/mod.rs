mod component;
mod render;
pub mod keymap;
pub mod state;
pub mod style;

pub use component::ATTR_ERROR;
pub use keymap::TextInputKeymap;
pub use state::TextInputEvent;
pub use style::{TextInputStyle, TextInputStyleType};

/// A single-line text input field implementing tuirealm's [`MockComponent`] and
/// [`tuirealm::Component<TextInputEvent, NoUserEvent>`].
///
/// All state is owned by the component.  Construct once and mount into a
/// tuirealm [`Application`](tuirealm::Application); do not rebuild per frame.
///
/// ```rust
/// let input = TextInput::default()
///     .with_title("Username")
///     .with_placeholder("e.g. alice")
///     .with_inactive_style(inactive)
///     .with_active_style(active)
///     .with_keymap(keymap);
///
/// app.mount(Id::Username, Box::new(input), vec![])?;
/// ```
pub struct TextInput {
    // --- framework state ---
    pub(crate) focused: bool,

    // --- editing state ---
    pub(crate) value:   String,
    /// Byte offset of the cursor within `value`.
    pub(crate) cursor:  usize,
    pub(crate) error:   Option<String>,

    // --- configuration (set once at construction) ---
    pub(crate) title:           String,
    pub(crate) placeholder:     String,
    pub(crate) inactive_style:  TextInputStyle,
    pub(crate) active_style:    TextInputStyle,
    pub(crate) keymap:          TextInputKeymap,
}

impl Default for TextInput {
    fn default() -> Self {
        Self {
            focused:        false,
            value:          String::new(),
            cursor:         0,
            error:          None,
            title:          String::new(),
            placeholder:    String::new(),
            inactive_style: TextInputStyle::default(),
            active_style:   TextInputStyle::default(),
            keymap:         TextInputKeymap::default(),
        }
    }
}

impl TextInput {
    /// Sets the title displayed above the input field.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Sets the placeholder shown when the value is empty.
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Style applied when this component does not have focus.
    pub fn with_inactive_style(mut self, style: TextInputStyle) -> Self {
        self.inactive_style = style;
        self
    }

    /// Style applied when this component has focus.
    pub fn with_active_style(mut self, style: TextInputStyle) -> Self {
        self.active_style = style;
        self
    }

    /// Overrides the default key bindings.
    pub fn with_keymap(mut self, keymap: TextInputKeymap) -> Self {
        self.keymap = keymap;
        self
    }

    // --- internal editing helpers (used by component.rs) ---

    pub(crate) fn push_char(&mut self, c: char) {
        self.value.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub(crate) fn pop_char(&mut self) {
        if self.cursor == 0 { return; }
        let mut pos = self.cursor - 1;
        while !self.value.is_char_boundary(pos) { pos -= 1; }
        self.value.remove(pos);
        self.cursor = pos;
    }

    pub(crate) fn delete_forward(&mut self) {
        if self.cursor < self.value.len() {
            self.value.remove(self.cursor);
        }
    }

    pub(crate) fn move_cursor_left(&mut self) {
        if self.cursor == 0 { return; }
        let mut pos = self.cursor - 1;
        while !self.value.is_char_boundary(pos) { pos -= 1; }
        self.cursor = pos;
    }

    pub(crate) fn move_cursor_right(&mut self) {
        if self.cursor >= self.value.len() { return; }
        let mut pos = self.cursor + 1;
        while pos <= self.value.len() && !self.value.is_char_boundary(pos) { pos += 1; }
        self.cursor = pos;
    }

    pub(crate) fn clear_value(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }
}
