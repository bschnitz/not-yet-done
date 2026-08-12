use std::io;
use std::sync::OnceLock;

use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::supports_keyboard_enhancement;

/// Cache the terminal's keyboard-enhancement capability so we only pay the
/// (potentially blocking) probe once per process.
static KITTY_SUPPORTED: OnceLock<bool> = OnceLock::new();

fn kitty_supported() -> bool {
    *KITTY_SUPPORTED.get_or_init(|| supports_keyboard_enhancement().unwrap_or(false))
}

/// Push the kitty keyboard enhancement flags so that ctrl+m, ctrl+i, ctrl+[
/// etc. arrive as distinct keys instead of being collapsed onto Enter / Tab /
/// Esc by the terminal. No-op on terminals that don't support the protocol.
pub fn enable_kitty_protocol() -> Result<()> {
    if !kitty_supported() {
        return Ok(());
    }
    execute!(
        io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )?;
    Ok(())
}

/// Pop the keyboard enhancement flags pushed by `enable_kitty_protocol`. Must
/// be called before any external program (editor, script) takes over the
/// terminal — otherwise it would receive kitty-encoded key sequences it
/// cannot decode.
pub fn disable_kitty_protocol() -> Result<()> {
    if !kitty_supported() {
        return Ok(());
    }
    execute!(io::stdout(), PopKeyboardEnhancementFlags)?;
    Ok(())
}

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
        KeyCode::Char(c) => {
            // Kitty's DISAMBIGUATE_ESCAPE_CODES delivers shift+letter as
            // (lower-case char + SHIFT modifier); plain terminals already
            // upper-case the char and drop the SHIFT bit. Normalise both
            // paths to the upper-case form so bindings like "G", "N",
            // "ctrl+H" match regardless of terminal mode.
            if key.modifiers.contains(KeyModifiers::SHIFT) && c.is_ascii_alphabetic() {
                c.to_ascii_uppercase().to_string()
            } else {
                c.to_string()
            }
        }
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Esc => "esc".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::BackTab => {
            parts.push("shift");
            "tab".to_string()
        }
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::PageUp => "pageup".to_string(),
        KeyCode::PageDown => "pagedown".to_string(),
        KeyCode::F(n) => format!("f{}", n),
        _ => "unknown".to_string(),
    };

    if parts.is_empty() {
        key_str
    } else {
        format!("{}+{}", parts.join("+"), key_str)
    }
}

/// Inverse of [`key_event_to_string`]: turn the canonical key string back
/// into a tuirealm `KeyEvent` so widgets that consume tuirealm events
/// (e.g. `FilePicker`) can be driven from the App's `&str` key pipeline.
/// Returns `None` for unrecognised strings.
pub fn key_string_to_tuirealm(key: &str) -> Option<tuirealm::event::KeyEvent> {
    use tuirealm::event::{Key, KeyEvent as TuiKeyEvent, KeyModifiers as TuiMods};
    let mut mods = TuiMods::NONE;
    let mut rest = key;
    loop {
        if let Some(r) = rest.strip_prefix("ctrl+") {
            mods |= TuiMods::CONTROL;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("alt+") {
            mods |= TuiMods::ALT;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("shift+") {
            mods |= TuiMods::SHIFT;
            rest = r;
        } else {
            break;
        }
    }
    let code = match rest {
        "enter" => Key::Enter,
        "esc" => Key::Esc,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "space" => Key::Char(' '),
        // Function keys `f1`..`f12` — needs at least one digit, else a bare
        // `f` would match here, fail to parse, and drop the whole key (a plain
        // letter must fall through to the char arm below).
        s if s.len() > 1 && s.starts_with('f') && s[1..].chars().all(|c| c.is_ascii_digit()) => {
            s[1..].parse::<u8>().ok().map(Key::Function)?
        }
        s => {
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            // Plain upper-case letters carry their shift in the case, not
            // the modifier (matches the encoder's behaviour).
            if c.is_ascii_uppercase() {
                mods.remove(TuiMods::SHIFT);
            }
            Key::Char(c)
        }
    };
    Some(TuiKeyEvent {
        code,
        modifiers: mods,
    })
}

/// Map a crossterm [`Event`] delivered by the async `EventStream` to the
/// canonical key string used in tui-keybindings.yaml. Returns `None` for
/// non-key events and for key Release/Repeat events (only `Press` is
/// acted on — avoids double-firing on terminals that also emit
/// Release/Repeat). This is the event-driven (1b) counterpart of the
/// former blocking `poll_event`: the `select!` loop owns the wait, this
/// only does the decode + debug logging.
pub fn event_to_key_string(event: &Event) -> Option<String> {
    if let Event::Key(key) = event {
        if key.kind == KeyEventKind::Press {
            let s = key_event_to_string(*key);
            debug_log_key(key, &s);
            return Some(s);
        }
    }
    None
}

/// When `NYD_KEY_DEBUG` is set, append every press event to
/// `/tmp/nyd-keys.log` as `<modifiers> <KeyCode> -> <emitted string>` so
/// we can diagnose terminal-specific encodings (kitty vs. xterm,
/// shift-as-modifier vs. case-folded) without instrumenting the UI.
fn debug_log_key(key: &KeyEvent, emitted: &str) {
    use std::io::Write;
    if std::env::var_os("NYD_KEY_DEBUG").is_none() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/nyd-keys.log")
    {
        let _ = writeln!(f, "{:?} {:?} -> {}", key.modifiers, key.code, emitted);
    }
}

#[cfg(test)]
mod tests {
    use super::key_string_to_tuirealm;
    use tuirealm::event::{Key, KeyModifiers};

    #[test]
    fn bare_f_maps_to_a_char_not_a_dropped_function_key() {
        // Regression: `"f"` used to match the `f1`..`f12` arm (empty digit
        // suffix parses `all(is_ascii_digit)` vacuously true), fail to parse,
        // and drop the whole key — so `f` never reached a text input (e.g. the
        // shortcut-menu search).
        let ev = key_string_to_tuirealm("f").expect("f must decode");
        assert_eq!(ev.code, Key::Char('f'));
        assert_eq!(ev.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn function_keys_still_decode() {
        assert_eq!(
            key_string_to_tuirealm("f1").map(|e| e.code),
            Some(Key::Function(1))
        );
        assert_eq!(
            key_string_to_tuirealm("f12").map(|e| e.code),
            Some(Key::Function(12))
        );
    }
}
