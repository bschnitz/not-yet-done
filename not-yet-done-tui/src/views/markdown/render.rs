//! Phase 1: turn a markdown body into styled, soft-wrapped table lines.
//!
//! Two pure functions plus a small style-interning helper. No state, no I/O —
//! they're called from the content-view build path (Phase 4) but are testable
//! in isolation here.
//!
//! Flow: `body` → [`render_markdown_lines`] (ratatui-markdown soft-wraps to
//! `width`) → `Vec<Line>` → [`lines_to_widget_lines`] (each `Line` becomes one
//! [`TableWidgetLine`] whose single cell carries the spans as styled
//! `segments`). The per-span styles are interned into a [`StyleMapBuilder`]
//! that is shared with the surrounding table's column styles, so the existing
//! `segments` + `StyleMap` render path (which colors each span's fg/modifiers
//! and applies selection as background only) needs no change.

use ratatui::style::Style;
use ratatui::text::Line;
use ratatui_markdown::markdown::MarkdownRenderer;

use not_yet_done_ratatui::{TableWidgetCell, TableWidgetLine};

use crate::ui::theme::Theme;

use super::MdTheme;

/// Render `body` as markdown, soft-wrapped to `width` columns, using the
/// app theme (via [`MdTheme`]). Returns the styled lines as produced by
/// `ratatui-markdown` (one logical paragraph may span several lines).
pub fn render_markdown_lines(body: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let renderer = MarkdownRenderer::new(width.max(1));
    let blocks = renderer.parse(body);
    renderer.render(&blocks, &MdTheme(theme))
}

/// Convert rendered markdown [`Line`]s into [`TableWidgetLine`]s. Each line
/// becomes one widget line with a single `from_segments` cell; every span's
/// style is interned into `builder` and referenced by id, so the styles end
/// up in the table's shared [`StyleMap`].
///
/// `highlight_on_select` is propagated to every produced line (the body block
/// is part of the row's selection highlight).
pub fn lines_to_widget_lines(
    lines: Vec<Line<'static>>,
    builder: &mut StyleMapBuilder,
    highlight_on_select: bool,
) -> Vec<TableWidgetLine> {
    lines
        .into_iter()
        .map(|line| {
            let segments: Vec<(String, Option<usize>)> = line
                .spans
                .into_iter()
                .map(|span| {
                    let id = builder.intern(span.style);
                    (span.content.into_owned(), Some(id))
                })
                .collect();
            TableWidgetLine::new(vec![TableWidgetCell::from_segments(segments)])
                .with_highlight_on_select(highlight_on_select)
        })
        .collect()
}

/// Builds the table's `StyleMap` incrementally, deduplicating identical
/// styles so repeated spans (most markdown text shares a handful of styles)
/// reuse one id.
///
/// Seed it with the existing per-column styles via [`StyleMapBuilder::from_styles`]
/// so markdown style ids don't collide with column style ids, then hand the
/// final `Vec<Style>` to `StyleMap::new`.
#[derive(Debug, Default)]
pub struct StyleMapBuilder {
    styles: Vec<Style>,
}

impl StyleMapBuilder {
    /// A builder pre-seeded with `styles` (their ids are `0..styles.len()`).
    pub fn from_styles(styles: Vec<Style>) -> Self {
        Self { styles }
    }

    /// Return the id of `style`, appending it if not already present.
    ///
    /// Dedup is a linear scan: the distinct-style count per rendered body is
    /// tiny (a few inline variants), so a `HashMap` (and `Style: Hash`) buys
    /// nothing.
    pub fn intern(&mut self, style: Style) -> usize {
        if let Some(i) = self.styles.iter().position(|s| *s == style) {
            return i;
        }
        self.styles.push(style);
        self.styles.len() - 1
    }

    /// Consume the builder into the flat style list for `StyleMap::new`.
    pub fn into_styles(self) -> Vec<Style> {
        self.styles
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThemeConfig;

    fn theme() -> Theme {
        Theme::new(ThemeConfig::default())
    }

    #[test]
    fn style_map_builder_dedups_and_preserves_seed() {
        let seed_a = Style::default().fg(ratatui::style::Color::Red);
        let seed_b = Style::default().fg(ratatui::style::Color::Green);
        let mut b = StyleMapBuilder::from_styles(vec![seed_a, seed_b]);
        // Seeded styles keep their ids.
        assert_eq!(b.intern(seed_a), 0);
        assert_eq!(b.intern(seed_b), 1);
        // A new style appends; interning it again returns the same id.
        let new = Style::default().fg(ratatui::style::Color::Blue);
        assert_eq!(b.intern(new), 2);
        assert_eq!(b.intern(new), 2);
        assert_eq!(b.into_styles().len(), 3);
    }

    #[test]
    fn renders_multiline_body_to_multiple_lines() {
        let t = theme();
        let body = "First paragraph.\n\nSecond paragraph with **bold** text.";
        let lines = render_markdown_lines(body, 80, &t);
        assert!(lines.len() > 1, "expected several rendered lines");
    }

    #[test]
    fn soft_wrap_narrow_yields_more_lines_than_wide() {
        let t = theme();
        let body = "This is a deliberately long single paragraph that must wrap \
                    across several physical lines when the available width is \
                    small, and across fewer lines when the width is large.";
        let narrow = render_markdown_lines(body, 30, &t).len();
        let wide = render_markdown_lines(body, 120, &t).len();
        assert!(narrow > wide, "narrow={narrow} wide={wide}");
    }

    #[test]
    fn lines_convert_to_widget_lines_with_interned_segments() {
        let t = theme();
        let body = "Plain text with **bold** and *italic* runs.";
        let lines = render_markdown_lines(body, 60, &t);
        let mut builder = StyleMapBuilder::default();
        let widget_lines = lines_to_widget_lines(lines, &mut builder, true);

        assert!(!widget_lines.is_empty());
        // Every line has exactly one cell built from segments.
        for wl in &widget_lines {
            assert_eq!(wl.cells.len(), 1);
            assert!(wl.highlight_on_select);
        }
        // The first non-empty line should carry several styled segments
        // (plain + bold + italic differ), all with a valid interned id.
        let styles = builder.into_styles();
        let multi = widget_lines
            .iter()
            .find(|wl| wl.cells[0].segments.len() > 1)
            .expect("a line with multiple inline runs");
        for (_text, id) in &multi.cells[0].segments {
            let id = id.expect("segment carries a style id");
            assert!(id < styles.len(), "id {id} out of range {}", styles.len());
        }
    }

    #[test]
    fn empty_body_does_not_panic() {
        let t = theme();
        let lines = render_markdown_lines("", 40, &t);
        let mut builder = StyleMapBuilder::default();
        let _ = lines_to_widget_lines(lines, &mut builder, false);
    }
}
