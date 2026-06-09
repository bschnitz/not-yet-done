//! Expand/collapse state for the Tasks-tab tree view.
//!
//! Decoupled from the rendering view so its semantics can be unit-tested
//! in isolation. The view passes `is_open` into the forest crate via
//! `TreeRenderOptions::is_expanded`.
//!
//! Model
//! -----
//! - `default_expand_depth` is the baseline depth at which all nodes are
//!   open. With `default_expand_depth = 2`, depths 0 and 1 are open
//!   by default → three levels are visible.
//! - `zr` flips into `AllExpanded` mode (every node open). `zm` resets
//!   back to `Default` mode and clears per-node flips — i.e. collapse
//!   only as far as the configured `default_expand_depth`, not all the
//!   way to roots. Full collapse-to-roots is reachable by setting
//!   `default_expand_depth: 0` in `tui.yaml`.
//! - Per-node toggles (`<space>`) are stored in `flipped`. A flipped
//!   node inverts the baseline implied by `mode` + `default_expand_depth`.
//! - `set_default_expand_depth` returns to `Default` mode and clears
//!   per-node overrides. This is what a config reload triggers.
//! - `transient_open` is a short-lived "forced open" set used by `/`-search
//!   to auto-expand the path to a match. It takes precedence over baseline
//!   and `flipped`. While the user keeps pressing `n`/`N` the set is
//!   replaced on each jump (so the previous path collapses again); the
//!   first non-search keystroke calls `commit_transient_open` which
//!   promotes still-closed transient entries into `flipped`, locking the
//!   path open. `clear_transient_open` drops it without locking (e.g. on
//!   search cancel).

use std::collections::HashSet;

use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Open if `depth < default_expand_depth` (modulo per-node flips).
    Default,
    /// Everything open (modulo per-node flips).
    AllExpanded,
}

#[derive(Debug, Clone)]
pub struct TasksTreeState {
    default_expand_depth: u32,
    mode: Mode,
    flipped: HashSet<Uuid>,
    transient_open: HashSet<Uuid>,
}

impl TasksTreeState {
    pub fn new(default_expand_depth: u32) -> Self {
        Self {
            default_expand_depth,
            mode: Mode::Default,
            flipped: HashSet::new(),
            transient_open: HashSet::new(),
        }
    }

    /// Whether `id` at `depth` is currently considered open. Leaves call
    /// this too (it's harmless — leaves have no children to expand).
    pub fn is_open(&self, id: &Uuid, depth: u32) -> bool {
        if self.transient_open.contains(id) {
            return true;
        }
        let base = self.baseline_open(depth);
        if self.flipped.contains(id) { !base } else { base }
    }

    /// Flip the open/closed state of `id` relative to baseline. Safe to
    /// call on leaves — has no visible effect.
    pub fn toggle(&mut self, id: Uuid) {
        if !self.flipped.insert(id) {
            self.flipped.remove(&id);
        }
    }

    pub fn expand_all(&mut self) {
        self.mode = Mode::AllExpanded;
        self.flipped.clear();
        self.transient_open.clear();
    }

    /// Collapse back to the configured `default_expand_depth`, dropping
    /// every per-node flip **and** any pending search auto-expansion.
    /// Despite the legacy `zM` vim-pun, this does **not** collapse to
    /// root — for that, set `tasks.tree.default_expand_depth: 0` in
    /// `tui.yaml`.
    pub fn reset_to_default(&mut self) {
        self.mode = Mode::Default;
        self.flipped.clear();
        self.transient_open.clear();
    }

    /// Apply a new default depth (e.g. after a config reload). Returns
    /// to `Default` mode and clears per-node overrides — the user's
    /// fresh config wins.
    pub fn set_default_expand_depth(&mut self, depth: u32) {
        self.default_expand_depth = depth;
        self.mode = Mode::Default;
        self.flipped.clear();
        self.transient_open.clear();
    }

    /// Replace the transient "forced open" set. Used by `/`-search to
    /// auto-expand the path to a match — each `n`/`N` jump replaces the
    /// previous path, so the old expansion collapses again.
    pub fn set_transient_open(&mut self, ids: HashSet<Uuid>) {
        self.transient_open = ids;
    }

    /// Drop the transient set without locking anything in. Used when
    /// search is cancelled and the user wants their original collapsed
    /// state back.
    pub fn clear_transient_open(&mut self) {
        self.transient_open.clear();
    }

