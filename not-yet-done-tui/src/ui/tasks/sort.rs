//! Sorting logic for ghost forests.
//!
//! Nodes are sorted by their maximum fuzzy-match score within their subtree
//! (descending), with alphabetical description as a tie-breaker.

use fuzzy_matcher::FuzzyMatcher;
use not_yet_done_forest::GhostNode;

use super::forest::{TaskItem, TaskQuery};

/// Sort a ghost forest (and all subtrees, recursively) by descending subtree
/// score, then alphabetically by description.
pub fn sort_ghost_forest(ghosts: &mut Vec<GhostNode<'_, TaskItem>>, query: &TaskQuery) {
    ghosts.sort_by(|a, b| {
        let score_a = max_score_in_subtree(a, query);
        let score_b = max_score_in_subtree(b, query);
        score_b
            .cmp(&score_a)
            .then_with(|| a.node.element.0.description.cmp(&b.node.element.0.description))
            .then_with(|| a.node.element.0.id.cmp(&b.node.element.0.id))
    });
    for ghost in ghosts.iter_mut() {
        sort_ghost_forest(&mut ghost.children, query);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_core::entity::task::{Model as Task, TaskStatus};
    use not_yet_done_forest::{Forest, TransformableForest};
    use uuid::Uuid;

    fn make_task(id: Uuid, desc: &str) -> Task {
        Task {
            id,
            description: desc.to_string(),
            status: TaskStatus::Todo,
            deleted: false,
            deleted_at: None,
            priority: 0,
            parent_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_tracked_at: None,
            path: None,
        }
    }

    /// Two tasks with identical descriptions must always sort in the same
    /// deterministic order (by UUID), regardless of input order.
    #[test]
    fn duplicate_names_sort_deterministically() {
        let id_a = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let id_b = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();

        let query = TaskQuery::new("", 0);

        // Order 1: A then B
        let forest1 = Forest::from_items(vec![
            TaskItem(make_task(id_a, "A new subitem")),
            TaskItem(make_task(id_b, "A new subitem")),
        ]);
        let mut ghosts1 = forest1.transform(&query);
        sort_ghost_forest(&mut ghosts1, &query);
        let order1: Vec<Uuid> = ghosts1.iter().map(|g| g.node.element.0.id).collect();

        // Order 2: B then A
        let forest2 = Forest::from_items(vec![
            TaskItem(make_task(id_b, "A new subitem")),
            TaskItem(make_task(id_a, "A new subitem")),
        ]);
        let mut ghosts2 = forest2.transform(&query);
        sort_ghost_forest(&mut ghosts2, &query);
        let order2: Vec<Uuid> = ghosts2.iter().map(|g| g.node.element.0.id).collect();

        assert_eq!(order1, order2, "Sort must be deterministic for duplicate names");
        // Smaller UUID should come first.
        assert_eq!(order1[0], id_a);
        assert_eq!(order1[1], id_b);
    }
}

/// Return the highest fuzzy-match score among all nodes in the subtree rooted
/// at `ghost`.  Returns 0 when there is no active query.
fn max_score_in_subtree(ghost: &GhostNode<'_, TaskItem>, query: &TaskQuery) -> i64 {
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
        .map(|c| max_score_in_subtree(c, query))
        .max()
        .unwrap_or(0);
    self_score.max(child_max)
}
