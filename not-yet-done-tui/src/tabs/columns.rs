//! Column configuration for the task table.

use ratatui::style::Color;

/// Metadata for a single column.
pub struct ColumnMeta {
    pub id: &'static str,
    /// Abbreviated header shown in the table.
    pub header: &'static str,
    /// Full display name shown in the column config popup.
    pub display_name: &'static str,
    /// Which theme color to use for this column.
    pub color_key: ColorKey,
    /// Whether this column can be hidden.
    pub hideable: bool,
}

/// Which theme color to use for a column.
#[derive(Debug, Clone, Copy)]
pub enum ColorKey {
    Tertiary,
    Accent,
    Secondary,
    TextMed,
}

/// All known tracking columns in their default order.
pub const ALL_TRACKING_COLUMNS: &[ColumnMeta] = &[
    ColumnMeta { id: "taskpath",   header: "Taskpath", display_name: "Task path",  color_key: ColorKey::TextMed,   hideable: true },
    ColumnMeta { id: "task",       header: "Task",     display_name: "Task",       color_key: ColorKey::TextMed,   hideable: true },
    ColumnMeta { id: "marker",     header: "⏱",        display_name: "Tracking",   color_key: ColorKey::Accent,    hideable: true },
    ColumnMeta { id: "started",    header: "Started",  display_name: "Started",    color_key: ColorKey::Secondary, hideable: true },
    ColumnMeta { id: "ended",      header: "Ended",    display_name: "Ended",      color_key: ColorKey::Secondary, hideable: true },
    ColumnMeta { id: "duration",   header: "Duration", display_name: "Duration",   color_key: ColorKey::Tertiary,  hideable: true },
    ColumnMeta { id: "own",        header: "Own",      display_name: "Own",        color_key: ColorKey::Tertiary,  hideable: true },
    ColumnMeta { id: "cumulated",  header: "Cumulated",display_name: "Cumulated",  color_key: ColorKey::Accent,    hideable: true },
    ColumnMeta { id: "links",      header: "🔗",       display_name: "Links",      color_key: ColorKey::Accent,    hideable: true },
];

/// Default tracking column order. Both `taskpath` (parent chain) and
/// `task` (leaf description) are shown by default; either can be hidden
/// in the column-config popup.
pub fn default_tracking_column_ids() -> Vec<String> {
    ALL_TRACKING_COLUMNS.iter()
        .map(|c| c.id.to_string())
        .collect()
}

/// Look up metadata for a tracking column id.
pub fn tracking_column_meta(id: &str) -> Option<&'static ColumnMeta> {
    ALL_TRACKING_COLUMNS.iter().find(|c| c.id == id)
}

/// Resolve a ColorKey to a theme color.
pub fn resolve_color(key: ColorKey, theme: &crate::ui::theme::Theme) -> Color {
    match key {
        ColorKey::Tertiary => theme.tertiary(),
        ColorKey::Accent => theme.accent(),
        ColorKey::Secondary => theme.secondary(),
        ColorKey::TextMed => theme.text_med(),
    }
}
