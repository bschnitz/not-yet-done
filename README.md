# not-yet-done

A terminal-based task and time tracking application with a rich TUI, CLI, and Waybar integration.

<!-- screenshot: full TUI with tasks tree view, action bar, status bar visible -->

![TUI Overview](docs/screenshots/tui-overview.png)

## Features

- **Hierarchical task management** — organize tasks in a tree structure with unlimited nesting
- **Time tracking** — start/stop tracking per task, with parallel tracking support
- **Rich TUI** — keyboard-driven interface with fuzzy filter, text search, hop-style jump navigation, saved filters, favorites, and configurable columns
- **CLI** — full command-line interface for scripting and automation
- **Waybar module** — CFFI module showing the active tracking in your status bar
- **Per-task notes** — Markdown notes per task, auto-organized in a directory tree matching the task hierarchy
- **Scripts** — run user scripts on the focused node or the filtered Trackings list via the `:script` fuzzy menu, with background, capture, and interactive modes
- **Filter DSL** — YAML-based query language with natural-language date expressions
- **Daily backups** — automatic daily database backup on startup

## Installation

```bash
# Build and install all binaries
cargo install --path not-yet-done-cli
cargo install --path not-yet-done-tui

# Build the Waybar module
cargo build --release -p not-yet-done-waybar
cp target/release/libnyd_waybar.so ~/.config/waybar/cffi/
```

Initialize the database on first run:

```bash
nyd db sync
```

## TUI

Start the TUI with:

```bash
not-yet-done-tui
```

### Tabs and Views

The TUI has two main tabs, each with sub-views:

**Tasks** — manage your task tree

| Sub-view | Key  | Description                                            |
| -------- | ---- | ------------------------------------------------------ |
| List     | `vl` | Flat list of all tasks matching the active filter      |
| Tree     | `vt` | Hierarchical tree view with indentation and connectors |

<!-- screenshot: tasks tab in tree view showing nested tasks with priority, status, notes indicator -->

![Tasks Tree View](docs/screenshots/tasks-tree.png)

#### Tasks tree expand / collapse

In tree view, branches can be folded individually:

| Key     | Action                                        |
| ------- | --------------------------------------------- |
| `enter` | Toggle expand/collapse on cursor              |
| `zr`    | Expand all branches                           |
| `zm`    | Collapse to configured `default_expand_depth` |

Collapsed parents render with a `▶` glyph and a trailing `(N)` count
showing how many direct children are hidden. Expanded parents render
with `▼`. The default number of visible levels is configured in
`tui.yaml`:

```yaml
tasks:
  tree:
    default_expand_depth: 2 # 0 = only roots, 1 = roots + their children, ...
```

The expand state is per-session — it is not persisted across
restarts. When a fuzzy filter is active, expand state is ignored
and every matching node (plus its ancestors) is shown.

**Trackings** — view and analyze time entries

| Sub-view  | Key | Description                                      |
| --------- | --- | ------------------------------------------------ |
| Normal    | `a` | Individual tracking entries with start/end times |
| Condensed | `v` | One row per task, durations summed               |
| Tree      | `t` | Task tree with cumulated durations per branch    |

<!-- screenshot: trackings tab in normal view with day grouping, showing group headers and footer with totals -->

![Trackings View](docs/screenshots/trackings-normal.png)

Trackings can be grouped by day, week, month, or year (`G` to cycle). Active tracking durations update live with adaptive intervals.

### Tab constellations

The set of top-level tabs and their switch keys is driven by named
_constellations_ in `tui.yaml`. A constellation is an ordered list of
tab names; the active one decides **which tabs are shown, in what order,
and which digit key selects each**:

```yaml
tabs:
  active: default
  sets:
    # Shorthand — a bare list of tab names (no icon, no popup shortcut):
    default:
      - Tasks
      - Trackings
      - Jira
      - Taiga
      - Analytics DB
      - Confluence
      - Stoat
    # Full form — adds an `icon` and a human-friendly `label` (both shown
    # in the switch popup) and a single-key `shortcut` (pressed in the
    # popup to switch here):
    my-corp:
      icon: ""
      label: My Corp
      shortcut: m
      tabs:
        - Tasks
        - Trackings
        - Stoat
```

A constellation may be written either as a bare list of tab names
(shorthand) or as a mapping with `tabs:` plus optional `icon:`, `label:`
and `shortcut:`. Both forms can be mixed freely under `sets:`. The popup
shows `icon label` (the `label` falls back to the set's key when
omitted), so a slug-style key like `work` can present as `Work`.

Tabs are referenced by display name: the built-in `Tasks` / `Trackings`
plus each view's `tab.name`. The list order assigns the **autonumber**
keys — `1`..`9`, then `0` for a tenth tab; an eleventh and beyond get no
digit and are reachable only via `Tab` / `Shift+Tab`. While a
constellation is active the visible tabs own every digit key, so the
legacy fixed `tab_jira: '3'` etc. bindings no longer apply.

Only the tabs named in the active constellation are shown; a configured
view whose `tab.name` is absent from the list is hidden (without being
unloaded). Tabs not referenced anywhere stay hidden until you add them.

**Why this exists:** the previous model wired tab keys to fixed actions
(`1`..`6`, positionally bound to Jira/Taiga/Postgres/Confluence), so a
fifth adapter tab (e.g. Stoat) got no key at all. Constellations make the
tab set and its numbering data-driven and let you keep several curated
layouts side by side (a lean `default` vs. a wider `my-corp`).

If no `tabs:` section is present the feature stays dormant: every
configured tab is shown in its `order:` with the legacy fixed keys, so
existing setups keep working unchanged. **Two tabs sharing a display
name is a hard error** (the name could no longer identify a tab) — the
TUI shows a startup modal and falls back to the legacy layout.

**Switching at runtime — the tab-set popup.** Press `Ctrl+X` (the
`tab_set_popup` global binding) to open a popup listing every
constellation with its icon. Press a set's `shortcut` key to switch to
it immediately, or use the arrow keys and `Enter` to pick one without a
shortcut; `Esc` cancels. Switching updates the active constellation and
rebuilds the tab layout on the spot.

> The switch is **session-only** — it is not written back to `tui.yaml`,
> so a restart returns to the configured `active` set. (`active` is also
> re-read on config reload via `:config` or editing `tui.yaml`.)

### Navigation

| Key                 | Action                   |
| ------------------- | ------------------------ |
| `↑` / `↓`           | Move cursor              |
| `gg` / `gj`         | Jump to first / last row |
| `Ctrl+u` / `Ctrl+d` | Scroll half page         |
| `Ctrl+b` / `Ctrl+f` | Scroll full page         |

#### Hop-Style Jump

Press `p` to enter jump mode:

