use ratatui::style::Style;

/// Identifies the visual part of a [`super::LeaderList`] to be styled.
///
/// A rendered line is `left + filler + right`, where `left = a + post` and
/// `right = pre + b`. Each of the three segments has its own style slot; the
/// `Cursor` slot is an overlay patched over the whole line of the selected row
/// (typically just a background colour).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaderListStyleType {
    /// Left segment — `a` plus its postfix.
    Left = 0,
    /// The filler run between the two segments.
    Filler = 1,
    /// Right segment — the prefix plus `b`.
    Right = 2,
    /// Overlay applied across the whole line of the selected row.
    ///
    /// Patched *over* the per-segment styles, so setting only a background here
    /// highlights the row while keeping each segment's foreground colour.
    Cursor = 3,
    /// The optional title/header line rendered above the entries.
    Title = 4,
    /// The optional status line (`N entries · Page x/y`) below the entries.
    Status = 5,
    /// The optional fuzzy-search prompt line (`/query`) above the entries.
    Search = 6,
    /// Overlay patched over the `left` segment of *marked* rows (multi-select).
    ///
    /// Like [`Cursor`], it is patched *over* the left style, so setting a
    /// foreground (and/or modifier) recolours a marked row's label — and its
    /// marker glyph — while leaving unmarked rows untouched. The cursor overlay
    /// is applied on top, so the selected row's background still wins.
    ///
    /// [`Cursor`]: LeaderListStyleType::Cursor
    Marked = 7,
}

/// Styling configuration for the [`super::LeaderList`] widget.
///
/// Every slot is `Option<Style>`: `None` means "not configured" and falls back
/// to `Style::default()` inside render code via [`resolved_style`]. Keeping the
/// slots optional lets an outer form or theme inject fallbacks.
///
/// [`resolved_style`]: LeaderListStyle::resolved_style
#[derive(Debug, Clone, Default)]
pub struct LeaderListStyle {
    /// Per-slot styles — indexed by `LeaderListStyleType as usize`.
    pub styles: [Option<Style>; 8],
}

impl LeaderListStyle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the style for one slot (builder style).
    pub fn set_style(mut self, slot: LeaderListStyleType, style: Style) -> Self {
        self.styles[slot as usize] = Some(style);
        self
    }

    /// Returns `Some(&style)` if this slot was explicitly configured, `None`
    /// otherwise. Useful for a form layer deciding whether to apply a fallback.
    pub fn style(&self, slot: LeaderListStyleType) -> Option<&Style> {
        self.styles[slot as usize].as_ref()
    }

    /// Returns the configured style or `Style::default()` as fallback.
    pub fn resolved_style(&self, slot: LeaderListStyleType) -> Style {
        self.styles[slot as usize].unwrap_or_default()
    }
}
