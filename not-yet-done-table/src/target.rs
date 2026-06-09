//! Render target abstraction for framework-agnostic table rendering.

/// A 2D character canvas that can be written to.
///
/// Implementations include [`CharBuf`] for testing and ratatui adapters
/// for real rendering.
pub trait RenderTarget {
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    fn put_char(&mut self, x: usize, y: usize, ch: char);
    fn put_str(&mut self, x: usize, y: usize, s: &str) {
        for (i, ch) in s.chars().enumerate() {
            if x + i < self.width() {
                self.put_char(x + i, y, ch);
            }
        }
    }
}

/// A plain character buffer for testing.
///
/// Each cell is a single `char`, initialized to space.
pub struct CharBuf {
    width: usize,
    height: usize,
    buf: Vec<Vec<char>>,
}

impl CharBuf {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            buf: vec![vec![' '; width]; height],
        }
    }

    /// Get the content of a single row as a string.
    pub fn row_str(&self, y: usize) -> String {
        if y < self.height {
            self.buf[y].iter().collect()
        } else {
            String::new()
        }
    }

    /// Get the full buffer content with trailing spaces trimmed per line.
    pub fn to_string_trimmed(&self) -> String {
        self.buf.iter()
            .map(|row| row.iter().collect::<String>().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl RenderTarget for CharBuf {
    fn width(&self) -> usize { self.width }
    fn height(&self) -> usize { self.height }
    fn put_char(&mut self, x: usize, y: usize, ch: char) {
        if x < self.width && y < self.height {
            self.buf[y][x] = ch;
        }
    }
}

impl std::fmt::Display for CharBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, row) in self.buf.iter().enumerate() {
            if i > 0 { writeln!(f)?; }
            let s: String = row.iter().collect();
            write!(f, "{}", s)?;
        }
        Ok(())
    }
}

/// Render a computed table into a [`RenderTarget`].
///
/// Writes header (if present) on row 0, then data rows starting from row 1
/// (or 0 if no header). Uses the separator between columns.
pub fn render_to_target<Id>(
    target: &mut dyn RenderTarget,
    table: &crate::layout::ComputedTable<Id>,
    separator: &str,
) where
    Id: Eq + std::hash::Hash + Clone,
{
    let mut y = 0;

    if let Some(header) = &table.header {
        write_row(target, y, &header.cells, separator);
        y += 1;
    }

    for row in &table.rows {
        if y >= target.height() { break; }
        write_row(target, y, &row.cells, separator);
        y += 1;
    }
}

fn write_row(target: &mut dyn RenderTarget, y: usize, cells: &[String], separator: &str) {
    let mut x = 0;
    for (i, cell) in cells.iter().enumerate() {
        if i > 0 {
            target.put_str(x, y, separator);
            x += separator.chars().count();
        }
        target.put_str(x, y, cell);
        x += cell.chars().count();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_buf_basic() {
        let mut buf = CharBuf::new(10, 2);
        buf.put_str(0, 0, "Hello");
        buf.put_str(0, 1, "World");
        assert_eq!(buf.row_str(0), "Hello     ");
        assert_eq!(buf.row_str(1), "World     ");
    }

    #[test]
    fn char_buf_trimmed() {
        let mut buf = CharBuf::new(10, 2);
        buf.put_str(0, 0, "Hi");
        buf.put_str(0, 1, "There");
        assert_eq!(buf.to_string_trimmed(), "Hi\nThere");
    }
}
