//! Task table row building: converts task data into widget rows.
//!
//! Provides `build_tree_rows` and `build_flat_rows` as free functions.
//! The actual table component is `DataTable`.

use std::collections::HashSet;

use uuid::Uuid;

use not_yet_done_forest::{TREE_COLUMN, TreeRenderOptions};

use crate::ui::tasks::forest::LocalUuid;
use not_yet_done_ratatui::{
    TableWidgetCell, TableWidgetRow,
};
use not_yet_done_table::{
    ColumnId, ColStrategy, MixedColSizer, TableConfig,
    Row as TableRow, compute_table,
};

use not_yet_done_content::SortKey;
use not_yet_done_core::entity::task::Model as Task;
use not_yet_done_core::repository::ResolvedTag;

use crate::components::sort_header::{header_cell, header_text, HeaderOverlay};
use crate::tabs::build_rendered_table;
use crate::tabs::columns::column_meta;
use crate::ui::tasks::view_helpers::format_local_date;

/// Concatenate the symbols of every tag attached to a task,
/// alphabetically by tag name. Tags without a symbol are skipped.
/// Empty string when nothing to show.
pub fn fmt_tag_symbols(tags: &[ResolvedTag]) -> String {
    let mut pairs: Vec<(String, String)> = tags
        .iter()
        .filter_map(|t| match t {
            ResolvedTag::Global(t) => t.symbol.as_ref().map(|s| (t.name.clone(), s.clone())),
            ResolvedTag::Project(t) => t.symbol.as_ref().map(|s| (t.name.clone(), s.clone())),
        })
        .collect();
    pairs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    pairs.into_iter().map(|(_, s)| s).collect()
}

/// Comma-separated tag names (alphabetical, case-insensitive sort).
pub fn fmt_tag_names(tags: &[ResolvedTag]) -> String {
    let mut names: Vec<String> = tags
        .iter()
        .map(|t| match t {
            ResolvedTag::Global(t) => t.name.clone(),
            ResolvedTag::Project(t) => t.name.clone(),
        })
        .collect();
    names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    names.join(", ")
}

/// Tasks-side mapping from column id (as used in the table) to the
/// sortable-column key the sort UI/persistence layer speaks. The tree
/// column's id is `TREE_COLUMN` but it sorts by `description`.
pub fn sortable_key_for(col_id: &str) -> &str {
    if col_id == TREE_COLUMN { "description" } else { col_id }
}

/// Header text for a column under the active overlay + applied sort.
fn header_label(
    col_id: &str,
    applied_sort: &[SortKey],
    overlay: &HeaderOverlay,
) -> String {
    let natural = column_meta(col_id).map(|m| m.header).unwrap_or("");
    header_text(natural, sortable_key_for(col_id), applied_sort, overlay)
}

/// Output of [`build_tree_rows`] / [`build_flat_rows`]. Includes the
/// column widths that the layout engine settled on so that views can
/// paint sort-mode overlays in pixel-correct positions.
pub struct BuiltRows {
    pub rows: Vec<TableWidgetRow>,
    pub row_ids: Vec<Uuid>,
    pub header: Option<Vec<TableWidgetCell>>,
    pub col_widths: Vec<usize>,
}

pub fn build_tree_rows(
    forest: &crate::ui::tasks::forest::TaskForest,
    tree_filter: &str,
    area_width: usize,
    tracked_ids: &HashSet<Uuid>,
    link_refs: &HashSet<String>,
    column_config: &[String],
    all_tasks: &[Task],
    tags_by_task: &std::collections::HashMap<Uuid, Vec<ResolvedTag>>,
    applied_sort: &[SortKey],
    overlay: &HeaderOverlay,
    tree_options: &TreeRenderOptions<LocalUuid>,
) -> BuiltRows {
    let label_fn = |col_id: &str| header_label(col_id, applied_sort, overlay);
    let rendered = build_rendered_table(
        forest, tree_filter, area_width, tracked_ids, link_refs, tags_by_task,
        column_config, &label_fn, tree_options,
    );

    let tree_col_idx = column_config.iter()
        .position(|id| id == TREE_COLUMN)
        .unwrap_or(0);
    let notes_col_idx = column_config.iter()
        .position(|id| id == "notes");
    let links_col_idx = column_config.iter()
        .position(|id| id == "links");

    let col_widths: Vec<usize> = rendered.header.as_ref()
        .map(|h| h.cells.iter().map(|s| s.chars().count()).collect())
        .unwrap_or_default();

    let header: Option<Vec<TableWidgetCell>> = rendered.header.map(|h| {
        h.cells.into_iter().enumerate().map(|(i, fitted)| {
            let col_id: &str = column_config.get(i).map(|s| s.as_str()).unwrap_or("");
            header_cell(&fitted, sortable_key_for(col_id), overlay)
        }).collect()
    });

    let mut row_ids = Vec::with_capacity(rendered.rows.len());
    let rows: Vec<TableWidgetRow> = rendered.rows.iter().map(|row| {
        let uuid = row.id.0;
        row_ids.push(uuid);
        let highlights = rendered.highlights.get(&row.id).cloned().unwrap_or_default();
        let cells: Vec<TableWidgetCell> = row.cells.iter().enumerate().map(|(i, cell)| {
            if i == tree_col_idx {
                TableWidgetCell::tree(cell.clone(), row.connector_chars, highlights.clone())
            } else if Some(i) == notes_col_idx {
                // Fill notes column dynamically.
                let task = all_tasks.iter().find(|t| t.id == uuid);
                let icon = if task.and_then(|t| crate::notes::find_notes_file(t, all_tasks)).is_some() {
                    "📝"
                } else {
                    " "
                };
                TableWidgetCell::plain(format!("{:width$}", icon, width = cell.chars().count().max(1)))
            } else if Some(i) == links_col_idx {
                let has_link = link_refs.contains(&format!("tasks/{uuid}"));
                let icon = if has_link { "🔗" } else { " " };
                TableWidgetCell::plain(format!("{:width$}", icon, width = cell.chars().count().max(1)))
            } else {
                TableWidgetCell::plain(cell.clone())
            }
        }).collect();
        TableWidgetRow::new(cells)
    }).collect();

    BuiltRows { rows, row_ids, header, col_widths }
}

