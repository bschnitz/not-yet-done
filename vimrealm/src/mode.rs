//! Editor modes.

/// The editing mode. Replace mode is deliberately absent for now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    /// Typing an ex command after `:`, or a pattern after `/`. The typed text
    /// lives on the editor, not in the mode, so the mode stays `Copy`.
    Command,
    /// Charwise selection: the span between the anchor and the cursor, both ends
    /// included.
    Visual,
    /// Linewise selection: whole lines between the anchor's and the cursor's.
    VisualLine,
}

impl Mode {
    /// Short label for a status line, in vim's shouting style.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Command => "COMMAND",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "VISUAL LINE",
        }
    }

    /// Whether a selection is being made — the two visual modes share almost
    /// every key, so most code only needs to ask this.
    pub fn is_visual(self) -> bool {
        matches!(self, Mode::Visual | Mode::VisualLine)
    }
}