    /// Promote still-closed transient entries into `flipped`, so the
    /// auto-expanded path stays open after the user moves on. `depths`
    /// maps each transient id to its tree depth (caller supplies it from
    /// the forest).
    pub fn commit_transient_open(&mut self, depths: &std::collections::HashMap<Uuid, u32>) {
        let pending: Vec<Uuid> = self.transient_open.drain().collect();
        for id in pending {
            let depth = depths.get(&id).copied().unwrap_or(0);
            let base = self.baseline_open(depth);
            let flipped = self.flipped.contains(&id);
            let resolved = if flipped { !base } else { base };
            if resolved {
                continue;
            }
            // Would close without the transient override → toggle the
            // flip so it stays open. Two sub-cases:
            //   • baseline closed, no flip → add flip (closed → open).
            //   • baseline open, user-flipped closed → remove flip.
            if flipped {
                self.flipped.remove(&id);
            } else {
                self.flipped.insert(id);
            }
        }
    }

    /// True iff there's an active transient expansion. Used by the view
    /// to decide whether to auto-commit on unrelated keystrokes.
    pub fn has_transient_open(&self) -> bool {
        !self.transient_open.is_empty()
    }

    fn baseline_open(&self, depth: u32) -> bool {
        match self.mode {
            Mode::Default => depth < self.default_expand_depth,
            Mode::AllExpanded => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<Uuid> {
        (0..n).map(|_| Uuid::new_v4()).collect()
    }

    #[test]
    fn default_depth_zero_collapses_everything_below_root() {
        let s = TasksTreeState::new(0);
        let id = Uuid::new_v4();
        assert!(!s.is_open(&id, 0));
        assert!(!s.is_open(&id, 1));
    }

    #[test]
    fn default_depth_two_opens_top_two_levels() {
        let s = TasksTreeState::new(2);
        let id = Uuid::new_v4();
        assert!(s.is_open(&id, 0));
        assert!(s.is_open(&id, 1));
        assert!(!s.is_open(&id, 2));
    }

    #[test]
    fn toggle_flips_baseline() {
        let mut s = TasksTreeState::new(2);
        let id = Uuid::new_v4();
        // Depth 0 is open by default. Toggle → closed.
        assert!(s.is_open(&id, 0));
        s.toggle(id);
        assert!(!s.is_open(&id, 0));
        // Toggle again → back to open.
        s.toggle(id);
        assert!(s.is_open(&id, 0));
    }

    #[test]
    fn toggle_opens_node_below_default_depth() {
        let mut s = TasksTreeState::new(1);
        let id = Uuid::new_v4();
        assert!(!s.is_open(&id, 2));
        s.toggle(id);
        assert!(s.is_open(&id, 2));
    }

    #[test]
    fn expand_all_opens_everything_clears_flips() {
        let mut s = TasksTreeState::new(0);
        let ids = ids(3);
        // Pre-flip one node so we can verify expand_all clears it.
        s.toggle(ids[0]);
        s.expand_all();
        for id in &ids {
            for d in 0..5 {
                assert!(s.is_open(id, d), "id at depth {} should be open", d);
            }
        }
    }

    #[test]
    fn reset_to_default_restores_default_depth_and_clears_flips() {
        let mut s = TasksTreeState::new(2);
        let ids = ids(3);
        s.expand_all();
        s.toggle(ids[0]); // flip on top of expand_all
        s.reset_to_default();
        // Mode back to Default → depth < 2 is open, ≥ 2 is closed.
        for id in &ids {
            assert!(s.is_open(id, 0), "depth 0 should be open after reset");
            assert!(s.is_open(id, 1), "depth 1 should be open after reset");
            assert!(!s.is_open(id, 2), "depth 2 should be closed after reset");
            assert!(!s.is_open(id, 3), "depth 3 should be closed after reset");
        }
    }

    #[test]
    fn reset_to_default_with_depth_zero_collapses_to_roots() {
        // `default_expand_depth: 0` is the escape hatch for users who
        // want vim-style "collapse to roots" from the `zm` chord.
        let mut s = TasksTreeState::new(0);
        let id = Uuid::new_v4();
        s.expand_all();
        s.reset_to_default();
        assert!(!s.is_open(&id, 0));
        assert!(!s.is_open(&id, 1));
    }

    #[test]
    fn toggle_after_expand_all_closes_individual() {
        let mut s = TasksTreeState::new(0);
        let id = Uuid::new_v4();
        s.expand_all();
        assert!(s.is_open(&id, 3));
        s.toggle(id);
        assert!(!s.is_open(&id, 3));
    }

    #[test]
    fn toggle_after_reset_to_default_opens_individual_below_depth() {
        let mut s = TasksTreeState::new(1);
        let id = Uuid::new_v4();
        s.expand_all();
        s.reset_to_default();
        // Depth 2 ≥ default_expand_depth=1 → closed by baseline.
        assert!(!s.is_open(&id, 2));
        s.toggle(id);
        assert!(s.is_open(&id, 2));
    }

    #[test]
    fn transient_open_overrides_baseline_closed() {
        let mut s = TasksTreeState::new(1);
        let id = Uuid::new_v4();
        // depth 2 is below default_expand_depth=1 → baseline closed.
        assert!(!s.is_open(&id, 2));
        s.set_transient_open([id].into_iter().collect());
        assert!(s.is_open(&id, 2));
    }

    #[test]
    fn clear_transient_open_reverts_to_baseline() {
        let mut s = TasksTreeState::new(1);
        let id = Uuid::new_v4();
        s.set_transient_open([id].into_iter().collect());
        assert!(s.is_open(&id, 2));
        s.clear_transient_open();
        assert!(!s.is_open(&id, 2));
    }

    #[test]
    fn commit_transient_open_locks_closed_nodes_via_flipped() {
        let mut s = TasksTreeState::new(1);
        let id = Uuid::new_v4();
        s.set_transient_open([id].into_iter().collect());
        let mut depths = std::collections::HashMap::new();
        depths.insert(id, 2u32);
        s.commit_transient_open(&depths);
        // Transient drained; node now stays open via `flipped`.
        assert!(!s.has_transient_open());
        assert!(s.is_open(&id, 2));
    }

    #[test]
    fn commit_transient_open_skips_already_open_nodes() {
        let mut s = TasksTreeState::new(2);
        let id = Uuid::new_v4();
        // depth 0 < default_expand_depth=2 → baseline open already.
        s.set_transient_open([id].into_iter().collect());
        let mut depths = std::collections::HashMap::new();
        depths.insert(id, 0u32);
        s.commit_transient_open(&depths);
        assert!(s.is_open(&id, 0));
        // Toggling now flips → closes (was not pre-flipped by commit).
        s.toggle(id);
        assert!(!s.is_open(&id, 0));
    }

    #[test]
    fn commit_transient_open_removes_user_collapse_when_baseline_is_open() {
        // User manually collapsed a depth-0 node (baseline open + flipped
        // = closed). Search expanded through it. After commit it must
        // stay open — so the flip must be dropped, not added.
        let mut s = TasksTreeState::new(2);
        let id = Uuid::new_v4();
        s.toggle(id); // baseline open at depth 0 → flipped → closed
        assert!(!s.is_open(&id, 0));
        s.set_transient_open([id].into_iter().collect());
        let mut depths = std::collections::HashMap::new();
        depths.insert(id, 0u32);
        s.commit_transient_open(&depths);
        assert!(s.is_open(&id, 0));
    }

    #[test]
    fn commit_transient_open_unflips_when_user_flip_already_opened_it() {
        // User had manually flipped a baseline-closed node open via
        // <space> before search ran. Commit should leave that flip alone
        // (not double-flip back to closed).
        let mut s = TasksTreeState::new(1);
        let id = Uuid::new_v4();
        s.toggle(id); // baseline closed at depth 2 → flipped → open
        assert!(s.is_open(&id, 2));
        s.set_transient_open([id].into_iter().collect());
        let mut depths = std::collections::HashMap::new();
        depths.insert(id, 2u32);
        s.commit_transient_open(&depths);
        assert!(s.is_open(&id, 2));
    }

    #[test]
    fn reset_to_default_also_drops_transient_open() {
        // Reported regression: after `/` search jumped through a branch
        // and committed (path now in `flipped`), then user pressed `n`
        // again (new transient on a different branch) and finally `zm`,
        // the last transient branch stayed open because `reset_to_default`
        // only touched `flipped`.
        let mut s = TasksTreeState::new(2);
        let id = Uuid::new_v4();
        s.set_transient_open([id].into_iter().collect());
        s.reset_to_default();
        // Depth 2 ≥ default_expand_depth=2 → must be closed now.
        assert!(!s.is_open(&id, 2));
    }

    #[test]
    fn expand_all_drops_transient_open_too() {
        let mut s = TasksTreeState::new(2);
        let id = Uuid::new_v4();
        s.set_transient_open([id].into_iter().collect());
        s.expand_all();
        assert!(!s.has_transient_open());
        // Everything open anyway in AllExpanded mode.
        assert!(s.is_open(&id, 2));
    }

    #[test]
    fn set_default_expand_depth_returns_to_default_mode_and_clears_flips() {
        let mut s = TasksTreeState::new(0);
        let id = Uuid::new_v4();
        s.expand_all();
        s.toggle(id);
        s.set_default_expand_depth(2);
        // Mode back to Default, depth=2. Flips cleared.
        let other = Uuid::new_v4();
        assert!(s.is_open(&other, 0));
        assert!(s.is_open(&other, 1));
        assert!(!s.is_open(&other, 2));
        // The earlier flip is gone too.
        assert!(s.is_open(&id, 1));
    }
}