pub fn build_flat_rows(
    task_rows: &[Task],
    tracked_ids: &HashSet<Uuid>,
    link_refs: &HashSet<String>,
    column_config: &[String],
    area_width: usize,
    all_tasks: &[Task],
    tags_by_task: &std::collections::HashMap<Uuid, Vec<ResolvedTag>>,
    applied_sort: &[SortKey],
    overlay: &HeaderOverlay,
) -> BuiltRows {
    use not_yet_done_core::entity::task::TaskStatus;

    let cols: Vec<ColumnId> = column_config.iter().map(|s| ColumnId::new(s)).collect();

    let header_row = {
        let mut r = TableRow::new(Uuid::nil());
        for col_id in column_config {
            let label = header_label(col_id, applied_sort, overlay);
            r = r.cell(col_id, label);
        }
        r.not_selectable()
    };

    let data_rows: Vec<TableRow<Uuid>> = task_rows.iter().map(|task| {
        let mut r = TableRow::new(task.id);
        for col_id in column_config {
            let text = match col_id.as_str() {
                TREE_COLUMN => task.description.clone(),
                "status" => match task.status {
                    TaskStatus::Todo => "󰄰".to_string(),
                    TaskStatus::InProgress => "󰄳".to_string(),
                    TaskStatus::Done => "󰄵".to_string(),
                    TaskStatus::Cancelled => "󰜺".to_string(),
                },
                "priority" => task.priority.to_string(),
                "tracking" => {
                    if tracked_ids.contains(&task.id) { "⏱".to_string() }
                    else { " ".to_string() }
                },
                "created_at" => format_local_date(task.created_at),
                "updated_at" => format_local_date(task.updated_at),
                "last_tracked_at" => task.last_tracked_at
                    .map(format_local_date)
                    .unwrap_or_default(),
                "notes" => {
                    if crate::notes::find_notes_file(task, all_tasks).is_some() {
                        "📝".to_string()
                    } else {
                        " ".to_string()
                    }
                },
                "links" => {
                    if link_refs.contains(&format!("tasks/{}", task.id)) {
                        "🔗".to_string()
                    } else {
                        " ".to_string()
                    }
                },
                "tag_symbols" => tags_by_task
                    .get(&task.id)
                    .map(|tags| fmt_tag_symbols(tags))
                    .unwrap_or_default(),
                "tag_names" => tags_by_task
                    .get(&task.id)
                    .map(|tags| fmt_tag_names(tags))
                    .unwrap_or_default(),
                _ => String::new(),
            };
            r = r.cell(col_id, text);
        }
        r
    }).collect();

    let mut strategies = std::collections::HashMap::new();
    for col_id in column_config {
        let strategy = if col_id == TREE_COLUMN {
            ColStrategy::Flex(1)
        } else {
            ColStrategy::Max
        };
        strategies.insert(ColumnId::new(col_id), strategy);
    }

    let config = TableConfig {
        max_width: area_width,
        separator: "  ".to_string(),
        sizer: Box::new(MixedColSizer { strategies }),
    };

    let computed = compute_table(&data_rows, &config, &cols, Some(&header_row));
    let col_widths = computed.col_widths.clone();

    let header: Option<Vec<TableWidgetCell>> = computed.header.map(|h| {
        h.cells.into_iter().enumerate().map(|(i, fitted)| {
            let col_id: &str = column_config.get(i).map(|s| s.as_str()).unwrap_or("");
            header_cell(&fitted, sortable_key_for(col_id), overlay)
        }).collect()
    });

    let mut row_ids = Vec::with_capacity(computed.rows.len());
    let rows: Vec<TableWidgetRow> = computed.rows.into_iter().map(|cr| {
        row_ids.push(cr.id);
        let cells: Vec<TableWidgetCell> = cr.cells.into_iter()
            .map(|c| TableWidgetCell::plain(c))
            .collect();
        TableWidgetRow::new(cells)
    }).collect();

    BuiltRows { rows, row_ids, header, col_widths }
}
