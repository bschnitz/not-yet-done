pub mod style;

pub use style::{BorderStyleType, HalfBorders, HalfPadding, TwoColumnStyle, ColumnWidth};

use ratatui::{buffer::Buffer, layout::Rect, style::Style, widgets::Widget};
use unicode_width::UnicodeWidthChar;

use style::{BorderChars};


/// A two-column layout widget.
///
/// Renders optional bordered panels side by side, with optional headers and a
/// configurable center divider.  Call [`render_layout`][Self::render_layout] to
/// obtain the two inner [`Rect`]s for rendering panel content.
///
/// # Layout model
///
/// ```text
/// Without collapse (panels adjacent):
///
///   ┌─ Header L ──┐┌─ Header R ──┐
///   │ content     ││ content     │
///   └─────────────┘└─────────────┘
///
/// With collapse (shared center column):
///
///   ┌─ Header L ──┬─ Header R ──┐
///   │ content     │ content     │
///   └─────────────┴─────────────┘
///
/// Top/bottom borders only, collapse, no side borders:
///
///   ─ Header L ─── Header R ───
///    content        content
///   ───────────────────────────
/// ```
#[derive(Debug, Clone)]
pub struct TwoColumnLayout<'a> {
    header_left:   Option<&'a str>,
    header_right:  Option<&'a str>,
    border_type:   BorderStyleType,
    left_borders:  HalfBorders,
    right_borders: HalfBorders,
    left_padding:  HalfPadding,
    right_padding: HalfPadding,
    collapse:      bool,
    left_width:    Option<ColumnWidth>,
    style:         TwoColumnStyle,
}

impl Default for TwoColumnLayout<'_> {
    fn default() -> Self { Self::new() }
}

impl<'a> TwoColumnLayout<'a> {
    pub fn new() -> Self {
        Self {
            header_left:   None,
            header_right:  None,
            border_type:   BorderStyleType::default(),
            left_borders:  HalfBorders::all(),
            right_borders: HalfBorders::all(),
            left_padding:  HalfPadding::default(),
            right_padding: HalfPadding::default(),
            collapse:      false,
            left_width:    None,
            style:         TwoColumnStyle::default(),
        }
    }

    // ── builder ──────────────────────────────────────────────────────────────

