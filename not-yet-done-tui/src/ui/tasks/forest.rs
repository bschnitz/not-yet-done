use fuzzy_matcher::FuzzyMatcher;
use chrono::{DateTime, Local};
use uuid::Uuid;

use not_yet_done_core::entity::task::{Model as Task, TaskStatus};
use not_yet_done_forest::{
    ColumnId, Forest, ForestItem, GhostNode, HasTreeShape, IntoRow, Row, TransformableForest,
    TreeDisplay, TreeNode,
};

use super::highlight::fill_highlight_ranges;
use super::sort::sort_ghost_forest;

// ---------------------------------------------------------------------------
// Local Uuid newtype
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalUuid(pub Uuid);

// ---------------------------------------------------------------------------
// TaskItem
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
        cells.insert(ColumnId::new("priority"), self.0.priority.to_string());
        cells.insert(ColumnId::new("created_at"), format_local_date(self.0.created_at));
        cells.insert(ColumnId::new("updated_at"), format_local_date(self.0.updated_at));
        Row {
            id: LocalUuid(self.0.id),
            cells,
        }
    }
}

impl TaskItem {
    pub fn status(&self) -> &TaskStatus {
        &self.0.status
    }
    pub fn deleted(&self) -> bool {
        self.0.deleted
    }
}

fn format_local_date(dt: DateTime<chrono::Utc>) -> String {
    let local: DateTime<Local> = dt.with_timezone(&Local);
    local.format("%Y-%m-%d").to_string()
}

// ---------------------------------------------------------------------------
// TaskQuery + ForestItem
// ---------------------------------------------------------------------------

pub struct TaskQuery {
    pub text: Option<String>,
    pub min_score: i64,
    pub matcher: fuzzy_matcher::skim::SkimMatcherV2,
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
            matcher: fuzzy_matcher::skim::SkimMatcherV2::default(),
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
// TaskForest
// ---------------------------------------------------------------------------

pub type TaskForestInner = Forest<TaskItem, LocalUuid>;

pub struct TaskForest(pub TaskForestInner);

impl TaskForest {
    pub fn new(tasks: Vec<Task>) -> Self {
        TaskForest(Forest::from_items(tasks.into_iter().map(TaskItem).collect()))
    }

    pub fn inner(&self) -> &TaskForestInner {
        &self.0
    }
}

impl TransformableForest<TaskQuery> for TaskForest {
    type Item = TaskItem;

    fn transform<'a>(&'a self, query: &TaskQuery) -> Vec<GhostNode<'a, TaskItem>> {
        // 1. Delegate filtering to the inner Forest's default impl.
        let mut ghost_roots =
            <TaskForestInner as TransformableForest<TaskQuery>>::transform(&self.0, query);

        // 2. Fill highlight_ranges (char index ranges into description).
        fill_highlight_ranges(&mut ghost_roots, query);

        // 3. Sort by max subtree score, then alphabetically.
        sort_ghost_forest(&mut ghost_roots, query);

        ghost_roots
    }
}

// ---------------------------------------------------------------------------
// Public constructors / accessors
// ---------------------------------------------------------------------------

pub fn build_forest(tasks: Vec<Task>) -> TaskForest {
    TaskForest::new(tasks)
}

pub fn find_task_in_forest(forest: &TaskForest, id: Uuid) -> Option<&TaskItem> {
    forest
        .inner()
        .roots()
        .iter()
        .find_map(|root| find_task_in_node(root, id))
}

fn find_task_in_node<'a>(node: &'a TreeNode<TaskItem>, id: Uuid) -> Option<&'a TaskItem> {
    if node.element.0.id == id {
        return Some(&node.element);
    }
    node.children
        .iter()
        .find_map(|child| find_task_in_node(child, id))
}
