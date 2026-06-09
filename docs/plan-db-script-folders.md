# DB Script Folders — Plan

Status: **planned** (no implementation yet).
Tracking memory: `project_db_script_folders.md`.

## Goal

Let users organise Postgres DB-level scripts in arbitrarily deep folder
structures under `db_scripts/<database>/…/<script>.sql`. Existing flat
scripts in the database root stay reachable side-by-side with the new
tree.

## Scope

- Storage: filesystem subdirs under `<instance_data_dir>/db_scripts/<db>/…`
- Adapter: new node type `postgres:db_script_dir` plus updated actions on
  the script leaf for rename / move
- TUI: id parser + dispatch for variable-depth ids, new ViewRequests,
  cmdline namespace `:db-script <sub>`, mark+paste move
- View config: recursive ChildDef marker so the YAML tree stays finite

## Out of scope (deliberate, possible follow-ups)

- Cross-database move
- Recursive directory delete (folder must be empty)
- Overwrite-on-conflict (rename/move into existing name fails hard)
- Drag-and-drop / mouse UX

## Design decisions

### D1 — On-disk layout

Real filesystem subdirs. `<instance>/db_scripts/<db>/<seg₁>/…/<segₙ>.sql`
for scripts; `<instance>/db_scripts/<db>/<seg₁>/…/<segₙ>/` for dirs.
External tools (`ls`, git) keep working; no escaping needed.

### D2 — Node-type model

Two node types. `postgres:db_script_dir` represents a folder; existing
`postgres:db_script` stays the leaf. Each owns its own action set
(no "polymorphic" node that is sometimes both).

| Node                               | Actions                                                                    |
| ---------------------------------- | -------------------------------------------------------------------------- |
| `postgres:db_scripts` (root group) | `add-script`, `add-dir`                                                    |
| `postgres:db_script_dir`           | `add-script`, `add-dir`, `rename`, `mark-move`, `paste-move`, `delete-dir` |
| `postgres:db_script` (leaf)        | `execute`, `edit`, `rename`, `mark-move`, `delete`                         |

`delete` on the leaf keeps the existing dispatch shape; `delete-dir` is
a deliberately different action name so the TUI dispatcher can tell
them apart without an extra filesystem probe (see D5).

### D3 — Node id format

`<db>/db_scripts/<seg₁>/…/<segₙ>` for both dirs and scripts. The
adapter's segment-walking `get_by_id` handles arbitrary depth as-is —
each level's `get_child` resolves one segment. Disambiguation
dir-vs-script happens at the filesystem level inside the parent's
`get_child` (try dir first, then `<seg>.sql`).

Backward compatibility: existing root-level scripts already match this
shape (`<db>/db_scripts/<script>`) and stay valid.

### D4 — Recursive ChildDef in view-config

New field `pub recursive: bool` on `ChildDef`. Semantics:

> A `recursive: true` ChildDef is an implicit member of its own
> `children:`. The effective child set at any depth is
> `{self, …declared_children}`.

Walker (`child_def_for_type_chain`) checks at each step: if the next
chain segment's type equals the current ChildDef's type and the current
def is `recursive: true`, stay on the current def. Otherwise behave
unchanged.

