// not-yet-done-tui/src/ui/tasks/forest.rs

use chrono::{DateTime, Local};
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use uuid::Uuid;

use not_yet_done_core::entity::task::{Model as Task, TaskStatus};
use not_yet_done_forest::{
    ColumnId, FilterableTable, Forest, ForestItem, GhostNode, HasTreeShape, IntoRow, Row,
    TransformableForest, TreeDisplay, TreeNode, TREE_COLUMN,
};

// ---------------------------------------------------------------------------
// Local Uuid newtype
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalUuid(pub Uuid);

// ---------------------------------------------------------------------------
// TaskItem newtype
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TaskItem(pub Task);

impl HasTreeShape<LocalUuid> for TaskItem {
    fn id(&self) -> LocalUuid {
        LocalUuid(self.0.id)
    }
    fn parent_id(&self) -> Option<LocalUuid> {
        self.0.parent_id.map(LocalUuid)
    }
}

impl TreeDisplay for TaskItem {
    fn description(&self) -> Option<&str> {
        Some(&self.0.description)
    }
}

impl IntoRow for TaskItem {
    type Id = LocalUuid;

    fn into_row(&self) -> Row<LocalUuid> {
        let mut cells = std::collections::HashMap::new();
        cells.insert(
            ColumnId::new("priority"),
            self.0.priority.to_string(),
        );
        cells.insert(
            ColumnId::new("created_at"),
            format_local_date(self.0.created_at),
        );
        cells.insert(
            ColumnId::new("updated_at"),
            format_local_date(self.0.updated_at),
        );
        Row {
            id: LocalUuid(self.0.id),
            cells,
        }
    }
}

fn format_local_date(dt: DateTime<chrono::Utc>) -> String {
    let local: DateTime<Local> = dt.with_timezone(&Local);
    local.format("%Y-%m-%d").to_string()
}

// ---------------------------------------------------------------------------
// TaskQuery + ForestItem
// ---------------------------------------------------------------------------

/// Query used for fuzzy-filtering the task forest.
/// Holds the matcher instance so it is constructed exactly once per query.
pub struct TaskQuery {
    pub text: Option<String>,
    pub min_score: i64,
    matcher: SkimMatcherV2,
}

impl std::fmt::Debug for TaskQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskQuery")
            .field("text", &self.text)
            .field("min_score", &self.min_score)
            .field("matcher", &"<SkimMatcherV2>")
            .finish()
    }
}

impl TaskQuery {
    pub fn new(text: impl Into<String>, min_score: i64) -> Self {
        let t = text.into();
        TaskQuery {
            text: if t.is_empty() { None } else { Some(t) },
            min_score,
            matcher: SkimMatcherV2::default(),
        }
    }
}

impl ForestItem<TaskQuery> for TaskItem {
    fn matches_filter(&self, query: &TaskQuery) -> bool {
        match &query.text {
            None => true,
            Some(pattern) => query
                .matcher
                .fuzzy_match(&self.0.description, pattern)
                .map(|score| score >= query.min_score)
                .unwrap_or(false),
        }
    }
}

// ---------------------------------------------------------------------------
// TaskForest — newtype wrapper so we can impl TransformableForest with sorting
// ---------------------------------------------------------------------------

pub type TaskForestInner = Forest<TaskItem, LocalUuid>;

/// Newtype around `Forest<TaskItem, LocalUuid>` that implements
/// `TransformableForest<TaskQuery>` with fuzzy-score sorting.
pub struct TaskForest(pub TaskForestInner);

impl TaskForest {
    pub fn new(tasks: Vec<Task>) -> Self {
        TaskForest(Forest::from_items(
            tasks.into_iter().map(TaskItem).collect(),
        ))
    }

    pub fn inner(&self) -> &TaskForestInner {
        &self.0
    }
}

impl TransformableForest<TaskQuery> for TaskForest {
    type Item = TaskItem;

