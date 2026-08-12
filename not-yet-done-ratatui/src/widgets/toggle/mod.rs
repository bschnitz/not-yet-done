mod component;
mod render;
pub mod style;

pub use style::{ToggleStyle, ToggleStyleType};

/// A single boolean toggle field implementing tuirealm's [`Component`].
///
/// Occupies **two rows** (title + value). State is owned by the component;
/// construct once and drive it via `perform(Cmd::Toggle)` / `attr(...)` — do not
/// rebuild per frame.
///
/// ```rust
/// use not_yet_done_ratatui::widgets::toggle::Toggle;
///
/// let toggle = Toggle::default()
///     .with_title("All day")
///     .with_labels("Yes", "No")
///     .with_value(false);
/// ```
///
/// [`Component`]: tuirealm::component::Component
pub struct Toggle {
    // --- framework state ---
    pub(crate) focused: bool,

    // --- value ---
    pub(crate) on: bool,

    // --- configuration (set once at construction) ---
    pub(crate) title: String,
    pub(crate) on_label: String,
    pub(crate) off_label: String,
    pub(crate) inactive_style: ToggleStyle,
    pub(crate) active_style: ToggleStyle,
}

impl Default for Toggle {
    fn default() -> Self {
        Self {
            focused: false,
            on: false,
            title: String::new(),
            on_label: "Yes".into(),
            off_label: "No".into(),
            inactive_style: ToggleStyle::default(),
            active_style: ToggleStyle::default(),
        }
    }
}

impl Toggle {
    /// Sets the title displayed above the value line.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Sets the initial boolean value.
    pub fn with_value(mut self, on: bool) -> Self {
        self.on = on;
        self
    }

    /// Sets the labels shown next to the marker for the on / off states.
    pub fn with_labels(
        mut self,
        on_label: impl Into<String>,
        off_label: impl Into<String>,
    ) -> Self {
        self.on_label = on_label.into();
        self.off_label = off_label.into();
        self
    }

    /// Style applied when this component does not have focus.
    pub fn with_inactive_style(mut self, style: ToggleStyle) -> Self {
        self.inactive_style = style;
        self
    }

    /// Style applied when this component has focus.
    pub fn with_active_style(mut self, style: ToggleStyle) -> Self {
        self.active_style = style;
        self
    }

    /// Returns the current boolean value.
    pub fn is_on(&self) -> bool {
        self.on
    }
}
