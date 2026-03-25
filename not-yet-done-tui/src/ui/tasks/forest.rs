use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use uuid::Uuid;

use not_yet_done_core::entity::task::{Model as Task, TaskStatus};
use not_yet_done_forest::{
    child_prefix, tree_connector, FilterableForest, Forest, ForestItem, HasTreeShape, TreeNode,
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
            matcher: SkimMatcherV2::default(), // ← genau einmal
        }
    }
}

impl ForestItem<TaskQuery> for TaskItem {
    fn matches_filter(&self, query: &TaskQuery) -> bool {
        match &query.text {
            None => true,
            Some(pattern) => {
                query
                    .matcher // ← wiederverwendet
                    .fuzzy_match(&self.0.description, pattern)
                    .map(|score| score >= query.min_score)
                    .unwrap_or(false)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TaskForest
// ---------------------------------------------------------------------------

pub type TaskForest = Forest<TaskItem, LocalUuid>;

// ---------------------------------------------------------------------------
// TreeRow
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
// ---------------------------------------------------------------------------

pub fn build_forest(tasks: Vec<Task>) -> TaskForest {
    TaskForest::from_items(tasks.into_iter().map(TaskItem).collect())
}

pub fn build_tree_rows(forest: &TaskForest, query: &str) -> Vec<TreeRow> {
    let task_query = TaskQuery::new(query, 20);

    let mut roots: Vec<&TreeNode<TaskItem>> = if task_query.text.is_none() {
        forest.roots().to_vec()
    } else {
        forest.filter(&task_query).to_vec()
    };

    // Sort roots by max subtree score (descending), then alphabetically by description.
    roots.sort_by(|a, b| {
        let score_a = max_score_in_subtree(a, &task_query);
        let score_b = max_score_in_subtree(b, &task_query);
        score_b.cmp(&score_a)
            .then_with(|| a.element.0.description.cmp(&b.element.0.description))
    });

    let mut result = Vec::new();
    for root in roots {
        collect_tree_rows(root, 0, true, "", &mut result, &task_query);
    }
    result
}

/// Recursively compute the maximum fuzzy match score in a subtree.
fn max_score_in_subtree(node: &TreeNode<TaskItem>, query: &TaskQuery) -> i64 {
    let self_score = match &query.text {
        None => 0,
        Some(pattern) => query
            .matcher
            .fuzzy_match(&node.element.0.description, pattern)
            .unwrap_or(0),
    };
    let child_max = node.children.iter()
        .map(|child| max_score_in_subtree(child, query))
        .max()
        .unwrap_or(0);
    self_score.max(child_max)
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
    query: &TaskQuery,
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
    // Sort children by max subtree score (descending) then alphabetically.
    let mut children: Vec<_> = node.children.iter().collect();
    children.sort_by(|a, b| {
        let score_a = max_score_in_subtree(a, query);
        let score_b = max_score_in_subtree(b, query);
        score_b.cmp(&score_a)
            .then_with(|| a.element.0.description.cmp(&b.element.0.description))
    });
    let child_count = children.len();
    for (i, child) in children.into_iter().enumerate() {
        collect_tree_rows(child, depth + 1, i == child_count - 1, &next_prefix, result, query);
    }
}