    fn transform<'a>(&'a self, query: &TaskQuery) -> Vec<GhostNode<'a, TaskItem>> {
        // Delegate filtering to the inner Forest's default TransformableForest impl,
        // then apply score-based sorting.
        let mut ghost_roots =
            <TaskForestInner as TransformableForest<TaskQuery>>::transform(&self.0, query);

        // Sort roots by max subtree score descending, then alphabetically.
        ghost_roots.sort_by(|a, b| {
            let score_a = max_score_in_ghost(a, query);
            let score_b = max_score_in_ghost(b, query);
            score_b
                .cmp(&score_a)
                .then_with(|| {
                    a.node.element.0.description
                        .cmp(&b.node.element.0.description)
                })
        });

        // Sort children recursively.
        for root in &mut ghost_roots {
            sort_ghost_children(root, query);
        }

        ghost_roots
    }
}

/// Recursively sort the children of a `GhostNode` by score then alphabetically.
fn sort_ghost_children(ghost: &mut GhostNode<'_, TaskItem>, query: &TaskQuery) {
    ghost.children.sort_by(|a, b| {
        let score_a = max_score_in_ghost(a, query);
        let score_b = max_score_in_ghost(b, query);
        score_b
            .cmp(&score_a)
            .then_with(|| {
                a.node.element.0.description
                    .cmp(&b.node.element.0.description)
            })
    });
    for child in &mut ghost.children {
        sort_ghost_children(child, query);
    }
}

/// Maximum fuzzy match score in a ghost subtree.
fn max_score_in_ghost(ghost: &GhostNode<'_, TaskItem>, query: &TaskQuery) -> i64 {
    let self_score = match &query.text {
        None => 0,
        Some(pattern) => query
            .matcher
            .fuzzy_match(&ghost.node.element.0.description, pattern)
            .unwrap_or(0),
    };
    let child_max = ghost
        .children
        .iter()
        .map(|c| max_score_in_ghost(c, query))
        .max()
        .unwrap_or(0);
    self_score.max(child_max)
}

// ---------------------------------------------------------------------------
// TreeRow — kept for the existing render_tree_row in view_pane.rs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TreeRow {
    pub id: Uuid,
    pub tree_cell: String,
    pub depth: usize,
    pub status: TaskStatus,
    pub deleted: bool,
    pub priority: i32,
}

// ---------------------------------------------------------------------------
// build_forest / build_tree_rows
//
// build_tree_rows now delegates entirely to FilterableTable::rows() so all
// tree-connector logic lives in the forest lib.
// ---------------------------------------------------------------------------

pub fn build_forest(tasks: Vec<Task>) -> TaskForest {
    TaskForest::new(tasks)
}

pub fn build_tree_rows(forest: &TaskForest, query: &str) -> Vec<TreeRow> {
    let task_query = TaskQuery::new(query, 20);

    // FilterableTable::rows() is available via the blanket impl because
    // TaskForest: TransformableForest<TaskQuery> and TaskItem: TreeDisplay + IntoRow.
    let rows = FilterableTable::rows(forest, &task_query);

    rows.into_iter()
        .map(|row| {
            let id = row.id.0;
            let tree_cell = row
                .cells
                .get(&ColumnId::new(TREE_COLUMN))
                .cloned()
                .unwrap_or_default();
            let priority = row
                .cells
                .get(&ColumnId::new("priority"))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);

            // status and deleted are not in IntoRow cells — retrieve from forest.
            let (status, deleted, depth) = forest
                .inner()
                .roots()
                .iter()
                .find_map(|root| find_in_node(root, id))
                .unwrap_or((TaskStatus::Todo, false, 0));

            TreeRow {
                id,
                tree_cell,
                depth,
                status,
                deleted,
                priority,
            }
        })
        .collect()
}

/// Recursively search a TreeNode subtree for a task by Uuid.
/// Returns `(status, deleted, depth)` if found.
fn find_in_node(node: &TreeNode<TaskItem>, id: Uuid) -> Option<(TaskStatus, bool, usize)> {
    fn recurse(
        node: &TreeNode<TaskItem>,
        id: Uuid,
        depth: usize,
    ) -> Option<(TaskStatus, bool, usize)> {
        if node.element.0.id == id {
            return Some((node.element.0.status.clone(), node.element.0.deleted, depth));
        }
        for child in &node.children {
            if let Some(found) = recurse(child, id, depth + 1) {
                return Some(found);
            }
        }
        None
    }
    recurse(node, id, 0)
}
