// Task-specific wiring between `not-yet-done-forest` and the Task entity.
//
// The orphan rules prevent implementing traits from `not-yet-done-forest`
// directly on `Task` (from `not-yet-done-core`) or `Uuid` (from `uuid`),
// since neither type is local to this crate.
//
// Solution: a thin newtype `TaskItem` that wraps `Task` and lives here.
// `Forest<TaskItem, LocalUuid>` is the concrete forest type.

use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use uuid::Uuid;

use not_yet_done_core::entity::task::{Model as Task, TaskStatus};
use not_yet_done_forest::{
    child_prefix, tree_connector, FilterableForest, Forest, ForestItem, HasTreeShape, TreeNode,
};

// ---------------------------------------------------------------------------
// Local Uuid newtype — satisfies orphan rules for HasTreeShape
// ---------------------------------------------------------------------------

/// Newtype around `Uuid` so we can implement foreign traits for it locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalUuid(pub Uuid);

// ---------------------------------------------------------------------------
// TaskItem newtype — wraps Task so we can implement foreign traits
// ---------------------------------------------------------------------------

/// Newtype wrapping `Task`.  Lives in this crate, so we can implement
/// `HasTreeShape` and `ForestItem` for it without violating orphan rules.
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

// ---------------------------------------------------------------------------
// TaskQuery + ForestItem
// ---------------------------------------------------------------------------

/// Query used for fuzzy-filtering the task forest.
#[derive(Debug, Clone, Default)]
pub struct TaskQuery {
    /// Fuzzy search term against `description`. `None` = no text filter.
    pub text: Option<String>,
    /// Minimum fuzzy score (sensible default: 10–30).
    pub min_score: i64,
}

impl TaskQuery {
    pub fn new(text: impl Into<String>, min_score: i64) -> Self {
        let t = text.into();
        TaskQuery {
            text: if t.is_empty() { None } else { Some(t) },
            min_score,
        }
    }
}

impl ForestItem<TaskQuery> for TaskItem {
    fn matches_filter(&self, query: &TaskQuery) -> bool {
        match &query.text {
            None => true,
            Some(pattern) => {
                let matcher = SkimMatcherV2::default();
                matcher
                    .fuzzy_match(&self.0.description, pattern)
                    .map(|score| score >= query.min_score)
                    .unwrap_or(false)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TaskForest — concrete forest type
// ---------------------------------------------------------------------------

pub type TaskForest = Forest<TaskItem, LocalUuid>;

// ---------------------------------------------------------------------------
// TreeRow — flat representation consumed by view_pane
// ---------------------------------------------------------------------------

/// A single displayable row in the tree view.
#[derive(Debug, Clone)]
pub struct TreeRow {
    pub id: Uuid,
    /// The full display string for the "tree" column (connector + description).
    pub tree_cell: String,
    /// Depth in the tree (0 = root).
    pub depth: usize,
    pub status: TaskStatus,
    pub deleted: bool,
    pub priority: i32,
}

// ---------------------------------------------------------------------------
// build_tree_rows — entry point for view_pane
// ---------------------------------------------------------------------------

/// Build a `TaskForest` from a flat list of tasks.
pub fn build_forest(tasks: Vec<Task>) -> TaskForest {
    TaskForest::from_items(tasks.into_iter().map(TaskItem).collect())
}

/// Produce a flat `Vec<TreeRow>` from a `TaskForest`, optionally filtered by
/// a fuzzy `query` string.  Pass `""` (empty) to show the full tree.
pub fn build_tree_rows(forest: &TaskForest, query: &str) -> Vec<TreeRow> {
    let task_query = TaskQuery::new(query, 20);

    let roots: Vec<&TreeNode<TaskItem>> = if task_query.text.is_none() {
        forest.roots()
    } else {
        forest.filter(&task_query)
    };

    let mut result = Vec::new();
    for root in roots {
        collect_tree_rows(root, 0, true, "", &mut result);
    }
    result
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn collect_tree_rows(
    node: &TreeNode<TaskItem>,
    depth: usize,
    is_last: bool,
    prefix: &str,
    result: &mut Vec<TreeRow>,
) {
    let task = &node.element.0;
    let connector = tree_connector(depth, is_last, prefix);
    let tree_cell = format!("{}{}", connector, task.description);

    result.push(TreeRow {
        id: task.id,
        tree_cell,
        depth,
        status: task.status.clone(),
        deleted: task.deleted,
        priority: task.priority,
    });

    let next_prefix = child_prefix(depth, is_last, prefix);
    let child_count = node.children.len();
    for (i, child) in node.children.iter().enumerate() {
        collect_tree_rows(child, depth + 1, i == child_count - 1, &next_prefix, result);
    }
}
