use chrono::{DateTime, Local};
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use uuid::Uuid;

use not_yet_done_core::entity::task::{Model as Task, TaskStatus};
use not_yet_done_forest::{
    ColumnId, Forest, ForestItem, GhostNode, HasTreeShape, IntoRow, Row, TransformableForest,
    TreeDisplay, TreeNode,
};

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
    pub matcher: SkimMatcherV2,
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

        // 2. Fill highlight_ranges via fuzzy_indices.
        fill_highlight_ranges(&mut ghost_roots, query);

        // 3. Sort by max subtree score, then alphabetically.
        sort_ghost_forest(&mut ghost_roots, query);

        ghost_roots
    }
}

// ---------------------------------------------------------------------------
// Highlight range helpers
// ---------------------------------------------------------------------------

fn fill_highlight_ranges(ghosts: &mut Vec<GhostNode<'_, TaskItem>>, query: &TaskQuery) {
    for ghost in ghosts.iter_mut() {
        if let (Some(pattern), Some(desc)) = (&query.text, ghost.node.element.description()) {
            // fuzzy_indices returns Option<(score, Vec<usize>)> where Vec<usize>
            // contains the matched *char* indices.
            if let Some((_score, indices)) = query.matcher.fuzzy_indices(desc, pattern) {
                ghost.highlight_ranges = char_indices_to_byte_ranges(desc, &indices);
            }
        }
        fill_highlight_ranges(&mut ghost.children, query);
    }
}

/// Convert a list of matched char indices into byte `Range`s, merging
/// consecutive chars into single ranges.
fn char_indices_to_byte_ranges(s: &str, char_indices: &[usize]) -> Vec<std::ops::Range<usize>> {
    if char_indices.is_empty() {
        return vec![];
    }

    // Collect (char_index, byte_offset, char_byte_len) for every char in s.
    let char_map: Vec<(usize, usize)> = s
        .char_indices()
        .map(|(byte_off, ch)| (byte_off, ch.len_utf8()))
        .collect();

    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    let mut iter = char_indices.iter().peekable();

    while let Some(&ci) = iter.next() {
        if ci >= char_map.len() {
            continue;
        }
        let start_byte = char_map[ci].0;
        let mut end_byte = start_byte + char_map[ci].1;
        let mut prev_ci = ci;

        // Extend range while the next char index is consecutive.
        while let Some(&&next_ci) = iter.peek() {
            if next_ci == prev_ci + 1 && next_ci < char_map.len() {
                iter.next();
                end_byte = char_map[next_ci].0 + char_map[next_ci].1;
                prev_ci = next_ci;
            } else {
                break;
            }
        }

        ranges.push(start_byte..end_byte);
    }

    ranges
}

// ---------------------------------------------------------------------------
// Sorting helpers
// ---------------------------------------------------------------------------

fn sort_ghost_forest(ghosts: &mut Vec<GhostNode<'_, TaskItem>>, query: &TaskQuery) {
    ghosts.sort_by(|a, b| {
        let score_a = max_score_in_ghost(a, query);
        let score_b = max_score_in_ghost(b, query);
        score_b
            .cmp(&score_a)
            .then_with(|| a.node.element.0.description.cmp(&b.node.element.0.description))
    });
    for ghost in ghosts.iter_mut() {
        sort_ghost_forest(&mut ghost.children, query);
    }
}

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
