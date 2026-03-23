use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use anyhow::Result;

/// Convert a crossterm KeyEvent into the canonical string representation
/// used in tui-keybindings.yaml (e.g. "q", "ctrl+c", "tab", "shift+tab").
pub fn key_event_to_string(key: KeyEvent) -> String {
    let mut parts: Vec<&str> = Vec::new();

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("ctrl");
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        parts.push("alt");
    }
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        // Printable shifted chars already carry the upper-case letter in the
        // key code, so we skip the "shift+" prefix there to avoid "shift+Q"
        // instead of "Q".  BackTab carries the shift semantics in its name.
        match key.code {
            KeyCode::BackTab | KeyCode::Char(_) => {}
            _ => parts.push("shift"),
        }
    }

    let key_str: String = match key.code {
        KeyCode::Char(c)   => c.to_string(),
        KeyCode::Enter     => "enter".to_string(),
        KeyCode::Esc       => "esc".to_string(),
        KeyCode::Tab       => "tab".to_string(),
        KeyCode::BackTab   => { parts.push("shift"); "tab".to_string() }
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Delete    => "delete".to_string(),
        KeyCode::Up        => "up".to_string(),
        KeyCode::Down      => "down".to_string(),
        KeyCode::Left      => "left".to_string(),
        KeyCode::Right     => "right".to_string(),
        KeyCode::Home      => "home".to_string(),
        KeyCode::End       => "end".to_string(),
        KeyCode::PageUp    => "pageup".to_string(),
        KeyCode::PageDown  => "pagedown".to_string(),
        KeyCode::F(n)      => format!("f{}", n),
        _                  => "unknown".to_string(),
    };

    if parts.is_empty() {
        key_str
    } else {
        format!("{}+{}", parts.join("+"), key_str)
    }
}

/// Poll for the next key-press event (non-blocking, 16 ms timeout ≈ 60 fps).
/// Returns `None` on timeout or non-key events.
pub fn poll_event() -> Result<Option<String>> {
    if event::poll(std::time::Duration::from_millis(16))? {
        if let Event::Key(key) = event::read()? {
            // Only react to Press events — avoids double-firing on Windows
            // where crossterm also emits Release/Repeat events.
            if key.kind == KeyEventKind::Press {
                return Ok(Some(key_event_to_string(key)));
            }
        }
    }
    Ok(None)
}
