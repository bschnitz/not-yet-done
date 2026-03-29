use ratatui::style::Style;

/// Width specification for the left column of a [`TwoColumnLayout`][super::TwoColumnLayout].
///
/// The right column always receives the remaining width.  In collapse mode the
/// shared center divider column is subtracted from the total before the ratio
/// is applied, so neither panel ever overlaps it.
///
/// ```text
/// Fixed(20)    — left panel is exactly 20 terminal columns wide
/// Percent(30)  — left panel takes 30 % of the available width
/// ```
#[derive(Debug, Clone, Copy)]
pub enum ColumnWidth {
    /// Left column takes exactly `n` terminal columns.
    Fixed(u16),
    /// Left column takes `p` percent of the available width (clamped to 0–100).
    Percent(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BorderStyleType {
    #[default]
    Plain,
    Rounded,
    Double,
    Thick,
}

pub(crate) struct BorderChars {
    pub top_left:        &'static str,
    pub top_right:       &'static str,
    pub bottom_left:     &'static str,
    pub bottom_right:    &'static str,
    pub horizontal:      &'static str,
    pub vertical:        &'static str,
    pub top_junction:    &'static str,
    pub bottom_junction: &'static str,
}

impl BorderStyleType {
    pub(crate) fn chars(self) -> BorderChars {
        match self {
            Self::Plain => BorderChars {
                top_left: "┌", top_right: "┐",
                bottom_left: "└", bottom_right: "┘",
                horizontal: "─", vertical: "│",
                top_junction: "┬", bottom_junction: "┴",
            },
            Self::Rounded => BorderChars {
                top_left: "╭", top_right: "╮",
                bottom_left: "╰", bottom_right: "╯",
                horizontal: "─", vertical: "│",
                top_junction: "┬", bottom_junction: "┴",
            },
            Self::Double => BorderChars {
                top_left: "╔", top_right: "╗",
                bottom_left: "╚", bottom_right: "╝",
                horizontal: "═", vertical: "║",
                top_junction: "╦", bottom_junction: "╩",
            },
            Self::Thick => BorderChars {
                top_left: "┏", top_right: "┓",
                bottom_left: "┗", bottom_right: "┛",
                horizontal: "━", vertical: "┃",
                top_junction: "┳", bottom_junction: "┻",
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct HalfBorders {
    pub top:    bool,
    pub right:  bool,
    pub bottom: bool,
    pub left:   bool,
}

impl Default for HalfBorders {
    fn default() -> Self {
        Self { top: true, right: true, bottom: true, left: true }
    }
}

impl HalfBorders {
    pub fn all() -> Self  { Self::default() }
    pub fn none() -> Self { Self { top: false, right: false, bottom: false, left: false } }

    pub fn top(mut self, v: bool) -> Self    { self.top    = v; self }
    pub fn right(mut self, v: bool) -> Self  { self.right  = v; self }
    pub fn bottom(mut self, v: bool) -> Self { self.bottom = v; self }
    pub fn left(mut self, v: bool) -> Self   { self.left   = v; self }
    pub fn horizontal(mut self, v: bool) -> Self { self.top = v; self.bottom = v; self }
    pub fn vertical(mut self, v: bool) -> Self   { self.left = v; self.right = v; self }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HalfPadding {
    pub top:    u16,
    pub right:  u16,
    pub bottom: u16,
    pub left:   u16,
}

impl HalfPadding {
    pub fn new() -> Self { Self::default() }
    pub fn all(v: u16) -> Self { Self { top: v, right: v, bottom: v, left: v } }

    pub fn horizontal(mut self, v: u16) -> Self { self.left = v; self.right  = v; self }
    pub fn vertical(mut self, v: u16) -> Self   { self.top  = v; self.bottom = v; self }

    pub fn top(mut self, v: u16) -> Self    { self.top    = v; self }
    pub fn right(mut self, v: u16) -> Self  { self.right  = v; self }
    pub fn bottom(mut self, v: u16) -> Self { self.bottom = v; self }
    pub fn left(mut self, v: u16) -> Self   { self.left   = v; self }
}

/// Visual styles for [`TwoColumnLayout`][super::TwoColumnLayout].
///
/// Filling order per panel (back to front):
///   1. `padding_*` — the band of cells between the border and the content rect
///   2. `content_*` — the inner content rect (after padding is subtracted)
///
/// Both default to `None` (transparent), so the two layers are independent.
#[derive(Debug, Clone, Default)]
pub struct TwoColumnStyle {
    /// Style applied to all border characters.
    pub border: Option<Style>,
    /// Style applied to the left-panel header text.
    pub header_left: Option<Style>,
    /// Style applied to the right-panel header text.
    pub header_right: Option<Style>,
    /// Background style for the left-panel padding cells only.
    pub padding_left: Option<Style>,
    /// Background style for the right-panel padding cells only.
    pub padding_right: Option<Style>,
    /// Background style for the left content area (inside padding).
    pub content_left: Option<Style>,
    /// Background style for the right content area (inside padding).
    pub content_right: Option<Style>,
}

impl TwoColumnStyle {
    pub fn new() -> Self { Self::default() }

    pub fn border(mut self, s: Style) -> Self        { self.border        = Some(s); self }
    pub fn header_left(mut self, s: Style) -> Self   { self.header_left   = Some(s); self }
    pub fn header_right(mut self, s: Style) -> Self  { self.header_right  = Some(s); self }
    /// Set the same style for both panel headers.
    pub fn headers(mut self, s: Style) -> Self {
        self.header_left  = Some(s);
        self.header_right = Some(s);
        self
    }

    /// Padding background for the left panel only.
    pub fn padding_style_left(mut self, s: Style) -> Self  { self.padding_left  = Some(s); self }
    /// Padding background for the right panel only.
    pub fn padding_style_right(mut self, s: Style) -> Self { self.padding_right = Some(s); self }
    /// Same padding background for both panels.
    pub fn padding_style(mut self, s: Style) -> Self {
        self.padding_left  = Some(s);
        self.padding_right = Some(s);
        self
    }

    pub fn content_left(mut self, s: Style) -> Self  { self.content_left  = Some(s); self }
    pub fn content_right(mut self, s: Style) -> Self { self.content_right = Some(s); self }
    /// Set the same style for both panel content areas.
    pub fn content(mut self, s: Style) -> Self {
        self.content_left  = Some(s);
        self.content_right = Some(s);
        self
    }

    pub(crate) fn resolved_border(&self)          -> Style { self.border.unwrap_or_default()        }
    pub(crate) fn resolved_header_left(&self)     -> Style { self.header_left.unwrap_or_default()   }
    pub(crate) fn resolved_header_right(&self)    -> Style { self.header_right.unwrap_or_default()  }
    pub(crate) fn resolved_padding_left(&self)    -> Style { self.padding_left.unwrap_or_default()  }
    pub(crate) fn resolved_padding_right(&self)   -> Style { self.padding_right.unwrap_or_default() }
    pub(crate) fn resolved_content_left(&self)    -> Style { self.content_left.unwrap_or_default()  }
    pub(crate) fn resolved_content_right(&self)   -> Style { self.content_right.unwrap_or_default() }
}
