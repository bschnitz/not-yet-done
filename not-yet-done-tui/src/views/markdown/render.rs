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

use not_yet_done_ratatui::{ImageLineRef, TableWidgetCell, TableWidgetLine};

use crate::ui::theme::Theme;
use crate::views::images::ImageStore;

use super::MdTheme;

/// Render `body` as markdown, soft-wrapped to `width` columns, using the
/// app theme (via [`MdTheme`]). Returns the styled lines as produced by
/// `ratatui-markdown` (one logical paragraph may span several lines).
pub fn render_markdown_lines(body: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let renderer = MarkdownRenderer::new(width.max(1));
    let blocks = renderer.parse(body);
    renderer.render(&blocks, &MdTheme(theme))
}

/// Like [`render_markdown_lines`], but with terminal graphics: every
/// `![alt](url)` standing alone on its line whose pixels `store` already has
/// becomes a block of blank lines with the picture's height, and the returned
/// second vector marks those lines with the back-reference the table widget
/// needs to draw into them. Every other line maps to `None`.
///
/// A picture the store doesn't have yet is *queued* by the store and rendered
/// as the ordinary `[image: …]` fallback span for now; when the bytes arrive
/// the caller rebuilds and this call reserves the space.
///
/// The placement↔URL pairing is positional: `ratatui-markdown` emits one
/// placement per resolved image, in the order it resolved them. That holds
/// because [`ImageStore`] refuses to resolve a picture that would reserve
/// zero cells — the one case in which the renderer would consume a resolved
/// entry without emitting a placement.
pub fn render_markdown_lines_with_images(
    body: &str,
    width: usize,
    theme: &Theme,
    store: &mut ImageStore,
) -> (Vec<Line<'static>>, Vec<Option<ImageLineRef>>) {
    if !store.enabled() || width == 0 {
        let lines = render_markdown_lines(body, width, theme);
        let refs = vec![None; lines.len()];
        return (lines, refs);
    }

    let renderer = MarkdownRenderer::new(width.max(1));
    let (blocks, resolved) = renderer.parse_with_images(body, store);
    let max_w = width.min(u16::MAX as usize) as u16;
    let max_h = store.max_height();
    let out = renderer.render_full(&blocks, &MdTheme(theme), &resolved, store, max_w, max_h);

    let mut refs: Vec<Option<ImageLineRef>> = vec![None; out.lines.len()];
    for (i, placement) in out.images.iter().enumerate() {
        let Some(image) = resolved.get(i) else {
            // Defensive: a desynced pairing must lose the picture, not
            // attach it to the wrong URL.
            break;
        };
        let col = placement.col.min(u16::MAX as usize) as u16;
        let key = store.register(&image.path, placement.width_cells, placement.height_cells);
        for (n, line_ref) in store.line_refs(key, col).into_iter().enumerate() {
            if let Some(slot) = refs.get_mut(placement.row + n) {
                *slot = Some(line_ref);
            }
        }
    }
    (out.lines, refs)
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
    lines_to_widget_lines_with_images(lines, Vec::new(), builder, highlight_on_select)
}