Validator: `recursive: true` is only valid on ChildDefs whose
`children:` does **not** already list the same node_type (would be
redundant); and `recursive: true` must come with a real action set
(otherwise it's just dead recursion).

### D5 — Action dispatch disambiguation

Action names carry the type, not the node_id shape. The TUI dispatcher
keys off `action_name` to decide which `ViewRequest` to emit:

- `delete` → existing `ConfirmDeleteAdapterDbScript` flow
- `delete-dir` → new `ConfirmDeleteAdapterDbScriptDir` flow (with
  not-empty check inside the adapter call, surfacing the error
  via `Notify`)
- `rename` (on either) → new `OpenDbScriptRenamePrompt { node_id }`;
  rename target type derived from the node_id at execute time
- `mark-move` (on either) → `MarkDbScriptForMove { node_id }`; App-side
  state, no dispatch beyond that
- `paste-move` (on dir or group) → `PasteDbScriptMove { target_node_id }`;
  App fetches marked source and dispatches the move

`mark-move` and `paste-move` actions return `ActionDispatch::Noop` from
the adapter (no work to do at the content layer); the TUI dispatcher
recognises the action name and emits the right ViewRequest from the
node id. Comment in `dispatch_to_view_request` documents this coupling.

### D6 — Cmdline namespace

`:db-script <sub>` (mirrors `:query <sub>` from SQ-8):

- `:db-script new <name>` — create script at current dir
- `:db-script new-dir <name>` — create empty dir at current dir
- `:db-script rename <name>` — rename selected
- `:db-script move <dest>` — move selected (or marked) into `<dest>`
- `:db-script delete` — delete selected (confirm popup, same as `d`)

Cmdline operates on the focused pane's selected node. `move` accepts
either an absolute-from-database-root path (`/foo/bar`) or a path
relative to current dir.

### D7 — Move semantics

- Same database only. Source must be marked first (`m` shortcut or
  noted via `:db-script move` with marked source).
- Target must be a dir node or the root group; pasting on a leaf is
  rejected.
- Entry retains its name. Name collision in target dir → error,
  surfaced via `Notify`.
- After a successful move, the source pane reloads; cursor follows the
  moved entry if visible.

## YAML — recursive ChildDef example

```yaml
- name: Scripts
  node_type: "postgres:db_scripts"
  tree_label: name
  shortcuts:
    a: add-script
    A: add-dir
  columns:
    - { key: name, label: Name, style: accent, sizing: max }
  children:
    - name: DB Script Dir
      node_type: "postgres:db_script_dir"
      tree_label: name
      recursive: true # the new marker
      shortcuts:
        a: add-script
        A: add-dir
        r: rename
        m: mark-move
        p: paste-move
        d: delete-dir
      columns:
        - { key: name, label: Name, style: accent, sizing: max }
      children:
        - name: DB Script
          node_type: "postgres:db_script"
          tree_label: script
          enter_action: execute
          shortcuts:
            x: execute
            e: edit
            r: rename
            m: mark-move
            d: delete
          columns:
            - { key: script, label: Script, style: accent, sizing: max }
          children:
            - name: DB Script Result # existing split-pagination anchor
              node_type: "postgres:db_script_result"
              # … unchanged
    - name: DB Script # flat root scripts (unchanged)
      node_type: "postgres:db_script"
      enter_action: execute
      shortcuts:
        x: execute
        e: edit
        r: rename
        m: mark-move
        d: delete
      columns:
        - { key: script, label: Script, style: accent, sizing: max }
      children:
        - name: DB Script Result
          # … unchanged
```

## Phases

### DSF-0 — Plan + memory

This doc + `project_db_script_folders.md` memory entry. Committed
before implementation so a `/compact` round-trip survives.

### DSF-1 — Storage layer (`query.rs`)

- `DbScriptEntry { Dir { rel_path: PathBuf }, Script { rel_path: PathBuf } }`
- `list_db_script_entries(instance, db, dir)` — single level, sorted
- `walk_db_script_entries(instance, db)` — full tree for the database
  (used by the existing `list_all_db_scripts` callers; result type
  bumped from `Vec<DbScriptEntry>`-flat to tree-aware)
- `create_db_script_dir(instance, db, rel_path)` — `mkdir -p` for the
  full path (also creates parents); errors if a file exists at any
  segment
- `delete_db_script_dir(instance, db, rel_path)` — empty-only; bubbles
  "not empty (N entries)" up to the caller
- `move_db_script_entry(instance, db, src_rel, dst_rel)` — works for
  both files and dirs (`std::fs::rename`); errors on cross-device
  (clear message; the instance dir is one mount) and on collision
- `rename_db_script_entry(instance, db, rel_path, new_name)` — thin
  wrapper around `move_db_script_entry` that only changes the last
  segment
- Tests for each, including non-empty dir delete, name collision on
  move/rename, missing source, parent-creation on dir-new

### DSF-2 — Adapter node types

- `db_script_dir_node_type()` constant
- `DbScriptDirNode` struct + `Node` impl:
  - `list()` returns dirs (`postgres:db_script_dir`) and scripts
    (`postgres:db_script`) at this dir
  - `get_child(seg)` resolves dir vs file via filesystem probe
  - `actions()` per D2 / D5
  - `invoke_action()` returns the right `ActionDispatch` (mostly
    `Noop` with the action_name acting as the disambiguator on the TUI
    side)
- `DbScriptsGroupNode.list()` updated to include both dir + script
  entries at the database root
- `DbScriptsGroupNode.get_child()` updated likewise
- `DbScriptNode.actions()` extended with `rename`, `mark-move` (and
  keeps `execute` / `edit` / `delete`)
- Adapter `get_by_id` walker: no change needed (segment-by-segment
  delegation already handles N segments)
- Tests for list/get_child at root and nested dir

### DSF-3 — Recursive ChildDef in view-config

- `pub recursive: bool` on `ChildDef`, `#[serde(default)]`
- `child_def_for_type_chain` walker update per D4
- Validator update + tests (legitimate recursion, accidental cycle
  without `recursive: true` still errors)
- Documentation in `docs/generic-view-spec.md`

### DSF-4 — TUI dispatch

- `parse_db_script_node_id` → `(database, rel_path: Vec<String>)`,
  accepts N segments
- New `ViewRequest` variants:
  - `OpenDbScriptDirNewPrompt { view_index, pane_id, database, parent_rel }`
  - `ConfirmDeleteAdapterDbScriptDir { …, database, rel_path }`
  - `OpenDbScriptRenamePrompt { …, database, rel_path, is_dir }`
  - `MarkDbScriptForMove { node_id }`
  - `PasteDbScriptMove { target_node_id }`
- `dispatch_to_view_request` extended to recognise the new action
  names (D5)
- App-side handlers: confirm-popup, rename-prompt, marked-state
  indicator in status bar (mirrors marked-link UX), paste handler that
  calls `move_db_script_entry` and reloads
- Unit tests at `app::node_actions` boundary

### DSF-5 — Cmdline (`:db-script <sub>`)

- Parser + dispatch in `execute_cmdline` (mirrors `:query` shape)
- Subcommands per D6
- Tests for each subcommand's parse / dispatch
- Tab completion for `move <dest>` against existing dir paths
  (nice-to-have, can defer if it grows the diff)

### DSF-6 — `postgres.yaml` cutover

- Update `~/.config/not_yet_done/views/postgres.yaml` per the
  YAML example above (recursive Dir branch + flat-script branch
  retained)
- Verify the validator green-lights it

### DSF-7 — Docs + smoke + install + commit

- README section on DB-script folders (placement: after the existing
  DB-Scripts section)
- New `docs/smoke-tests.md` section (per memory
  `feedback_smoke_tests_central` — central file, no per-phase
  splits): create dir, create script in nested dir, rename, move,
  delete-non-empty rejection, flat scripts still reachable
- `cargo build --release && cargo install --path not-yet-done-tui --offline`
- Commit per phase; final smoke after DSF-6