1. Type a character — all visible rows containing that character are highlighted
2. Labels appear inline after each match
3. Type the label to jump to that row
4. Single matches jump immediately

<!-- screenshot: jump mode active, showing yellow labels next to matched characters, non-matching rows dimmed -->

![Jump Mode](docs/screenshots/jump-mode.png)

### Task Operations

| Key      | Action                                                        |
| -------- | ------------------------------------------------------------- |
| `a`      | Add a new task (opens editor)                                 |
| `e`      | Edit selected task                                            |
| `Ctrl+n` | Edit subtree structure (add/move/delete tasks in tree editor) |
| `d`      | Soft-delete selected task                                     |
| `u`      | Restore last deleted task                                     |
| `o`      | Open/edit notes for selected task                             |
| `s`      | Start/stop tracking on selected task                          |

### Filtering

**Fuzzy filter** — press `f` to type a fuzzy filter. Matches are shown instantly. Press `Enter` to accept, `Esc` to cancel.

**Text search** — press `/` to search. `n` / `N` jump between matches.

**Query filter** — press `Q` to open a YAML filter editor with full DSL support (see [Filter DSL](#filter-dsl)). Filters apply live on each save.

**Saved filters** — press `q` to pick from saved filters. Filters are persisted across sessions. The last active filter is restored on startup.

**Favorites** — in the saved filter picker, press `Ctrl+f` on a filter to assign a keyboard shortcut for instant activation.

### Command Line

Press `:` to open an ex-style command line. Type any CLI command without the `nyd` prefix:

```
:backup create
:task add "New task"
:track start <task-id>
```

Output is shown as a modal popup.

In-process commands (executed by the TUI itself, not via subprocess):

- `:linkprune` — bulk-delete link rows whose endpoints no longer
  resolve (deleted tasks, gone tickets, etc.). Asks for confirmation
  before any DB writes.
- `:dismiss-notifications` — clear the notification bar, sticky
  notification, and most recent query-error banner. Mirrors the
  `dismiss_notifications` keybinding (default `Z` — lower-case `z`
  is reserved as the chord prefix for tasks-tree `zr`/`zm`).
- `:cut-node` (default `mc`) — mark the currently selected task as
  the move source. The tree is _not_ touched until `:paste-node`
  runs; the cut can be cancelled with `Esc` or overwritten by
  another `:cut-node`.
- `:paste-node` (default `mp`) — reparent the cut task so the
  currently selected task becomes its new parent. Refuses any move
  that would create a cycle (target equals the cut node, or sits
  inside the cut node's subtree) and shows a modal error; in those
  cases the tree is left untouched and the cut stays armed so the
  user can pick a different target.
- `:jump <Tab>[:<sub>]` — programmatic tab + sub-tab switch.
  Recognises `Tasks` (sub: `list` / `tree`), `Trackings` (sub:
  `normal` / `condensed` / `tree`), and any content tab by its
  configured name (case-insensitive). Used by user scripts to drive
  the TUI from outside; also typeable directly. Modal error on
  unknown tab or sub-view.
- `:focus-task [-i] /seg/seg/...` — in the Tasks:tree sub-view, walk
  the task hierarchy from the roots down, matching each
  `/`-separated segment against task descriptions. Default is
  **case-sensitive substring** matching; pass `-i` before the path
  to fold case across all segments. Each segment may opt into regex
  matching with the `re:` prefix — e.g.
  `/work/clients/acme/tickets/re:\b42\b` to match ticket 42
  but not 420/421. Patterns use the Rust `regex` crate; with `-i`
  the inline flag `(?i)` is auto-prepended (override per-segment
  with `re:(?-i)...` if needed). Auto-expands the ancestor path and
  parks the cursor on the matched node. Modal error (tree
  unchanged) on unknown flag, malformed regex, path not found,
  ambiguous segment, or when the active sub-view isn't Tasks:tree.
- `:reload-tasks` — synchronously refetch the task rows from the
  database. Use this after an external mutation (e.g. a script that
  ran `nyd task add`) so a following `:focus-task` in the same
  command chain sees the new row. Works from any tab; silent on
  success, modal-error on DB failure.
- `:focus-node [-i] <Tab>[:<view>] /<col>|<pattern>` — the
  content-view counterpart to `:focus-task`. Switches to the named
  content tab (and optional sub-view), then parks the cursor on the
  first row whose `<col>` matches `<pattern>`. Without an explicit
  column hint (`/<pattern>`), the pattern is matched against
  `label` plus all metadata values. `re:` opts into regex (e.g.
  `re:\b42\b`); `-i` switches both substring and regex matching to
  case-insensitive. Single-segment only — drill-down paths
  (`/schema/table/...`) are reserved for tree-shaped content views
  and currently return a modal error. Modal errors also on unknown
  tab/view, unknown column, no match, or ambiguous match.
  Example: `:focus-node Taiga:items /ref|acme#42`.
- `:query <subcommand>` — namespace for saved-query operations:
  `apply` activates a saved query (read), `edit` / `new` / `delete`
  manage the saved-query bodies stored by the active content tab's
  adapter.
  - `:query apply [--var k=v]* [-t <Tab>[:<view>]] <name>` — activate
    the saved query `<name>` on a content tab and **synchronously**
    reload so a following command in the same script (typically
    `:focus-node`) sees the new rows. Without `-t` the currently
    active content tab is used; with `-t` the named tab (and
    optional sub-view) is switched to first. `<name>` is matched
    case-insensitively against the merged YAML + DB saved-query
    list of the active view and may contain whitespace. Modal
    error on unknown tab/view, unknown name, or adapter error
    during reload.
    Example: `:query apply -t Taiga:items Open issues`.

    **Query variables.** Saved queries can carry adapter-specific
    placeholders (Taiga: `${name:default}`). At apply time the
    adapter reports which variables it needs; if any required
    variable (one without a default) is unset, the TUI opens a
    small input popup before the load. Pre-fill values from
    scripts with `--var key=value` — the popup is skipped when all
    required variables are covered. Interactive entry points
    (the keyboard shortcut for a saved query, the query menu's
    Apply action) always open the popup so the user can confirm
    or override defaults.
    Example: `:query apply --var project=alpha -t Taiga:items "Open per project"`.

  - `:query edit <name>` / `:query new <name>` / `:query delete <name>`
    — manage saved-query bodies on the **active content tab**. The
    body file is owned by the adapter (one file per query under the
    adapter's per-instance data dir); `edit` opens the existing file
    in `$EDITOR`, `new` opens an empty buffer that becomes a new file
    on first save, and `delete` removes the body **and** any DB
    shortcut row for that name. Modal error when the active tab is
    not a content tab, the adapter exposes no filesystem-backed store,
    or — for `edit` — the named query doesn't exist. Names may contain
    whitespace. Adapter-specific body validation only happens at apply
    time, not on save.

- `:db-script-new <database> <script>` — Postgres-only legacy cmdline
  that creates an empty DB-level script under
  `<instance_data_dir>/db_scripts/<database>/<script>.sql` and
  immediately opens it in the editor. Refuses names containing `/`,
  `\`, leading `.`, whitespace, or that already exist. Use `x` to
  execute the script (cursor-paginated result pane) and `d` to delete
  it after a confirm popup. For folder-aware operations, prefer the
  `:db-script <sub>` namespace below.

- `:db-script <sub>` (DSF) — folder-aware namespace that operates on
  the focused content pane's selected row. Subcommands:
  - `:db-script new <name>` — create a script in the currently
    focused dir (or root if the selected row is the DB-Scripts group).
    Mkdir's parents so nested creation works.
  - `:db-script new-dir <name>` — create an empty folder in the
    focused dir. Reached via `A` on a DB-Scripts group or folder row.
  - `:db-script rename <name>` — rename the selected entry. Reached
    via `r`.
  - `:db-script move <dest>` — move the marked source (set via `m`)
    or the selected row into `<dest>`. `<dest>` is absolute when it
    starts with `/`, otherwise relative to the focused dir.
    Cross-database moves are rejected.
  - `:db-script delete` — confirm-then-delete the selected row.
    Empty folders only — non-empty folders surface a "not empty (N)"
    error from the storage layer. Reached via `d`.

  Shortcuts on DB-Scripts rows (defaults; user-overridable in
  `postgres.yaml`):
  - Group node `Scripts`: `a` add-script, `A` add-dir.
  - Folder node `DB Script Dir`: `a` add-script, `A` add-dir, `r`
    rename, `m` mark-move, `p` paste-move, `d` delete-dir.
  - Script leaf `DB Script`: `x` execute, `e` edit, `r` rename,
    `m` mark-move, `d` delete.

  Marked-source indicator appears in the status bar as `⚓ marked:
move: <node-id>` until paste or `Esc` clears it.

- `:config [name]` — open a fuzzy picker of all YAML configs under
  `~/.config/not_yet_done/`. With `name`, pre-filters or jumps
  straight to the unique match. Selecting a file opens it in
  `$EDITOR`; on save the config is re-applied in-process — granular
  for view yamls (only the affected tab is rebuilt), full for
  `tui.yaml` and adapter configs. Parse / validation errors leave
  the running config untouched and reopen the editor with the
  error rendered as a YAML-comment banner at the top of the
  buffer.

<!-- screenshot: command line open at bottom showing ":backup create" being typed -->

![Command Line](docs/screenshots/command-line.png)

#### Cmdline shortcuts

Single-key shortcuts for cmdline commands can be defined in
`tui.yaml`. They bypass the `:` prompt and fire the bound command
directly. Only triggered when the key has no other typed-action
binding, so they can't shadow existing keys.

```yaml
cmdline_shortcuts:
  F2: "config tui"
  "<c-comma>": "config"
```

**Built-in defaults** (active when the section is absent from your
`tui.yaml`):

| Key  | Command      | Effect                                              |
| ---- | ------------ | --------------------------------------------------- |
| `T`  | `tag`        | Open the tag-management menu                        |
| `mc` | `cut-node`   | Mark the currently selected task as the move source |
| `mp` | `paste-node` | Move the cut task under the currently selected task |

Multi-character keys (e.g. `mc`, `mp`) participate in chord-prefix
detection: typing `m` stashes the key and waits for the second
character. Single keys can still be safely shadowed because the
shortcut lookup runs only when no typed-action handler claimed the
key, and chord-prefix detection now also considers shortcut keys
so the user gets the usual "stash + complete" semantics.

Defining `cmdline_shortcuts:` in your own `tui.yaml` replaces the
defaults wholesale — copy the entries you want to keep.

### Column Configuration

Press `c` to open the column configurator. Toggle columns on/off and reorder them. Available columns vary by view:

- **Tasks**: description, status, priority, notes, created, updated, last tracked
- **Trackings normal**: marker, task, started, ended, duration
- **Trackings condensed**: marker, task, duration
- **Trackings tree**: marker, task, own duration, cumulated duration

## CLI

The CLI binary is `nyd` (installed as `not-yet-done-cli`).

### Task Management

```bash
# Add a task
nyd task add "Build API endpoint"
nyd task add "Design schema" --parent <parent-task-id>

# List all tasks
nyd task list

# Edit a task
nyd task edit <id> --description "Updated name"

# Soft-delete
nyd task delete <id>

# Export a subtree as nested JSON
nyd task tree <root-id> --pretty
nyd task tree <root-id> --last-tracked-since "2026-04-01" --pretty
nyd task tree AcmeCorp --pretty  # resolve by description prefix
```

### Time Tracking

```bash
# Start tracking (stops other active trackings by default)
nyd track start <task-id>
nyd track start <task-id> --parallel    # keep others running

# Stop tracking
nyd track stop                          # stop all active
nyd track stop --task-id <uuid>         # stop specific task

# View summary
nyd track summary                       # today
nyd track summary --from "last monday"  # since Monday

# Export as JSON
nyd track export --from "2026-04-01" --pretty

# Split a tracking at a time point
nyd track split <entry-id> "14:30"

# Move a tracking to a different start time
nyd track move <entry-id> "09:00" --gravity start

# Restore a soft-deleted tracking
nyd track restore <entry-id>
```

### Query / Filter

```bash
# Run a filter from a YAML file
nyd query run --entity tracking --file filter.yaml

# With debug output (shows resolved dates and FilterExpr)
nyd query run --entity task --file filter.yaml --debug

# Pipe from stdin
echo 'query: [deleted, =, false]' | nyd query run --entity task
```

### Database & Backup

```bash
# Sync schema (run after updates)
nyd db sync

# Backups
nyd backup create
nyd backup list
nyd backup restore 20260323-185627-nyd.db
```

## Filter DSL

Filters are YAML documents with a `query:` key. Used in the TUI editor (`Q`), saved filters, favorites, and the CLI `query run` command.

### Basic Syntax

```yaml
# Simple leaf: [column, operator, value]
query:
  [description, has, meeting]

# Named filter
name: Active tasks
query:
  and:
    - [deleted, =, false]
    - [status, in, [todo, in_progress]]
```

### Operators

| Operator      | Aliases     | Example                                            |
| ------------- | ----------- | -------------------------------------------------- |
| `=`           | `==`, `eq`  | `[status, =, todo]`                                |
| `!=`          | `<>`, `ne`  | `[status, !=, done]`                               |
| `>`           | `gt`        | `[priority, gt, 3]`                                |
| `>=`          | `ge`, `gte` | `[created_at, '>=', '2 weeks ago']`                |
| `<`           | `lt`        | `[priority, lt, 5]`                                |
| `<=`          | `le`, `lte` | `[started_at, '<=', yesterday]`                    |
| `like`        |             | `[description, like, '%api%']`                     |
| `not_like`    |             | `[description, not_like, '%test%']`                |
| `has`         |             | `[description, has, meeting]` → `LIKE '%meeting%'` |
| `is_null`     |             | `[parent_id, is_null]`                             |
| `is_not_null` |             | `[ended_at, is_not_null]`                          |
| `in`          |             | `[status, in, [todo, in_progress]]`                |
| `not_in`      |             | `[status, not_in, [done, cancelled]]`              |

### Tree Operators

Tree operators use a 2-element shorthand `[op, value]` and query the task hierarchy via the materialized `path` column:

| Operator       | Example                  | Matches                                    |
| -------------- | ------------------------ | ------------------------------------------ |
| `in_tree`      | `[in_tree, Globex]`      | Globex **and** all tasks below it          |
| `has_ancestor` | `[has_ancestor, Globex]` | All tasks below Globex (not Globex itself) |

The value can be an exact description or a LIKE pattern:

```yaml
# All tasks in the Globex subtree
[in_tree, Globex]

# Tasks below any node matching "Ticket"
[has_ancestor, '%Ticket%']

# Combine with other filters
query:
  and:
    - [deleted, =, false]
    - [in_tree, Globex]
    - [status, =, todo]
```

Tree operators work in both task and tracking filters. In tracking filters, they match against the tracked task's tree position.

### Compound Expressions

```yaml
query:
  and:
    - [deleted, =, false]
    - or:
        - [priority, ">=", 5]
        - [description, has, urgent]
    - not: [status, =, cancelled]
```

### Date Expressions

String values are automatically resolved as natural-language dates:

| Expression     | Resolves to                |
| -------------- | -------------------------- |
| `yesterday`    | Yesterday at midnight      |
| `last monday`  | Most recent Monday         |
| `2 weeks ago`  | 14 days before now         |
| `1 month ago`  | One month before now       |
| `april 1`      | April 1st of current year  |
| `last april`   | April 1st of previous year |
| `2026-04-01`   | Exact date                 |
| `3h`           | 3 hours from now           |
| `6 months ago` | 6 months before now        |

### Available Fields

**Task filters**: `id`, `description`, `status`, `deleted`, `deleted_at`, `priority`, `parent_id`, `path`, `created_at`, `updated_at`, `last_tracked_at`

**Tracking filters**: `id`, `task_id`, `predecessor_id`, `started_at`, `ended_at`, `deleted`, `created_at`

**Task fields in tracking filters** (prefix with `t.`): `t.description`, `t.status`, `t.priority`, `t.deleted`, `t.parent_id`, etc.

### Query Options

Options are set via a top-level `options:` key:

```yaml
query:
  and:
    - [deleted, =, false]
    - [last_tracked_at, ge, 2 months ago]
options:
  include_ancestors: true
```

| Option              | Default | Description                                  |
| ------------------- | ------- | -------------------------------------------- |
| `include_ancestors` | `false` | Include all parent tasks of matching results |

This is useful for tree views: filter for recently-tracked tasks but still see the full tree structure with all parent nodes.

### Examples

```yaml
# All trackings from this month for a specific task description
name: This month meetings
query:
  and:
    - [deleted, =, false]
    - [started_at, '>=', april 1]
    - [t.description, has, meeting]

# High priority open tasks
name: Priority tasks
query:
  and:
    - [deleted, =, false]
    - [status, in, [todo, in_progress]]
    - [priority, '>=', 5]

# Trackings without an end time (still running)
query:
  [ended_at, is_null]

# Recently-tracked tasks with full tree context
name: Recent work
query:
  and:
    - [deleted, =, false]
    - [last_tracked_at, ge, 2 months ago]
options:
  include_ancestors: true

# All trackings in the Globex subtree
query:
  and:
    - [deleted, =, false]
    - [in_tree, Globex]
```

## Notes

Each task can have a Markdown notes file. Press `o` in the TUI to open notes in your configured editor.

Notes are stored at `~/.local/share/not_yet_done/notes/` in a directory tree that mirrors the task hierarchy:

```
notes/
  a1b2c3d4_project-alpha/
    e5f6a7b8_design-api.md
    e5f6a7b8_design-api/
      c9d0e1f2_schema-v2.md
```

- File names: `{id-prefix}_{slugified-description}.md`
- Empty notes (whitespace only) are automatically deleted on save
- Notes move with their task when reparented
- Notes are soft-deleted (renamed with `_deleted_at_` suffix) when a task is deleted

## Scripts

The `:script` fuzzy menu (also bound to `x` in the Trackings + Tasks
tabs and to per-view `type: script` actions in content tabs) lists,
runs, edits, creates and deletes user scripts for the current context.
One menu, three contexts:

| Trigger                                    | Context               | JSON argument                                                                                                             | Script directory                                         |
| ------------------------------------------ | --------------------- | ------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------- |
| Trackings tab `x` (or `:script`)           | Filtered tracking ids | `{"tracking_ids": [..], "filter_min_date": .., "filter_max_date": ..}`                                                    | `~/.local/share/not_yet_done/tracking/scripts/`          |
| Tasks tab `x` (or `:script`)               | Selected task         | `{"task": {"id": "..", "description": "..", "parent_id": ".."\|null, "ancestors": [{"id":"..", "description":".."}, …]}}` | `~/.local/share/not_yet_done/scripts/tasks/`             |
| Content view `type: script` (or `:script`) | Selected node         | `{"node": {"ref": "..", "id": "..", "node_type": "..", "tab": "..", "instance": "..", "fields": {..}}}`                   | `~/.local/share/not_yet_done/scripts/<tab>/<view-path>/` |

The `<view-path>` is the **pane's** view hierarchy — the root
`ViewDef.node_type`, followed by each drilled-into `ChildDef.node_type`
— with `:` and `/` replaced by `_`. It is **not** the type of the
currently selected item, so the menu stays stable as you cycle through
a pane that mixes node types (e.g. a Taiga `items` view that merges
issues, userstories, tasks, epics → scripts all live in
`scripts/taiga/taiga_item/`). The selected item's `node_type` is still
passed in the JSON payload (`node.node_type`).

After drilling from items into comments, scripts live in
`scripts/taiga/taiga_item/taiga_comment/`.

### Menu keys

| Key      | Default   | Action                                                                                 |
| -------- | --------- | -------------------------------------------------------------------------------------- |
| `enter`  | run       | Run the highlighted script. With a typed name that doesn't match: create a new script. |
| `ctrl+e` | edit      | Open the highlighted script in `$EDITOR`.                                              |
| `ctrl+d` | delete    | Delete the highlighted script file.                                                    |
| `esc`    | close     | Close the menu.                                                                        |
| `+name`  | force-new | Force "create new" even when `name` fuzzy-matches an existing script.                  |

Bare names (no extension) default to `.py`.

### Wiring up a per-view trigger

`type: script` is just another action in the view YAML; default key
is opt-in per view (so unrelated content tabs aren't shadowed). The
`:script` cmdline always works regardless.

```yaml
# ~/.config/not_yet_done/views/jira.yaml
views:
  - name: tickets
    node_type: "jira:issue"
    actions:
      - name: script
        key: x
        type: script
```

### Script Modes

Declare the mode in a comment within the first 10 lines:

```python
#!/usr/bin/env python3
# mode: background
```

| Mode                   | Description                                                                      |
| ---------------------- | -------------------------------------------------------------------------------- |
| `background`           | Silent execution; stderr shown as notification (default)                         |
| `capture`              | Output captured and shown in editor                                              |
| `interactive`          | TUI yields terminal to script                                                    |
| `interactive+capture`  | Interactive + output shown in editor                                             |
| `commands`             | Background-style; script writes `{"commands": [...]}` JSON to `$NYD_OUTPUT_FILE` |
| `interactive+commands` | Interactive variant of `commands`                                                |

#### Commands mode — letting a script drive the TUI

In `commands` / `interactive+commands` mode the TUI exposes a path via
the `NYD_OUTPUT_FILE` environment variable. After the script exits,
the TUI parses that file as JSON of the form:

```json
{
  "commands": ["jump Tasks:tree", "focus-task /work/clients/acme"]
}
```

Each entry is fed to the same dispatcher as the `:` cmdline, so any
in-process command works (e.g. `jump`, `focus-task`, `tag`,
`cut-node` / `paste-node`, `dismiss-notifications`, …). Entries may
have an optional leading `:` — both forms are accepted.

The schema is intentionally open: unknown top-level keys are
tolerated for forward-compatibility, so future versions can add
metadata (e.g. `version`, `requires`) without breaking existing
scripts. Only `commands` is currently consumed.

Example — Taiga item that jumps to the matching local ticket task,
creating the task on the fly if it doesn't exist yet:

```python
#!/usr/bin/env python3
# mode: commands
import json, os, re, subprocess, sys

CLI = "not-yet-done-cli"

node = json.load(open(sys.argv[1]))["node"]
ref = node["fields"]["ref"]                 # e.g. "acme#42"
project, number = ref.split("#", 1)
subject = node["fields"].get("subject", "")
desc = f"#{number} - {subject}".rstrip(" -")

# `\b42\b` keeps 42 from also matching 420/421 — last segment
# opts into regex via the `re:` prefix.
ticket = f"/work/clients/{project}/tickets/re:\\b{re.escape(number)}\\b"
parent = f"/work/clients/{project}/tickets"

def show(p):
    return subprocess.run([CLI, "task", "show", "--path", p, "-i"],
                          capture_output=True, text=True)

if show(ticket).returncode != 0:
    p = show(parent)
    if p.returncode != 0:
        sys.exit(f"Parent path not found: {parent}\n{p.stderr}")
    parent_id = json.loads(p.stdout)["id"]
    subprocess.check_call([CLI, "task", "add", desc, "--parent", parent_id])

with open(os.environ["NYD_OUTPUT_FILE"], "w") as f:
    json.dump({"commands": [
        "jump Tasks:tree",
        "reload-tasks",
        f"focus-task -i {ticket}",
    ]}, f)
```

The `reload-tasks` step is what makes the auto-create round-trip
visible to the subsequent `focus-task` — without it the TUI still
holds the pre-add snapshot of `task_rows` and the new leaf is
invisible to the walker.

The mirror-image flow — selected local task → matching Taiga item —
uses `:focus-node`. The script lives under
`<data_dir>/not_yet_done/scripts/tasks/` (the flat Tasks-tab script
directory shared by list + tree) and emits one command that switches
to the Taiga tab and parks the cursor on the row whose `ref` column
matches:

```python
#!/usr/bin/env python3
# mode: commands
import json, os, re, sys
task = json.load(open(sys.argv[1]))["task"]
# Convention: task description starts with "#<num> - <subject>";
# the ancestor directly above the "tickets" folder is the Taiga
# project slug.
m = re.match(r"#(\d+)\b", task["description"])
ancestors = task["ancestors"]
i = next(
    j for j, a in enumerate(ancestors)
    if a["description"].lower() == "tickets" and j > 0
)
slug = ancestors[i - 1]["description"]
ref = f"{slug}#{m.group(1)}"
with open(os.environ["NYD_OUTPUT_FILE"], "w") as f:
    json.dump({"commands": [f"focus-node -i Taiga:items /ref|{ref}"]}, f)
```

`ancestors` walks root → parent (the task itself is not included), so
the index that holds the project slug is one less than the index of
the `tickets` folder. `-i` is mandatory here: task folder names in the
tree are typically capitalised (`Acme`) while Taiga's `ref` column
is always lower-case (`acme#43`), so without case-folding the
cross-system jump silently misses.

### Input / Output

Scripts receive two arguments:

1. **JSON file** — path to a temp file containing the context-specific JSON described above
2. **Output file** — write to this path to signal completion and optionally provide output

In `commands` mode the output file path is additionally exposed as
`$NYD_OUTPUT_FILE` for convenience (scripts that don't need the
positional output-file arg can ignore it).

### Interactive Scripts

For scripts that need a terminal (e.g. opening in a split), configure `interactive_command`:

```yaml
script:
  interactive_command: "kitty @ launch --location=vsplit sh -c '{script} {json_file} {output_file}'"
```

Placeholders: `{script}` (path to the script file), `{json_file}` (the
context JSON written to a temp file), `{output_file}` (marker file the
TUI watches for completion).

### Create-new Template Resolution

When the user types a new name and hits Enter, the scaffold inserted
into the new script is resolved in this order (first hit wins):

1. **Per-view** — `script_template:` on the active `views[]` entry in
   `~/.config/not_yet_done/views/*.yaml` (content tabs only).
2. **Trackings** — `tracking.script_template:` in `tui.yaml`
   (Trackings tab only).
3. **Global fallback** — `script.template:` in `tui.yaml`. Always
   present; ships with a generic `{"node": {...}}` scaffold.

Layers 1 + 2 are optional. The Trackings layer exists because the JSON
shape there (`{tracking_ids, filter_min_date, filter_max_date}`)
differs from the generic node shape — without a tracking-specific
default, new scripts would scaffold against the wrong schema. Per-view
overrides are useful when a content tab's nodes have rich, named
fields that a tailored starter can reference directly (e.g. Taiga
items' `ref`/`assignee`/`status`).

```yaml
# tui.yaml
tracking:
  script_template: |
    #!/usr/bin/env python3
    # mode: background
    import json, sys
    with open(sys.argv[1]) as f:
        data = json.load(f)
    print(f"Got {len(data['tracking_ids'])} tracking(s)")

script:
  template: |
    #!/usr/bin/env python3
    # mode: background
    import json, sys
    with open(sys.argv[1]) as f:
        node = json.load(f)["node"]
    print(node["ref"])
```

```yaml
# ~/.config/not_yet_done/views/taiga.yaml
views:
  - name: items
    node_type: "taiga:item"
    script_template: |
      #!/usr/bin/env python3
      # mode: background
      import json, sys
      with open(sys.argv[1]) as f:
          node = json.load(f)["node"]
      ref = node["fields"].get("ref", "?")
      print(f"Taiga item #{ref}")
```

## View Retries

Adapter-backed views in `~/.config/not_yet_done/views/*.yaml` can opt
in to automatic retries on transient load failures. Set `retries: N`
on a view to allow `1 + N` total attempts per `list()` call (root,
drill-down, and tree expansion under that view):

```yaml
views:
  - name: databases
    node_type: "postgres:database"
    retries: 2 # 1 initial attempt + 2 retries = 3 attempts max
    actions:
      - name: refresh
        key: r
        type: reload
```

Default is `retries: 0` (legacy behaviour: error sticks immediately).

While a retry is in flight, the auth-status banner shows
`Retrying (n/total): <last error>` so you can see how many attempts
remain. When combined with an adapter-level timeout (e.g. the Postgres
adapter's `query_timeout_secs`) the banner overlays the countdown:
`Retrying (2/3) — list databases (3s/7s): <last error>`.

**Trade-off**: each retry attempt pays the adapter's own timeout
budget. A Postgres view with `query_timeout_secs: 7` and `retries: 2`
can hang up to 21s on a fully broken backend before the error becomes
sticky. Pick `retries` to match how transient the failures you
actually see are.

Adapters that talk HTTP carry their own per-request timeout in the same
spirit. The **Taiga** adapter takes `request_timeout_secs` (default 20)
in its adapter config: it caps every API call — including the metadata
fetch that the edit editor blocks on — so a dead connection surfaces an
error instead of freezing the UI, and the adapter reconnects + retries
once on a transport failure before giving up. A separate
`connect_timeout_secs` caps just the connection handshake (default
`min(request_timeout_secs, 10)`); raise it on a high-latency link where
the derived 10s cap would abort a healthy-but-slow connect. See
`docs/examples/views/taiga-adapter.yaml`.

## Manual Connect

By default an adapter-backed tab spawns its initial `list()` call as
soon as the TUI starts (and again on first activation of a still-
unloaded subtab). For adapters with expensive or unreliable
connections — Postgres-over-SSH-tunnel via Bastion, slow VPN-gated
APIs — this is a problem: the connection attempt blocks for the full
timeout × retry budget when the prerequisite (VPN, tunnel) isn't up,
and the user just sees `Fetch failed: timed out` after a long wait.

Set `manual_connect: true` on the adapter section to suppress all
auto-loads for that tab:

```yaml
tab:
  name: Postgres
  order: 5

adapter:
  type: postgres
  config: postgres-adapter.yaml
  manual_connect: true # don't load until I press the reload key

views:
  - name: databases
    node_type: "postgres:database"
    actions:
      - name: refresh
        key: r
        type: reload
```

While unloaded the view's banner reads
`Auto-connect disabled — press \`r\` to connect`(it names the first`type: reload`action of the active subtab). After the user presses`r` the adapter connects and behaves normally; switching back to an
already-loaded subtab shows the cached data without re-fetching.

If you set `manual_connect: true` but the active view has no
`type: reload` action configured, the banner degrades to
`Auto-connect disabled — no \`reload\` action configured for this view`
so the misconfiguration is visible at a glance.

## Confluence Adapter

Read/write adapter for Atlassian Confluence Server / Data-Center
(tested against Confluence 9.2.19; Atlassian Cloud is **not** supported
— its REST surface differs enough that it needs a separate adapter).

### Setup

Two YAML files in `~/.config/not_yet_done/views/`:

- **`confluence-adapter.yaml`** — credentials, cache, TLS. See
  [`docs/examples/views/confluence-adapter.yaml`](docs/examples/views/confluence-adapter.yaml).
- **`confluence.yaml`** — tab/sub-tab layout, columns, actions. See
  [`docs/examples/views/confluence.yaml`](docs/examples/views/confluence.yaml).

Auth uses the same Crowd-SSO cookie pattern as the Jira adapter: a
user-supplied script writes
`JSESSIONID=...; crowd.token_key=...; atlassian.xsrf.token=...` to
stdout. The path lives in `auth.bindings[].provider.script`.

### Sub-tabs

| Sub-tab    | Default key | Listing                                                                         |
| ---------- | ----------- | ------------------------------------------------------------------------------- |
| **spaces** | (default)   | All spaces; drill into a row to inline-expand its top-level pages               |
| **search** | (cycle)     | CQL-driven results; `q` opens the saved-queries menu (same shape as Jira/Taiga) |

Each page row recursively exposes three child branches: nested
**pages**, **attachments**, and **comments** — the same recursive
ChildDef the spaces sub-tab uses, so any depth of the page tree
behaves identically.

### Filtering spaces (`space_keys`)

By default the spaces sub-tab lists every space the user can read.
On large Crowd-SSO instances that easily reaches three digits — both
the initial fetch and the tree-mode listing become unwieldy. Add a
whitelist in `confluence-adapter.yaml`:

```yaml
space_keys:
  - DOCS
  - PROD
  - SUPPORT
```

The adapter passes the keys to the server via repeated `spaceKey=`
query params and then reorders the response to match the YAML order
(alphabetical-API order is suppressed). Keys that don't resolve
(typos, lost-access, deleted spaces) are silently skipped, so a
single bad entry never brick the whole sub-tab.

Omit the field to keep the historic "list everything" behaviour.

### Page actions

| Key       | Action            | Notes                                                                                                                              |
| --------- | ----------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `p`       | preview           | Toggles the body.storage preview pane. First toggle lazy-hydrates `GET /content/{id}?expand=body.storage,...`.                     |
| `e`       | edit              | Opens the page's `body.storage` in `$EDITOR` (pretty-printed via `xmllint --format`). Save writes `PUT /content/{id}` `version+1`. |
| `a`       | create-child page | Opens a small `title:` + empty `<p></p>` buffer. Save POSTs as a child of the current page.                                        |
| `c`       | add comment       | Opens an empty XHTML buffer; save POSTs `type=comment, container={page_id}`.                                                       |
| `Shift+A` | upload attachment | Opens the FilePicker (multi-select); each chosen file is POSTed to `/rest/api/content/{id}/child/attachment` (one POST each).      |
| `y`       | clone             | Opens an editor pre-filled with the source page's title + " (Clone)" + body. Save POSTs a new page under the same parent.          |
| `Shift+D` | delete (Trash)    | Confirm popup, then `DELETE /rest/api/content/{id}`. The page survives in Confluence's Trash and can be restored from the web UI.  |
| `o`       | open in browser   | Opens the page in `$BROWSER` via the `webui` URL.                                                                                  |

On a space row, `a` creates a top-level page in that space (same
editor buffer as `a` on a page row).

### Edit conflicts — 3-way merge on `409`

If someone else edits the page upstream while you're typing, the
`PUT` returns `409`. The adapter re-fetches and runs a
[`diffy`](https://crates.io/crates/diffy)-based 3-way merge between
the version you opened, your edits, and the upstream version.

- **Disjoint changes** auto-merge silently — the new version goes
  through, the editor closes with a `Merged on top of v{n}` banner.
- **Overlapping changes** come back into the editor with
  `<<<<<<< ours` / `=======` / `>>>>>>> theirs` markers and a
  `Merge conflict — resolve and save again` banner; resolve the
  markers manually and save.

Comments don't auto-merge (they're small enough that manual re-edit
is cheaper); a `409` on a comment edit reopens the buffer with an
error banner and you re-type.

### Comment + attachment actions

| Where       | Key       | Action                                                                                                             |
| ----------- | --------- | ------------------------------------------------------------------------------------------------------------------ |
| Comment row | `p`       | Toggle body preview (body XHTML rides in on the list response — no extra round-trip).                              |
| Comment row | `e`       | Edit body via `PUT /content/{comment_id}`. Same Reopen-with-banner on `409`/parse errors as pages, no 3-way merge. |
| Comment row | `Shift+D` | Delete via `DELETE /content/{comment_id}` (generic `ConfirmDeleteContentNode` confirm popup).                      |
| Attachment  | `d`       | Download to a temp dir (cached by attachment-id) and spawn `xdg-open` detached.                                    |

### Tree-aware search (`/` on the spaces sub-tab)

The spaces sub-tab's `/` action is a **server-side** content search
(CQL via `/rest/api/content/search`) that returns hits across all
pages and expands the lazy tree to the first match. n/N cycles
through the remaining hits in tree-render order (configured-space
order → ancestor DFS → page title) — the cache survives manual
expand/collapse, so you can poke around a sub-tree between presses.

When `space_keys:` is set, the search is automatically scoped to
those spaces (`space in (...)` is injected into the CQL).

Notifications:

- `Tree find "q": 3 hits — n/N to navigate` on result land.
- Per-press status-bar hint: `n/N  Tree find "q": 2/3[, truncated]`.
- `Tree find "q": no matches` if the server returned nothing.
- `Tree find "q": loading…` while the request is in flight.

Cache invalidation: Esc on the input, `r` (reload), or opening
the input again with a fresh query all drop the cached hits and
return n/N to the local `/`-search dispatch.

Local-filter-while-typing (`f`, `fuzzy_filter`) still works
side-by-side — it filters the spaces already on screen by key/name.

### CQL saved queries

CQL bodies live as one-string-per-file under
`<XDG_DATA_HOME>/not_yet_done/confluence/<instance_id>/queries/`. The
search sub-tab's `q` menu lists every saved file; pressing Enter
applies it, `Ctrl+f` binds a chord shortcut (persisted in the shared
`query_shortcut` table).

`:query new <name>`, `:query edit <name>`, `:query delete <name>`
manage entries the same way they do for Jira/Taiga. See
[`docs/examples/views/saved/confluence/recent-pages.yaml`](docs/examples/views/saved/confluence/recent-pages.yaml)
for a seed example.

### Caveats

- **Cross-space clone** is not surfaced at adapter level — the
  TUI doesn't have a Space-Picker for adapter actions. Clone lands
  in the source space; move the resulting page via the Confluence
  UI if you need it elsewhere.
- **Permanent purge** (`DELETE ?status=trashed`) is implemented in
  the client but intentionally not surfaced as a TUI shortcut.
  Restore or purge from the Confluence web Trash.

## Stoat Adapter (Chat)

Adapter for **Stoat**, a chat platform (fork of Revolt). Unlike the
issue/wiki/DB adapters it is a **streaming** adapter: the server list
and live updates arrive push-only over a WebSocket. A single background
gateway task owns the socket (`Authenticate → Ready → events`, heartbeat,
reconnect) and keeps an in-memory tree of servers/channels/users; chat
state is deliberately **not** cached to disk (only the session token and
view sort state are persisted).

> **Status: Phase 4 (read + live + write + live structure).** Opening the tab logs in,
> discovers the WebSocket URL, and connects the gateway — the status
> banner walks `Connecting → Ready` (or `NeedsCreds → … → Ready`). The
> tree fills from the `Ready` snapshot **automatically** (no manual
> reload): **servers → channels → messages**, with a Markdown preview
> (`p`) of the selected message. Channel structure is read live from
> gateway state; message bodies are pulled over REST on demand (latest
> ~50 per channel — older-message backfill is still to come). **Live
> updates:** while you have a channel open, new/edited/deleted messages
> and reactions refresh it on arrival; a reconnect resyncs every view.
> **Write:** in a channel's message list, `a` composes a new message
> (empty `$EDITOR`, Markdown), `e` edits the selected message, `d`
> deletes it, and `+` reacts via a small emoji picker. Editing or
> deleting another user's message is rejected by the server with a clean
> error. **Live structure:** a channel created/renamed/deleted, and any
> category change (add/remove/rename/reassign/reorder), update the tree
> without a reload — only joining or leaving a whole server still needs a
> reconnect.

### Setup

Two YAML files in `~/.config/not_yet_done/views/`:

- **`stoat-adapter.yaml`** — base URL + credentials. See
  [`docs/examples/views/stoat-adapter.yaml`](docs/examples/views/stoat-adapter.yaml).
- **`stoat.yaml`** — tab/sub-tab layout. See
  [`docs/examples/views/stoat.yaml`](docs/examples/views/stoat.yaml).

You configure only the **base domain** (`url:`); the API path (`/api`)
and the WebSocket URL are self-discovered via `GET /api/`. Auth uses the
`password-login` mechanism. Because that mechanism fixes the binding
field names to `username` + `password`, and Stoat logs in by email, the
**`username` field carries your login email address**. Only the returned
session token is persisted — never the password. Multi-factor auth is
not supported yet (a login that returns an MFA ticket fails with a clear
message).

## Waybar Integration

The Waybar CFFI module shows the currently active tracking in your status bar.

### Setup

Add to your Waybar config:

```json
"modules-right": ["cffi/nyd", ...],
"cffi/nyd": {
    "module_path": "~/.config/waybar/cffi/libnyd_waybar.so",
    "icon": "⏱",
    "max_chars": 20,
    "interval_ms": 5000
}
```

| Option        | Default | Description                       |
| ------------- | ------- | --------------------------------- |
| `icon`        | `⏱`     | Icon before task name             |
| `max_chars`   | `20`    | Max description length before `…` |
| `interval_ms` | `5000`  | Update interval in ms             |

### Styling

CSS widget name: `#nyd-tracking`. Class `active` is added when tracking is running.

```css
#nyd-tracking {
  color: #161320;
  background: #f5a97f;
}
```

Duration is displayed as: `30s`, `22min`, `1.5h`, `10h`.

<!-- screenshot: waybar with nyd-tracking pill showing "⏱ Build API endpoi… 1.5h" -->

![Waybar Module](docs/screenshots/waybar.png)

## Configuration

All TUI configuration lives in `~/.config/not_yet_done/tui.yaml`.

### File Locations

| Purpose    | Path                                            |
| ---------- | ----------------------------------------------- |
| TUI config | `~/.config/not_yet_done/tui.yaml`               |
| Database   | `~/.local/share/not_yet_done/nyd.db`            |
| Backups    | `~/.local/share/not_yet_done/backups/`          |
| Notes      | `~/.local/share/not_yet_done/notes/`            |
| Scripts    | `~/.local/share/not_yet_done/tracking/scripts/` |

The `DATABASE_URL` environment variable overrides the configured database path.

### Editor

Editors are configured as **named profiles** under `editors:`. The mandatory
`default` profile is used everywhere unless a view action selects another by
name (see `editor:` on actions in the [generic view spec](docs/generic-view-spec.md)).

```yaml
editors:
  default: # used everywhere unless an action overrides it
    command: "" # default: $EDITOR or vi
    inline: true # true = in-terminal, false = detached window
    pause_tui: false # pause TUI when launching detached editor
    indent: 4 # indentation for the tree editor
  compose-below: # an example second profile, selected via `editor: compose-below`
    # `--bias 20` → the new editor window takes 20% of the height (80:20
    # split-down); `hsplit` puts it below the TUI.
    command: "kitty @ launch --location=hsplit --bias 20 sh -c '{env}nvim {file}; mv {file} {file}.done'"
    inline: false
    pause_tui: true
```

Why named profiles: different actions want different editor geometries — a
short chat compose fits a slim split below the terminal, a long ticket edit a
full vsplit. The editor is always a separate process (your `$EDITOR`, e.g. via
Kitty); a TUI pane cannot host it (no PTY embedding), so the split is realised
by the terminal through the profile's `command`. An action references a profile
with its `editor:` field; an unknown name is a hard config-load error.

### Tracking

```yaml
tracking:
  allow_parallel: false # allow multiple simultaneous trackings
```

### Theme

All colors are configurable as hex values under `theme:`. See the [full color reference](#theme-colors) or use one of the defaults (Catppuccin Mocha, Gruvbox Dark).

### Navigation

```yaml
navigation:
  jump_chars: "abcdefghijklmnopqrstuvwxyz" # characters for jump labels
```

### Theme Colors

<details>
<summary>Full color table (click to expand)</summary>

| Field               | Description                      |
| ------------------- | -------------------------------- |
| `bg`                | Main background                  |
| `surface`           | Panel/bar background             |
| `surface_2`         | Secondary surface (selected row) |
| `primary`           | Primary accent                   |
| `primary_dim`       | Dimmed primary                   |
| `on_primary`        | Text on primary backgrounds      |
| `accent`            | Main accent color                |
| `accent_dim`        | Dimmed accent                    |
| `text_high`         | High-contrast text               |
| `text_med`          | Medium-contrast text             |
| `text_dim`          | Subdued text                     |
| `success`           | Success indicators               |
| `error`             | Error indicators                 |
| `warning`           | Warning indicators               |
| `secondary`         | Secondary accent                 |
| `tertiary`          | Tertiary accent                  |
| `tree_connector`    | Tree branch connector lines      |
| `tab_active`        | Active main tab FG               |
| `tab_active_bg`     | Active main tab BG               |
| `sub_tab_active`    | Active sub-tab FG                |
| `sub_tab_active_bg` | Active sub-tab BG                |
| `toolbar_bg`        | Action/status bar background     |
| `focused_bg`        | Focused element background       |
| `form_bg`           | Form panel background            |

</details>

## Debugging

The TUI honours two opt-in environment variables; both are no-ops unless
set, so leaving them off costs nothing.

### `NYD_DEBUG=1` — HTTP request and error log

When set, every outbound HTTP request made by a content adapter (Taiga,
Jira, …) is appended to a debug log along with its response status, and
every error surfaced to the user (`set_query_error`, `notify_error`,
content load failures) is mirrored into the same file. Each entry is
prefixed with a local timestamp so the file is greppable across runs.

- Default path: `/tmp/nyd-debug.log`
- Override path: `NYD_DEBUG_LOG=/path/to/file`
- Response bodies are only written for non-2xx responses (truncated at
  ~2 KB to keep the log readable).

Run the TUI with `NYD_DEBUG=1 not-yet-done-tui` and `tail -f
/tmp/nyd-debug.log` in a second terminal to watch the request stream
live. Press **F12** in the TUI at any time to open the most recent
error in `$EDITOR`.

### `NYD_KEY_DEBUG=1` — terminal key event log

When set, every key press the TUI receives is appended to
`/tmp/nyd-keys.log` as `<modifiers> <KeyCode> -> <emitted-string>`. Use
this when a keybinding doesn't fire to confirm whether the terminal is
delivering the expected event (kitty's keyboard-protocol vs. plain
xterm encodings, terminal-level shortcut conflicts, etc.).

## Architecture

For the full picture — crate dependency graph, the `ContentAdapter`
abstraction, the dirty-gated render loop, the message/request enums, auth
orchestration and the views layer — see
[`docs/architecture.md`](docs/architecture.md). Architecture decision
records live under [`docs/decisions/`](docs/decisions/).

```
not-yet-done-core       # Domain logic, entities, repositories, services (SeaORM + SQLite)
not-yet-done-cli        # CLI binary (nyd) using tusks for argument parsing
not-yet-done-tui        # TUI binary using ratatui + tuirealm
not-yet-done-ratatui    # Custom ratatui widgets (Table, TextInput, SelectList, Grid)
not-yet-done-waybar     # Waybar CFFI module (cdylib)
not-yet-done-forest     # Tree rendering with post-order fold
not-yet-done-table      # Column layout computation
not-yet-done-macros     # Derive macros (ColumnRegistry)
```

## License

MIT
