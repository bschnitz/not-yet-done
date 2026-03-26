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
    });
    for ghost in ghosts.iter_mut() {
        sort_ghost_forest(&mut ghost.children, query);
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
