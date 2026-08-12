//! Markdown rendering for content views (Stoat chat bodies, long-text columns).
//!
//! The heavy lifting is done by `ratatui-markdown` (ratatui 0.29, pinned
//! `=0.3.6`). This module is the **TUI-layer** glue only — the table/layout
//! crates know nothing about markdown. See `docs/plan-markdown-render.md`.
//!
//! Phase 0 (this file + [`theme_bridge`]) wires the dependency and bridges
//! `ratatui-markdown`'s `RichTextTheme` onto our own [`crate::ui::theme::Theme`]
//! so no colors are hardcoded. Later phases add the render → widget-line
//! conversion.

pub mod render;
pub mod theme_bridge;

pub use render::{
    StyleMapBuilder, lines_to_widget_lines, lines_to_widget_lines_with_images,
    render_markdown_lines, render_markdown_lines_with_images,
};
pub use theme_bridge::MdTheme;
