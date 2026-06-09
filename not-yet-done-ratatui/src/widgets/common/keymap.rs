use crossterm::event::{KeyCode, KeyModifiers};
use tuirealm::event::{Key, KeyEvent as TuiKeyEvent, KeyModifiers as TuiKeyModifiers};

/// A single crossterm-level keybinding (used by Form widgets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyBinding {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyBinding {
    pub fn new(code: KeyCode) -> Self {
        Self {
            code,
            modifiers: KeyModifiers::NONE,
        }
    }

    pub fn ctrl(code: KeyCode) -> Self {
        Self {
            code,
            modifiers: KeyModifiers::CONTROL,
        }
    }
}

/// One or more tuirealm key events that trigger the same action.
///
/// ```rust
/// use tuirealm::event::{Key, KeyEvent, KeyModifiers};
/// use not_yet_done_ratatui::Keys;
///
/// // Single key:
/// let k = Keys::one(Key::Char(' '), KeyModifiers::NONE);
///
/// // Multiple keys:
/// let k = Keys::one(Key::Char(' '), KeyModifiers::NONE)
///     .or(Key::Char(' '), KeyModifiers::CONTROL);
/// ```
#[derive(Debug, Clone)]
pub struct Keys(pub Vec<TuiKeyEvent>);

impl Keys {
    /// A binding with a single key.
    pub fn one(code: Key, modifiers: TuiKeyModifiers) -> Self {
        Self(vec![TuiKeyEvent { code, modifiers }])
    }

    /// Shorthand for a plain key without modifiers.
    pub fn plain(code: Key) -> Self {
        Self::one(code, TuiKeyModifiers::NONE)
    }

    /// Shorthand for Ctrl + key.
    pub fn ctrl(code: Key) -> Self {
        Self::one(code, TuiKeyModifiers::CONTROL)
    }

    /// Add an alternative key that also triggers this action.
    pub fn or(mut self, code: Key, modifiers: TuiKeyModifiers) -> Self {
        self.0.push(TuiKeyEvent { code, modifiers });
        self
    }

    /// Shorthand: add a Ctrl + key alternative.
    pub fn or_ctrl(self, code: Key) -> Self {
        self.or(code, TuiKeyModifiers::CONTROL)
    }

    /// Shorthand: add a plain (no modifier) alternative.
    pub fn or_plain(self, code: Key) -> Self {
        self.or(code, TuiKeyModifiers::NONE)
    }

    /// Returns `true` if `key` matches any of the configured bindings.
    pub fn matches(&self, key: &TuiKeyEvent) -> bool {
        self.0.iter().any(|k| k == key)
    }

    /// Human-readable rendering of the bindings, joined by `/`.
    /// E.g. `Keys::plain(Enter).or_ctrl(Char('o'))` → `"Enter/C-o"`.
    /// Use this when surfacing keybindings in help footers so the
    /// display reflects whatever the user has actually configured.
    pub fn display(&self) -> String {
        self.0
            .iter()
            .map(format_key_event)
            .collect::<Vec<_>>()
            .join("/")
    }
}

fn format_key_event(ev: &TuiKeyEvent) -> String {
    let code = format_key_code(&ev.code);
    let mut parts: Vec<&str> = Vec::new();
    if ev.modifiers.intersects(TuiKeyModifiers::CONTROL) {
        parts.push("C");
    }
    if ev.modifiers.intersects(TuiKeyModifiers::ALT) {
        parts.push("M");
    }
    // SHIFT is implicit in printable uppercase chars, so we only label
    // it when the key code itself doesn't carry the case.
    if ev.modifiers.intersects(TuiKeyModifiers::SHIFT)
        && !matches!(ev.code, Key::Char(c) if c.is_ascii_alphabetic())
    {
        parts.push("S");
    }
    if parts.is_empty() {
        code
    } else {
        format!("{}-{}", parts.join("-"), code)
    }
}

fn format_key_code(code: &Key) -> String {
    match code {
        Key::Char(' ') => "Space".to_string(),
        Key::Char(c) => c.to_string(),
        Key::Enter => "Enter".to_string(),
        Key::Esc => "Esc".to_string(),
        Key::Backspace => "BS".to_string(),
        Key::Tab => "Tab".to_string(),
        Key::BackTab => "S-Tab".to_string(),
        Key::Left => "←".to_string(),
        Key::Right => "→".to_string(),
        Key::Up => "↑".to_string(),
        Key::Down => "↓".to_string(),
        Key::Home => "Home".to_string(),
        Key::End => "End".to_string(),
        Key::PageUp => "PgUp".to_string(),
        Key::PageDown => "PgDn".to_string(),
        Key::Delete => "Del".to_string(),
        Key::Insert => "Ins".to_string(),
        Key::Function(n) => format!("F{n}"),
        other => format!("{other:?}"),
    }
}

/// Allow using a single `TuiKeyEvent` where `Keys` is expected.
impl From<TuiKeyEvent> for Keys {
    fn from(ev: TuiKeyEvent) -> Self {
        Self(vec![ev])
    }
}
