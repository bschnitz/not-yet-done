//! The `load` hook for rows that reach a pane outside its root load.
//!
//! [`App::run_load_hooks`] runs in the `LoadMsg::ContentItems` handler and
//! sees the pane's root rows. A tree gets the rest through two more
//! messages, and without this module a hook would colour or patch the top
//! level only:
//!
//!   - `LoadMsg::Subtree` — an eager tree (`supports_eager_subtree`) loads
//!     every expanded level in one call. That call re-delivers the root rows
//!     too, and `apply_subtree` re-caches them, so the hook runs *here* on
//!     all of them and the `ContentItems` seam stands aside (see
//!     [`App::eager_subtree_follows`]) instead of running the script twice.
//!   - `LoadMsg::TreeChildren` — one parent's children, expanded lazily.
//!   - a single row patched in place without a load: the adapter's own
//!     refresh (`Invalidation::Row`, the live tick) and the re-fetch after
//!     an editor save (`App::patch_content_row`).
//!
//! Only rows of the pane's own level type are handed over: the hook is bound
//! to that level's script scope, and a child level of a different type has
//! a scope (and scripts) of its own.
//!
//! `order` is refused on these seams ([`LoadSeam::Tree`]). Highlights are
//! kept per row: a load that covers the whole tree replaces the pane's map,
//! a partial one takes back what it previously painted on its own rows and
//! adds what it paints now.

use std::collections::HashMap;

use not_yet_done_content::{NodeSummary, QueryKind, Subtree};

use crate::app::App;
use crate::app::script_hook::LoadSeam;
use crate::views::content_highlights::ScriptHighlights;
use crate::views::content_view::PaneId;

impl App {
    /// Whether this pane's root load is followed by an eager subtree load
    /// that re-delivers its rows — the same test `spawn_subtree_load` makes
    /// before it spawns. The `ContentItems` seam skips its hook run then,
    /// because the `Subtree` seam runs it over the whole tree anyway.
    pub(super) fn eager_subtree_follows(&self, view_index: usize, pane_id: PaneId) -> bool {
        let Some(cv) = self.content_view(view_index) else {
            return false;
        };
        let Some(pane) = cv.find_pane(pane_id) else {
            return false;
        };
        if cv.adapter.is_none() || pane.eager_subtree_depth(&cv.view_defs).is_none() {
            return false;
        }
        match pane.root_load_request(&cv.view_defs) {
            Some(req) => !(req.kind == QueryKind::Extended && req.query.is_some()),
            None => false,
        }
    }

    /// Run the `load` hooks over an eagerly loaded subtree and patch its rows
    /// in place, before `apply_subtree` caches them. `parent_path` empty
    /// means the subtree is the whole tree.
    pub(super) fn run_subtree_load_hooks(
        &mut self,
        view_index: usize,
        pane_id: PaneId,
        hook_depth: u8,
        parent_path: &[String],
        subtree: &mut Subtree,
    ) {
        let Some(level_type) = self.pane_level_type(view_index, pane_id) else {
            return;
        };
        let mut rows = Vec::new();
        collect_level_rows(subtree, &level_type, &mut rows);
        if rows.is_empty() {
            return;
        }
        let highlights =
            self.run_load_hooks(view_index, pane_id, hook_depth, LoadSeam::Tree, &mut rows);
        let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
        let mut patched: HashMap<String, NodeSummary> =
            rows.into_iter().map(|r| (r.id.clone(), r)).collect();
        write_back(subtree, &level_type, &mut patched);
        self.store_tree_highlights(
            view_index,
            pane_id,
            parent_path.is_empty(),
            &ids,
            highlights,
        );
    }

    /// The same for one parent's lazily loaded children. Rows of another
    /// level type are left alone.
    pub(super) fn run_tree_children_load_hooks(
        &mut self,
        view_index: usize,
        pane_id: PaneId,
        hook_depth: u8,
        child_node_type: &str,
        items: &mut [NodeSummary],
    ) {
        if items.is_empty()
            || self.pane_level_type(view_index, pane_id).as_deref() != Some(child_node_type)
        {
            return;
        }
        let highlights =
            self.run_load_hooks(view_index, pane_id, hook_depth, LoadSeam::Tree, items);
        let ids: Vec<String> = items.iter().map(|r| r.id.clone()).collect();
        self.store_tree_highlights(view_index, pane_id, false, &ids, highlights);
    }

