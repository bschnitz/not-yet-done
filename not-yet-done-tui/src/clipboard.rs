//! System clipboard access.
//!
//! Two paths, tried in order:
//!
//! 1. **`arboard`** (the `clipboard` feature) — talks to the desktop
//!    clipboard directly. Reliable locally, and the only one that can *read*.
//! 2. **OSC 52** — asks the terminal emulator to put the text on the
//!    clipboard for us. Works with no desktop connection at all, which makes
//!    it the path that survives an SSH session; kitty allows it by default
//!    (`clipboard_control` includes `write-clipboard`). Write-only: the
//!    matching read is a query whose reply would have to be parsed off stdin
//!    while the event reader owns it, which is not worth it here.
//!
//! Copying therefore always has a fallback, pasting needs the feature.

use std::io::{self, Write};

/// Put `text` on the system clipboard. Returns `false` only when every
/// available path failed, so callers can report it rather than silently
/// losing a copy.
// Its only caller today is the mouse drag selection, so a build without the
// `mouse` feature has no user for it — the write path is not mouse-specific,
// though, and stays available to whatever copies next.
#[cfg_attr(not(feature = "mouse"), allow(dead_code))]
pub fn copy(text: &str) -> bool {
    #[cfg(feature = "clipboard")]
    {
        if arboard::Clipboard::new()
            .and_then(|mut c| c.set_text(text.to_string()))
            .is_ok()
        {
            return true;
        }
    }
    osc52_copy(text).is_ok()
}

/// Read text from the system clipboard. `None` when the `clipboard` feature
/// is off or the clipboard holds no text.
#[cfg(feature = "clipboard")]
pub fn paste() -> Option<String> {
    arboard::Clipboard::new()
        .ok()
        .and_then(|mut c| c.get_text().ok())
}

#[cfg(not(feature = "clipboard"))]
pub fn paste() -> Option<String> {
    None
}

/// Hand the text to the terminal emulator with an OSC 52 sequence.
#[cfg_attr(not(feature = "mouse"), allow(dead_code))]
fn osc52_copy(text: &str) -> io::Result<()> {
    let mut out = io::stdout();
    out.write_all(b"\x1b]52;c;")?;
    out.write_all(base64(text.as_bytes()).as_bytes())?;
    out.write_all(b"\x07")?;
    out.flush()
}

/// Standard base64 with padding — the encoding OSC 52 expects. Hand-rolled
/// to keep a dependency out of the tree for the one place we need it.
#[cfg_attr(not(feature = "mouse"), allow(dead_code))]
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_the_rfc_examples() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    /// Non-ASCII must survive as its UTF-8 bytes — a selection copied out of
    /// a tree pane is full of box-drawing glyphs and emoji.
    #[test]
    fn base64_encodes_utf8_bytes() {
        assert_eq!(base64("├─ ✓".as_bytes()), "4pSc4pSAIOKckw==");
    }
}
