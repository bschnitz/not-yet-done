/// How selection markers are rendered next to each item.
#[derive(Debug, Clone)]
pub enum SelectionMarker {
    /// No visible marker.
    None,
    /// `[x]` / `[ ]`
    Checkbox,
    /// `(●)` / `( )`
    Radio,
    /// Custom strings for selected/unselected state.
    Custom {
        selected: &'static str,
        unselected: &'static str,
    },
}

impl Default for SelectionMarker {
    fn default() -> Self {
        Self::None
    }
}

impl SelectionMarker {
    /// Returns the display string for a given selection state.
    pub fn text(&self, selected: bool) -> &str {
        match self {
            Self::None => "",
            Self::Checkbox => {
                if selected {
                    "[x] "
                } else {
                    "[ ] "
                }
            }
            Self::Radio => {
                if selected {
                    "(●) "
                } else {
                    "( ) "
                }
            }
            Self::Custom {
                selected: s,
                unselected: u,
            } => {
                if selected {
                    s
                } else {
                    u
                }
            }
        }
    }
}

/// Selection mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    /// Multiple items can be selected.
    Multi,
    /// Only one item at a time.
    Single,
}

impl Default for SelectionMode {
    fn default() -> Self {
        Self::Multi
    }
}

/// How the filter input matches items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    /// Case-insensitive substring match. Preserves item order.
    Substring,
    /// Fuzzy match via `fuzzy-matcher`'s SkimMatcher. Results are sorted by
    /// descending score (best match first).
    Fuzzy,
}

impl Default for FilterMode {
    fn default() -> Self {
        Self::Substring
    }
}
