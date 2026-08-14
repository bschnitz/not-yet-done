# 0003 — Resolving tree levels by `node_type_chain` instead of by depth

- **Status:** accepted, implemented (`0f998c9`)
- **Date:** 2026-06-05
- **Affects:** `not-yet-done-tui` (`content_view.rs` —
  `build_tree_data_rows`, `current_columns`, `tree_current_actions`,
  `tree_active_child_def`, the new `cursor_tree_level` /
  `cursor_node_type_chain`; `content_tree.rs` — resolution helpers)

## Context

The generic `ContentView` renders hierarchical adapter data as a tree:
nested nodes are flattened into a linear list of rows (`TreeEntry`) and
laid out through `not-yet-done-table`. At render time every row needs its
**level** in the tree — that is what determines the column set, which
column carries the `indent+glyph+label` (the "label column"), the actions
and the preview config.

That level was resolved in **two** ways, and the two could disagree:

1. **By depth** (`tree_level_at_depth(depth)` and relatives) — a walk that
   starts at the root and at every step takes the **first** tree-continuing
   child (`first_tree_child`).
2. **By `node_type_chain`** (`tree_level_for_chain(&chain)`) — the exact
   type path that every `TreeEntry` carries anyway.

A **multi-branch tree** with branches of differing depth breaks the depth
variant: the same depth maps to a **different** type per branch. Take the
Stoat chat:

- Branch A (uncategorized): `server(0) → channel(1) → message(2, leaf)`
- Branch B (category): `server(0) → category(1) → channel(2) → message(3)`

`tree_label_at_depth(2)` walks branch A → `message` (no `tree_label`) →
`None`. But the channels **under a category** sit at depth 2 as well
(branch B) → their label column was not found → those rows render as
**blank lines**.

Over time the symptom showed up at several levels of different adapters
(Confluence `name`/`title`, then Stoat). The "fix" each time was the
convention **"give the `tree_label` keys the same name across all
levels"** — a workaround that only holds under the single-chain assumption
and breaks again with every new branch of a different depth.

## Decision

A row's `node_type_chain` is its **unambiguous coordinate** in the tree;
`depth` is a lossy projection of it. Therefore: **every resolution that has
a row (or the cursor row) at hand goes through the chain, never through the
depth.** Two parts that belong together:

1. **A single source of truth.** The new `cursor_tree_level()` /
   `cursor_node_type_chain()` resolve the column set, the label column, the
   actions and the preview of the cursor row from the chain.
   `current_columns`, `tree_current_actions` (active level) and
   `tree_active_child_def` were migrated onto them; the dead depth helpers
   `tree_label_at_depth` / `tree_columns_at_depth` are gone.

2. **The label column as a designated slot.** The label column is
   determined **once** from the cursor level (its `tree_label` — a fixed
   key of the active column set). **Every** row paints its
   `indent+glyph+label` into exactly that column, regardless of its own
   level. Because the label column and the column set come from the **same**
   level, they are consistent by construction — the earlier cross-level key
   alignment convention falls away entirely.

The one remaining invariant — `tree_label` has to be a key of the level's
**own** columns — is already enforced by the config validator
(`view_config.rs`, `check_tree`/`walk_tree_child`). The former silent
failure mode (a blank line) is now a loud config error.

## Options (and why they were rejected)

1. **Resolve per row by its _own_ chain** (label column = the column
   holding the key of that row's level). Fixes the Stoat case (all levels
   use `name`) but leaves the convention in place for **heterogeneous**
   keys: if a level has `title` while the active column set only has
   `name`, the row still comes out blank. It moves the problem instead of
   removing it.
2. **Keep the convention and enforce it in the validator** (all
   `tree_label`s of one chain must have the same name). Rejected: it fixes
   an artificial coupling between levels that have nothing to do with each
   other (why should a channel label key match a category label key?), and
   it constrains heterogeneous trees for no reason.
3. **Render a separate table/column geometry per depth.** Rejected: a large
   rebuild of the table layer; the actual bug was the resolution, not the
   single-table layout.

## Consequences

- The old convention "`tree_label` keys have to align across levels" is
  **obsolete**. Multi-branch trees with branches of differing depth and
  divergent label keys render correctly (Stoat
  `server → category → channel`, Postgres `schemas`/`scripts`, Confluence
  page trees).
- A visible consequence of part 2: in trees with **different** label keys
  per level, the label **column position** moves with the cursor when it
  changes level. That is consistent with the existing behaviour (the header
  and column set change per cursor level anyway) and was accepted
  deliberately.
- The regression test
  `tree_renders_deep_branch_label_despite_divergent_keys` builds a
  multi-branch tree (uneven depth, keys `name` vs `title`) and checks that
  the deep branch rows render non-empty labels — verified against the old
  depth resolution, where it failed.
- **Left alone deliberately:** `tree_self_at_depth` in the tree-find walker
  (`tree_find_dispatch_step`) and the `current_children` fallback stay
  depth-based — the walker has no `node_type_chain` at hand there, and
  neither is part of the render path. If tree-find ever jumps to the wrong
  place on a multi-branch tree, that is the next place to look.
