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
- **Scripts** — run user scripts on the focused node or a view's filtered set via the `:script` fuzzy menu, with background, capture, and interactive modes
- **Filter DSL** — YAML-based query language with natural-language date expressions
- **Anonymization** — `NYD_ANON=1` masks real customer/ticket/person names with deterministic, format-preserving fakes across every adapter, for safe screenshots and screencasts of a production instance
- **Daily backups** — the core DB (`nyd.db`) is backed up once a day on startup; the split-out `tasks.db` is backed up through a configurable [lifecycle hook](#lifecycle-hooks) (`backup` bound to the adapter's `connected` event with a 24h throttle), so it happens however you launch — TUI or any `nyd tasks …` command. The Tasks/Trackings tabs also expose a manual `backup` action (`B`), and `nyd-t backup` manages them from the CLI

## Installation

```bash
# Build and install all binaries
cargo install --path not-yet-done-cli
cargo install --path not-yet-done-tui

# Build the Waybar module
cargo build --release -p not-yet-done-waybar
cp target/release/libnyd_waybar.so ~/.config/waybar/cffi/
```

No database setup step is needed: each adapter is self-contained and creates
(and schema-syncs) its backing SQLite file on first open — start the TUI or run
any `nyd <instance>` command and the store is initialized automatically.

## TUI

Start the TUI with:

```bash
not-yet-done-tui
```

### Tabs and Views

Every tab is an adapter-backed _content tab_, configured under
`~/.config/not_yet_done/views/*.yaml` and shown according to the active
[tab constellation](#tab-constellations). Task and time tracking are no
exception — they are adapter-backed views like everything else.

**Tasks** — manage your task tree. Task management lives in the
adapter-backed **`Tasks`** content tab (`views/tasks.yaml`); see
[`docs/examples/views/tasks.yaml`](docs/examples/views/tasks.yaml) for a
fully-commented reference. It has two sub-views:

| Sub-view | Key | Description                                            |
| -------- | --- | ------------------------------------------------------ |
| Tree     | `t` | Hierarchical tree view with indentation and connectors |
| List     | `v` | Flat list of all tasks matching the active filter      |

<!-- screenshot: tasks tab in tree view showing nested tasks with priority, status, notes indicator -->

![Tasks Tree View](docs/screenshots/tasks-tree.png)

#### Tasks tree expand / collapse

In tree view, branches can be folded individually:

| Key     | Action                                           |
| ------- | ------------------------------------------------ |
| `enter` | Toggle expand/collapse on cursor                 |
| `zr`    | Expand all branches                              |
| `zm`    | Collapse to the view's configured `expand_depth` |

Collapsed parents render with a `▶` glyph and a trailing `(N)` count
showing how many direct children are hidden. Expanded parents render
with `▼`. The default number of visible levels is the `expand_depth` of
the tree view in `views/tasks.yaml`:

```yaml
# views/tasks.yaml — on the tree view
expand_depth: 2 # 0 = only roots, 1 = roots + their children, ...
```

The expand state is per-session — it is not persisted across
restarts. When a fuzzy filter is active, expand state is ignored
and every matching node (plus its ancestors) is shown.

**Trackings** — view and analyze time entries. Like Tasks, this is an
adapter-backed content tab (`views/trackings.yaml`); see
[`docs/examples/views/trackings.yaml`](docs/examples/views/trackings.yaml)
for a fully-commented reference.

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

Tabs are referenced by display name — each view's `tab.name` (e.g. the
adapter-backed `Tasks` and `Trackings`). The list order
assigns the **autonumber**
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
- `:jump <Tab>` — programmatic tab switch. Matches any content tab by
  its configured `tab.name` (case-insensitive; e.g. `Tasks`,
  `Trackings`). Content tabs don't take a sub-view, so a trailing
  `:<sub>` is reported as a modal error. Used by user scripts to drive
  the TUI from outside; also typeable directly. Modal error on unknown
  tab.
- `:focus-node [-i] <Tab>[:<view>] /<col>|<pattern>` — parks the
  cursor on a matching row of a content tab. Switches to the named
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
- `:tree-find <Tab>[:<view>] <query>` — the **tree-mode** sibling of
  `:focus-node`. Switches to the named content tab/sub-view, forces a
  fresh reload (so out-of-process CLI mutations are in the adapter
  snapshot before the search runs), then runs a server-side tree
  search and **lazily expands to the first hit**, parking the cursor
  on it. Use this — not `:focus-node` — to jump into a tree whose
  target sits several levels deep (e.g. the adapterized Tasks tab,
  where ticket nodes live under `work → client → tickets`). The tab
  name may be double-quoted to allow spaces:
  `:tree-find "Tasks" <query>`. The `<query>` is adapter-defined;
  the local task adapter additionally accepts an exact-id escape
  `id:<uuid>` (used by scripted jumps that already resolved the node
  id via the CLI). Modal errors on unknown tab/view or when the
  active view isn't a tree.
  Example: `:tree-find "Tasks" id:550e8400-…`.
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

There are **two** command-line binaries, by design:

| Binary  | Crate                   | Role                                                                                                                                                                                                                                                               |
| ------- | ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `nyd`   | `not-yet-done-cli`      | Generic front-end over the `ContentAdapter` protocol — drives **foreign** systems (Jira, Confluence, Postgres, Taiga, Stoat) and our own tasks/trackings/projects _as adapters_, all through one uniform interface.                                                |
| `nyd-t` | `not-yet-done-task-cli` | Dedicated **Tasks & Time-Tracking** CLI on the native domain core (`not-yet-done-task-core`). Produces typed, domain-shaped output (e.g. `track export`'s joined tracking+task JSON, `task tree`'s nested hierarchy) and graded exit codes that scripts depend on. |

Both build on the **same core**: the in-process TUI adapters and `nyd-t` each
talk to `not-yet-done-task-core` in their own idiom. Adapters are _interop
boundaries_ (a uniform protocol for many systems); `nyd-t` is the domain's own
front-end. See [decision 0004](docs/decisions/0004-zwei-cli-binaries-adapter-vs-domain.md)
for the why.

### `nyd` — generic adapter front-end

`nyd` is a **thin generic front-end over the `ContentAdapter` protocol**. There
are no task-, tracking- or project-specific subcommands: tasks, trackings and
projects are reached as adapter instances (`nyd tasks …`, `nyd trackings …`,
`nyd projects …`), and the terse everyday forms (`nyd add`, `nyd track <id>`,
`nyd summary`, …) are **aliases** over those generic verbs. See
[Generic adapter commands](#generic-adapter-commands) and
[Aliases & `config edit`](#aliases--config-edit) below.

> The former hard-coded `task` / `project` / `track` / `query` / `db sync`
> commands were removed from `nyd` once their adapter replacements landed —
> `track split`/`move` are now adapter actions (`nyd trackings do split …`),
> projects are an adapter (`nyd projects do create …`), and ad-hoc filters run
> through any adapter's `--query`. The native, domain-shaped equivalents live
> in `nyd-t` (below). The only built-in commands remaining in `nyd` are `tag`
> and `backup` (which still operate on the legacy core DB).

### `nyd-t` — Tasks & Time-Tracking CLI

`nyd-t` (installed from `not-yet-done-task-cli`) is the native domain CLI. It
operates on the split-out **task database** — `NYD_TASKS_DB` when set,
otherwise the per-host default `<data-local>/not_yet_done/tasks.db` (the same
file the Tasks/Trackings adapters open when their config carries no explicit
`database:` DSN). It does **not** read the core config's `database.url` (that
points at the legacy `nyd.db`).

Run `nyd-t <group> --help` for the full, self-documenting reference; the groups
are:

```bash
# Tasks — CRUD, path resolution, subtree export
nyd-t task add "Write report" --parent <id> --project Work --tag urgent
nyd-t task list --project Work
nyd-t task tree <id|description-prefix> [--last-tracked-since 2026-04-01] [--pretty]
nyd-t task show --path /Work/Clients/Acme/Tickets [-i]   # graded exit codes: 4=not found, 5=ambiguous
nyd-t task edit <id> --description … --add-tag … --remove-project …
nyd-t task delete <id>

# Time tracking — start/stop, summary, export, reschedule
nyd-t track start <task-id> [--parallel]
nyd-t track stop [--task-id <id>]                        # omit to stop all
nyd-t track summary [--from 2026-03-01] [--to today] [--task-id <id>]
nyd-t track export [<ids>…] [--task-id <id>] [--from …] [--to …] \
                   [--active-only] [--sort-by-started-at asc|desc] [--pretty]
nyd-t track move  <id> "yesterday 9am" [--gravity start|end] [--offset +1h] [--json]
nyd-t track split <id> "10:30" [--task <other-task-id>]
nyd-t track restore <id>

# Projects, tags, schema, backups
nyd-t project add "Work" --description …       # list / edit / delete too
nyd-t tag add "urgent" --fg "#FFFFFF" --bg "#FF5733" --symbol ""   # list/edit/new/delete
nyd-t db sync                                  # create/upgrade the task DB schema
nyd-t backup create | list | restore <file>   # backs up the task DB (tasks.db)

# Ad-hoc filter queries (debug/inspect a FilterExpr before saving it)
echo 'query: [deleted, =, false]' | nyd-t query run --entity task     # JSON to stdout
nyd-t query run --entity tracking --file filter.yaml [--debug]        # --debug dumps the resolved FilterExpr
```

The `track export` / `task tree` JSON shapes and `task show` exit codes are a
**stable contract** the user's TUI batch scripts rely on (daily reports,
hour-totals, “goto task” from Jira/Taiga).

### Tags & Backup

Tag styling (fg/bg/symbol) has no generic adapter path yet, so tag management
stays a built-in command; `backup` keeps the legacy core-DB backups until it
becomes an adapter action.

```bash
# Tags (global or project-specific)
nyd tag list
nyd tag add "urgent" --fg "#FFFFFF" --bg "#FF5733" --symbol ""
nyd tag new                                   # create interactively via $EDITOR
nyd tag edit global-tag:<uuid> --name "blocked"
nyd tag delete global-tag:<uuid>

# Backups
nyd backup create
nyd backup list
nyd backup restore 20260323-185627-nyd.db
```

### Generic adapter commands

Besides the task-specific commands above, every configured adapter instance
(each `views/*.yaml`) is addressable generically as `nyd <instance> <verb>`.
The verbs drive the same `ContentAdapter` protocol the TUI uses, so they work
for _every_ adapter — tasks, trackings, Jira, Taiga, Postgres, Confluence,
Stoat — without the CLI hard-coding anything about them:

```bash
# Read verbs
nyd tasks ls                       # list children of the root
nyd tasks ls <id> --tree --depth 2 # a subtree, 2 levels deep
nyd tasks ls --query 'status=open' # filtered (adapter's query language)
nyd tasks show <id>                # one node's fields
nyd tasks actions <id>             # actions available on a node
nyd tasks actions --type task:item # …or on a node type
nyd tasks values tags              # enumerate a value source
nyd tasks ls -o json               # any read verb takes -o table|json

# Name a node by a label path instead of an opaque id (--path / -p): one
# segment per level, matched against child labels by substring (case-folded
# with -i) or by regex when prefixed `re:`.
nyd tasks show --path /Inbox/Groceries -i
nyd tasks ls   --path '/Inbox/re:^Week \d+$'

# Group a tree into date/value buckets (--group-by / -g), for adapters that
# support adapter-side grouping. Spec is `col[:bucket][:order]`, where bucket
# is day|week|month|year and order is asc|desc:
nyd trackings ls --type tracking:tree-group --group-by started_at:day:desc --tree

# Mutating verb: `do <action> [id]` runs a node action. The action's input
# shape (seen in `actions`) decides how input is sourced:
nyd tasks do add -m "$(cat task.md)"      # editor action, body inline (else $EDITOR)
nyd tasks do toggle-tracking <id>         # no-input action
nyd tasks do delete <id> --yes            # confirm-gated action needs --yes
nyd pg do run --field name=report --field db=live   # form action

# Trackings carry split/move as form actions (the generic replacement for the
# legacy `track split`/`track move`): pass the form fields with --field.
nyd trackings do split <id> --field at="2026-03-22 09:15"
nyd trackings do split <id> --field at="10:30" --field task=<other-task-id>
nyd trackings do move  <id> --field start="yesterday 9am"
nyd trackings do move  <id> --field start=2026-03-20 \
  --field gravity=start --field offset=+1h --field allow_future=true

# Projects are an adapter too: the generic verbs manage them with no
# project-specific command. create/edit/delete are all form actions.
nyd projects ls                                   # list projects
nyd projects do create --field name=Acme --field description=Widgets
nyd projects do edit <id> --field name="Acme Inc."   # omitted fields stay
nyd projects do delete <id> --field cascade=true     # also soft-delete tasks
```

Node ids are opaque and adapter-owned; for the local task/tracking forests you
can use a unique **id prefix** (like a git short hash). Where a verb takes a
node id (`ls`, `show`, `do`), `--path /A/B` is an alternative that walks down by
label — the CLI analogue of drilling in by name in the TUI. It uses only the
protocol's per-level listing, so it works for any adapter; each segment must
resolve to exactly one child (ambiguous or unmatched segments error and list
the candidates).

`--group-by` rides along in the list request; only adapters that advertise
adapter-side grouping act on it (others ignore it with a warning). Grouping is
tied to the adapter's bucket node type, so select it with `--type` — or hide
that behind an alias (see below). The bucket key is ISO-formatted, so the
group order is chronological.

### Aliases & `config edit`

The generic verbs are deliberately explicit. **Aliases** give a short name to a
fixed invocation, so everyday use stays terse without the CLI growing
adapter-specific commands. They live in `~/.config/not_yet_done/cli.yaml`:

```yaml
aliases:
  toggle: [tasks, do, toggle-tracking, "{0}"] # nyd toggle <id>
  find: [tasks, ls, --query, "{@}"] # nyd find status=open
  new: [tasks, do, add, --value, "{parent}"] # nyd new --parent <id>
```

Trailing args split into **positionals** (bare tokens) and **named** values
(`--key value`), substituted into the template: `{0}`/`{1}` pick a positional,
`{@}` splices all of them, `{name}` takes a named value. The first expanded
token must name an adapter instance, so an alias is just a shorthand for a
generic verb (it can't reach the built-in commands). A small set of defaults
ships compiled in; a `cli.yaml` entry of the same name overrides one:

| Alias     | Expands to                                                                      | Replaces           |
| --------- | ------------------------------------------------------------------------------- | ------------------ |
| `add`     | `tasks do add` (editor; `-m` for inline)                                        | `task add`         |
| `edit`    | `tasks do edit {0}`                                                             | `task edit`        |
| `rm`      | `tasks do delete {0}`                                                           | `task delete`      |
| `track`   | `tasks do toggle-tracking {0}`                                                  | `track start/stop` |
| `toggle`  | `tasks do toggle-tracking {0}` (synonym)                                        | —                  |
| `tree`    | `tasks ls --tree`                                                               | `task tree`        |
| `summary` | `trackings ls --tree --type tracking:tree-group --group-by started_at:day:desc` | `track summary`    |

The adapter exposes a single `toggle-tracking` action (start when stopped, stop
when running), so the old `track start` / `track stop` pair collapses into the
one `track` alias.

> **Why aliases instead of more built-in commands?** Block D makes the CLI a
> thin, fully generic front-end over the adapter protocol so it works for every
> adapter automatically. Per-adapter convenience verbs would re-introduce the
> coupling we removed. Aliases keep that convenience in _user_ config, where it
> costs the codebase nothing and the user can shape it per workflow.

Edit config files in `$EDITOR` (seeding `cli.yaml` with a documented template
on first use):

```bash
nyd config edit cli      # ~/.config/not_yet_done/cli.yaml (default target)
nyd config edit tui      # ~/.config/not_yet_done/tui.yaml
nyd config edit tasks    # ~/.config/not_yet_done/views/tasks.yaml
```

## Filter DSL

Filters are YAML documents with a `query:` key. Used in the TUI editor (`Q`), saved filters, favorites, and any adapter's `--query` on the CLI (`nyd <instance> ls --query …`).

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

The `:script` fuzzy menu (also reachable via per-view `type: script` and
`scope: filtered_set` actions in content tabs — including the
adapter-backed `Tasks` and `Trackings` tabs) lists, runs, edits, creates
and deletes user scripts for the current context. One menu, two contexts:

| Trigger                                    | Context           | JSON argument                                                                                           | Script directory                                         |
| ------------------------------------------ | ----------------- | ------------------------------------------------------------------------------------------------------- | -------------------------------------------------------- |
| Content view `scope: filtered_set` batch   | Filtered node ids | `{"tracking_ids": [..], "filter_min_date": .., "filter_max_date": ..}`                                  | `~/.local/share/not_yet_done/scripts/<tab>/<view-path>/` |
| Content view `type: script` (or `:script`) | Selected node     | `{"node": {"ref": "..", "id": "..", "node_type": "..", "tab": "..", "instance": "..", "fields": {..}}}` | `~/.local/share/not_yet_done/scripts/<tab>/<view-path>/` |

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
  "commands": ["tree-find \"Tasks\" id:550e8400-e29b-41d4-a716-446655440000"]
}
```

Each entry is fed to the same dispatcher as the `:` cmdline, so any
in-process command works (e.g. `jump`, `tree-find`, `focus-node`,
`tag`, `cut-node` / `paste-node`, `dismiss-notifications`, …). Entries
may have an optional leading `:` — both forms are accepted.

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

TAB = "Tasks"   # display name of the adapter Tasks tab

def show(p):
    return subprocess.run([CLI, "task", "show", "--path", p, "-i"],
                          capture_output=True, text=True)

def task_id_at(p):
    r = show(p)
    return json.loads(r.stdout)["id"] if r.returncode == 0 else None

task_id = task_id_at(ticket)
if task_id is None:
    p = show(parent)
    if p.returncode != 0:
        sys.exit(f"Parent path not found: {parent}\n{p.stderr}")
    parent_id = json.loads(p.stdout)["id"]
    subprocess.check_call([CLI, "task", "add", desc, "--parent", parent_id])
    task_id = task_id_at(ticket)          # re-resolve the new leaf's id

with open(os.environ["NYD_OUTPUT_FILE"], "w") as f:
    json.dump({"commands": [f'tree-find "{TAB}" id:{task_id}']}, f)
```

`:tree-find` switches to the adapter Tasks tab, reloads it (so the
just-created task is in the adapter snapshot — the reload and the jump
happen in one command, no separate refetch step), and lazily
expands to the node. Passing the resolved task **id** via the
`id:<uuid>` escape keeps the jump exact even when the Taiga subject
and the local description have drifted apart.

The mirror-image flow — selected local task → matching Taiga item —
uses `:focus-node`. The script lives under
`<data_dir>/not_yet_done/scripts/tasks/task_item/` (the `Tasks`
tab's script directory, shared by its list + tree views) and emits one
command that switches to the Taiga tab and parks the cursor on the row
whose `ref` column matches:

```python
#!/usr/bin/env python3
# mode: commands
import json, os, re, sys
node = json.load(open(sys.argv[1]))["node"]
# Convention: task description starts with "#<num> - <subject>";
# the ancestor directly above the "tickets" folder is the Taiga
# project slug.
m = re.match(r"#(\d+)\b", node["label"])
# `fields.ancestors` is a JSON-array *string* of {"id", "description"}.
ancestors = json.loads(node["fields"]["ancestors"])
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
2. **Global fallback** — `script.template:` in `tui.yaml`. Always
   present; ships with a generic `{"node": {...}}` scaffold.

Layer 1 is optional. Per-view overrides are useful when a view's JSON
shape or fields differ from the generic node scaffold — for example a
`scope: filtered_set` batch view (JSON `{tracking_ids, filter_min_date,
filter_max_date}`) on the Trackings tab, or a Taiga `items` view whose
nodes carry rich, named fields (`ref`/`assignee`/`status`) a tailored
starter can reference directly.

```yaml
# ~/.config/not_yet_done/views/trackings.yaml — on the batch view
script_template: |
  #!/usr/bin/env python3
  # mode: background
  import json, sys
  with open(sys.argv[1]) as f:
      data = json.load(f)
  print(f"Got {len(data['tracking_ids'])} tracking(s)")
```

```yaml
# tui.yaml
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

It is a thin frontend over the same in-process `trackings` content adapter the
TUI and `nyd` use — it does **not** open the database itself. This means it
reads whatever database that view is configured for (the split-out `tasks.db`),
stays correct when the storage backend changes, and requires a configured
`~/.config/not_yet_done/views/trackings.yaml`. If no `trackings` view is
configured, the module simply shows nothing.

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

| Purpose              | Path                                                     |
| -------------------- | -------------------------------------------------------- |
| TUI config           | `~/.config/not_yet_done/tui.yaml`                        |
| CLI config (aliases) | `~/.config/not_yet_done/cli.yaml`                        |
| Adapter view files   | `~/.config/not_yet_done/views/*.yaml`                    |
| Core database        | `~/.local/share/not_yet_done/nyd.db`                     |
| Task database        | `~/.local/share/not_yet_done/tasks.db`                   |
| Backups              | `~/.local/share/not_yet_done/backups/`                   |
| Hook throttle state  | `~/.local/state/not_yet_done/hooks.json`                 |
| Notes                | `~/.local/share/not_yet_done/notes/`                     |
| Scripts              | `~/.local/share/not_yet_done/scripts/<tab>/<view-path>/` |

`DATABASE_URL` overrides the core database path; `NYD_TASKS_DB` overrides the
task database path (otherwise each adapter uses the `database:` DSN from its
view file, defaulting to `tasks.db`).

### Lifecycle hooks

A **hook** binds an adapter action to a point in the adapter's lifetime,
configured per instance in its view file (`views/*.yaml`). The only hook so far
is `connected`, fired right after the adapter is built — for the in-process
tasks/trackings adapter that is **every program start** (TUI launch or any `nyd
tasks …` / `nyd trackings …` command). This is how the daily `tasks.db` backup
works — it is no longer hard-coded:

```yaml
# views/tasks.yaml — sibling of the `adapter:` block
hooks:
  connected:
    - run: backup # adapter action id (same one the `B` key triggers)
      on: {} # target node: omit for root | { id: <node-id> } | { query: <q> }
      with: {} # action inputs: { value: …, text: … } (none needed for backup)
      when: { throttle: 24h } # fire at most once per window (s/m/h/d); omit to always fire
```

Each binding runs an adapter action, throttled via the host state file
`~/.local/state/not_yet_done/hooks.json` (shared across front-ends, so the
backup fires once a day whichever front-end you launch first). Change the
`throttle`, point `run` at a different action, or drop the block to disable
auto-backup. Hooks are best-effort: a bad action or unwritable state file is
logged and never blocks startup. See
[decision 0005](docs/decisions/0005-host-crate-und-lifecycle-hooks.md) for the
design.

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

| Field                | Description                      |
| -------------------- | -------------------------------- |
| `bg`                 | Main background                  |
| `surface`            | Panel/bar background             |
| `surface_2`          | Secondary surface (selected row) |
| `primary`            | Primary accent                   |
| `primary_dim`        | Dimmed primary                   |
| `on_primary`         | Text on primary backgrounds      |
| `accent`             | Main accent color                |
| `accent_dim`         | Dimmed accent                    |
| `text_high`          | High-contrast text               |
| `text_med`           | Medium-contrast text             |
| `text_dim`           | Subdued text                     |
| `success`            | Success indicators               |
| `error`              | Error indicators                 |
| `warning`            | Warning indicators               |
| `secondary`          | Secondary accent                 |
| `tertiary`           | Tertiary accent                  |
| `tree_connector`     | Tree branch connector lines      |
| `taskpath_separator` | `kind: path` segment separator   |
| `group_header`       | Group-header rows + total footer |
| `tab_active`         | Active main tab FG               |
| `tab_active_bg`      | Active main tab BG               |
| `sub_tab_active`     | Active sub-tab FG                |
| `sub_tab_active_bg`  | Active sub-tab BG                |
| `toolbar_bg`         | Action/status bar background     |
| `focused_bg`         | Focused element background       |
| `form_bg`            | Form panel background            |

</details>

## Anonymization (`NYD_ANON`)

The app normally runs against **production** backends (real Jira/Taiga/
Confluence instances, the real task/tracking DB). When you want to
screenshot or screencast it for a product demo, no real customer, ticket
or person names may appear. Setting `NYD_ANON=1` (truthy: `1`/`true`/
`yes`/`on`) makes **every** adapter emit plausible fake data instead —
across the TUI, the `nyd` CLI and the Waybar module alike, because the
switch sits at the single `host::factories()` chokepoint, not in any one
frontend. With the flag off there is zero overhead.

```bash
NYD_ANON=1 not-yet-done-tui
```

What it does and, just as importantly, what it deliberately does **not**:

- **Format-preserving fakes.** A Jira key stays key-shaped
  (`PREFIX-123` → `ACME-123`), a Taiga ref stays ref-shaped
  (`slug#12` → `code#12`), people stay names, filenames keep their
  extension. The view still _looks_ real.
- **Kind-preserving labels.** A Postgres or Stoat tree keeps telling you
  _what_ each node is: real names become `<adjective>_<noun>` placeholders
  (`big_database`, `nifty_schema`, `mellow_table`, `swift_server`,
  `jolly_channel`), while the structural signposts — "Schemas", "Tables",
  "DB Scripts" — stay verbatim. A Jira status maps to a generic pool
  (`To Do`/`In Progress`/`Done`/…) so a customised workflow status can't
  leak a customer term.
- **Real times and structure.** Durations, timestamps and the tree shape
  pass through verbatim — for a time-tracker the real durations are the
  whole point of the screenshot.
- **Deterministic and consistent.** The same real value always maps to
  the same fake (keyed on a stable hash of the real name, not a DB id),
  this run and the next — so the same task reads identically in the Tasks
  tree, in a tracking's `task` column and in its `taskpath`, and a
  re-recorded screencast stays coherent.
- **Safe by default.** Anonymization is a mandatory contract with a safe
  fallback, not an opt-in capability: an adapter (or just a new column)
  that defines no realism override still gets scrubbed by the generic
  `StandardAnonymizer` — it can never silently leak.
- **A read/display mask only — mind the editor.** Editable and
  exportable bodies (an open editor, a content preview, custom-query
  results, a downloaded node) are **not** faked, on purpose: faking a body
  the user then saves would overwrite the real data. So the table rows
  behind an open editor are clean, but the open body is not — when
  capturing, don't show an open editor or a raw body preview of a real
  row.
- **Not a security guarantee.** It hides plain-text names and keys, not
  correlation via tree shape or time distribution. It is meant for the
  demo/screenshot use case, not as protection against an adversary who
  also has the real database.

Design rationale and the full list of scrubbed vs. raw surfaces are in
[ADR 0006](docs/decisions/0006-anonymisierung-content-layer.md).

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

The original design analysis on whether the (now-removed) native Tasks
and Trackings tabs could move onto the `ContentAdapter` abstraction —
since fully realized; both are adapter-backed tabs today — lives in
[`docs/adapterize-tasks-trackings.md`](docs/adapterize-tasks-trackings.md);
the phased implementation plan derived from it is in
[`docs/plan-adapterize-tasks-trackings.md`](docs/plan-adapterize-tasks-trackings.md).

```
# Frontends
not-yet-done-tui          # TUI binary (ratatui + tuirealm)
not-yet-done-cli          # nyd — generic adapter front-end + tag/backup/config
not-yet-done-task-cli     # nyd-t — native Tasks/Trackings domain CLI
not-yet-done-waybar       # Waybar CFFI module (cdylib)

# Host — one adapter-wiring path shared by all front-ends
not-yet-done-host         # Factory registry, resolve_adapter, lifecycle hooks

# Content adapter contract + backends
not-yet-done-content      # ContentAdapter/Node trait + auth orchestration
not-yet-done-local-adapter# Tasks/Trackings/Projects as adapters (over task-core)
not-yet-done-jira-adapter not-yet-done-taiga-adapter
not-yet-done-postgres-adapter not-yet-done-confluence-adapter
not-yet-done-stoat-adapter not-yet-done-transport   # SSH tunnel

# Data core
not-yet-done-core         # nyd.db: settings, saved queries, links, tags
not-yet-done-task-core    # tasks.db: task/tracking domain, bootstrap, backup
not-yet-done-filter       # YAML filter DSL

# UI building blocks
not-yet-done-forest       # Tree rendering with post-order fold
not-yet-done-table        # Column layout computation
not-yet-done-ratatui      # Custom ratatui widgets / inline editor
not-yet-done-grid-core    # Grid layout core
not-yet-done-macros       # Derive macros (ColumnRegistry)
```

## License

MIT