    /// The same for one row patched in place (`Invalidation::Row`, or the
    /// re-fetch after an editor save): run the hooks of every
    /// pane whose level shows rows of this type, so the patched row keeps
    /// the cells and colours a full load would have given it.
    pub(super) fn run_row_load_hooks(&mut self, view_index: usize, summary: &mut NodeSummary) {
        let panes = self
            .content_view(view_index)
            .map(|cv| cv.all_pane_ids())
            .unwrap_or_default();
        let hook_depth = self.load_hook_depth;
        let type_id = summary.node_type.type_id.clone();
        for pane_id in panes {
            let mut rows = [summary.clone()];
            self.run_tree_children_load_hooks(view_index, pane_id, hook_depth, &type_id, &mut rows);
            let [row] = rows;
            *summary = row;
        }
    }

    /// The node type of the pane's own level — the one its script scope is
    /// named after.
    fn pane_level_type(&self, view_index: usize, pane_id: PaneId) -> Option<String> {
        let cv = self.content_view(view_index)?;
        let pane = cv.find_pane(pane_id)?;
        Some(pane.root_load_request(&cv.view_defs)?.node_type_id)
    }

    fn store_tree_highlights(
        &mut self,
        view_index: usize,
        pane_id: PaneId,
        whole_tree: bool,
        ids: &[String],
        highlights: ScriptHighlights,
    ) {
        let Some(pane) = self
            .content_view_mut(view_index)
            .and_then(|cv| cv.find_pane_mut(pane_id))
        else {
            return;
        };
        if whole_tree {
            pane.script_highlights = highlights;
        } else {
            pane.script_highlights
                .forget_rows(ids.iter().map(String::as_str));
            pane.script_highlights.merge(highlights);
        }
    }
}

/// Every row of `level_type` in the subtree, depth first, cloned — the hook
/// payload is one flat list of rows whatever their depth.
fn collect_level_rows(subtree: &Subtree, level_type: &str, out: &mut Vec<NodeSummary>) {
    for node in &subtree.items {
        if node.summary.node_type.type_id == level_type {
            out.push(node.summary.clone());
        }
        collect_level_rows(&node.children, level_type, out);
    }
}

/// Put the patched metadata back on the rows it was cloned from. A patch
/// touches `metadata` only, so that is all that travels.
fn write_back(subtree: &mut Subtree, level_type: &str, patched: &mut HashMap<String, NodeSummary>) {
    for node in &mut subtree.items {
        if node.summary.node_type.type_id == level_type {
            if let Some(row) = patched.remove(&node.summary.id) {
                node.summary.metadata = row.metadata;
            }
        }
        write_back(&mut node.children, level_type, patched);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::{Metadata, NodeType, SubtreeNode};

    fn node(id: &str, type_id: &str, children: Vec<SubtreeNode>) -> SubtreeNode {
        SubtreeNode {
            summary: NodeSummary {
                id: id.to_string(),
                label: id.to_string(),
                node_type: NodeType {
                    type_id: type_id.to_string(),
                    mime_type: "text/plain".into(),
                    syntax: None,
                    file_extension: ".txt".into(),
                    display_name: type_id.to_string(),
                },
                metadata: Metadata::default(),
                has_children: None,
            },
            children: Subtree {
                items: children,
                page: None,
            },
        }
    }

    #[test]
    fn collects_every_depth_but_only_the_level_type() {
        let tree = Subtree {
            items: vec![node(
                "a",
                "task",
                vec![
                    node("b", "task", vec![node("c", "task", vec![])]),
                    node("n", "note", vec![]),
                ],
            )],
            page: None,
        };
        let mut rows = Vec::new();
        collect_level_rows(&tree, "task", &mut rows);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn write_back_reaches_nested_rows() {
        let mut tree = Subtree {
            items: vec![node("a", "task", vec![node("b", "task", vec![])])],
            page: None,
        };
        let mut rows = Vec::new();
        collect_level_rows(&tree, "task", &mut rows);
        rows[1]
            .metadata
            .fields
            .push(not_yet_done_content::MetadataField {
                key: "x".into(),
                value: "1".into(),
                display_label: "X".into(),
                editable: false,
                allowed_values: None,
            });
        let mut patched = rows.into_iter().map(|r| (r.id.clone(), r)).collect();
        write_back(&mut tree, "task", &mut patched);
        let b = &tree.items[0].children.items[0].summary;
        assert_eq!(b.metadata.fields[0].value, "1");
    }
}