/// [`lines_to_widget_lines`] plus the image back-references produced by
/// [`render_markdown_lines_with_images`]: line `i` carries `refs[i]`, so the
/// blank lines reserved for a picture are the ones the table paints into.
/// A shorter (or empty) `refs` simply leaves the remaining lines unmarked.
pub fn lines_to_widget_lines_with_images(
    lines: Vec<Line<'static>>,
    refs: Vec<Option<ImageLineRef>>,
    builder: &mut StyleMapBuilder,
    highlight_on_select: bool,
) -> Vec<TableWidgetLine> {
    let mut refs = refs.into_iter();
    lines
        .into_iter()
        .map(|line| {
            let image = refs.next().flatten();
            let segments: Vec<(String, Option<usize>)> = line
                .spans
                .into_iter()
                .map(|span| {
                    let id = builder.intern(span.style);
                    (span.content.into_owned(), Some(id))
                })
                .collect();
            let widget_line = TableWidgetLine::new(vec![TableWidgetCell::from_segments(segments)])
                .with_highlight_on_select(highlight_on_select);
            match image {
                Some(r) => widget_line.with_image(r),
                None => widget_line,
            }
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

    // -- inline images ----------------------------------------------------

    use crate::views::images::ImageStore;
    use std::sync::Arc;

    /// A store that can show pictures, with a known 10x20 px cell size.
    fn image_store(max_height: u16) -> ImageStore {
        ImageStore::with_picker(
            Some(ratatui_image::picker::Picker::halfblocks()),
            max_height,
        )
    }

    fn pixels(w: u32, h: u32) -> Arc<image::DynamicImage> {
        Arc::new(image::DynamicImage::ImageRgba8(image::RgbaImage::new(w, h)))
    }

    #[test]
    fn a_store_without_graphics_renders_the_plain_fallback() {
        let t = theme();
        let mut store = ImageStore::with_picker(None, 20);
        let (lines, refs) = render_markdown_lines_with_images("![alt](u1)", 40, &t, &mut store);
        assert_eq!(lines.len(), refs.len());
        assert!(refs.iter().all(Option::is_none));
        assert!(store.take_wanted().is_empty(), "nothing to download");
    }

    #[test]
    fn an_undownloaded_picture_is_queued_and_stays_text() {
        let t = theme();
        let mut store = image_store(20);
        let (lines, refs) =
            render_markdown_lines_with_images("hi\n\n![alt](u1)", 40, &t, &mut store);
        assert!(refs.iter().all(Option::is_none), "nothing to draw yet");
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(text.contains("[image: alt]"), "fallback span: {text}");
        assert_eq!(store.take_wanted(), vec!["u1".to_string()]);
    }

    #[test]
    fn a_downloaded_picture_reserves_its_lines_and_marks_them() {
        let t = theme();
        let mut store = image_store(20);
        // 200x80 px at 10x20 px per cell → 20x4 cells.
        store.insert_decoded("u1", Some(pixels(200, 80)));
        let (lines, refs) = render_markdown_lines_with_images("![alt](u1)", 40, &t, &mut store);
        assert_eq!(lines.len(), refs.len());

        let marked: Vec<&ImageLineRef> = refs.iter().flatten().collect();
        assert_eq!(marked.len(), 4, "one ref per reserved row");
        let key = marked[0].key;
        for (i, r) in marked.iter().enumerate() {
            assert_eq!(r.key, key, "all rows belong to the same picture");
            assert_eq!((r.width, r.height), (20, 4));
            assert_eq!(r.row_in_image, i as u16);
        }
        // The reserved lines carry no text — the picture goes on top of them.
        let reserved: Vec<usize> = refs
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().map(|_| i))
            .collect();
        for i in reserved {
            assert!(lines[i].spans.iter().all(|s| s.content.is_empty()));
        }
    }

    #[test]
    fn two_pictures_keep_their_own_urls_and_sizes() {
        let t = theme();
        let mut store = image_store(20);
        store.insert_decoded("u1", Some(pixels(200, 80))); // 20x4 cells
        store.insert_decoded("u2", Some(pixels(100, 40))); // 10x2 cells
        let (_lines, refs) =
            render_markdown_lines_with_images("![a](u1)\n\ntext\n\n![b](u2)", 40, &t, &mut store);

        let mut sizes: Vec<(u16, u16)> = Vec::new();
        for r in refs.iter().flatten() {
            if !sizes.contains(&(r.width, r.height)) {
                sizes.push((r.width, r.height));
            }
        }
        assert_eq!(sizes, vec![(20, 4), (10, 2)], "in document order");
        // The keys must differ, and match what the store registered per URL.
        let keys: Vec<u64> = refs.iter().flatten().map(|r| r.key).collect();
        assert_eq!(keys[0], store.register("u1", 20, 4));
        assert_eq!(*keys.last().unwrap(), store.register("u2", 10, 2));
    }

    #[test]
    fn a_mix_of_pending_and_ready_pictures_pairs_the_right_one() {
        let t = theme();
        let mut store = image_store(20);
        // Only the *second* picture is downloaded: the first stays text, so
        // the single placement must be paired with `u2`, not `u1`.
        store.insert_decoded("u2", Some(pixels(100, 40)));
        let (_lines, refs) =
            render_markdown_lines_with_images("![a](u1)\n\n![b](u2)", 40, &t, &mut store);
        let keys: Vec<u64> = refs.iter().flatten().map(|r| r.key).collect();
        assert!(!keys.is_empty(), "the ready picture is drawn");
        assert!(keys.iter().all(|k| *k == store.register("u2", 10, 2)));
        assert_eq!(store.take_wanted(), vec!["u1".to_string()]);
    }

    #[test]
    fn widget_lines_carry_the_image_refs_through() {
        let t = theme();
        let mut store = image_store(20);
        store.insert_decoded("u1", Some(pixels(200, 80)));
        let (lines, refs) = render_markdown_lines_with_images("![alt](u1)", 40, &t, &mut store);
        let mut builder = StyleMapBuilder::default();
        let widget_lines = lines_to_widget_lines_with_images(lines, refs, &mut builder, true);
        assert_eq!(widget_lines.iter().filter(|l| l.image.is_some()).count(), 4);
        assert!(widget_lines.iter().all(|l| l.highlight_on_select));
    }

    #[test]
    fn the_plain_converter_leaves_every_line_imageless() {
        let t = theme();
        let lines = render_markdown_lines("plain **text**", 40, &t);
        let mut builder = StyleMapBuilder::default();
        let widget_lines = lines_to_widget_lines(lines, &mut builder, true);
        assert!(widget_lines.iter().all(|l| l.image.is_none()));
    }
}
