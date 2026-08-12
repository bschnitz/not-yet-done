//! Styling slots.
//!
//! Every slot is `Option<Style>`: `None` means "not configured", so a host can
//! leave the widget alone and let its own theme flow in later. Render code goes
//! through [`VimStyle::resolved`], which falls back to a sensible default —
//! nothing here hardcodes a colour that a host cannot override.

use ratatui::style::{Modifier, Style};

/// The visual parts of the editor that can be styled independently.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimStyleType {
    /// Buffer text.
    Text = 0,
    /// The block cursor of normal and visual mode. Insert and command mode use
    /// the terminal's own cursor instead, which no style can reach.
    Cursor = 1,
    /// Line-number gutter.
    Gutter = 2,
    /// The mode indicator (`-- INSERT --`) on the status line.
    Mode = 3,
    /// The rest of the status line.
    Status = 4,
    /// The `:` command line and its messages.
    CommandLine = 5,
    /// The visual-mode selection.
    Selection = 6,
}

const SLOTS: usize = 7;

#[derive(Debug, Clone, Default)]
pub struct VimStyle {
    styles: [Option<Style>; SLOTS],
}

impl VimStyle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, slot: VimStyleType, style: Style) -> &mut Self {
        self.styles[slot as usize] = Some(style);
        self
    }

    /// Builder form of [`Self::set`].
    pub fn with(mut self, slot: VimStyleType, style: Style) -> Self {
        self.set(slot, style);
        self
    }

    pub fn get(&self, slot: VimStyleType) -> Option<Style> {
        self.styles[slot as usize]
    }

    /// The style to draw with: the configured one, else the built-in fallback.
    pub fn resolved(&self, slot: VimStyleType) -> Style {
        self.get(slot).unwrap_or_else(|| fallback(slot))
    }
}

/// Defaults that only use modifiers, never colours — so the widget inherits
/// whatever palette the terminal or the host block already sets.
fn fallback(slot: VimStyleType) -> Style {
    match slot {
        VimStyleType::Cursor => Style::default().add_modifier(Modifier::REVERSED),
        // Not reversed: the cursor is, and it has to stay visible *inside* the
        // selection it sits in.
        VimStyleType::Selection => Style::default().add_modifier(Modifier::UNDERLINED),
        VimStyleType::Gutter => Style::default().add_modifier(Modifier::DIM),
        VimStyleType::Mode => Style::default().add_modifier(Modifier::BOLD),
        _ => Style::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_slots_fall_back_without_colour() {
        let style = VimStyle::new();
        assert_eq!(style.get(VimStyleType::Text), None);
        assert_eq!(style.resolved(VimStyleType::Text), Style::default());
        assert!(
            style
                .resolved(VimStyleType::Cursor)
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert_eq!(
            style.resolved(VimStyleType::Cursor).fg,
            None,
            "no hardcoded colour"
        );
    }

    #[test]
    fn a_configured_slot_wins() {
        let style = VimStyle::new().with(
            VimStyleType::Text,
            Style::default().add_modifier(Modifier::ITALIC),
        );
        assert!(
            style
                .resolved(VimStyleType::Text)
                .add_modifier
                .contains(Modifier::ITALIC)
        );
    }
}
