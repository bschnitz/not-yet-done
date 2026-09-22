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

/// Turn on mouse reporting.
///
/// Deliberately *not* crossterm's `EnableMouseCapture`: that also sets 1003
/// (any-motion tracking), which reports every cell the pointer crosses even
/// with no button down. In a render loop that parks at ~0 % CPU while idle
/// (see `docs/decisions/0001-render-loop-dirty-gating.md`) that would be a
/// stream of wake-ups for nothing. We ask for exactly what we use:
///
/// * `1000` — button press/release,
/// * `1002` — motion *while a button is held*, i.e. drag,
/// * `1006` — SGR encoding, without which coordinates break past column 223.
#[cfg(feature = "mouse")]
pub fn enable_mouse() -> Result<()> {
    use std::io::Write;
    let mut out = io::stdout();
    out.write_all(b"\x1b[?1000h\x1b[?1002h\x1b[?1006h")?;
    out.flush()?;
    Ok(())
}

/// Turn mouse reporting back off, in the reverse order. Must run before any
/// external program takes over the terminal — otherwise an editor that does
/// not speak SGR mouse would receive the raw reports as garbage input.
#[cfg(feature = "mouse")]
pub fn disable_mouse() -> Result<()> {
    use std::io::Write;
    let mut out = io::stdout();
    out.write_all(b"\x1b[?1006l\x1b[?1002l\x1b[?1000l")?;
    out.flush()?;
    Ok(())
}

#[cfg(not(feature = "mouse"))]
pub fn enable_mouse() -> Result<()> {
    Ok(())
}

#[cfg(not(feature = "mouse"))]
pub fn disable_mouse() -> Result<()> {
    Ok(())
}

/// Put the terminal into the input modes the TUI runs in: kitty keyboard
/// disambiguation plus mouse reporting.
///
/// The two always travel together — every point that hands the terminal to a
/// child process has to drop both and pick both back up — so they are one
/// call rather than two that can drift apart at the seven-odd suspend sites.
pub fn resume_input_modes() -> Result<()> {
    enable_kitty_protocol()?;
    enable_mouse()?;
    Ok(())
}

/// Inverse of [`resume_input_modes`]. Call before suspending the TUI for an
/// editor, a script, or the final teardown.
pub fn suspend_input_modes() -> Result<()> {
    let _ = disable_mouse();
    disable_kitty_protocol()?;
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

/// The decoder lives in `not-yet-done-ratatui` now, beside the widgets it
/// feeds; re-exported here because the App's key pipeline is this module.
pub use not_yet_done_ratatui::key_string_to_tuirealm;

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
