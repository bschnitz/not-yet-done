use ratatui::style::{Color, Style};

use crate::widgets::select_list::SelectListStyle;
use crate::widgets::text_input::TextInputStyle;

/// Styling configuration for [`FilePicker`](super::FilePicker).
///
/// Each sub-component (the two [`TextInput`]s and the two [`SelectList`]s)
/// is given its own active/inactive style pair via these fields. The
/// picker forwards them to the embedded widgets when [`FilePicker::with_style`]
/// is called; sub-components that don't receive an override keep their
/// own defaults.
///
/// The chrome slots (`panel_bg`, `title_style`, `help_keys_style`,
/// `help_labels_style`) drive the top title + bottom help bar that
/// `FilePicker::view` renders when [`FilePicker::with_title`] is set.
///
/// Mirrors the active/inactive style convention used by [`TextInput`] and
/// [`SelectList`] themselves — the same `inactive_*`/`active_*` builders
/// you'd hand directly to those widgets are dropped into the matching
/// field here.
#[derive(Default, Clone)]
pub struct FilePickerStyle {
    /// Style overrides forwarded to the Directory + Glob text inputs.
    pub text_input_inactive: Option<TextInputStyle>,
    pub text_input_active: Option<TextInputStyle>,
    /// Style overrides forwarded to the Files + Selected select lists.
    pub select_list_inactive: Option<SelectListStyle>,
    pub select_list_active: Option<SelectListStyle>,
    /// Background fill for the chrome panel (the full picker area when a
    /// title is set).
    pub panel_bg: Option<Color>,
    /// Style for the title row at the top of the panel.
    pub title_style: Option<Style>,
    /// Style for the key glyphs in the help bar (e.g. `Ctrl+O`).
    pub help_keys_style: Option<Style>,
    /// Style for the descriptive labels next to the keys (e.g. `submit`).
    pub help_labels_style: Option<Style>,
    /// Style for the transient paste-error banner rendered below the
    /// title row when [`FilePicker::paste_clipboard_path`] could not
    /// resolve the clipboard content to a path.
    pub paste_error_style: Option<Style>,
}

impl FilePickerStyle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_text_input_inactive(mut self, style: TextInputStyle) -> Self {
        self.text_input_inactive = Some(style);
        self
    }

    pub fn with_text_input_active(mut self, style: TextInputStyle) -> Self {
        self.text_input_active = Some(style);
        self
    }

    pub fn with_select_list_inactive(mut self, style: SelectListStyle) -> Self {
        self.select_list_inactive = Some(style);
        self
    }

    pub fn with_select_list_active(mut self, style: SelectListStyle) -> Self {
        self.select_list_active = Some(style);
        self
    }

    pub fn with_panel_bg(mut self, bg: Color) -> Self {
        self.panel_bg = Some(bg);
        self
    }

    pub fn with_title_style(mut self, style: Style) -> Self {
        self.title_style = Some(style);
        self
    }

    pub fn with_help_keys_style(mut self, style: Style) -> Self {
        self.help_keys_style = Some(style);
        self
    }

    pub fn with_help_labels_style(mut self, style: Style) -> Self {
        self.help_labels_style = Some(style);
        self
    }

    pub fn with_paste_error_style(mut self, style: Style) -> Self {
        self.paste_error_style = Some(style);
        self
    }
}
