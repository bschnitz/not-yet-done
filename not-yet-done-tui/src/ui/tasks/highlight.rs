//! Highlight range computation for fuzzy-matched tree nodes.
//!
//! All ranges are **char-index** based (not byte offsets), matching the
//! convention used throughout the forest crate after refactoring.

use not_yet_done_forest::TreeDisplay;
use fuzzy_matcher::FuzzyMatcher;
use not_yet_done_forest::GhostNode;

use super::forest::{TaskItem, TaskQuery};

/// Walk the ghost tree recursively and fill `highlight_ranges` on every node
/// whose description matches the query.
///
/// Ranges are char-index ranges into `TreeDisplay::description()`.
pub fn fill_highlight_ranges(ghosts: &mut Vec<GhostNode<'_, TaskItem>>, query: &TaskQuery) {
    for ghost in ghosts.iter_mut() {
        if let (Some(pattern), Some(desc)) = (&query.text, ghost.node.element.description()) {
            // `fuzzy_indices` returns matched *char* indices directly.
            if let Some((_score, char_indices)) = query.matcher.fuzzy_indices(desc, pattern) {
                ghost.highlight_ranges = merge_consecutive_char_indices(&char_indices);
            }
        }
        fill_highlight_ranges(&mut ghost.children, query);
    }
}

/// Convert a sorted list of matched char indices into contiguous `Range<usize>`s
/// by merging consecutive indices into single ranges.
///
/// Example: `[0, 1, 2, 5, 6]` → `[0..3, 5..7]`
fn merge_consecutive_char_indices(char_indices: &[usize]) -> Vec<std::ops::Range<usize>> {
    if char_indices.is_empty() {
        return vec![];
    }

    let mut ranges = Vec::new();
    let mut iter = char_indices.iter().peekable();

    while let Some(&start) = iter.next() {
        let mut end = start + 1;
        // Extend while the next index is consecutive.
        while iter.peek().map(|&&next| next == end).unwrap_or(false) {
            iter.next();
            end += 1;
        }
        ranges.push(start..end);
    }

    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_empty() {
        assert_eq!(merge_consecutive_char_indices(&[]), vec![]);
    }

    #[test]
    fn merge_single() {
        assert_eq!(merge_consecutive_char_indices(&[3]), vec![3..4]);
    }

    #[test]
    fn merge_consecutive() {
        assert_eq!(merge_consecutive_char_indices(&[0, 1, 2]), vec![0..3]);
    }

    #[test]
    fn merge_gaps() {
        assert_eq!(
            merge_consecutive_char_indices(&[0, 1, 5, 6]),
            vec![0..2, 5..7]
        );
    }

    #[test]
    fn merge_all_separate() {
        assert_eq!(
            merge_consecutive_char_indices(&[0, 2, 4]),
            vec![0..1, 2..3, 4..5]
        );
    }
}
