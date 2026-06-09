//! Column configuration for the task table.

use not_yet_done_content::{SortDirection, SortKey, SortableColumn};
use not_yet_done_core::entity::task::Model as Task;
use not_yet_done_forest::TREE_COLUMN;
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

/// All known task columns in their default order.
pub const ALL_COLUMNS: &[ColumnMeta] = &[
    ColumnMeta { id: "status",     header: "St",      display_name: "Status",     color_key: ColorKey::Tertiary,  hideable: true },
    ColumnMeta { id: "priority",   header: "Pri",     display_name: "Priority",   color_key: ColorKey::Tertiary,  hideable: true },
    ColumnMeta { id: "tracking",   header: "Tr",      display_name: "Tracked",    color_key: ColorKey::Accent,    hideable: true },
    ColumnMeta { id: "tag_symbols", header: "T",      display_name: "Tag symbols", color_key: ColorKey::Accent,    hideable: true },
    ColumnMeta { id: TREE_COLUMN,  header: "Task",    display_name: "Task",       color_key: ColorKey::TextMed,   hideable: false },
    ColumnMeta { id: "tag_names",  header: "Tags",    display_name: "Tag names",   color_key: ColorKey::Tertiary,  hideable: true },
    ColumnMeta { id: "created_at", header: "Created", display_name: "Created At", color_key: ColorKey::Secondary, hideable: true },
    ColumnMeta { id: "updated_at", header: "Updated", display_name: "Updated At", color_key: ColorKey::Secondary, hideable: true },
    ColumnMeta { id: "last_tracked_at", header: "Tracked", display_name: "Last Tracked", color_key: ColorKey::Accent, hideable: true },
    ColumnMeta { id: "notes",  header: "N",      display_name: "Notes",       color_key: ColorKey::Tertiary,  hideable: true },
    ColumnMeta { id: "links",  header: "🔗",     display_name: "Links",       color_key: ColorKey::Accent,    hideable: true },
];

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

/// Default column order. `tag_names` is excluded — it ships hidden so
/// the table stays narrow for users who don't care about tag text;
/// they can enable it via the column-config popup (`c`).
pub fn default_column_ids() -> Vec<String> {
    ALL_COLUMNS
        .iter()
        .filter(|c| c.id != "tag_names")
        .map(|c| c.id.to_string())
        .collect()
}

/// Default tracking column order. Both `taskpath` (parent chain) and
/// `task` (leaf description) are shown by default; either can be hidden
/// in the column-config popup.
pub fn default_tracking_column_ids() -> Vec<String> {
    ALL_TRACKING_COLUMNS.iter()
        .map(|c| c.id.to_string())
        .collect()
}

/// Look up metadata for a task column id.
pub fn column_meta(id: &str) -> Option<&'static ColumnMeta> {
    ALL_COLUMNS.iter().find(|c| c.id == id)
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

/// Columns the Tasks view can sort on. Order mirrors the natural reading
/// order in the table; the sort-hint UI will surface them in this order.
pub fn task_sortable_columns() -> Vec<SortableColumn> {
    vec![
        SortableColumn { key: "status".into(),          label: "Status".into() },
        SortableColumn { key: "priority".into(),        label: "Priority".into() },
        SortableColumn { key: "description".into(),     label: "Task".into() },
        SortableColumn { key: "created_at".into(),      label: "Created".into() },
        SortableColumn { key: "updated_at".into(),      label: "Updated".into() },
        SortableColumn { key: "last_tracked_at".into(), label: "Last Tracked".into() },
    ]
}

/// Apply a (potentially multi-column) sort to a task list in place.
/// Unknown columns are ignored. A stable sort preserves prior order
/// among equal keys, so multi-column sort works as expected.
pub fn sort_tasks(tasks: &mut [Task], sort: &[SortKey]) {
    if sort.is_empty() {
        return;
    }
    tasks.sort_by(|a, b| {
        for key in sort {
            let ord = compare_task_column(a, b, &key.column);
            let ord = match key.direction {
                SortDirection::Asc => ord,
                SortDirection::Desc => ord.reverse(),
            };
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        std::cmp::Ordering::Equal
    });
}

fn compare_task_column(a: &Task, b: &Task, column: &str) -> std::cmp::Ordering {
    use not_yet_done_core::entity::task::TaskStatus;
    fn status_rank(s: &TaskStatus) -> u8 {
        match s {
            TaskStatus::InProgress => 0,
            TaskStatus::Todo       => 1,
            TaskStatus::Done       => 2,
            TaskStatus::Cancelled  => 3,
        }
    }
    match column {
        "status"          => status_rank(&a.status).cmp(&status_rank(&b.status)),
        "priority"        => a.priority.cmp(&b.priority),
        "description"     => a.description.to_lowercase().cmp(&b.description.to_lowercase()),
        "created_at"      => a.created_at.cmp(&b.created_at),
        "updated_at"      => a.updated_at.cmp(&b.updated_at),
        "last_tracked_at" => a.last_tracked_at.cmp(&b.last_tracked_at),
        _                 => std::cmp::Ordering::Equal,
    }
}
