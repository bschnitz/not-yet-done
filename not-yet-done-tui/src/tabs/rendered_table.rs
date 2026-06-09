//! Build a rendered table for the tree view from a TaskForest.

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use crate::ui::tasks::forest::{find_task_in_forest, LocalUuid, TaskForest, TaskQuery};
use not_yet_done_forest::{
    ColStrategy, ColumnId, IntoRow, MixedColSizer, RenderableTree, RenderedTable, Row, TableLayout,
    TreeRenderOptions, render_table, TREE_COLUMN,
};

/// Build a fully rendered [`RenderedTable`] for the given forest, filter
/// string, and column configuration.
/// `tracked_ids` marks which tasks are actively being tracked.
/// `column_ids` determines which columns are shown and in what order.
/// `header_label_for` resolves the header text for each column id —
/// callers can inject sort indicators or sort-mode overlay labels here.
pub fn build_rendered_table(
    forest: &TaskForest,
    query_str: &str,
    area_width: usize,
    tracked_ids: &HashSet<Uuid>,
    link_refs: &HashSet<String>,
    tags_by_task: &HashMap<Uuid, Vec<not_yet_done_core::repository::ResolvedTag>>,
    column_ids: &[String],
    header_label_for: &dyn Fn(&str) -> String,
    tree_options: &TreeRenderOptions<LocalUuid>,
) -> RenderedTable<LocalUuid> {
    let query = TaskQuery::new(query_str, 20);
    let cols: Vec<ColumnId> = column_ids.iter().map(|s| ColumnId::new(s)).collect();
    let tree_rows = RenderableTree::tree_rows_with_options::<LocalUuid>(forest, &query, tree_options);

    if tree_rows.is_empty() {
        return RenderedTable {
            header: None,
            rows: vec![],
            highlights: HashMap::new(),
        };
    }

    let mut data_rows: Vec<Row<LocalUuid>> = tree_rows
        .iter()
        .filter_map(|tr| find_task_in_forest(forest, tr.id.0).map(|item| item.into_row()))
        .collect();

    // Fill in tracking + links columns (both derived from external state,
    // not the Task model itself).
    for row in &mut data_rows {
        let is_tracked = tracked_ids.contains(&row.id.0);
        row.cells.insert(
            ColumnId::new("tracking"),
            if is_tracked { "⏱".to_string() } else { " ".to_string() },
        );
        let has_link = link_refs.contains(&format!("tasks/{}", row.id.0));
        row.cells.insert(
            ColumnId::new("links"),
            if has_link { "🔗".to_string() } else { " ".to_string() },
        );
        let tags = tags_by_task.get(&row.id.0).map(|v| v.as_slice()).unwrap_or(&[]);
        row.cells.insert(
            ColumnId::new("tag_symbols"),
            crate::components::task_table::fmt_tag_symbols(tags),
        );
        row.cells.insert(
            ColumnId::new("tag_names"),
            crate::components::task_table::fmt_tag_names(tags),
        );
    }

    let header = {
        let mut cells = HashMap::new();
        for col_id in column_ids {
            cells.insert(ColumnId::new(col_id), header_label_for(col_id));
        }
        Row {
            id: LocalUuid(Uuid::nil()),
            cells,
        }
    };

    let sizer = MixedColSizer {
        strategies: {
            let mut m = HashMap::new();
            for col_id in column_ids {
                let strategy = if col_id == TREE_COLUMN {
                    ColStrategy::Flex(1)
                } else {
                    ColStrategy::Max
                };
                m.insert(ColumnId::new(col_id), strategy);
            }
            m
        },
    };
    let layout = TableLayout {
        max_width: area_width,
        separator: "  ".to_string(),
        sizer: Box::new(sizer),
    };

    render_table(tree_rows, data_rows, &layout, &cols, Some(header))
}