    pub fn header_left(mut self, h: &'a str) -> Self  { self.header_left  = Some(h); self }
    pub fn header_right(mut self, h: &'a str) -> Self { self.header_right = Some(h); self }

    pub fn border_type(mut self, t: BorderStyleType) -> Self { self.border_type = t; self }

    /// Borders for the left panel only.
    pub fn left_borders(mut self, b: HalfBorders) -> Self { self.left_borders = b; self }
    /// Borders for the right panel only.
    pub fn right_borders(mut self, b: HalfBorders) -> Self { self.right_borders = b; self }
    /// Same border configuration for both panels.
    pub fn borders(mut self, b: HalfBorders) -> Self {
        self.left_borders  = b;
        self.right_borders = b;
        self
    }

    /// Inner padding for the left panel only.
    pub fn left_padding(mut self, p: HalfPadding) -> Self { self.left_padding = p; self }
    /// Inner padding for the right panel only.
    pub fn right_padding(mut self, p: HalfPadding) -> Self { self.right_padding = p; self }
    /// Same inner padding for both panels.
    pub fn padding(mut self, p: HalfPadding) -> Self {
        self.left_padding  = p;
        self.right_padding = p;
        self
    }

    /// When `true`, the inner edges of the two panels share a single column
    /// connected at top/bottom by junction characters (`┬` / `┴`).
    pub fn collapse(mut self, v: bool) -> Self { self.collapse = v; self }

    /// Set the width of the left column.
    ///
    /// The right column receives the remaining space.  In collapse mode the
    /// center divider column is excluded before the width is computed.
    ///
    /// Without this call the two columns split the available width evenly.
    pub fn left_width(mut self, w: ColumnWidth) -> Self {
        self.left_width = Some(w);
        self
    }

    pub fn style(mut self, s: TwoColumnStyle) -> Self { self.style = s; self }

    /// Compute the outer width of the left panel in terminal columns.
    ///
    /// `total` is the full widget width.  In collapse mode one column is
    /// reserved for the shared center divider and excluded from the
    /// calculation so that `left_w + 1 + right_w == total` always holds.
    fn compute_left_width(&self, total: u16) -> u16 {
        let center = u16::from(self.collapse);
        let available = total.saturating_sub(center);
        match self.left_width {
            None => available / 2,
            Some(ColumnWidth::Fixed(n))   => n.min(available),
            Some(ColumnWidth::Percent(p)) => {
                let p = p.min(100) as u32;
                ((available as u32 * p / 100) as u16).min(available)
            }
        }
    }

    // ── main entry point ──────────────────────────────────────────────────────

    /// Render the layout decoration into `buf` and return `(left_inner, right_inner)`.
    ///
    /// Both returned [`Rect`]s already account for borders and padding; render
    /// panel content directly into them.
    ///
    /// The `content_left` / `content_right` styles from [`TwoColumnStyle`] fill
    /// the entire area inside the borders, **including** the padding cells.  This
    /// means setting a background colour on a content style will colour the
    /// padding uniformly together with the content area.
    pub fn render_layout(self, area: Rect, buf: &mut Buffer) -> (Rect, Rect) {
        if area.width < 2 || area.height < 1 {
            return (Rect::default(), Rect::default());
        }

        let chars  = self.border_type.chars();
        let bstyle = self.style.resolved_border();
        let hl_sty = self.style.resolved_header_left();
        let hr_sty = self.style.resolved_header_right();

        let left_w = self.compute_left_width(area.width);

        let center_x: Option<u16> = if self.collapse {
            Some(area.x + left_w)
        } else {
            None
        };

        let left_outer = Rect {
            x:      area.x,
            y:      area.y,
            width:  left_w,
            height: area.height,
        };

        let right_outer = if let Some(cx) = center_x {
            Rect {
                x:      cx + 1,
                y:      area.y,
                width:  area.width.saturating_sub(left_w + 1),
                height: area.height,
            }
        } else {
            Rect {
                x:      area.x + left_w,
                y:      area.y,
                width:  area.width - left_w,
                height: area.height,
            }
        };

        let collapsed = center_x.is_some();

        for y in area.top()..area.bottom() {
            let is_top = y == area.top();
            let is_bot = y == area.bottom() - 1;

            self.render_half_row(
                left_outer, y, is_top, is_bot,
                &self.left_borders, self.header_left, hl_sty, bstyle,
                buf, &chars,
                false,
                collapsed,
            );

            if let Some(cx) = center_x {
                let ch = self.center_char(is_top, is_bot, &chars);
                buf.set_string(cx, y, ch, bstyle);
            }

            self.render_half_row(
                right_outer, y, is_top, is_bot,
                &self.right_borders, self.header_right, hr_sty, bstyle,
                buf, &chars,
                collapsed,
                false,
            );
        }

        let no_pad      = HalfPadding::default();

        // Padding layer: the full area inside the borders.
        let left_pad_rect  = self.inner_rect(left_outer,  &self.left_borders,  &no_pad, collapsed, true);
        let right_pad_rect = self.inner_rect(right_outer, &self.right_borders, &no_pad, collapsed, false);
        Self::fill_rect_style(left_pad_rect,  self.style.resolved_padding_left(),  buf);
        Self::fill_rect_style(right_pad_rect, self.style.resolved_padding_right(), buf);

        // Content layer: inside the borders AND inside the padding.
        let left_inner  = self.inner_rect(left_outer,  &self.left_borders,  &self.left_padding,  collapsed, true);
        let right_inner = self.inner_rect(right_outer, &self.right_borders, &self.right_padding, collapsed, false);
        Self::fill_rect_style(left_inner,  self.style.resolved_content_left(),  buf);
        Self::fill_rect_style(right_inner, self.style.resolved_content_right(), buf);

        (left_inner, right_inner)
    }

    // ── internal helpers ──────────────────────────────────────────────────────

    fn center_char(&self, is_top: bool, is_bot: bool, chars: &BorderChars) -> &'static str {
        let has_v = self.left_borders.right || self.right_borders.left;
        if is_top {
            let has_h = self.left_borders.top || self.right_borders.top;
            match (has_h, has_v) {
                (true,  true)  => chars.top_junction,
                (true,  false) => chars.horizontal,
                (false, true)  => chars.vertical,
                (false, false) => " ",
            }
        } else if is_bot {
            let has_h = self.left_borders.bottom || self.right_borders.bottom;
            match (has_h, has_v) {
                (true,  true)  => chars.bottom_junction,
                (true,  false) => chars.horizontal,
                (false, true)  => chars.vertical,
                (false, false) => " ",
            }
        } else {
            if has_v { chars.vertical } else { " " }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_half_row(
        &self,
        outer:        Rect,
        y:            u16,
        is_top:       bool,
        is_bot:       bool,
        borders:      &HalfBorders,
        header:       Option<&str>,
        header_style: Style,
        border_style: Style,
        buf:          &mut Buffer,
        chars:        &BorderChars,
        left_free:    bool,
        right_free:   bool,
    ) {
        if outer.width == 0 { return; }

        let is_top_bdr = is_top && borders.top;
        let is_bot_bdr = !is_top_bdr && is_bot && borders.bottom;

        if is_top_bdr {
            self.render_h_border(
                outer, y, true, borders,
                header, header_style, border_style,
                buf, chars, left_free, right_free,
            );
        } else if is_bot_bdr {
            self.render_h_border(
                outer, y, false, borders,
                None, Style::default(), border_style,
                buf, chars, left_free, right_free,
            );
        } else {
            if !left_free && borders.left {
                buf.set_string(outer.x, y, chars.vertical, border_style);
            }
            if !right_free && borders.right && outer.width > 1 {
                buf.set_string(outer.x + outer.width - 1, y, chars.vertical, border_style);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_h_border(
        &self,
        outer:        Rect,
        y:            u16,
        is_top:       bool,
        borders:      &HalfBorders,
        header:       Option<&str>,
        header_style: Style,
        border_style: Style,
        buf:          &mut Buffer,
        chars:        &BorderChars,
        left_free:    bool,
        right_free:   bool,
    ) {
        let w  = outer.width;
        let x0 = outer.x;

        let left_ch: &str = if left_free {
            chars.horizontal
        } else if is_top {
            if borders.left { chars.top_left    } else { chars.horizontal }
        } else {
            if borders.left { chars.bottom_left } else { chars.horizontal }
        };

        let right_ch: &str = if right_free {
            chars.horizontal
        } else if is_top {
            if borders.right { chars.top_right    } else { chars.horizontal }
        } else {
            if borders.right { chars.bottom_right } else { chars.horizontal }
        };

        buf.set_string(x0, y, left_ch, border_style);
        if w == 1 { return; }

        buf.set_string(x0 + w - 1, y, right_ch, border_style);
        if w <= 2 { return; }

        let int_x   = x0 + 1;
        let int_end = x0 + w - 1;

        if is_top {
            self.render_header_fill(int_x, int_end, y, header, header_style, border_style, buf, chars);
        } else {
            for x in int_x..int_end {
                buf.set_string(x, y, chars.horizontal, border_style);
            }
        }
    }

    fn render_header_fill(
        &self,
        x_start:      u16,
        x_end:        u16,
        y:            u16,
        header:       Option<&str>,
        header_style: Style,
        border_style: Style,
        buf:          &mut Buffer,
        chars:        &BorderChars,
    ) {
        let padded: String = match header {
            Some(h) if !h.is_empty() => format!(" {h} "),
            _ => String::new(),
        };

        let mut x = x_start;
        for ch in padded.chars() {
            let cw = ch.width().unwrap_or(1) as u16;
            if x + cw > x_end { break; }
            buf.set_string(x, y, &ch.to_string(), header_style);
            x += cw;
        }
        while x < x_end {
            buf.set_string(x, y, chars.horizontal, border_style);
            x += 1;
        }
    }

    fn inner_rect(
        &self,
        outer:     Rect,
        borders:   &HalfBorders,
        padding:   &HalfPadding,
        collapsed: bool,
        is_left:   bool,
    ) -> Rect {
        let (ls, rs) = if collapsed {
            if is_left {
                (u16::from(borders.left) + padding.left, padding.right)
            } else {
                (padding.left, u16::from(borders.right) + padding.right)
            }
        } else {
            (
                u16::from(borders.left)  + padding.left,
                u16::from(borders.right) + padding.right,
            )
        };

        let ts = u16::from(borders.top)    + padding.top;
        let bs = u16::from(borders.bottom) + padding.bottom;

        Rect {
            x:      outer.x + ls,
            y:      outer.y + ts,
            width:  outer.width.saturating_sub(ls + rs),
            height: outer.height.saturating_sub(ts + bs),
        }
    }

    fn fill_rect_style(rect: Rect, style: Style, buf: &mut Buffer) {
        if style == Style::default() { return; }
        for y in rect.top()..rect.bottom() {
            for x in rect.left()..rect.right() {
                buf[(x, y)].set_style(style);
            }
        }
    }
}

impl Widget for TwoColumnLayout<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let _ = self.render_layout(area, buf);
    }
}
