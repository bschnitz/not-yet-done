/// An abstraction over a 2-D character canvas.
///
/// The core render logic calls only these two methods, which lets us swap
/// between a plain `Vec<Vec<char>>` (for `grid-render-sim` tests) and a
/// ratatui `Buffer` (for the real TUI component) without duplicating any
/// drawing code.
pub trait RenderTarget {
    /// Write a single character at absolute position `(x, y)`.
    fn put_char(&mut self, x: u16, y: u16, ch: char);

    /// Read the character currently at `(x, y)` (used for crossing logic).
    fn get_char(&self, x: u16, y: u16) -> char;
}

// ── CharBuf ───────────────────────────────────────────────────────────────────

/// A plain 2-D character buffer used by `grid-render-sim`.
///
/// Coordinates are always absolute (origin at `(0, 0)`), matching the
/// convention used in `grid-render-sim` where the area always starts at `(0,0)`.
pub struct CharBuf {
    buf:    Vec<Vec<char>>,
    width:  usize,
    height: usize,
}

impl CharBuf {
    /// Create a blank buffer of the given dimensions.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            buf:    vec![vec![' '; width]; height],
            width,
            height,
        }
    }

    /// Consume the buffer and return its rows as owned `String`s.
    pub fn into_lines(self) -> Vec<String> {
        self.buf.into_iter().map(|row| row.into_iter().collect()).collect()
    }
}

impl RenderTarget for CharBuf {
    fn put_char(&mut self, x: u16, y: u16, ch: char) {
        let (xi, yi) = (x as usize, y as usize);
        if yi < self.height && xi < self.width {
            self.buf[yi][xi] = ch;
        }
    }

    fn get_char(&self, x: u16, y: u16) -> char {
        let (xi, yi) = (x as usize, y as usize);
        if yi < self.height && xi < self.width {
            self.buf[yi][xi]
        } else {
            ' '
        }
    }
}
