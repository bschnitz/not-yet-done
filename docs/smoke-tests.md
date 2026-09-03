# Smoke Tests

Central collection of manual smoke tests for not-yet-done. On every new
feature or larger refactor: add the matching tests here, not in separate
documents. The done markers (`[x]` / `[ ]`) stay in place so that one can
see what has been green at least once.

When a bug is found: stop, diagnose, fix or record it BEFORE moving on.
Findings that turn into separate tasks get a short note under the item.

## Jira ContentView — issue level (phase 1)

- [x] The list loads (`assignee = currentUser() ORDER BY updated DESC`)
- [x] `e` edits the issue (action `edit_full`, `InputSpec::Editor`) →
      3b template, any change, `:wq` → "X updated"
- [ ] Clear the summary → `:wq` → reopen with an error banner; the
      previous summary is restored automatically (no blank buffer any
      more — the user does not have to type blindly)
      → reopen suffix bug (`.md` instead of `.jira`) fixed in `main.rs`,
      re-test
- [x] Concurrent browser edit on a disjoint field → auto-merge
      notification
- [x] Concurrent browser edit on the same line → reopen with conflict
      markers
- [x] `:q!` without a change → "Edit cancelled"
- [ ] `t` opens the transition picker (action `transition`,
      `InputSpec::Picker`) → the options are loaded, pick with
      Enter → "X transitioned"
      → the user YAML was outdated (`custom_action: transition` instead
      of `id: transition`). Migrated, re-test.
- [x] `r` reload → fresh list
- [x] `f` fuzzy filter → typing filters, `enter` closes
- [ ] `/` text search → typing jumps, `n`/`N` next/prev
      → the user YAML had no `search` action. Added, re-test.
      → in tasks/trackings, `/` is an older, separate bug — its own task.
- [x] `q` opens the query menu, `Q` opens the query editor

## Jira ContentView — drill-down (phase 1)

- [x] `C` (navigate) drills into the comments → the list loads
- [ ] `e` edits a comment (`edit_full`) → "Comment updated"
      → works. Extended in phase 2: the edit action is hidden for
      comments by other authors (see below).
- [ ] `a` (create) opens the editor (`create_comment`) → enter a body,
      `:wq` → the drill-down list refreshes, the new comment is visible
      → the user YAML was outdated (`type: create` without `id`).
      Migrated, re-test.
- [x] `backspace` back to the issues
- [ ] `a` (navigate) drills into the attachments → read-only list, no
      edit action
- [ ] Mark an attachment, `o` → the file is downloaded to
      `$TMPDIR/not_yet_done/jira_attachments/<id>-<filename>` and
      launched via `xdg-open` (in the background, no TUI pause).
      Notification "opened &lt;filename&gt;". A second `o` on the same
      attachment opens it without re-downloading.

## Jira ContentView — `edit_with_comments` (phase 2)

`Shift+e` on an issue: opens the 3b header plus all comments in one
buffer (newest→oldest). Own comments are editable inline / deletable via
`del`; new comments through an `--- add ---` block. Comments by others
carry `[not yours]` in their header line and are refused on save;
conflicts end up in the banner reopen.

- [ ] `Shift+e` opens a buffer with the header and all comments in
      newest→oldest order
- [x] The header line of a comment by somebody else reads
      `--- @author <ts> [not yours] (id=…) ---`; own comments are unmarked.
      Same wording as the Taiga buffer. (Confirmed headless, same `EDITOR`
      trick as in the Taiga section below.)
- [ ] `E` (Markdown mode) on the same issue: the heading of a foreign
      comment keeps `[not yours]`, and saving an unrelated change does not
      lose it — the marker sits before the id, so it survives
      wiki→md→wiki.
- [ ] `o w` (`export_workspace`) / the `o p` preview of the same issue:
      `ticket.md` carries **no** marker — it is a read-only snapshot, not
      an editing buffer.
- [ ] Edit the body of an own comment → `:wq` → notification
      "X updated, comments: ~1" (or with `+`/`-`/`~` counts when there
      are several operations)
- [ ] Delete an own comment via `del` (or `delete`, case-insensitive,
      as the only non-blank line) → `:wq` → DELETE request,
      "comments: -1"
- [ ] `--- add ---` plus a body at the end → POST a new comment, the
      drill-down shows it afterwards
- [ ] Edit someone else's comment → banner reopen
      `# ─── COMMENTS CHANGED UPSTREAM ───` with one bullet per foreign
      edit attempt plus a restore from fresh
- [ ] Concurrent browser edit on a comment while the editor is open →
      banner reopen with re-rendered fresh comments plus the user edit
      as a banner bullet
- [ ] Header edit and own comment edit at the same time → both go
      through, one combined notification
- [ ] In the JiraCommentNode drill-down on a foreign comment: the edit
      action is not in the action bar (only own comments are editable)
- [ ] Reopen → the user accepts the foreign version (deletes their own
      edit, leaves fresh in place) → the next `:wq` goes through without
      a banner

## Schema validation (phase 1, strict)

`ActionDef` has `#[serde(deny_unknown_fields)]`; `validate()` runs at app
start on every YAML file recognised as a view config (has `tab` +
`adapter`). On error, `exit(1)` with a diagnostic.

- [ ] A `create` action without `id:` in the YAML → the app does not
      start, message "type='create' requires `id` (e.g. id:
      create_comment)"
- [ ] A `custom` action without `id:` in the YAML → the app does not
      start, message "type='custom' requires `id`"
- [ ] `navigate` without `navigate_to:` → the app does not start,
      message "type='navigate' requires `navigate_to`"
- [ ] Legacy fields (`edit:`, `custom_action:`, `query_template:`) in
      the YAML → the app does not start, serde error "unknown field"
- [ ] An adapter credential file (no `tab:`/`adapter:`) in the views
      directory → silently skipped (no app crash)

## Action bar / status bar

- [ ] `e` (edit), `f` (fuzzy_filter), `/` (search), `q` (queries),
      `Q` (edit query), `Shift+e` (edit + comments) appear in the action
      bar
      → `/` is now configured in the user YAML, re-test.
- [x] `r` (reload), `c`/`a` (navigate), `t` (custom transition) appear
      in the status bar only
- [ ] On drill-down the hints change to the child-level config
      (comments: `e`/`a`/`f`; attachments: no edit actions)
      → `a` on the comments child now has `id: create_comment`,
      re-test.

### The status bar derives nav/fold hints from the claims

The status bar no longer enumerates `back`/`open`/paging/fold chords by
hand; it derives them from the same set of claims the dispatcher uses
(`ContentPane::build_claims`). Every nav/fold action that can currently
be triggered therefore appears in the bar automatically.

- [ ] **Tasks / trackings tree**: the status bar shows
      `[zm] collapse all`, `[zr] expand all` and `[⌫] collapse` — at
      every cursor depth, as long as the view is in tree mode (was not
      visible before).
- [ ] On a grouped flat view, `cycle group` appears in addition; on a
      tree view with a `tree_aggregate` column, `aggregate`.
- [ ] **Paging**: `prev page`/`next page` appear only when there
      actually is a page in that direction (the gate now sits in the
      claim, no longer in the bar).
- [ ] `open` appears only when the cursor row can be expanded or drilled
      into; `back` only after a drill-down.

### Active marking of the action-bar hints

The upper action bar marks every shortcut that is currently **active**
("armed") with the accent colour plus bold plus underline. Every
`ActionHint` carries its own `active` flag — the component no longer
knows any special cases.

- [ ] **Jump**: press `J` (or the configured `jump_mode` key) → the
      `jump` hint is marked as long as the hop overlay is open; after a
      pick or `Esc` the marking goes away. Across all content tabs
      (tasks, trackings, Jira, …) and in all views (list/tree/condensed).
- [ ] **Track**: start a tracking in tasks / trackings (`t`/`s`) → the
      `track` hint stays marked as long as a tracking is running; stop →
      marking gone.
- [ ] **Cut**: put a node on the move clipboard with `a m` (mark-move) →
      the `cut` hint is marked until paste/abort/tab switch.
- [ ] **Editor**: open an editor (`e`/`a`) → the corresponding hint
      (`edit`/`add`/…) is marked as long as the edit session is open.
- [ ] Rebinding test: move `jump_mode` to a different key in `tui.yaml`
      → the hint shows the new key AND still marks correctly (identity
      via the configured key, not hardcoded).
- [ ] **Link hop**: in a Stoat chat with visible links (a bare URL
      and/or a markdown `[text](url)`) press `f` → every link gets a
      green label; type the label → the URL opens in the browser (opener
      from `navigation.link_opener`, default `xdg-open`), the TUI does
      not block. `Esc` closes the overlay; a pane without links → notice
      "No links on screen". Generic on every content tab (not just
      Stoat).

## EditSession — Jira (refactor phase 7)

- [x] Edit an issue via `e` → see the 3b layout, change any field,
      `:wq` → the app stays responsive during the save (Jira is slow,
      easy to observe), notification "X updated"
- [x] During the save (a 5–30 s window) press `e` again → notification
      "Saving previous edit, please wait…"
- [x] Edit an issue, delete the summary → `:wq` → error banner
      ("Summary is required") in the reopen
- [x] Edit an issue, change a **different** field in the browser, `:wq`
      locally → auto-merge: no reopen, notification "X updated
      (auto-merged with upstream changes)". Disjoint body lines (line 1
      local, line 5 upstream) also go through automatically.
- [x] Edit an issue, change **the same line** differently in the
      browser, `:wq` locally → reopen with a banner plus git-style
      markers (`<<<<<<< ours`, `=======`, `>>>>>>> theirs`) exactly on
      the conflicting line. Resolve by deleting one side plus the
      markers, save → "X updated"
- [x] Leave the markers in place in the reopen and `:wq` → error banner
      "unresolved conflict marker — keep one side and remove the
      markers"
- [x] Edit an issue, `:q!` without a change → notification "Edit
      cancelled"
- [x] Create a comment (`C` drills into the comments, `a` =
      ContentChildCreate) → the drill-down list refreshes

## EditSession — tasks

- [x] `n` add-task in the tree subview → the parent is inherited
- [x] `n` add-task in the list subview → no parent
- [x] `e` edit-task → change the tracking toggle in the form, save →
      active_trackings is updated, the action bar reflects it
- [x] `r` restructure → edit the subtree, several `:w` while the editor
      is open → live_apply runs, the IDs are recognised correctly (no
      double insert)
- [x] Restructure with a parse error → the query_error bar shows the
      error, it disappears on the next successful save
- [x] Edit the notes (`o`) → saves; save with an empty buffer → the file
      is deleted (re-test after the fix: TaskNotesSession::new used to
      create a 0-byte file, so the empty save matched the empty template
      and triggered cancel detection instead of commit→delete)

## EditSession — trackings

- [x] Create a tracking script → it is placed under
      `<data>/not_yet_done/tracking/scripts/` with `chmod 755`
- [x] Run a script (background / capture / interactive) → each mode
      works; with capture, an output editor opens (read-only)
- [x] Edit the tracking query filter via the query menu → live apply on
      `:w`, save with a name → the favorite-shortcut prompt appears

## `:script` fuzzy menu (trackings + tasks + content)

- [ ] Trackings tab: `x` opens the menu with the scripts under
      `<data>/not_yet_done/tracking/scripts/`; `X` is no longer bound
      (removed)
- [ ] Enter on an existing script → runs it (the JSON argument contains
      `tracking_ids` + `filter_min_date` + `filter_max_date` as before)
- [ ] Type a name with no match + Enter → opens an empty editor on a new
      script under the matching scripts directory
- [ ] `+name` as input + Enter → forces CreateNew even when `name`
      matches an existing script
- [ ] Ctrl+E → opens the selected script in the editor
- [ ] Ctrl+D → deletes the selected script (with a notification)
- [ ] Tasks tab (list **and** tree, both subviews): `x` opens the menu
      with the scripts under `<data>/not_yet_done/scripts/tasks/` (flat,
      shared pool). The per-view title is "Scripts · Tasks".
- [ ] Tasks tab, `x` without a selection → notification "No task
      selected", the menu does not open.
- [ ] Tasks tab, running a script → the JSON argument has the shape
      `{"task": {…}}` with the keys `id`, `description`, `parent_id` (a
      UUID or null) and `ancestors` (a list of objects with `id` and
      `description`). `ancestors` runs root→parent (excluding self). A
      root task has a null `parent_id` and an empty `ancestors` list.
- [ ] Tasks tab, `:script` (cmdline) → the same menu as `x`.
- [ ] Tasks tree, cursor on a task in a deep path (e.g.
      `Work/Clients/acme/Tickets/#42 - …`): `ancestors` contains exactly
      the 4 parents in root→parent order.
- [ ] Tasks tab, a script in `# mode: commands` emits
      `focus-node Taiga:items /ref|<slug>#<n>` → the tab switches to
      Taiga, the cursor parks on the ticket (the reverse direction of
      the `goto_task.py` flow).
- [ ] `:script` in a content tab with a selected node → menu with the
      scripts under `<data>/not_yet_done/scripts/<tab>/<view-path>/`;
      running one delivers a JSON object with a `node` key holding
      `ref`, `id`, `label`, `node_type`, `tab`, `instance` and `fields`
      (`label` = the display label of the row, e.g. the task
      description)
- [ ] Taiga `items` view with mixed node types (issue + userstory + task + epic): the script menu shows **the same** list regardless of the
      selected node — path `scripts/taiga/taiga_item/`. The JSON
      `node.node_type` still carries the item type (`taiga_issue` /
      `taiga_userstory` / …)
- [ ] Drill down into a ChildDef (e.g. `taiga:item` → `taiga:comment`):
      the script menu now shows the scripts from
      `scripts/taiga/taiga_item/taiga_comment/`, and the JSON contains
      the fields of the selected comment
- [ ] A per-view `actions: - {name: script, key: x, type: script}` in a
      view YAML → pressing `x` triggers the menu; without that entry,
      `x` does nothing (no global default on content tabs)
- [ ] **Batch scope (`scope: filtered_set`)** — trackings, flat
      `trackings` view (`x` with `scope: filtered_set`): running a
      script delivers JSON with the keys `tracking_ids`,
      `filter_min_date` and `filter_max_date` (NOT a `node` key) —
      exactly the legacy shape expected by `daily_report.py` /
      `hours_report.py` / `equalize_trackings.py`; the migrated scripts
      under `<data>/not_yet_done/scripts/trackings/tracking_entry/` run
      unchanged.
- [ ] Batch scope, `tracking_ids` follows what is visible: without a
      fuzzy filter = all rows of the active query; with an active fuzzy
      filter (`f`) = exactly the matching set.
- [ ] Batch scope, date bounds: with the active query
      `started_at gt last month` → `filter_min_date` is the resolved
      start of the month (RFC3339), `filter_max_date` is `null` (no
      upper bound).
- [ ] An interactive script with a `{json_file}` placeholder in
      `interactive_command` → served by both paths (trackings +
      content); the old `{tracking_json_file}` has been renamed, so
      tui.yaml needs a one-time adjustment
- [ ] Taiga `items` view, cursor on a ticket with a `ref` like
      `acme#42`, `:script` → run `goto_task.py` → the TUI jumps to the
      **adapter** tab "Tasks" (NOT the legacy tasks tab) and
      expands/parks on the task at the path
      `/work/.../<slug>/tickets/<…42…>`. The script emits exactly one
      `tree-find "Tasks" id:<uuid>`.
- [ ] Taiga `items` view, auto-create path: cursor on a ticket whose
      local task does NOT exist yet (e.g. a new ticket number).
      `:script` → `goto_task.py`:
  - The script calls `task add` via the CLI, creates
    `#<n> - <subject>` under the `tickets` parent and then re-resolves
    its `id`.
  - `tree-find` forces a fresh reload of the adapter tab **before**
    searching → the task just created is immediately visible (parity
    with the old `reload-tasks`), and the cursor parks on it.
  - Running it again is idempotent (the task then already exists → just
    jump+focus). The tree shows NO duplicates.
  - If the parent path (`/work/.../<slug>/tickets`) does not exist at
    all: a modal error from the script (stderr).
- [ ] `:tree-find` directly (without a script): `:tree-find "Tasks"
<text>` (a description substring) jumps to tasks and parks on the
      first match; `n`/`N` cycle through the others. The same command
      with `id:<uuid>` instead of the text parks exactly on that node.
      A modal error for an unknown tab/view, or when the active view is
      not a tree (with a pointer to `:focus-node`).

## `:query apply` — saved-query activation via cmdline

- [ ] On a content tab with at least one saved query defined in YAML,
      `:query apply <name>` (without `-t`) → the named query becomes
      active in the current view (the action bar shows
      `Filter: <name>`), rows are reloaded; losing the previous cursor
      is fine.
- [ ] `:query apply foo bar baz` with whitespace in the name → the name
      is interpreted as one whole token (whitespace stays part of the
      match string, compared case-insensitively).
- [ ] `:query apply -t Taiga:items <name>` from another tab → switches
      to Taiga:items first, then activates the query and reloads. If
      `<name>` is only a YAML default, this also works for a tab that
      has never been visited.
- [ ] `:query apply -t Taiga:nonexistent foo` → modal error "unknown
      view 'nonexistent' for tab 'Taiga' (available: …)", no tab switch.
- [ ] `:query apply unknown-name` → modal error listing the available
      saved queries.
- [ ] On a tasks or trackings tab without `-t`: modal error "not on a
      content tab".
- [ ] Command chain from a `# mode: commands` script:
      `query apply -t Taiga:items <q>` followed by
      `focus-node -i Taiga:items /ref|<slug>#<num>` → the saved query is
      already active at the `focus-node` step and the cursor parks on
      the ticket (synchronous reload between the two steps).
- [ ] `:query` without a subcommand → modal error pointing at
      `:query apply`. `:query foo` → modal error "unknown subcommand
      'foo'".

## `:query edit/new/delete` — saved-query body management

- [ ] On a content tab with a Jira or Taiga adapter, `:query new Test
foo` → `$EDITOR` opens on an empty buffer with the suffix `.yaml`.
      Enter content, save, close the editor → notification "Saved query
      'Test foo'", the file appears under
      `<XDG_DATA_HOME>/not_yet_done/<adapter>/<instance>/queries/Test foo.yaml`,
      and the Q menu (key `q`) shows it.
- [ ] Immediately afterwards, `:query new Test foo` again → modal error
      "'Test foo' already exists (use :query edit to modify)", no
      editor.
- [ ] `:query edit Test foo` → the editor opens with the previously
      saved content. Change the content, save, close → notification,
      the file is updated.
- [ ] `:query edit unknown` → modal error "no saved query named
      'unknown' (use :query new to create)".
- [ ] `:query delete Test foo` → the file and any shortcut entry are
      gone, notification "Deleted saved query 'Test foo'", the Q menu no
      longer lists it.
- [ ] `:query delete unknown` → no action, quietly (idempotent, no
      modal), but a notification all the same.
- [ ] On a tasks or trackings tab: `:query edit/new/delete foo` → modal
      error "not on a content tab".
- [ ] On a Postgres tab (an adapter without `saved_query_store()` → not
      migrated yet) → modal error "adapter 'postgres' has no saved-query
      store".
- [ ] The Q menu orders the list such that new queries show up right
      away without a restart (the adapter store is re-read before every
      Q-menu invocation).

## `:query apply` — variables + popup

- [ ] Taiga saved query with `project=${proj:alpha}` as the YAML
      default: shortcut key (e.g. `1`) → the popup opens with a `proj`
      field pre-filled with `alpha`. Enter → reload with
      `project=alpha`. Esc → no reload, the popup closes, the old rows
      remain.
- [ ] Same query: open the popup, change the value to `beta`, Enter →
      reload with `project=beta`. The action bar shows `Filter: <name>`
      as before.
- [ ] Query with `${proj}` (no default, therefore required): shortcut →
      the popup shows the label `proj (required)` and an empty field.
      Enter on the empty field → inline error "'proj' is required", the
      popup stays open. Enter a value + Enter → reload.
- [ ] The `Apply` action of the query menu on a query with variables →
      the same popup, the same behaviour as the shortcut (always a
      popup).
- [ ] `:query apply --var proj=alpha -t Taiga:items <name>` on the same
      query → no popup, a direct reload with `project=alpha`. Suitable
      for scripts.
- [ ] The same command with two `--var` arguments (e.g.
      `--var proj=alpha --var x=42`) → all of them are pre-filled. The
      order of `--var` and `-t` does not matter.
- [ ] `:query apply -t Taiga:items <name>` without `--var` on a query
      with only optional variables (all have defaults) → no popup (CLI
      path), reload with the defaults.
- [ ] `:query apply -t Taiga:items <name>` on a query with one required
      variable and without a `--var` for it → the popup opens, because
      at least one required variable is not covered.
- [ ] `:query apply --var=oops -t ...` (no `=` in the value) → modal
      error "--var expects k=v".
- [ ] Saved query without variables (no `${...}`) → behaviour unchanged,
      no popup, direct reload.
- [ ] Tab switching and other popups: the query-var popup behaves like
      other modals — while open it swallows all keys except Esc.

## EditSession — query menu (all three tabs)

- [x] Tasks tab: create a new filter, save → DB persist + favorite
      prompt
- [x] Trackings tab: edit an existing filter → no prompt
- [x] ContentView: edit a query with and without save_name

## EditSession — editor paths

- [x] Launch mode (the user default): a kitty split opens, `:wq` closes
      the split, the app is responsive
- [x] Inline mode (`editor.inline: true` in tui.yaml): the TUI pauses,
      the editor gets the terminal, resume after `:wq` — still works as
      before (sync await, may block with a slow backend)

## Tasks/trackings — `/` text search

- [ ] Tasks tab: `/` opens the search bar in the action bar (`/ ` +
      cursor + a "type to search…" placeholder)
- [ ] Typing narrows the selection to the first match. The row jumps to
      the first hit automatically
- [ ] `n`/`N` jump to the next/previous match
- [ ] `enter` closes the search bar, the selection stays on the hit
- [ ] `esc` with an empty query closes the bar; with a non-empty query →
      clear first, then close (two `esc`)
- [ ] Trackings tab: the same behaviour

## Jira — free-text search (`s`)

- [ ] Jira tab, root level: `s` opens the action bar with the prompt
      `? ` and the placeholder configured in `jira.yaml` (`Jira-Suche`,
      no `[n/m]` counter)
- [ ] Typing does nothing locally (no filter, no selection jumps — the
      input only goes out on submit)
- [ ] Plain-text input + `enter` → the active query becomes
      `text ~ "<input>"` (no `ORDER BY`, `{key_or}` stays empty); the
      reload runs and the query name is cleared (an anonymous query).
      The hits are sorted by Lucene score (best match first), not by
      date
- [ ] Input in issue-key form (`ABC-123`) + `enter` → the query becomes
      `issuekey = "ABC-123" OR text ~ "ABC-123"`; that ticket shows up
      in the result
- [ ] `esc` with an empty query closes the bar; with a non-empty query →
      clear first, then close
- [ ] Input containing `"`/`\` characters → the JQL stays valid (escaped
      via `\"` / `\\`); the search runs without a 400
- [ ] Prompt override: when `prompt:` is removed from the YAML, the
      placeholder falls back to `free-text search…`
- [ ] `q` (query menu) and `Q` (query editor) still work — `s` must not
      take anything away from the other search paths
- [ ] **Bug regression**: pressing `s` in a content tab no longer
      triggers a tracking toggle (before: the last tracking from
      trackings was started)

## Jira — toggle watch (`w`)

- [ ] Press `w` on an unwatched issue → the status bar shows
      `<KEY>: watching`; after a short reload the ticket shows up in the
      saved query "Watched Tickets" (`ctrl+w`)
- [ ] Press `w` on a watched issue → the status bar shows
      `<KEY>: no longer watching`; after the reload the ticket has
      disappeared from the `ctrl+w` view
- [ ] Error case (e.g. auth status not ok / issue unreachable) → the
      status bar shows `Action failed: …`, no crash, the view stays put

## Jira — saved queries (ctrl shortcuts)

The bodies live under
`<XDG_DATA_HOME>/not_yet_done/jira/<instance>/queries/<name>.yaml`, the
shortcuts in the DB table `query_shortcut` (scope
`jira:<instance>:tickets`).

- [ ] `ctrl+i` loads "My Tickets" (`assignee = currentUser()`)
- [ ] `ctrl+w` loads "Watched Tickets" (`watcher = currentUser()`) —
      must NOT collide with the `w` toggle-watch
- [ ] `ctrl+m` loads "Mentioned In" — must NOT be interpreted as `enter`
      (the kitty protocol is required, otherwise it collides with the
      selection action)
- [ ] The `q` menu lists exactly the bodies from the adapter's queries
      directory (no YAML `saved:` leftovers any more). The action bar
      shows the shortcuts on the saved query — `My Tickets [ctrl+i]`
      etc.
- [ ] `:query delete <name>` deletes the body **and** the shortcut row;
      after an app restart both are gone.

## Jira — labels / assignee / mentions in the edit template

- [ ] `e` on an issue: the editable section now shows `summary:`,
      `labels:` (CSV with `ll-…` slugs), `assignee:` (a `uu-…` slug or
      empty)
- [ ] At the end of the template there is a block headed
      `#### CACHE / available labels & users (do not edit) ####` with
      the slugs from the cache
- [ ] Add/remove a label via its `ll-…` slug → saves correctly (the list
      is replaced, not appended to)
- [ ] Change the assignee via a `uu-…` slug → saves; an empty value
      un-assigns the issue
- [ ] An unknown `ll-foo` or `uu-foo` → reopen with a banner error
      ("unknown label slug …" / "unknown user slug …")
- [ ] `Shift+E` (`edit_with_comments`): in every comment body,
      `[~JDOE1]` is displayed as `@uu-jane-doe`
- [ ] Save a comment unchanged → no change event (a clean round trip, no
      update sent to Jira)
- [ ] Write a `@uu-…` in an `--- add ---` block → it arrives at Jira as
      a `[~KEY]` mention
- [ ] An unknown `@uu-…` in a comment → reopen with a banner error
- [ ] Colliding slugs (two labels normalising to the same value) get a
      `-2`, `-3` suffix; deterministic across restarts

## Jira — merge-only user/label cache (issue-based)

Background: the old bulk pull (`/rest/api/2/user/search?username=.`) is
broken — the server caps at 100 hits without saying so. Instead the
cache is now fed purely from issues that were actually loaded (assignee,
reporter, creator, comment authors and labels) and is strictly additive:
whatever is in it stays in it; existing entries only get their
`display_name` updated on a re-merge.

- [ ] First app start after the update: stderr shows, once, a line of
      the form "cleaned up N orphan jira_user and M orphan jira_label
      row(s) from previous schema" (legacy rows from the old `run_sync`
      path with a different connection_id)
- [ ] Second start: the message does **not** come again (nothing left to
      clean up)
- [ ] Open an issue with a known reporter/creator who was not in the
      CACHE list so far → the CACHE section at the end of the buffer now
      lists them with a `uu-…` slug
- [ ] DB inspection: after opening the issue, the reporter/creator is a
      row in `jira_user` (the same `connection_id` as the existing
      entries — a UUID v5 derived from the Jira URL)
- [ ] An issue with a comment author who was not in the cache so far →
      the author shows up as a `uu-…` slug in the CACHE section, and
      their `[~KEY]` is resolved to `@uu-…` in the comment body
- [ ] A user is renamed in Jira → after re-opening an issue in which
      they appear: the `display_name` is updated in the cache (in the DB
      and in the CACHE section); the `username` stays stable
- [ ] A user is deactivated in Jira → they stay in the cache
      (merge-only, nothing is ever deleted); slugs in old comments keep
      working
- [ ] App restart → the cache is hydrated from the DB; the CACHE section
      is filled right at the first issue open (no bulk API call needed
      any more — no "loading" state)
- [ ] A `[~KEY]` in a comment whose KEY is not in the cache yet:
      `Shift+e` resolves the KEY via `/rest/api/2/user?username=KEY`,
      merges it into the cache + DB and renders a `@uu-…` slug in that
      comment
- [ ] `[~UNKNOWN]` (a KEY that does not exist in Jira either) → the
      lookup errors, the KEY stays verbatim as `[~UNKNOWN]` in the
      render (no crash)
- [ ] CLI export `nyd content list jira:user` → an API call, and the
      result is additionally merged into the cache + DB (the next
      session finds the users there)
- [ ] An old YAML with a `cache:` block (`preload`, `label_ttl`,
      `user_ttl`) → the app starts without a validation error and the
      block is silently ignored (the fields are dead)

## Sort-hint mode (phase 6)

The default keybinding is `S`. Two phases, both rendering the overlay
directly in the table headers (no action-bar overlay). Column widths
stay stable across all phases.

- [ ] Tasks: `S` activates sort mode. In the sortable headers a label
      letter (`a`, `b`, `c`, …) appears at position 0, overwriting the
      first characters of the original header (e.g. `Status` →
      `atatus`, `Pri` → `bri`, `Task` → `cask`). Non-sortable columns
      (e.g. `Tr`, `N`) are dimmed.
- [ ] Tasks: columns that are already sorted keep their sort arrow next
      to the label when `S` is pressed (`Status ▲` → `atatus ▲`); the
      column width does not change when entering sort mode.
- [ ] Tasks: pressing a label letter switches to phase 2. Above the
      chosen header, the overlay `(d)esc/(a)sc/(c)lear` appears (in the
      accent colour). The header underneath keeps its original width —
      other columns do **not** shift, and the overlay may visually cover
      adjacent dimmed headers.
- [ ] Tasks: `a` (asc) / `d` (desc) / `c` (clear) perform the action.
      The sorted column then shows `▲` or `▼` next to the original
      header.
- [ ] Tasks: multi-column sorting is additive. After sorting by the
      `Status` column, pressing `S` → `Pri` → `a` applies **both** sorts.
      Both columns show an arrow with an index subscript (`Status ▲₁`,
      `Pri ▲₂`).
- [ ] Tasks: `c` on one of the sorted columns removes only that one sort
      level from the stack; the remaining sorts stay active.
- [ ] Tasks: the sort persists across a restart (`settings` table, key
      `tasks.sort`); the arrow is still visible after the restart.
- [ ] Tasks: in tree mode the sibling order stays consistent with the
      chosen sort column.
- [ ] Jira ContentView: `S` shows labels in the adapter headers (the
      sortable fields come from the YAML); a pick plus a direction
      triggers a reload with the new sort.
- [ ] Jira/Taiga ContentView: the sort persists per `query_scope` across
      a restart (in the dedicated `jira_view_sort_state` /
      `taiga_view_sort_state` tables in the adapter DB).
- [ ] `Esc` in either phase closes the mode without a change; the
      headers return to their original rendering.
- [ ] Switching tabs while sort mode is active → the mode closes
      automatically.
- [ ] Trackings (the native tab): `S` shows **no** sort hint in the
      status bar (the native trackings tab is deliberately excluded).

## Trackings — group order (`o`) + item sort (`S`)

Adapter tab "Trackings" on the grouped flat view (`key: a`, default
`group_by: started/day/desc`).

- [ ] The status bar shows `o order ↓` (descending = newest days
      first). Press `o` → the day groups flip to oldest-first, the
      indicator becomes `o order ↑`. Press `o` again → back. The
      **order of the entries within** a day does **not** change.
- [ ] `o` leaves the bucket granularity untouched: switch to week with
      `zg` first, then `o` → it flips the week order but stays on
      `week` (it does not fall back to day).
- [ ] Grouping set to "No grouping" via the `zg`/`u` menu → `o` is a
      no-op (no `order` hint in the bar, nothing happens).
- [ ] `S` opens the sort picker; **all** data columns are selectable
      (Active/Task/Task path/Started/Ended/Duration), not just a subset.
      Column `Duration` + `asc` → the entries **within each day group**
      are shortest-first, numerically correct (90 s before 600 s, not
      lexicographically "600" before "90"). The footer shows the active
      sort.
- [ ] `S` → `Started` + `desc` → the entries per group are in
      chronological order (date sorting, not string sorting). Running
      entries (`ended` = "running") sort to the end when sorting by
      `Ended`.
- [ ] `o` and `S` are orthogonal: first `S` duration asc, then `o` →
      the day groups flip, the duration sorting **within** them stays.

## Auth — explicit invalidation (phase 5)

- [ ] Cmdline `:invalidate-session` on a content tab: the status bar
      reports "Session invalidated, re-authenticating…", the
      `auth_session` row for the connection is gone, the next list call
      triggers a re-auth (the cookie script runs / the JWT is fetched
      again), and the list loads through.
- [ ] Cmdline `:invalidate-credentials` on a content tab with a
      `prompt` provider (Taiga): the status bar reports "Credentials
      invalidated, re-authenticating…", the resolver caches and the
      prompt cache are empty, and the credentials popup appears again.
- [ ] Cmdline `:invalidate-session` on a non-content tab
      (tasks/trackings) → modal "… only works on a content tab", no
      action.
- [ ] The YAML action `type: invalidate_session` with a keybinding
      fires identically to the cmdline variant. Tip:
      `- { name: forget session, key: I, type: invalidate_session }` in
      `views[].actions`.

## Taiga — notifications subtab (#149–#152)

Precondition: `~/.config/not_yet_done/views/taiga.yaml` contains the
`notifications` view (`node_type: taiga:notification`, `key: n`, with
actions including `mark as read` and `open ticket`).

- [ ] On the Taiga tab: `n` switches to the notifications subtab and the
      list loads; the columns Read/Event/Ref/Project/Actor/Created/
      Subject are visible; the default sort has two levels, read asc and
      created desc — all unread ones appear as a block at the top
      (newest first within the block), with the read block below it in
      the same date order. `i` switches back to the items view.
- [ ] When the list fails (e.g. an expired JWT), a red banner
      "Fetch failed: …" appears at the top of the content area instead
      of a wordlessly empty table (regression guard: `fetch_error` used
      not to be rendered anywhere).
- [ ] The pagination footer shows the total count, and paging via
      Next/Prev works (provided there is more than one default page).
- [ ] Sort-hint mode (`S`) shows the notification sort columns (created,
      event, project, actor, read, subject); switching asc/desc sorts
      in place without a reload.
- [ ] `m` on an unread row → the status bar shows "Notification #X
      marked as read", the view reloads, the Read value of that row
      switches to read; another `m` on an already-read row →
      "Action `mark_as_read` not exposed by node" (or similar; no API
      call).
- [ ] `e` (open ticket) on a notification → the edit editor opens for
      the **linked** ticket (not for the notification); saving acts on
      the ticket; closing returns to the notifications list.
- [ ] `e` on a notification with an unknown `content_type` (e.g.
      `wiki_page`) → a notification saying the `target_id` is empty (the
      action is a no-op rather than a crash).
- [ ] `:invalidate-session` on the notifications subtab fires
      identically to the items subtab; the next list loads through after
      the re-auth.

## Postgres adapter — phase A (databases only)

Precondition: `~/.config/not_yet_done/views/postgres.yaml` from the repo
(which defines the Postgres tab with the databases subtab) plus a
hand-maintained `~/.config/not_yet_done/views/postgres-adapter.yaml`
with a transport block and a postgres block. Example skeleton:

```yaml
# Optional: hard deadline for each postgres call. On timeout, the
# session + transport are torn down and the next call reconnects
# lazily. Omit for libpq-style "wait forever".
query_timeout_secs: 7

transport:
  mode: ssh_tunnel # or: direct
  # `ssh` is a list of hops. The first entry connects via local TCP;
  # every subsequent entry runs a fresh SSH handshake over a
  # direct-tcpip channel of the previous hop, so its `host:port` is
  # resolved by the predecessor (e.g. `localhost:2222` on hop #2 means
  # localhost relative to hop #1). The **last** hop opens the
  # direct-tcpip channel to `target`.
  ssh:
    - host: bastion.example.invalid
      port: 22
      user: alice
      auth:
        kind: public_key # or: password | agent
        identity: ~/.ssh/id_ed25519
        passphrase: # optional, only if key is encrypted
          type: command
          script: pass ssh/bastion-key
    # Optional second hop (DBeaver-style jump server). Drop this entry
    # for a single-hop tunnel.
    - host: localhost
      port: 2222
      user: someone
      auth:
        kind: password
        password:
          type: keyring
          service: nyd-ssh-jump
          account: someone
  target:
    host: db.internal.invalid
    port: 5432

postgres:
  user: dbuser
  password:
    type: keyring
    service: nyd-postgres-prod
    account: dbuser
  admin_database: postgres # optional; default `postgres`
  sslmode: prefer # optional; one of disable | prefer | require
```

- [ ] **Direct mode**: `mode: direct`, with `target` pointing straight
      at a locally reachable Postgres → the Postgres tab appears, the
      databases subtab loads; the Name / Owner / Encoding columns are
      filled; the templates (`template0`, `template1`) are not included;
      sorted alphabetically.
- [ ] **SSH tunnel mode with `kind: agent`**: `ssh-add -l` shows at
      least one identity → the tab loads without a password prompt,
      `ss -lntp | grep 127.0.0.1:` shows an ephemeral listener, and the
      databases subtab loads through. Closing the tab makes the listener
      disappear.
- [ ] **SSH tunnel with `kind: public_key`** and an encrypted key: the
      `passphrase` provider fires exactly once (e.g. one `pass` call),
      and subsequent listings use the cached session (no second call).
- [ ] **SSH tunnel with `kind: password`** (bastion) and
      `password.type: keyring`: the login pulls the SSH password from
      the keyring; a wrong password → the tab shows the error banner
      "ssh auth failed: server rejected credentials" instead of an empty
      list.
- [ ] **Two-hop chain (jump server, DBeaver equivalent)**: `ssh:`
      contains two entries — e.g. a public key on hop #1 and a password
      on hop #2. Opening the tab authenticates both hops; on the bastion,
      `ss -tnp | grep <hop2-port>` shows an outgoing connect from the
      forked sshd process. A wrong password on hop #2 → banner "ssh auth
      failed (hop #1): …" (index 1 = the second entry). Hop #1 OK plus a
      wrong hop #2 host → banner "ssh channel error: open hop #1 (…): …".
- [ ] Faulty Postgres auth (e.g. a wrong `postgres.password`): the tab
      shows the error banner "postgres connect: password authentication
      failed for user …"; after correcting it and reloading, the list
      loads.
- [ ] Tunnel drop under load: cut the SSH session from the server (e.g.
      `pkill -f "sshd: alice"`) → the next reload reconnects silently
      (lazy reconnect in `PostgresClient`).
- [ ] **`query_timeout_secs: N` (adapter top level, optional)**: with
      e.g. `query_timeout_secs: 7` in `views/postgres-adapter.yaml`.
      Simulate a half-closed tunnel (e.g. a local iptables DROP rule on
      the forward port, or pause the bastion sshd process with
      `kill -STOP`). On the next reload the banner shows a countdown
      `… (0s/7s) → (1s/7s) → …`. After 7 s: the error banner
      "… : timed out after 7s; connection reset", followed by `Ready`.
      Another `r` → a fresh tunnel and session are built up lazily.
- [ ] **View retries (`retries: N` on a view in `views/postgres.yaml`
      or similar)**: with `retries: 2` and `query_timeout_secs: 7`.
      Block the tunnel as above. Expected: the banner shows the
      `Connecting/Busy` countdown of the 1st attempt; after 7 s the text
      flips to "Retrying (2/3) — list databases (0s/7s): … timed out
      after 7s; connection reset"; after another 7 s, "Retrying (3/3) —
      …"; after ~21 s in total the error becomes sticky as a
      "Fetch failed: …" banner and the retry status disappears. Release
      the tunnel, then `r` → the successful reload clears `fetch_error`.
- [ ] **`retries: 0` (the default)**: the same setup without
      `retries:` — the banner shows only one attempt and goes straight
      to "Fetch failed: …" without any `Retrying`.
- [ ] **Manual connect (`adapter.auto_connect: never`, the default)**:
      in `views/postgres.yaml`, either omit `auto_connect` in the
      `adapter:` block **or** set it to `never`. Start the TUI. Expected:
      the Postgres tab immediately shows the banner "Auto-connect
      disabled — press `r` to connect"; **no** connection attempts, no
      timeouts in the logs and no waiting. Switching subtabs with
      `d`/`t`/`s` also triggers **no** load — every subtab shows the same
      banner. The first `r` starts the first load (the banner becomes
      the `Connecting/Busy` countdown). After a successful load the
      banner disappears and normal behaviour is restored. Switching to
      an already-loaded subtab shows the cache; switching to one that
      has not been loaded shows "Auto-connect disabled …" again.
- [ ] **Manual connect without a reload action**: on a view with
      `auto_connect: never` but **without** a `type: reload` in
      `actions:`. The banner reads "Auto-connect disabled — no `reload`
      action configured for this view"; the tab stays empty permanently
      (a soft error, no crash).
- [ ] **Manual connect off**: `auto_connect: startup` → the tab loads
      automatically at start; no regression.
- [ ] **Without `query_timeout_secs`**: the previous behaviour — the
      banner shows only the elapsed time (`… (3s)`), no auto reset.
      (Optional, for regression only.)
- [ ] The databases subtab has **no** actions (only the default reload
      via `r` if that is bound globally); `e` / `c` / `a` show "Action …
      not exposed by node".
- [ ] `transport.mode: ssh_tunnel` without an `ssh:` block → the tab
      appears with a red banner "Invalid Postgres config:
      transport.mode=ssh_tunnel requires an `ssh:` block"; the list is
      empty.
- [ ] An unknown field under `postgres:` (e.g. `schema:`) → the tab
      stays empty with the banner "Invalid Postgres config: unknown
      field `schema` …" instead of silently accepting it.

### Postgres — table rows drill-down (`o` → split right)

Precondition: `views/postgres.yaml` contains the `Rows` child with
`key: o`, `split: right ratio 0.8`, `pagination: server page_size 100`.
The drill-down uses `query_rows` with `ORDER BY ctid` and
`LIMIT/OFFSET`.

- [ ] Press `o` on a table row → a new pane opens on the right
      (ratio 1:4: narrow on the left, wide on the right), accent border
      on the right pane, dimmed border on the left. The right pane loads
      up to 100 rows.
- [ ] Columns are derived from the data automatically (no `columns:`
      entry in the YAML), all of them equally wide, the header shows the
      Postgres column names.
- [ ] NULL values appear as `(null)`; non-text columns (for example
      `int`, `timestamptz`, `bool`, `jsonb`) are rendered as text
      correctly.
- [ ] `>` loads the next block of 100 (the footer shows `100–199`), `<`
      goes back to the previous one. This also works on a table with more
      than 200 rows.
- [ ] On a small table (< 100 rows): `>` is a no-op (no has_next).
- [ ] A table with an unusual column name (for example one containing
      `"`) → the query must not crash, the column is quoted correctly.
- [ ] `backspace` closes the right pane / drills back (depending on the
      pane position); the list of tables in the left pane is preserved.

## Split-Pane (Phase 2 + 3 + 4)

Phase 2 added manual splits, the defaults are now `wv`/`ws`/`wq`
(leader `w`, formerly `ctrl+w`). Phase 3 opens splits during a drill-down
via `split:` in the ChildDef. Phase 4 adds a letter tag per pane and a
`<leader><letter>` switcher. The `pane_tags` alphabet is filtered against
the action keys (v/s/q in the defaults) automatically so that nothing
collides.

### Phase 2 — manual splits

- [ ] Open any content tab (Jira/Taiga/Postgres) → `wv` → a second pane
      on the right, accent border on the right pane, dimmed border on the
      left one; a fetch fires for the new pane.
- [ ] `wq` on the right pane → it closes, the left pane gets the focus
      back, the border disappears (single leaf).
- [ ] `ws` → split at the bottom; navigate independently in each pane.
- [ ] Drill into a Postgres database in the left pane; split with `wv`;
      drill on differently in the right pane → both keep their own
      drill-down state.
- [ ] Switch subtabs (for example Taiga tickets ↔ notifications) while a
      split is open in the current subtab → the other subtab tree
      (single leaf) appears, the original split state is intact when
      switching back.
- [ ] The sort overlay (`S` in Jira/Taiga) only appears on the focused
      pane.
- [ ] Action bar / breadcrumb adapt to the focused pane after a split.
- [ ] Input mode guard: while a fuzzy filter (`f`), a search (`/`) or the
      cmdline (`:`) is active → `w` is written into the input field as an
      ordinary character, NOT interpreted as the leader (otherwise `w`
      could not be typed in the search text).

### Phase 3 — `split:` on a ChildDef

- [ ] Configure a `children:` entry with
      `split: { direction: right, ratio: 0.5 }` in a YAML view → the
      drill key (Enter / Open) opens the child level as a new pane to the
      right of the current one, focus on the new pane.
- [ ] `direction: bottom` → new pane at the bottom.
- [ ] `direction: left` / `direction: top` → new pane to the left of /
      above the source.
- [ ] `ratio: 0.7` → the new (drilled) pane gets 70 % of the space.
- [ ] Without `split:` → classic in-place drill-down (no new pane).
- [ ] Press the back key (`Esc`/`h`) in the new split pane → it returns
      to the parent list (the NavFrame holding the parent items was
      carried over to the new pane during the split drill).
- [ ] `wq` on the split-drill pane → closes back to the single-pane view;
      the source pane is unchanged.
- [ ] Adapter cache: split-drill twice in a row (close first, then again)
      → the second request is served from the cache immediately, no
      duplicate HTTP calls (the `Arc<dyn ContentAdapter>` is shared).

### Phase 4 — letter tags + `w<letter>`

Default alphabet `asdfghjkl`, auto-filtered against v/s/q (= the action
keys of the default window bindings) → effective alphabet `adfghjkl`.

- [ ] Single pane: no border, no visible tag.
- [ ] `wv` → split to the right. The left pane shows `a` at the top left
      of its border, the right one `d` (each styled: the focused pane in
      the accent colour and bold, the unfocused one dimmed). `s` is
      skipped because it is reserved as an action key.
- [ ] `wd` → the focus jumps to pane `d`; the border titles swap colours
      (`d` → bold/accent, `a` → dim). The action bar now shows the
      actions of pane `d`.
- [ ] Press `wv` once more in the focused pane → a third pane with tag
      `f` (the next free letter after `a` and `d`).
- [ ] `wa` → focus back on the first pane. Tag assignments stay stable
      (no re-layout of the letters).
- [ ] Close pane `d` (`wq`) → tag `d` is free again. Split the remaining
      pane `f` once more → the new pane gets `d` (recycled) instead of
      `g`.
- [ ] `w` followed by an unassigned letter (for example `z`, or a letter
      that is in the alphabet but carries no pane) → no action, the chord
      is aborted cleanly (`window_pending` back to `None`).
- [ ] Set `pane_tags: "qwert"` in `tui.yaml` → after a restart panes get
      their tags from `wert` (`q` is the close action and is filtered
      out).
- [ ] Change the `window:` bindings in `tui.yaml` to `gv`/`gs`/`gq` →
      after a restart the switcher works with leader `g` (`gv` = split
      right, `ga` = switch to pane `a`). The default alphabet is filtered
      against v/s/q (same filter output).
- [ ] Switching subtabs with active splits: the tag allocation of one
      tree does not affect the other (each tree has its own allocation,
      starting at `a`).

### Pane tags in the shortcut surfaces

- [ ] Split a Jira/Postgres/SQLite pane with `wv`, then `o s` (shortcut
      overview) → a `Window ...` section listing `w v`/`w s`/`w q`/`w
h`/`w l` **and** one row per pane (`w a`, `w d`, …) named after the
      pane it focuses; the pane you are in reads `(current)`.
- [ ] Drill the right pane into a child level → its row's name gains the
      level (`… › <level>`), and it stays put while the cursor moves
      within that pane.
- [ ] `o k` (shortcut menu) → the same rows in the context scope; try to
      rebind a `w <tag>` row → refused as read-only, while `w v` is
      rebindable as before.
- [ ] Hold `w` → the which-key popup is headed `Window ...` and lists the
      tags with their pane names.
- [ ] On a view **without** `window_ops: true` (for example Tasks) none
      of these rows appear in the overview, the menu's context scope or
      the popup.
- [ ] Unsplit pane → the static chords are listed, but no tag row.

### Phase 4 follow-up — chord precedence + action-bar mode

- [ ] **Chord precedence over other handlers**: a pane `s` (with tag `s`)
      cannot actually come into existence, because `s` is reserved as an
      action key — so try a subtab switch as a stand-in: in a view with
      subtab key `i` (Taiga **items**) press `w`, then `i` → the subtab
      does **not** change, the chord resolves cleanly as **unbound** (the
      action bar is normal again). Previously the subtab switch would
      have snatched the `i` away.
- [ ] With saved-query shortcuts configured (for example `1` loads "My
      Bugs"), press `w1` → no saved query is loaded, the chord is
      discarded.
- [ ] **Action-bar mode indicator**: press `w` → on the left the action
      bar shows, in bold and in the accent colour, the label `WINDOW`
      followed by the hints `v split right`, `s split down` and
      `q close pane` (with several panes open also `<a/d/…> switch
pane`).
- [ ] Resolve any chord (for example `wv`) → the action bar falls back to
      the normal hint list (without the `WINDOW` label).
- [ ] Press `w` while an input is active (`f` fuzzy / `/` search / `:`
      cmdline) → no `WINDOW` display, `w` is written into the input
      field.

## Coupled-Split (Phase 1)

Precondition: `~/.config/not_yet_done/views/postgres.yaml` sets
`split.coupled: true` on the `Rows` child. Parent pane = table list,
child pane = rows view.

- [ ] Focus the table list, press `o` on table T1 → the right split
      opens, loads the rows of T1, the focus is on the new rows pane.
- [ ] Switch back into the table pane (`wa` or similar), select another
      table T2, press `o` → the existing rows pane reloads onto T2 (no
      new split appears). The focus stays on the table pane.
- [ ] Several switches in a row (T1 → T2 → T3) → always the same rows
      pane, the columns adapt to the respective table (auto-derived
      columns).
- [ ] Press `wq` in the rows pane (close it manually) → the backlink in
      the table pane is released; the next `o` opens a new split again.
- [ ] Close the table pane (the parent) with `wq` → the rows pane (the
      child) is closed along with it (cascade), the focus moves to a
      remaining pane (for example the schema pane in a deeper drill
      hierarchy).
- [ ] Cascade collision guard: if only the two coupled panes are open
      (tables + rows, nothing else), `wq` on the table pane → the close
      is discarded (the tree would otherwise be empty); the user has to
      close the child manually first.
- [ ] A different ChildDef without `coupled: true` (or without `split:`)
      → behaviour unchanged; classic split drill or in-place drill-down.

## Action-Chains (Phase 2)

Precondition: a chain is defined at ChildDef level in a content-tab YAML
(for example `postgres.yaml`), such as:

```yaml
- name: Rows
  key: o
  node_type: "postgres:row"
  split: { direction: right, ratio: 0.8, coupled: true }
  action_chains:
    "ctrl+n":
      [window.focus_parent, common.list_next, content.open, window.focus_child]
    "ctrl+p":
      [window.focus_parent, common.list_prev, content.open, window.focus_child]
```

- [ ] `ctrl+n` in the coupled rows pane → the focus jumps to the parent
      pane (table/schema), the next row is selected, `content.open`
      hot-replaces the rows pane, and afterwards `window.focus_child`
      moves the focus back into the rows pane holding the data of the
      next row. The sequence feels atomic (no flicker).
- [ ] `ctrl+p` likewise, backwards.
- [ ] Add a global chain to `tui.yaml` under
      `key_bindings.action_chains:`, for example
      `"ctrl+]": [common.list_next]`. In every tab `ctrl+]` moves one
      step down — in tasks/trackings too, because the chain ends up in
      the global scope.
- [ ] A ChildDef chain overrides a global chain: define the same key in
      the ChildDef and globally → the ChildDef chain runs, the global one
      stays silent.
- [ ] Disable a chain on one level: `"ctrl+n": ~` in a ViewDef → in that
      subtab `ctrl+n` does nothing (even if a chain is defined globally;
      no fall-through).
- [ ] Abort on error: the chain consists of `window.focus_parent`
      followed by `content.open`, and no parent pane can be linked from
      the focus → a notification reading `chain ctrl+x: step N aborted:
…`, the following steps are not executed.
- [ ] Validation at config load: `[global.quit]` as a chain entry → the
      app start aborts with a `not chainable in V1` error.
- [ ] Validation at config load: `[content.warp]` as a chain entry → the
      app start aborts with an `unknown content action` error.
- [ ] Validation at config load: `[list_next]` (without the `common.`
      prefix) → the app start aborts with a `missing <section>.` error.
- [ ] Chain bindings do NOT take effect while a popup or a mode input is
      active (cmdline `:`, search `/`, fuzzy `f`, saved-query menu,
      adapter credentials popup). Type a chain key in between → the popup
      swallows it as usual.

## Column-Cursor

Per-view / per-ChildDef opt-in (`column_cursor: true`). It adds a column
selection on top of the row selection: the row via the `RowSelected`
style, the column via `ColumnSelected`, the intersecting cell via
`CellSelected`. Navigation: `ColumnLeft`/`ColumnRight` (default
`left`/`h`, `right`/`l`). Currently enabled for the Postgres `Rows`
level (and only there).

Precondition: `not-yet-done-tui` installed from the current `master`, and
`~/.config/not_yet_done/views/postgres.yaml` carries
`column_cursor: true` on the `Rows` child.

- [ ] Postgres tab → database → schema → table → `o` (rows). In the rows
      pane: row highlight as usual; on top of that the first column is
      highlighted; the cell at (row 0, column 0) is styled differently
      again where they intersect.
- [ ] `l` and `right` move the column cursor to the right; clamped at the
      last cell of the row (no wrap).
- [ ] `h` and `left` move to the left; clamped at 0.
- [ ] Coupled chain switch (`ctrl+n` / `ctrl+p`): after `content.open`
      followed by `window.focus_child` the column cursor is back at 0 (a
      fresh drill into a `column_cursor` child starts at 0).
- [ ] Drill back out (for example `backspace` from rows into tables): the
      column highlight disappears (tables has `column_cursor: false`).
- [ ] Drill back into rows: the column cursor sits at 0 (fresh drill;
      NavFrame restoration only applies if both levels have the cursor
      enabled).
- [ ] Page switch via `>` / `<` in the rows pane: the column position is
      preserved (for example cursor on column 3, page 2 still shows the
      cursor on column 3, provided the row has at least that many columns
      — clamped otherwise).
- [ ] Other views (tasks, trackings, Jira, Taiga): no column highlight.
      `h`/`l` do nothing (or behave according to whatever other user
      mappings exist, but in any case no column movement).
- [ ] User override in `tui.yaml` under `key_bindings.common`:
      `column_left: ["a"]` → `a` moves the column cursor; `left` / `h`
      are overridden.

## Configurable row nav (bug fix)

ContentView no longer routes `j`/`k`/`g`/`G` in a hardcoded way; all four
keys go through `CommonAction::ListNext/Prev/First/Last`.

- [ ] Set `key_bindings.common.list_next: ["x"]` in `tui.yaml` → in the
      Postgres tab and in tasks `x` now moves down; `j` and `down` no
      longer react (unless an additional default entry brings them back).
- [ ] Default without a user override: `j`/`k`/`down`/`up` work in all
      tabs (tasks, trackings, Postgres, Jira, Taiga).
- [ ] `h` no longer triggers a `back` action and `l` no longer triggers an
      `open` drill (unless the user brings them back via
      `key_bindings.content.back` or `.open`).

## Horizontal scroll (coupled to the column cursor)

When the column cursor moves to a column whose right edge lies outside
the pane width, the table scrolls horizontally along (snapping to the
column boundary). The indicators `‹` / `›` in the header row report
hidden columns on the left/right. Only active with `column_cursor: true`
(variant A, coupled to the existing flag).

Precondition: a Postgres tab with a table whose row width exceeds the
pane width — tables with many or with wide columns are suitable. The rows
pane of the default `postgres.yaml` split is 80 % wide; very wide tables
are enough, otherwise make the terminal narrower.

- [ ] Postgres → database → schema → table → `o` (rows): the cursor sits
      on the left at column 0; no `‹` indicator; `›` appears when columns
      on the right are hidden.
- [ ] Press `l` repeatedly: as soon as the cursor column runs out of the
      visible area on the right, the view scrolls along by one column
      (header and rows in sync). The cursor highlight stays fully
      visible.
- [ ] The left indicator `‹` shows up as soon as `scroll_col_offset > 0`
      and disappears again when scrolling back to 0.
- [ ] `h` back: the view scrolls back (snapping to the column boundary,
      no half columns). With the cursor at 0 the `‹` is gone.
- [ ] Coupled chain (`ctrl+n` / `ctrl+p`): after drilling onto a new row
      and `window.focus_child` the cursor starts at column 0 → the scroll
      is reset to 0.
- [ ] Drill back out (tables, without `column_cursor`): no indicator, no
      traces of scrolling — the table is rendered truncated as before.
- [ ] Drill back in (rows): cursor and scroll start fresh at 0.
- [ ] Page switch `>` / `<` with the cursor far to the right: the cursor
      position stays; the scroll offset is reset in `set_rows` and
      `view()` re-snaps immediately, so that the cursor column is visible
      again.
- [ ] Make the terminal very narrow (1–2 columns fit): cursor and scroll
      stay coherent, no crash, no empty render.
- [ ] Make the terminal very wide (all columns fit): no indicator, the
      scroll offset stays 0 or snaps back to 0.
- [ ] Views without `column_cursor` (tasks, trackings, Jira, Taiga, the
      list of databases): no `‹`/`›` indicators, no horizontal scroll,
      render behaviour identical to before the feature.

## Auto column sizing (Postgres rows)

For tables with a dynamic schema (the `current_columns` auto fallback —
above all Postgres rows) the adapter builds
`ColumnDef { sizing: "auto" }`. In the sizer,
`width = clamp(max(header_w, content_max), min, max)` applies with the
defaults `min=5`, `max=11`. Auto columns do not respect the pane budget —
horizontal scrolling catches the overflow.

- [ ] Postgres → drill through database → schema → table and land in a
      table with many columns. Symptom **before**: `2…  …  …  …  …`.
      **After**: every column shows either its header (if ≤ 11 chars) or
      its content up to 11 chars; no more `…` spam.
- [ ] A column with a header longer than 11 (for example
      `transaction_timestamp`): the header is cut to 11 chars
      (`transactio…` or similar, via fit_aligned), the content likewise.
- [ ] A column with a short header and very short content (`id` with the
      values `1`, `2`): the column is at least 5 characters wide (min
      floor).
- [ ] Horizontal scrolling takes effect when the columns do not fit into
      the pane in sum: the `‹`/`›` indicators appear, `l`/`h` (or the
      configured column-cursor keys) navigate across column boundaries.
- [ ] Per-column override in the YAML: set a column to
      `sizing: "auto(3, 30)"` in a `ChildDef.columns` list. The column
      respects the new bounds (min 3, max 30).
- [ ] Tables with explicitly configured columns (the database / schema /
      table lists with `sizing: "max"`): unchanged, no auto behaviour.
- [ ] Tables without `column_cursor` (tasks, Jira) stay unchanged — the
      auto fallback does not apply there (they all have `columns:`
      lists).

## Postgres query editor (`Q`)

Pressing `Q` on a Postgres table (drill-down
`Database → Schema → Tables → <table>`, at the rows level) opens an
external `$EDITOR` with a `.sql` file. Layout:

```
-- Scratch area: notes, helper SELECTs. Lines above the marker
-- below are ignored on every :w.

-- ▼ THIS SQL WILL BE EXECUTED ON SAVE ▼

SELECT * FROM "<schema>"."<table>";
```

On every `:w` the area **below** the marker is executed against the
adapter. Successful result sets replace the items in the active pane
(dynamic columns pick up the auto sizing). Errors land in the familiar
query error bar — the file is **not** modified. On `:wq` with a last run
that errored, the file is reopened with a comment banner block
(`-- ─── ERRORS ───`). Successfully executed buffers are persisted under
`<XDG_DATA_HOME>/not_yet_done/postgres/<instance_id>/queries/<schema>/<table>.sql`
(crash-resistant, survives a restart).

- [ ] Postgres tab → database → schema → tables → a table. The action bar
      shows `Q edit query` (or a comparable hint).
- [ ] `Q` opens `$EDITOR`. The default buffer contains the marker line
      plus `SELECT * FROM "<schema>"."<table>";`. The quotes used are
      `"…"`, schema and table identical to the drill-down source.
- [ ] `:w` without changes → the items are reloaded with the default
      `SELECT *`. The status bar shows `N row(s)` or similar.
- [ ] Change the query to `WHERE id = …` and `:w` → the pane filters down
      to the matching rows, the columns adapt.
- [ ] Introduce a syntax error (`SELEC * …`) and `:w` → the query error
      bar shows the Postgres error. The items in the pane stay unchanged.
      The file (in the editor) shows **no** banner yet.
- [ ] `:wq` with the broken query → the editor reopens with a leading
      `-- ─── ERRORS ───` block. The banner disappears on the next `:w`
      with a corrected query (no stacking).
- [ ] Multi-statement query, for example `BEGIN; UPDATE … ; ROLLBACK;`.
      On `:w` the status bar shows the result of the **last** statement
      (`ROLLBACK` → `0 row(s) affected`). With a final `SELECT * FROM …`
      its rows are rendered.
- [ ] `UPDATE …` without `RETURNING` as the only statement → the pane
      stays unchanged (no items), the status bar shows
      `<n> row(s) affected`. After another `:w` with a `SELECT` the
      updated items become visible.
- [ ] Remove the marker line from the buffer and `:w` → the **entire**
      buffer (including the "scratch" comment) is executed. Postgres
      ignores SQL comments, so effectively the `SELECT` runs. Write the
      marker back → the scratch protection is in effect again.
- [ ] Close the editor (`:wq` with a successful last run) → done, no
      reopen loop. The TUI returns to the result list.
- [ ] `Q` again on the same table: the persisted buffer (including the
      last `WHERE` clause) appears, **not** the default. The file path
      `<XDG_DATA_HOME>/not_yet_done/postgres/<instance_id>/queries/<schema>/<table>.sql`
      exists.
- [ ] `Q` on a **different** table: its own buffer, its own file. Table
      editors do not affect each other.
- [ ] At root level (the list of databases, no drill-down): `Q` does
      **not** trigger the SQL editor (the existing JQL/JSON editor path
      stays unchanged).
- [ ] Other adapters (Jira / Taiga): `Q` opens **no** SQL editor (either
      a notification "Adapter does not support custom queries" or the
      previous logic, depending on the level).

### Multi-instance (two Postgres tabs)

Precondition: two view configs in `~/.config/not_yet_done/views/` with
the same `adapter.type: postgres` but different `adapter.id:` (for
example `id: prod` and `id: staging`), which may both point at the same
database.

- [ ] App start: both tabs load, no duplicate-ID error.
- [ ] In tab `prod`, table `public.users` → `Q` → type a query of your
      own and `:w`.
- [ ] In tab `staging`, the same table `public.users` → `Q` → type a
      different query and `:w`. Both pane contents show their respective
      filter.
- [ ] On disk: two separate paths exist —
      `…/postgres/prod/queries/public/users.sql` and
      `…/postgres/staging/queries/public/users.sql`.
- [ ] Restart the app, press `Q` in both tabs → each loads the buffer
      last saved for that tab, no cross-talk.

## Tree mode (phases 0–7)

Precondition: `~/.config/not_yet_done/views/postgres.yaml` carries
`tree_label: name` on the `databases` view **and** on the ChildDefs
`Schema` and `Table`. `Rows` stays without a `tree_label` (leaf →
split). A reachable Postgres instance is configured.

### Render + expand/collapse (phases 1–3)

- [ ] App start, Postgres tab → `databases` subtab. Cursor on row 0,
      the glyph `▶` in front of the database name, all other columns
      (`Owner`, `Encoding`) filled — cursor level = root.
- [ ] `Enter` (or `l`) on a database → the glyph switches to `▼`, below
      it indented schema rows with their own `▶`. The header columns do
      **not** switch (the cursor is still on root).
- [ ] Move the cursor onto a schema row with `j` → the header switches to
      the schema columns (`Name`, `Owner`), the schema row now shows
      content in both columns. The database row above it only has content
      in the `Name` column (with `▼`), the other columns are empty.
- [ ] `Enter` on a schema row → the tables appear indented, the cursor
      stays on the schema and can move down onto a table with `j`.
- [ ] Cursor on a table → the header switches to the table columns
      (`Name`/`Owner`/`Rows (est.)`).
- [ ] `Enter` (or `l`) on a table row → a split pane on the right with
      the `Rows` content (the tree pane on the left is preserved). The
      tree pane keeps its cursor and its expanded set.
- [ ] Open a second subtree: expand another database while the first one
      is still open — both subtrees are visible at the same time.
- [ ] `Enter` on an expanded row (with `▼`) → it collapses, the children
      disappear, the glyph goes back to `▶`.

### Back key (phase 4)

- [ ] Cursor on a schema row below an expanded database → `gh` (or the
      configured back key) → the cursor jumps back to the database row
      **and** the database collapses (`▼` → `▶`, children gone).
- [ ] Cursor on a database row (depth 0) → `gh` → no-op (no window close,
      no pane close).

### Smart collapse on `backspace` (used to be `c`)

> Since the action-bar refactor the smart collapse sits on `backspace`
> (no longer on `c`), so that `c` opens the ColumnConfig popup
> **everywhere**. `backspace` is already the "one level back" gesture
> (`Back`) anyway: in tree mode `Back` collapses to the parent node,
> which deliberately overlaps with the smart collapse.

- [ ] Cursor on an expanded database row (with `▼`) → `backspace` → the
      database collapses (`▼` → `▶`, children gone), **the cursor stays
      on the same row**.
- [ ] Cursor on a schema row (depth 1) that is **not** expanded itself →
      `backspace` → the parent database collapses and the cursor jumps up
      to the database row.
- [ ] Cursor on an expanded schema row (depth 1, with `▼`) → `backspace`
      → the schema collapses, the cursor stays on the schema row.
- [ ] Cursor on a collapsed top-level database (depth 0, `▶`) →
      `backspace` → no-op (no window close, no beep).

### Action-bar active state + `c` = columns (refactor 2026-06-15)

- [ ] `c` opens the ColumnConfig popup **on every content tab** — in tree
      mode too (tasks/trackings/Postgres tree). The shortcut appears as
      `c columns` in the upper action bar.
- [ ] While a popup or mode is open, the matching top-bar hint lights up
      (accent, bold and underlined): `d delete` during the y/n
      confirmation, `q queries` during the query menu, `u group` during
      the group menu, `c columns` during the ColumnConfig popup,
      `e edit`/`a add` while the editor session is open, `J jump` in jump
      mode, `s track` while a tracking is running, `C cut` with an armed
      cut, and the script hint while a detached script is running.
- [ ] There is **nothing** left in the upper action bar that cannot be
      activated: Confluence's `o open in browser` and `d download` have
      moved down into the status bar.

### Pagination inside the tree (phase 5)

Precondition: a database with more schemas than the configured
`page_size` (or a schema with more tables than `page_size`).

- [ ] Expand a database with many schemas → below the loaded schemas the
      placeholder `… load more` appears as the last row (glyph `…`).
- [ ] Cursor on the placeholder → `Enter` loads the next page and appends
      it **above** the placeholder. If further pages are available, the
      placeholder stays visible as the last row; otherwise it disappears.
- [ ] Do not double-click while a pagination load is running — the
      placeholder must not be triggered twice (the cursor moves to the
      first newly loaded row beforehand).

### Filter / search in the tree (phase 6)

Precondition: the `databases` view has `actions.fuzzy_filter` (key `f`)
and `actions.search` (key `/`) — both defined on exactly ONE tree level
(a validator constraint).

- [ ] `f` at root level → type `pub` (or another snippet matching at
      least one database) → only matching databases are visible; already
      expanded subtrees of matching databases stay open. Databases that
      do not match disappear along with their children.
- [ ] With the filter active, now expand another database (its children
      are not in the cache yet) → the async load runs, new schemas
      appear. The filter is defined on the **root** level, so it does not
      apply to schemas (all schemas are shown).
- [ ] (Optional, if `fuzzy_filter` has been moved to the `Schema`
      ChildDef temporarily for the test) Filter active with a snippet
      matching only one schema → expand a new database → the newly loaded
      schemas are checked against the active filter, non-matching ones
      stay hidden. (Unit test:
      `tree_apply_children_respects_active_filter_at_load_time`.)
- [ ] Clear the filter (`Esc` / backspace until empty) → all rows are
      back.
- [ ] `/` search → type → the cursor jumps to the next matching row. The
      search skips pagination placeholders; n/N cycle through the
      matches.

### Keymap per cursor level (phase 7)

- [ ] At root level the action bar shows the actions of the `databases`
      view (at least `f` filter, `/` search).
- [ ] Move the cursor down onto a schema with `j` → the action bar
      switches to the schema-level actions (provided any are defined on
      the Schema ChildDef). The global actions (`fuzzy_filter`, `search`,
      `text_search`) of the root view stay available.
- [ ] Move the cursor further down onto a table → the action bar switches
      again.

### Refresh + action chains

- [ ] `r` on the tree pane (cursor at root) → the root list is reloaded,
      the expanded set is preserved (where possible).
- [ ] In split mode (tree on the left, rows on the right): `ctrl+n` /
      `ctrl+p` — the chain `window.focus_parent`, `common.list_next`,
      `content.open`, `window.focus_child` — navigates from the rows pane
      to the next row of the table above. The tree pane stays unchanged.

## Multi-tree continuation + DB-level scripts (MT-1 … MT-4)

Precondition: `~/.config/not_yet_done/views/postgres.yaml` has **two**
tree-continuing children below the `databases` view:

- `Schema` (`node_type: "postgres:schema"`, `tree_label: name`)
- `DB Script` (`node_type: "postgres:db_script"`, `tree_label: script`)

Both on the same level, with different `node_type`s → the validator
accepts them (rule 3 of MT-1a).

### Validator check (MT-1a)

- [ ] App start with the config above → no validator error, the tab loads
      normally.
- [ ] Copy two tree children with an **identical** `node_type` into one
      view (for example `node_type: "postgres:schema"` twice) → the app
      refuses the reload with the error **"ambiguous tree continuation —
      duplicate node_type 'postgres:schema' used by both tree-continuing
      children …"**.

### Multi-branch expand (MT-1b/c/d + MT-2)

Preparation: create at least one DB script by hand, so that the DB
scripts branch has something visible when expanded:

```sh
mkdir -p ~/.local/share/not_yet_done/postgres/<instance_id>/db_scripts/<db>
printf '%s\n%s\n%s\n' '-- scratch' '-- ▼ THIS SQL WILL BE EXECUTED ON SAVE ▼' 'SELECT 1;' \
  > ~/.local/share/not_yet_done/postgres/<instance_id>/db_scripts/<db>/hello.sql
```

- [ ] App start, Postgres tab → `databases` subtab → expand a database
      (`Enter`/`l`). **Two** branches appear below the database, in the
      order given in the YAML: schemas first, then the DB script `hello`.
      Each with its own `▶` glyph.
- [ ] During the load (briefly) → the banner shows **"loading"** until
      both branches are loaded. They only appear together, not one at a
      time.
- [ ] The branches follow the order of the YAML children (schema before
      DB script), regardless of which adapter call finished first.
- [ ] Expand the schema row further → the classic schema → table path
      still works.
- [ ] The DB script row is a leaf → no further `▶`, `Enter` does nothing
      (no drill-down defined).
- [ ] Collapse the database (`c` or `gh`) → **both** branches disappear.
- [ ] Expand the database again → both branches come back, same content.

### Mixed scripts subtab (MT-3)

- [ ] `s` (scripts subtab) → the list contains both table-level scripts
      (for example `default` from `queries/<db>/<schema>/<table>/`) and
      DB-level scripts (for example `hello` from `db_scripts/<db>/`).
- [ ] A DB-level row has the `Schema`/`Table` columns **empty**,
      `Database` and `Script` are filled.
- [ ] A table-level row has all four columns filled.

### Storage layout (MT-2)

- [ ] On disk: DB scripts live under
      `<instance_data_dir>/db_scripts/<db>/<script>.sql`, separate from
      the existing `queries/<db>/<schema>/<table>/<script>.sql`.
- [ ] Non-`.sql` files in the DB scripts directory (`notes.txt` or
      similar) are ignored, no crash.
- [ ] A missing `db_scripts/` directory ⇒ an empty DB scripts branch (no
      error).

## Cursor pagination & per-node actions (CP-1 … CP-9)

Precondition: a configured Postgres tab with the DB scripts branch as in
[multi-tree continuation](#multi-tree-continuation--db-level-scripts-mt-1--mt-4).
The per-node actions are set in `postgres.yaml`:

```yaml
- name: Scripts
  node_type: "postgres:db_scripts"
  actions:
    - { key: a, id: add }
  children:
    - name: DB Script
      node_type: "postgres:db_script"
      actions:
        - { key: X, id: execute }
        - { key: e, id: edit }
        - { key: d, id: delete }
      children:
        - name: DB Script Result
          node_type: "postgres:db_script_result"
          split: { direction: right, ratio: 0.8, coupled: true }
          pagination: { mode: cursor, page_size: 100 }
          column_cursor: true
          keybindings: { back: null }
```

### Add + edit (CP-9)

- [ ] Put the cursor on the **Scripts** group node (or directly on an
      existing DB script entry) and press `a` → the cmdline opens
      pre-filled with `:db-script-new <database> ` (with a trailing
      space). The user only types the name and presses Enter → a
      notification "Created DB script '<name>'", the editor opens on the
      new file automatically.
- [ ] The file ends up at
      `<instance_data_dir>/db_scripts/<database>/<name>.sql` with the
      default template (the scratch note, the marker line reading
      "THIS SQL WILL BE EXECUTED ON SAVE" between the two `▼`, and
      `SELECT 1;` as the body).
- [ ] `:db-script-new` with an invalid name (containing `/`, `\`,
      whitespace, a leading `.`, or empty) → a modal error, no file is
      created.
- [ ] `:db-script-new <db> <existing-name>` → a modal error "already
      exists"; the file on disk is unchanged.

### Execute + cursor result pane (CP-8 + CP-4 … CP-6)

- [ ] Press `x` on a DB script → a result pane opens next to it on the
      right (80/20 split). The columns follow dynamically from the
      SELECT.
- [ ] Build a body with `INSERT … SELECT * FROM generate_series(1, 500)`
      or similar, then a SELECT, `x` → the first 100 rows appear. `>` →
      the next 100. `<` → the cursor is re-opened at the start (NO
      SCROLL).
- [ ] Multi-statement body: a `CREATE TEMP TABLE t(x int)`, then an
      `INSERT INTO t VALUES (1),(2),(3)`, then `SELECT * FROM t;` → it
      runs and paginates over the final SELECT.
- [ ] DDL-only body (`VACUUM`, `ANALYZE`) → currently a notification
      "unpaged ExecuteQuery not implemented yet" (CP-9 not shipped).
- [ ] Close the result pane (`wq`/`Esc` on the pane) →
      `pg_stat_activity` shows **no** leftover idle-in-tx entry with the
      cursor statement.
- [ ] Start another long-running query while a cursor pane is open → the
      timeout (`query_timeout_secs`) tears down the whole pool; on the
      next `>` the cursor pane shows a "cursor lost" banner (no crash).
- [ ] Press `Enter` on a DB script (instead of `x`) → identical
      behaviour to `x`: the result pane opens, pagination works. This
      works both in flat mode (the scripts subtab) and in tree mode
      (databases → database → DB scripts → script row). Regression bait:
      before the fix the synthetic child `postgres:db_script_result` was
      reached through the generic drill path and ended in a
      `Fetch failed` error saying the node type
      `postgres:db_script_result` was not available. The routing now goes
      through `enter_action: execute` on the `DB Script` ChildDef in
      `postgres.yaml`. On top of that, `current_children` in tree mode
      now uses the `node_type_chain` of the selected row (instead of
      walking the first chain), otherwise the split would branch off into
      the wrong branch (for example schemas → schema → table).

### Edit (CP-8)

- [ ] Press `e` on a DB script → the SQL editor opens with the stored
      body. `:w` persists it **without** re-executing; a result pane that
      may be open does not change. The user has to press `x` explicitly
      to see the new version.
- [ ] Press `e` in **tree mode** as well (via `databases` → database
      expanded → the `DB Scripts` branch expanded → cursor on an
      individual script row) → the same editor opens. During the brief
      adapter pre-query the status bar may show the
      `list databases (Ns/Ms)` busy banner (that is expected —
      `get_by_id` validates the database name); the editor opens
      **afterwards**, not instead. Regression bait: before the fix the
      `EditorRequest` returned by the async dispatch was swallowed in
      `poll_load`.

### Delete (CP-9)

- [ ] Press `d` on a DB script → a confirmation popup "Delete DB script
      '<name>' in database '<db>'? (y/n)". `y` → a notification "Deleted
      DB script '<name>'", the row disappears (the pane is reloaded), the
      file is removed from disk.
- [ ] Pressing `d` again on an already deleted or missing file is
      idempotent — no error, the notification appears anyway.
- [ ] `n`/`Esc` in the confirmation popup → the file stays, the row
      stays visible.

### DB script folders (DSF)

Preconditions: a Postgres tab, tree mode (`d` on the tab), a database
expanded → the `Scripts` branch visible. `postgres.yaml` contains the DSF
cutover (DB Script Dir plus `recursive: true`). User config:
`~/.config/not_yet_done/views/postgres.yaml`.

#### DSF-1/2 adapter

- [ ] `a` on `Scripts` → the cmdline is pre-filled with
      `db-script new ` — type a script name and press Enter → a
      notification "Created DB script '<name>'", a new row appears below
      Scripts.
- [ ] `A` on `Scripts` → the cmdline shows `db-script new-dir ` — type a
      folder name and press Enter → a notification "Created DB-script
      folder '<name>'", a new folder row with a `▶` glyph appears below
      Scripts.
- [ ] Expand the folder row with `Enter`/`l` → empty (no children). `a` →
      the cmdline shows `db-script new ` — create a script below the
      folder. Filesystem check:
      `<instance_data_dir>/db_scripts/<db>/<folder>/<script>.sql`.

#### DSF-3 recursive ChildDef

- [ ] Folder row → `A` → create a new subfolder. The subfolder is
      expandable itself (`▶`). `A` inside it → one more level. Depths
      ≥ 3 work without a YAML change — `recursive: true` on the
      `DB Script Dir` ChildDef makes it its own tree-continuing child.

#### DSF-4 mark/paste move

- [ ] `m` on a script → a status-bar pill "⚓ marked: move:
      <db>/db_scripts/.../script" plus a notification "Marked '...' for
      move".
- [ ] Cursor on a folder row → `p` → a notification "Moved '<src>' →
      '<dst>' in <db>"; the source row disappears from its old parent,
      the folder row now contains the moved script. The pill disappears.
- [ ] Esc after `m` → a notification "DB-script move cancelled", the pill
      is gone.
- [ ] Mark a folder with `m` and `p` onto another folder → the whole
      folder subtree is moved (recursively, with its content).
- [ ] Attempt a cross-database paste (mark a script in DB1, paste it into
      DB2) → the error notification "Cross-database move not supported
      (DB1 → DB2)". The mark is kept, so that the user can pick a
      suitable target.

#### DSF-4 delete dir

- [ ] `d` on an empty folder row → a confirmation popup "Delete empty
      DB-script folder '<rel_path>' in '<db>'? (y/n)". `y` → a
      notification "Deleted DB-script folder '<rel_path>'", the row
      disappears.
- [ ] `d` on a **non-empty** folder row → confirm with `y` → the error
      notification "Delete folder failed: not empty (N entries)". The
      folder and its content stay unchanged.
- [ ] `n`/`Esc` in the confirmation → the folder stays.

#### DSF-5 cmdline namespace

- [ ] `:db-script` without a subcommand → the modal error "expects a
      subcommand (new | new-dir | rename | move | delete)".
- [ ] `:db-script unknown` → a modal error listing the valid
      subcommands.
- [ ] `:db-script new` without a name → the modal error "expects
      <name>".
- [ ] `:db-script rename foo/bar` → the modal error "invalid name
      'foo/bar' (no slashes or leading dot)".
- [ ] `:db-script move /` (absolute root) on a marked script → moves the
      script to the root of the `db_scripts/<db>/` directory.
- [ ] `:db-script move foo` (relative) with the cursor in the folder
      `bar` → the target directory becomes `bar/foo`.
- [ ] `:db-script delete` with the cursor on the `Scripts` group row →
      the modal error "selected row is the group node".

#### DSF-3 validator

- [ ] Set `recursive: true` without a `tree_label` in `postgres.yaml` and
      reload the tab → the validator error "recursive: true requires
      tree_label" (granular reload, the previous state stays active until
      it is fixed).

### DB-Script Table-Name Completions (TC-1 … TC-5)

Condition: a Postgres tab, at least one database with a few base tables
(`pg_class.relkind = 'r'`, schemas other than
`pg_catalog`/`information_schema`).

- [ ] Press `e` on a DB script → the editor opens. At the end of the
      buffer there is a single line of the form
      `-- table completions: tt_public__users, tt_public__orders, …`.
      Order: alphabetical by `(schema, table)`. No table from
      `pg_catalog`/`information_schema`/`pg_…` is listed.
- [ ] Copy a token or type it by hand: write
      `SELECT * FROM tt_public__users;` above the `QUERY_MARKER`, `:w`,
      then `x` on the row → the result pane shows the rows of
      `public.users`. The substitution replaced `tt_public__users` with
      `"public"."users"`.
- [ ] A table with a single underscore in its name (for example
      `user_orders`) verifies the boundary match:
      `tt_public__user_orders` becomes `"public"."user_orders"`.
      `tt_public__user` (if it exists) would **not** partially consume
      the `user_orders` token (the regex `\b` at `_` boundaries).
- [ ] An unknown token (`tt_xxx__yyy` with a non-existent table) stays
      unchanged. Postgres reports a syntax error at or near
      `"tt_xxx__yyy"` — the literal token is visible in the error banner,
      so that the user spots the typo immediately.
- [ ] `:w` without a change → the file on disk contains **no**
      `-- table completions:` line (verify with `cat` on
      `<instance_data_dir>/db_scripts/<db>/<script>.sql`). When opening
      it again with `e` the completion block is at the end again —
      regenerated from the current table list, not read from the file.
- [ ] Change or delete the completion line manually in the editor, then
      `:w` → that does not affect the persistence (the strip works on the
      prefix match `-- table completions: `). The next open shows the
      freshly computed line.
- [ ] An adapter without tables (an empty database) → the completion line
      is not appended at all (no orphaned header). The editor opens as
      usual.
- [ ] The substitution only fires if the query body contains `tt_` (fast
      path) — the steps above must not trigger a measurable extra round
      trip against the database when the user does not use tokens.
      Regression bait: before the feature there was no `tt_` path at all,
      the adapter executed the query verbatim.

### Node action resolver (CP-1)

- [ ] Press `Q` in a rows pane (`postgres:row`) → it opens the Q SQL
      editor of the parent table node (via
      `- { key: Q, id: edit_sql, target: parent }`).
- [ ] Keys that are bound by no action at all pass through as before
      (cursor movement, and so on).
- [ ] YAML with an empty action ID (`- { key: x, id: "" }`) or with two
      `actions:` entries on the same key → a validator error on reload,
      the tab goes into the broken state.

## Cross-app Linking (L1–L11)

Preconditions: at least one configured Jira view and one Taiga view, a
few tasks in the tasks database. Default key bindings for the actions
(all under the `gl` prefix): `glm` (mark), `glp` (paste), `glo` (open the
popup), `glb` / `glf` (jump back/forward), `:linkprune` (cmdline).

### Mark + paste (L5/L6)

- [ ] Cursor on a task in the tasks tab → `glm` → on the left the status
      bar shows the pill "⚓ marked: tasks/<uuid>"; a notification "Link
      mark armed: tasks/<uuid>".
- [ ] Switch tabs (to Jira, say) → the pill stays visible.
- [ ] On an issue in Jira → `glp` → a notification "Linked:
      jira/<inst>/<KEY> → tasks/<uuid>"; a database check with
      `select * from link;` shows the row.
- [ ] `glp` again on another Jira row → a second link row, the mark is
      kept.
- [ ] `Esc` (outside of popups and modals) → the pill disappears, a
      notification "Link mark cleared".
- [ ] `glp` without a mark → "No link mark armed (press M on a row
      first)"; no database write. (The notification wording is left over
      from L5, before the rebind; the behaviour is correct.)
- [ ] `glm` and `glp` on the same row → "Cannot link a node to itself";
      no database write.
- [ ] Postgres tab plus `glm` → "Nothing to mark for linking" (Postgres
      has no stable IDs).

### The `glo` popup (L7)

- [ ] `glo` on the Jira row pasted earlier → the popup "Links ·
      jira/…"; one line "← tasks/<uuid>" (incoming).
- [ ] `Enter` on it → the tab jumps to tasks, the pasted task is
      focused.
- [ ] `glo` there again → the popup shows "→ jira/<inst>/<KEY>"
      (outgoing).
- [ ] Typing filters the list; `↑`/`↓` move; `Esc` closes; `d` deletes
      the selected entry and refreshes the popup. If nothing is left →
      the popup closes and a notification "No more links for this node"
      appears.
- [ ] `glo` on a row without links → a notification "No links for this
      node".

### Stale link confirmation (L8)

Preparation: insert a broken link by hand, for example

```sh
sqlite3 ~/.local/share/not_yet_done/not_yet_done.db <<SQL
INSERT INTO link (id, source_ref, target_ref, created_at)
VALUES (lower(hex(randomblob(4)))||'-1111-1111-1111-111111111111',
        'tasks/<live-task-uuid>',
        'jira/<live-inst>/MISSING-9999',
        datetime('now'));
SQL
```

- [ ] `glo` on the live task → the popup shows
      "→ jira/<inst>/MISSING-9999".
- [ ] `Enter` → a confirmation modal reading "Stale link …", then on its
      own line either "no content tab …" or "Stale: ticket not found …"
      or similar, and finally "Delete from link table? (y/n)".
- [ ] `y` → a notification "Stale link deleted", the database row is
      gone, the `🔗` marker disappears on the next rebuild.
- [ ] With a second stale insert (for example
      `target_ref = 'nope/whatever'`): `glo` and `Enter` → the
      confirmation modal, `n` (or any other key) → a notification
      "Cancelled"; the row stays.
- [ ] With `target_ref = 'postgres/main/qrow:1'`: `glo` and `Enter` →
      **no** confirmation modal, but a notification "Link open failed:
      …NotSupported…" instead (Postgres is NotSupported in v1, not
      stale).

### Has-links column (L9)

- [ ] After a successful mark and paste above: in the tasks tab the `🔗`
      column carries a check mark for the linked task, other tasks do
      not.
- [ ] Trackings tab: running and historical trackings of **that** task
      show `🔗` as well (a fallback via `tasks/<uuid>`).
- [ ] Trackings tab in tree mode: the task node lights up, so does the
      entry node (the same fallback lookup).
- [ ] A Jira view without a `source: has_links` column → nothing
      changes.
- [ ] Add a column to an issue list in
      `~/.config/not_yet_done/views/jira.yaml`:

  ```yaml
  - key: links
    label: "🔗"
    source: has_links
    sizing: fixed(2)
  ```

  Restart the TUI → the linked Jira issue shows `🔗`, others do not.

- [ ] `glo` and `d` on the linked row → the `🔗` marker disappears on
      the next rebuild (a tab switch is enough).
- [ ] Postgres tab: the `🔗` column never lights up, no matter whether a
      stale row sits in the database (Postgres is excluded by design).

### Jump history `glb` / `glf` (L10)

- [ ] `glo` in the tasks tab → `Enter` on an outgoing Jira link → the
      tab switches to Jira, the issue is focused, there is no
      notification (no explicit "jumped" toast — intentional).
- [ ] `glb` → the tab switches back to tasks, the cursor sits on the
      original task, a notification "← tasks/<uuid>".
- [ ] `glf` → the tab switches to Jira again, the issue is focused, a
      notification "→ jira/<inst>/<KEY>".
- [ ] `glb` from the "initial state" (no link jumps made) → a
      notification "No back-history"; no tab switch.
- [ ] Several hops: `glo`/`Enter` from A→B, then from B→C → `glb` goes
      back to B, another `glb` back to A; `glf` twice brings C back.
- [ ] After `glb` to B: perform a **new** link jump B→D there → the
      forward branch (C) is discarded, `glf` from D no longer brings C
      ("No forward-history").
- [ ] Switching tabs via `1`/`2`/`3` pushes **nothing** into the history.
- [ ] A stale jump (a previously linked issue deleted on the server, or
      an adapter instance renamed): `glb` onto it → a notification
      "Back-jump failed: …", the entry is discarded from the stack (the
      next `glb` picks up the entry below it).

### `:linkprune` (L11)

Preparation: 2–3 live links plus at least one obviously stale one (as in
L8, say, or a tasks link onto a soft-deleted task).

- [ ] `:` → the cmdline opens, type `linkprune`, press `Enter`.
- [ ] A modal appears: "N of M link(s) are stale:", then one line per
      link of the form "tasks/… → jira/… (reason)", and finally "Delete
      all? (y/n)"; the list contains at most 5 sample refs, followed by
      "… and X more".
- [ ] `y` → the modal closes, a notification "Pruned N stale link(s)";
      the database check shows only the live links; the `🔗` column
      updates immediately (no tab switch needed).
- [ ] `:linkprune` again → the modal "Scanned M link(s). None are
      stale." (with M = the current live count).
- [ ] With an empty link table: `:linkprune` → the modal "No links in
      the database.".
- [ ] `:linkprune extra-arg` → the modal ":linkprune takes no
      arguments", no action.
- [ ] With a deliberately broken database connection (or the `link_repo`
      offline): `:linkprune` → the modal "link scan failed: …", no
      confirmation modal, no delete.
- [ ] Soft-deleted tasks and trackings count as stale: delete a task
      (lower-case `d`) → `:linkprune` lists the links pointing at it;
      after `y` they are gone.

### Deep link into a collapsed subtree (Confluence)

Background: a link points at a page that is not loaded at the moment. The
host then asks the adapter via `locate_node_path` where the node sits and
expands the path. Confluence returns
`[<space key>, <ancestor ids…>, <page id>]` for that — exactly the path
shape that `tree find` uses as well.

Precondition: a Confluence tab in tree mode, a page at least two levels
below the space home page.

- [ ] Navigate to a deep Confluence page, `glm` → the pill "⚓ marked:
      confluence/<inst>/<pageid>" (Confluence has stable IDs, so **no**
      "Nothing to mark" as with Postgres/SQLite).
- [ ] Onto a task in tasks → `glp` → the link is created.
- [ ] Confluence tab: **collapse everything** (`zm`) and switch tabs, so
      that the target row is definitely not loaded.
- [ ] `glo` on the task → `Enter` → the Confluence tab becomes active,
      the space and all ancestor pages expand, the cursor sits on the
      target page. No "is not among the loaded rows".
- [ ] Counter-check with a flat view: switch to a subtab without tree
      mode in the Confluence tab, then follow the same link → a
      notification "… is not on the current page (flat view — no path to
      expand)".
- [ ] Space whitelist: set `space_keys` in
      `views/confluence-adapter.yaml` so that the space of the target
      page is **missing** → restart the TUI, follow the link → no
      expansion, a notification "…can't locate it" (the tree has no node
      there). Reset the whitelist afterwards.
- [ ] Move the page to the trash in the Confluence web UI → follow the
      link → the stale confirmation modal (not NotSupported), `n` leaves
      the row in place.
- [ ] A comment as the target: `glm` on a comment of a deep page → link
      it → follow it → the path ends on the page, the comment itself is
      only focused if the subtab shows comments as children.

## In-app config editor (`:config`)

A cross-cutting feature: open the YAML configs under
`~/.config/not_yet_done/` in the external `$EDITOR` and reload them
in-process after saving — without restarting the TUI.

### 1. Fuzzy picker

- [ ] `:config` opens a SearchablePopup with all `*.yaml` files
      (recursively) under `~/.config/not_yet_done/`. The labels are
      relative to the config root (for example `tui.yaml`,
      `views/jira.yaml`, `views/jira-adapter.yaml`).
- [ ] Typing filters the list (fuzzy, the same matcher as the `gl`/`gs`
      popups).
- [ ] `Enter` opens the selected file in `$EDITOR`. `Esc` closes the
      picker.
- [ ] `:config jira` (with an argument) opens the picker list already
      filtered; if the argument matches exactly one file, the editor
      opens directly without the intermediate step.

### 2. Edit + save + reload — granular (view YAML)

- [ ] Edit a view YAML (for example change `tab.name` in
      `views/jira.yaml`), save and close `$EDITOR`.
- [ ] The notification "Reloaded view jira.yaml" appears.
- [ ] The tab name in the tab bar shows the new value.
- [ ] Other tabs (tasks, trackings, Taiga, Postgres) are unchanged — no
      data lost, no cursor jumps.

### 3. Edit + save + reload — full (tui.yaml / adapter YAML)

- [ ] Edit `tui.yaml` (change a theme colour, say) and save.
- [ ] The notification "tui.yaml reloaded"; the new theme takes effect
      immediately.
- [ ] Tasks and trackings tab: the data is reloaded (spawn_load ran), the
      selection is back at the default — accepted.
- [ ] Edit an adapter YAML (for example change the subdomain in
      `views/jira-adapter.yaml`) and save.
- [ ] The notification "All views reloaded after views/jira-adapter.yaml
      change"; the Jira tab uses the new adapter immediately (a new auth
      domain, say).

### 4. Failure case — parse error

- [ ] Insert an unclosed bracket into a YAML (`queries: [unclosed`) and
      save.
- [ ] The editor does **not** close — it is reopened immediately with the
      same buffer, an error banner above it (`# ─── ERRORS ───` …
      `# • YAML parse: …` … `# ─────────────────`).
- [ ] The file on disk was **not** overwritten (the old content stays).
      The old config keeps running normally.
- [ ] Leave the banner block in place and save a second time → only one
      banner block remains visible at the end (no stacking).
- [ ] Fix the error and save → the editor closes, the reload message
      appears as normal.

### 5. Failure case — validation error (semantic)

- [ ] Set `adapter.type: nonexistent` in `views/jira.yaml` and save.
- [ ] The file is written to disk (unlike with the parse error — the
      syntax was fine after all).
- [ ] The editor opens again with an error banner reading "Reload …
      failed: no adapter factory registered for type 'nonexistent'".
- [ ] In the background: the Jira tab **keeps running with the old
      config** (the old adapter is active). The user can tab over and see
      that nothing was lost.

## Cmdline shortcuts (`cmdline_shortcuts:`)

In `tui.yaml`, arbitrary `:command` strings can be bound to individual
keys or chords — without going through the `:` prompt.

```yaml
cmdline_shortcuts:
  F2: "config tui"
  "<c-comma>": "config"
```

- [ ] With the entry above: `F2` opens `tui.yaml` in the editor
      immediately, `Ctrl+,` opens the `:config` picker.
- [ ] A shortcut only fires if the key binds **no** typed action —
      standard keys (`q`, `j`, `k`, …) stay unchanged.
- [ ] A shortcut fires **before** the chord prefix fallback: a
      single-character shortcut on `m` would not break the `gl` chord
      detection (because `m` is not a prefix of `gl`), but it would also
      not be available if `m` were already an action.
- [ ] After `:config tui` → edit a shortcut → save: the new shortcuts
      take effect immediately (the tui.yaml reload includes them).
- [ ] **Built-in default** `T` → `tag`: with a tui.yaml without a
      `cmdline_shortcuts:` section (or with a section that does not
      define `T`, and where no `cmdline_shortcuts:` entry of the user's
      own existed before), `T` opens the tag menu. As soon as the user
      sets `cmdline_shortcuts:` explicitly, their block replaces the
      defaults — they have to add `T: tag` themselves if they want to
      keep it.

## Tasks tree expand/collapse

An expandable tree in the tasks tab. Bindings: `vt`/`vl` (tree/list
sub-view), `enter` (toggle the cursor node), `zr` (expand everything),
`zm` (back to `default_expand_depth`; **not** all the way to the roots,
set `default_expand_depth: 0` for that). Config:

```yaml
tasks:
  tree:
    default_expand_depth: 2 # 0 = roots only, 1 = + direct children, ...
```

- [x] In the tasks tab: `vt` → the tree sub-view is active; `vl` → the
      list sub-view is active (renamed from `t`/`l`).
- [x] Initially the tree shows only the first `default_expand_depth+1`
      levels — deeper nodes are hidden; their direct parents carry a `▶`
      plus an `(N)` suffix (N = the number of direct children).
- [ ] `enter` on a collapsed parent → it expands, the glyph switches to
      `▼`, the children become visible.
- [ ] `enter` on an expanded parent → it collapses, the glyph switches to
      `▶`, the children disappear, the `(N)` suffix appears.
- [ ] `enter` on a leaf node → no-op (the cursor stays, the tree is
      unchanged).
- [ ] `zr` → all branches expanded; all parents show `▼`, no `(N)`
      suffixes.
- [ ] `zm` → the tree goes back to `default_expand_depth` (2 levels open,
      say, deeper ones closed again); the per-node `enter` toggles are
      discarded in the process.
- [ ] Set `default_expand_depth: 0` via `:config tui`, then `zm` → now
      only the roots are visible (vim-style full collapse).
- [ ] Fuzzy filter active (`f` plus text) → the expand state is ignored;
      every match (plus its ancestors) appears, no `(N)` suffixes.
- [ ] Clear the filter → the previous expand state is back.
- [ ] `:config tui` → `default_expand_depth: 3` → save → the tree renders
      with the new depth immediately (via the reload pipeline).
- [ ] Restart the TUI: the expand state is **not** persisted; the tree is
      back at `default_expand_depth`.
- [ ] The list sub-view (`vl`) shows neither glyphs nor `(N)` — list mode
      is untouched.
- [ ] Trackings tab → the tree (`t`) shows **no** glyphs and **no** `(N)`
      suffixes (the trackings tree is not configured as expandable).
- [ ] The action bar (in the tree sub-view) shows the hint
      `↵ expand/collapse`.
- [ ] The default for `dismiss_notifications` is now `Z` (formerly `z`,
      which collided with `zr`/`zm`). Trigger a notification (an
      erroneous command, say), press `Z` in normal mode → the
      notification disappears. `:dismiss-notifications` without an
      argument does the same. `:dismiss-notifications foo` → the modal
      ":dismiss-notifications takes no arguments".

### `/` search through collapsed branches

In the tree sub-view, `/` now searches **all** tasks (hidden ones
included). If the cursor jumps to a match in a collapsed branch with
`n`/`N`, only the ancestor chain of the current hit expands. On the next
`n`/`N` the previous path collapses again; any other key (`j`, `k`,
`<space>`, Enter to close, …) "commits" the current path — it stays open,
and the next `n` is a fresh auto-expansion again.

- [ ] A tree with `default_expand_depth: 2` and sub-tasks deeper than 2.
      `/` plus typing a string that only occurs in a deep sub-task → the
      path to the hit opens automatically, the cursor lands on the match.
- [ ] Keep typing (append letters) → on every query change the old path
      collapses and the new path to the first match opens.
- [ ] Several hits in different sub-branches: `n` jumps to the next
      match → the previous branch collapses again, the new branch opens.
      `N` likewise, backwards.
- [ ] During an `n`/`N` sequence: `j` (or any other key) → the currently
      open path **stays** open. Then press `n` again → the new branch
      opens and the path just "pinned" stays open too (two visible
      paths).
- [ ] `Enter` (accept the search) commits the currently open path as
      well.
- [ ] `Esc` (cancel the search, empty query) → the auto-expansion is
      discarded, the tree falls back to the previous expand state.
- [ ] `Esc` with a non-empty query → the query is cleared, the
      auto-expansion is discarded, the search stays active.
- [ ] Fuzzy filter active (`f` plus text) plus a `/` search → the search
      only covers the tasks made visible by the fuzzy filter (no
      auto-expansion needed, everything is visible already).
- [ ] Before `/`: collapse a top-level branch manually with `enter`. `/`
      matches a task in that branch → the branch opens transiently.
      Commit → the branch stays open, `enter` on it closes it normally
      again (the transient state was promoted into `flipped` cleanly, not
      flipped twice).

## Tasks tree cut / paste (`:cut-node` / `:paste-node`)

Re-parent tasks in the tree without the edit form: first `mc` (or
`:cut-node`) on the source, then move the cursor onto the new parent,
then `mp` (or `:paste-node`). The database update happens **only** on
paste — nothing is touched before that.

- [ ] Tasks tab, pick any task, `mc` → a notification "Cut: … — paste
      with :paste-node (mp)". The tree is unchanged.
- [ ] Pick another task, `mp` → a notification "Moved: …". The tree is
      rebuilt, the source now hangs below the target.
- [ ] `mc` without a selection in tasks → the modal ":cut-node — no task
      selected".
- [ ] `mp` without a preceding `mc` → the modal ":paste-node — nothing
      cut (use :cut-node / mc first)".
- [ ] `mc` on a task A, then `mc` again on task B → the last cut wins
      (the notification describes B).
- [ ] `mc` on A, `Esc` → a notification "Cut cancelled". A subsequent
      `mp` → the modal "nothing cut".
- [ ] `mc` on A, cursor on A itself, `mp` → the modal "cannot paste a
      task onto itself". A stays cut (a second attempt is possible).
- [ ] `mc` on A, cursor on a descendant of A, `mp` → the modal "cannot
      move a task into its own subtree". The tree is unchanged.
- [ ] `mc` on A, cursor on the current parent of A, `mp` → a
      notification "already a child of the target", no database write,
      the cut is cleared.
- [ ] `mc` on A, `mp` on the root task B → A becomes a child of B, a
      notification "Moved: …".
- [ ] `mc` in the tasks tab, then switch to the trackings or a content
      tab and press `mp` → the modal ":paste-node only works on the Tasks
      tab".
- [ ] `:cut-node foo` → the modal ":cut-node takes no arguments".
      Likewise for `:paste-node foo`.
- [ ] Default bindings: `m` on its own does not land directly (it is
      stashed as a chord prefix); only `mc` / `mp` fire.

## `:jump` and `:focus-task`

Programmatic navigation for scripts and power cmdline users.
`:jump <Tab>[:<sub>]` switches the tab/subtab, `:focus-task /a/b/c`
looks up the matching node in the tasks tree and expands the path.

- [ ] `:jump Tasks` → switches to the tasks tab, the subtab is
      unchanged.
- [ ] `:jump Tasks:tree` → tasks tab, subtab tree. Coming from the list
      subtab: the selection is preserved (set_pending_focus).
- [ ] `:jump Tasks:list` likewise.
- [ ] `:jump Tasks:foobar` → the modal ":jump — unknown Tasks sub-view
      'foobar' (list|tree)".
- [ ] `:jump Trackings:condensed` → trackings tab, condensed subtab.
      (Trackings rebuilds via `rebuild_trackings_table`.)
- [ ] `:jump <name-of-a-content-tab>` (case-insensitive) → switches to
      the matching content tab.
- [ ] `:jump doesnotexist` → the modal ":jump — unknown tab
      'doesnotexist'".
- [ ] `:jump` without an argument → the modal ":jump expects one
      argument, e.g. :jump Tasks:tree".

- [ ] In Tasks:tree, `:focus-task /seg1/seg2/...` with an unambiguous
      path chain → the path expands, the cursor parks on the deepest
      node. The default match is a **case-sensitive substring**.
- [ ] `:focus-task /Work` (capital W) no longer hits the `work` task
      (the default is case-sensitive); `:focus-task -i /Work` finds it.
- [ ] `:focus-task -i /…` with mixed case in the segments matches
      anyway, for substring segments as well as `re:` segments.
- [ ] `:focus-task /work/clients/acme/tickets/re:\b42\b` → matches a
      task with "42" in the description, NOT 420 or 421. (Word boundary
      separation across digits.)
- [ ] `:focus-task /…/re:[broken(` (a broken regex) → the modal "invalid
      regex 're:…' — …", the tree is unchanged.
- [ ] `:focus-task -x /…` (an unknown flag) → the modal "unknown flag
      '-x' (only -i is supported)", the tree is unchanged.
- [ ] `:focus-task /unknown` → the modal "no task matching 'unknown' at
      root level", the tree is unchanged.
- [ ] `:focus-task /work/unknown` → the modal "… under 'work'", the tree
      is unchanged.
- [ ] `:focus-task work/x` (without the leading `/`) → the modal
      "expects a /-rooted path …".
- [ ] `:focus-task /` → the modal "path is empty".
- [ ] `:focus-task -i /` → the modal "path is empty" (the flag is
      consumed, the path is empty).
- [ ] `:focus-task /<ambig>` when several root tasks match → the modal
      "'<ambig>' is ambiguous: 'task A', 'task B', …".
- [ ] `:focus-task /…` in the Tasks:list subtab → the modal "only works
      in the Tasks:tree sub-view".
- [ ] `:focus-task /…` from the trackings or a content tab → the modal
      "only works on the Tasks tab".

## `:reload-tasks`

A synchronous refetch of the `task_rows` from the database. Its main
purpose: in a command chain from a script (see the next section), wedge
it between an external `nyd add` (an alias for `nyd tasks do add`) and a
subsequent `:focus-task`, so that the newly created task is seen.

- [ ] In the TUI tasks tab; in a second terminal run `nyd add` with a
      message of your own → the new row is NOT visible yet in the running
      TUI (not even after switching tabs back and forth, because
      `set_active_tab` only reloads while `Idle`). `:reload-tasks` → the
      row appears in the tree immediately (the parent auto-expands if it
      was open before).
- [ ] `:reload-tasks` from the trackings tab → the tasks are reloaded in
      the background, the active tab stays trackings (no auto switch).
- [ ] `:reload-tasks` from a content tab → likewise, no tab switch.
- [ ] `:reload-tasks foo` → the modal "takes no arguments".
- [ ] A tasks filter is active (fuzzy or a saved filter, say) → the
      reload respects the active filter (the same arguments as
      `spawn_load`); new rows that do not satisfy the filter do not show
      up.
- [ ] The database is unreachable during the reload → the modal
      "reload-tasks — …"; the active `task_rows` stay unchanged (no
      wipe).

## `:focus-node`

The content-view counterpart to `:focus-task`. It switches to the named
content tab (plus an optional sub-view) and parks the cursor on the first
row whose column matches the pattern. Default: case-sensitive substring;
`-i` folds case; `re:` opts into a regex. Single segment only (drill-down
comes later).

- [ ] From any tab; `:focus-node Taiga:items /ref|acme#42` → the tab
      switches to Taiga, sub-view items, the cursor parks on the row with
      `ref = acme#42`. Subsequent n/N navigation respects the new
      position.
- [ ] `:focus-node Taiga:items /acme#42` (no column hint) → it matches,
      because the pattern occurs in the concatenated label plus fields;
      in case of collisions (with the subject, say) write `ref|...`
      explicitly instead.
- [ ] `:focus-node Taiga:items /id|userstory:4242` → matches via the
      composite_id. (Inconvenient for scripts, because the
      `composite_id` cannot be derived from the ref/slug; but the form is
      supported.)
- [ ] `:focus-node Taiga:items /label|<exact start of the subject>` →
      matches a substring in NodeSummary.label.
- [ ] `:focus-node Taiga:items /ref|re:\bacme#42\b` → a regex with a
      word boundary, does not also match `acme#420`.
- [ ] `:focus-node -i Taiga:items /ref|ACME#42` → case-insensitive,
      matches a row with `ref = acme#42`.
- [ ] `:focus-node Taiga:items /foo|x` (foo exists in no metadata
      column) → the modal "unknown column 'foo' (available: …)", the
      cursor is unchanged.
- [ ] `:focus-node Taiga:items /ref|nope` → the modal "no row matching
      'ref|nope'".
- [ ] `:focus-node Taiga:items /ref|acme` (matches several tickets) →
      the modal "'ref|acme' is ambiguous: 'userstory:1', 'userstory:2',
      …".
- [ ] `:focus-node Taiga:items /a/b` (two segments) → the modal
      "multi-segment drill-down paths are not yet supported".
- [ ] `:focus-node Taiga:items ref|x` (without the leading `/` in the
      path) → the modal "expects a /-rooted path …".
- [ ] `:focus-node Taiga:items /` → the modal "path is empty".
- [ ] `:focus-node Tasks:tree /work` → the modal "…not a content tab"
      (tasks is not a content tab; `:focus-task` applies to tasks).
- [ ] `:focus-node doesnotexist:items /ref|x` → the modal "'…' is not a
      content tab …".
- [ ] `:focus-node Taiga:foobar /ref|x` → the modal "unknown view
      'foobar' for tab 'Taiga' (available: items, notifications)".
- [ ] `:focus-node -x Taiga:items /ref|x` (an unknown flag) → the modal
      "unknown flag '-x' (only -i is supported)".
- [ ] If the items view has never been loaded (`auto_connect: never`):
      `:focus-node Taiga:items /ref|acme#42` finds 0 rows → the modal "no
      row matching …"; the user has to trigger `r` (reload) first.

## CLI `tasks show --path`

Path-based lookup through the **generic** `--path` resolver (D3b-1) on the
adapter instance `tasks` (the former hard-coded `task show` is gone). One
segment per level, matched against child labels by substring (case-folded with
`-i`) or by a `re:` regex; every segment must match exactly one child. On
success the node fields go to stdout (`-o json` for JSON), exit 0.

> ⚠ The generic resolver has different error messages and exit codes than the
> old `task show`. Re-verify these items against the actual behaviour of
> `adapter_cli` (substring vs. `re:`, "ambiguous … list the candidates") at the
> next smoke run and update the expected strings here.

- [ ] `nyd tasks show --path /Work/Clients/Acme/Tickets` → the node fields on
      stdout, exit 0.
- [ ] `nyd tasks show -i --path /work/clients/acme/tickets` → same hit as
      above, exit 0.
- [ ] `nyd tasks show --path '/Inbox/re:^Week \d+$'` → the regex segment
      matches.
- [ ] `nyd tasks show --path /nope` → error (unmatched segment), exit ≠ 0.
- [ ] `nyd tasks show --path /<ambig>` where several children match → error
      listing the candidates, exit ≠ 0.
- [ ] Pipe-friendly: `nyd tasks show --path … -o json | jq -r '.id'` yields a
      valid UUID (smoke test for the JSON schema).

## Script `mode: commands` (script → TUI command relay)

Scripts marked `# mode: commands` write JSON of the form
`{"commands": [...]}` into `$NYD_OUTPUT_FILE` and drive the TUI through it
(e.g. `jump`, `focus-task`, `tag`, …). `interactive+commands` does the same,
except that the script additionally gets the terminal.

- [ ] Background variant: the script writes `{"commands": ["jump Tasks:tree"]}`
      into `$NYD_OUTPUT_FILE`, the tab switches to Tasks:tree.
- [ ] Several commands in a row: a list of `jump Tasks:tree` followed by
      `focus-task /work/foo` → jump, path expand and focus run in order.
- [ ] The script writes nothing → notification "Script finished", no commands
      executed (a no-op rather than an error).
- [ ] The JSON does not parse → modal _"Script output is not valid JSON: …"_.
- [ ] JSON without a `commands` array → modal _"missing `commands` array"_.
- [ ] An entry in `commands` is not a string → modal _"command entry is not a
      string"_, the remaining entries still run.
- [ ] An entry with a leading `:` (`":jump Tasks:tree"`) is accepted just like
      one without.
- [ ] The script exits with a non-zero status → modal _"Script exited with …"_,
      the commands are NOT executed.
- [ ] Forward compatibility: JSON carrying `commands` plus extra keys such as
      `version` and `metadata` is accepted, unknown keys are ignored.
- [ ] Stderr from the script still shows up as a notification.
- [ ] `interactive+commands`: the script runs interactively (the TUI yields the
      terminal); once it finishes, the commands from the detached output file
      are executed.
- [ ] The output file in `/tmp` is gone after the run (cleanup).

## SQ-8 — Postgres-Script-Shortcuts via `query_shortcut` (DB)

Per-table Postgres scripts no longer keep their hotkey in a `.shortcuts.yaml`
next to the script's `.sql`, but in the `query_shortcut` table (scope = the
NodeRef path `postgres/<inst>/<db>/schemas/<schema>/tables/<table>`, name = the
script file stem, shortcut = the chord). Apply-on-chord is symmetric to
Jira/Taiga: a global trigger for as long as the focused pane sits on exactly
that table.

- [ ] **Existing data:** migrate the 6 jira/taiga rows with the `:` separator
      to the NodeRef form using `/`:

  ```sh
  sqlite3 ~/.local/share/not_yet_done/nyd.db \
      "UPDATE query_shortcut SET scope = REPLACE(scope, ':', '/') WHERE scope LIKE '%:%';"
  ```

  Afterwards the Jira/Taiga saved-query hotkeys (`ctrl+i`, `ctrl+m`, `ctrl+w`
  and so on) show up in the tab again as a highlighted apply hint and fire
  their query.

- [ ] Postgres tab → focus one concrete table (drill down into schema `public`
      → cursor on `users`) → `q queries` → an existing script shows `[chord]`
      in the list if it was bound earlier.
- [ ] `Ctrl+e` in the script list → modal "Press a shortcut key" → press a free
      key → notification "Bound shortcut", or nothing. Afterwards a `sqlite3`
      query on `~/.local/share/not_yet_done/nyd.db` selecting scope, name and
      shortcut from `query_shortcut` where the scope starts with `postgres/`
      shows the entry with its NodeRef path.
- [ ] Close the popup, leave the cursor on the same table → press the bound
      chord → the script result opens in the rows split (same behaviour as
      Enter-on-apply from the menu).
- [ ] Same chord, cursor on a DIFFERENT table → the chord is NOT claimed
      (`PostgresTableScriptShortcut` is conditioned on the table NodeRef), the
      key keeps its default behaviour.
- [ ] `d` (delete) in the Q menu on a script with a shortcut → both the file
      AND the `query_shortcut` row are gone (check with sqlite3).
- [ ] Restart the TUI → the bindings survive (persisted in the DB).
- [ ] Filesystem check: after binding and deleting there is NO `.shortcuts.yaml`
      file left in `~/.local/share/not_yet_done/postgres/*/queries/**`.

## Jira multi-hop workflow transitions (TR-1 … TR-7)

Background: the transition picker records every observed workflow edge in the
cache (`jira_workflow_edge`) and enumerates multi-step chains from it, so that
users can go `Ready → In Progress → Done` in a single step without clicking
through the intermediate states one by one.

### Setup

- The backing DB is `~/.local/share/not_yet_done/jira-cache.sqlite`. The
  `jira_workflow_edge` table is created automatically on the first adapter
  start (SeaORM auto-sync).
- Never reset the cache by hand — the hop limit is 4, and self-loops are
  recorded but not traversed.

### Smoke (cold start, empty cache)

- [ ] Empty the table before starting the TUI: a `sqlite3` call on
      `~/.local/share/not_yet_done/jira-cache.sqlite` running
      `DELETE FROM jira_workflow_edge;`.
- [ ] Open the transition picker on a Jira issue → the options match exactly
      the direct transitions. The label is just the target status, no `*` (all
      of them are direct). No duplicate entry when two transitions lead to the
      same status — the first one wins.
- [ ] Close the picker → the DB holds edges for the current status: select
      `from_status_name`, `transition_name` and `to_status_name` from
      `jira_workflow_edge`.

### Smoke (the snowball grows, multi-hop appears)

- [ ] Open an issue in status `Ready` → picker → direct transitions (no `*`).
- [ ] Open other issues in `In Progress` and in `Review`, open the picker once
      on each, then press Esc (do not transition!).
- [ ] Back on the first issue → picker → an entry `Done*` (or similar) now
      shows up as well, reachable via multi-hop. The `*` marks "not direct";
      the intermediate states are NOT part of the label any more.
- [ ] If there is both a direct transition to `Done` and a multi-hop chain to
      `Done`, only a single entry `Done` appears (without `*`) — the direct
      path wins.

### Smoke (chain success)

- [ ] Picker → pick a multi-hop entry (e.g. a 2-hop one) → Enter.
- [ ] Status bar: `<KEY> → <final status>`.
- [ ] The issue detail shows the final status; the Jira web UI confirms that
      both transitions ran.

### Smoke (chain failure with refresh)

- [ ] Construct a workflow with a `required field` on the second hop (e.g. the
      transition demands `resolution`). Reset the test issue to the starting
      status.
- [ ] Picker → pick a multi-hop entry that contains the required-field hop →
      Enter.
- [ ] Status bar: "Chain stopped at step 2/3 (now in \<intermediate status\>)"
      followed by the Jira error body.
- [ ] The issue detail in the TUI shows the current intermediate status (hop 1
      persisted successfully, hop 2 aborted) — not the original starting
      status.

### Smoke (picker hint bar)

- [ ] Open any picker action (`transition`, or another one such as the Postgres
      cell picker if it exists) → the hint bar in the popup footer shows an
      apply hint for Enter and a close hint for Esc.
- [ ] In the detail pane: the style matches the other menu hints (query menu,
      tag menu). No doubled styling, no broken layout.

### Edge cases

- [ ] An issue key without a `-` (should not happen, but in case the test
      config is broken): recording aborts silently, the direct picker keeps
      working.
- [ ] `db.url` in the Jira adapter YAML set to `none` or empty: recording is a
      no-op, the picker only shows direct transitions, no crash.
- [ ] A self-loop transition in the workflow (e.g. `To Do → To Do` for
      attaching a file): it is written as an edge, but NOT offered as a path
      option.

## SearchablePopup — intrinsic navigation (SP-1 … SP-7)

Background: `SearchablePopup` now carries its own set of bindings for `next`,
`prev`, `backspace`, `cursor_left` and `cursor_right` (the `PopupAction` enum,
configurable under `popup:` in `tui.yaml`, defaulting to `ctrl+j`, `ctrl+k`,
`backspace`, `left` and `right` plus the arrow keys as a secondary binding for
`Next`/`Prev`). The intrinsic hints appear in the hint bar automatically — the
transition picker thus matches QueryMenu, TagMenu and ScriptMenu both visually
and functionally.

### Smoke (the transition picker gains visible navigation)

- [ ] Select a Jira issue with ≥3 available transitions, open the picker (the
      adapter's `Action` trigger, e.g. `t`).
- [ ] The hint bar at the bottom edge of the popup shows next, prev, apply and
      close hints (icons come from the `key_icons` map; `↓`/`↑` for the arrows,
      possibly with `ctrl+j` after a slash).
- [ ] Arrow up/down AND `Ctrl+J`/`Ctrl+K` both navigate.
- [ ] Typing filters the list; `Backspace` adds an erase hint (only while the
      query is non-empty).

### Smoke (other pickers unchanged)

- [ ] QueryMenu (`q` on Trackings/Tasks/Content) — the hint bar now shows
      `next`/`prev` in front of the existing hints (apply, edit, shortcut,
      delete, close).
- [ ] TagMenu (`:tag`), ScriptMenu (`:script`) and the `gl` link popup behave
      the same: arrows and `Ctrl+J`/`Ctrl+K` work, the hint bar shows them.
- [ ] The `:config` picker still works: typing filters, arrow navigation, Enter
      opens the file.

### Smoke (custom bindings via tui.yaml)

- [ ] In `~/.config/not-yet-done/tui.yaml` set `next: ctrl+n` and
      `prev: ctrl+p` under `keybindings.popup:`.
- [ ] Restart the TUI (or `:config` → tui.yaml → save → granular reload).
- [ ] The transition picker now shows `^N next  ^P prev …` and exactly those
      keys navigate.

## Node actions live in `actions:` (NA-1 … NA-6)

Background: the `shortcuts:` map is gone. A level binds an adapter action
through the same `actions:` list as everything else, with `type: node` as the
default — so `- { key: d, id: delete }` is a complete entry. `name:` is
optional and falls back to the label the adapter reports for that id.

- [ ] **NA-1 — the migrated keys still fire.** In each of jira, taiga, tasks,
      trackings, stoat, postgres, sqlite: the keys that used to sit in
      `shortcuts:` do exactly what they did before (`d` delete, `s`
      toggle-tracking, `X`/`e` on script rows, `C`/`P` channel cut/paste).
- [ ] **NA-2 — the label comes from the adapter.** An entry without `name:`
      shows the adapter's own label in the action/status bar and in `Ctrl+Y`,
      not the raw id. Adding `name:` to that entry overrides it for this level
      only.
- [ ] **NA-3 — `target: parent`.** `Q` in a rows pane opens the SQL editor of
      the parent table; at the un-drilled root (no parent) the binding is
      simply absent instead of erroring.
- [ ] **NA-4 — `force: true` at runtime, not just at load.** Postgres/SQLite:
      `Q` on a table row opens the adapter's SQL editor, **not** the built-in
      query editor. Remove the `force: true` → the tab goes into the broken
      state with the collision named. (Regression bait: `force` used to be
      honoured by the validator alone, so the built-in still won at runtime.)
- [ ] **NA-5 — a subtab key cannot be forced away.** Give a level
      `- { key: <k>, id: … , force: true }` where `<k>` is a subtab switch key
      of the same tab → still a load-time conflict, because that claim covers
      the whole tab. Only a different key fixes it.
- [ ] **NA-6 — `Ctrl+Y` rebinds a node action in place.** Put the cursor on a
      migrated entry, assign a free key → the YAML line keeps its `id:` and its
      trailing comment, only `key:` changes. Rebind an adapter action that the
      level does not mention yet → a new `- { key: …, id: … }` line is appended
      to that level's `actions:`, the surrounding comments untouched.

## Shortcut Hints (SH-1 … SH-7)

Background: YAML node actions (e.g. `- { key: a, id: add }`,
`- { key: Q, id: edit_sql, target: parent }`) are rendered as action-bar or
status-bar hints for
the currently selected row. The hints are row-specific and come from the
`Node::actions()` lookup per `node_id`, fetched asynchronously and cached. Free
of races because the cache key is the `node_id`.

### Smoke (Postgres — database subtab in tree mode)

- [ ] Postgres tab → subtab `5` (database) → move the cursor up and down in
      tree mode. Cursor on a DB-scripts group row → the action bar shows
      `a: add`.
- [ ] Tree-expand the scripts group (`l`/`Enter`) → cursor on an individual
      DB-script leaf → the action bar shows `X: execute`, `e: edit`,
      `d: delete` — `a: add` is NOT visible any more (the leaf has no `add` in
      `actions()`).
- [ ] First cursor move onto a new row: the hints may be missing briefly and
      appear as soon as the adapter response arrives (a few ms at most). On the
      second visit to the same row they are there immediately (cache hit).

### Smoke (rows view with a `parent:` shortcut)

- [ ] Postgres tab → pick a table → open the rows view. The action bar shows
      `Q: edit sql` (from the ViewDef shortcut `Q: parent:edit_sql`), resolved
      through the parent table node. Move the cursor within the row list → the
      hint stays stable (the target is the parent, not the current row).

### Smoke (cache invalidation on reload)

- [ ] On a row with existing hints (e.g. a DB-script leaf with `x e d` visible)
      → `r` (reload) → the list is reloaded, the hints are fetched again for
      the now-selected row and reappear.

### Regression bait

- [ ] Repeated fast cursor movement up and down across different rows: no
      duplicate fetches, no stale hints from an older row (cache key =
      `node_id`, pending requests deduplicated).
- [ ] Jira issue list: as the cursor moves between issues, the shortcut hints
      of each row always belong to that row's own `actions()` (no issue shows
      its neighbour's hints).

## EIP — edit-in-place for DB scripts

The ChildDef flag `editor_in_place: true` puts the editor's temp file into the
target directory instead of `$TMPDIR`, so that language servers (e.g.
`postgres-language-server`) find the project context.

**Setup**: place an empty or arbitrary `postgres-language-server.jsonc` under
`<instance_data_dir>/db_scripts/<db>/`.

- [ ] Open the DB scripts tree, edit a script with `e` → the vim/`$EDITOR`
      status line shows a path **inside** the `db_scripts/<db>/` directory,
      prefixed `.nyd_tmp_…` and suffixed `.sql` (not `/tmp/…`).
- [ ] After `:w` and `:q` the `.nyd_tmp_…` temp file in the directory is gone
      and the real script carries the written content.
- [ ] The `postgres-language-server.jsonc` next to the scripts takes effect for
      the edit session (LSP diagnostics / hover, depending on the server
      setup).
- [ ] With `editor_in_place: false` (or the default) the temp file lands in
      `/tmp/…` again.
- [ ] Edit a `.py` or `.md` script: the temp-file suffix picks up the real
      extension (`.nyd_tmp_xyz.py`), no SQL template is inserted.
- [ ] Kill the TUI hard in the middle of an edit session → the `.nyd_tmp_…`
      file stays behind in the directory (the prefix marks it clearly as junk;
      it can be removed by hand).

## AE — adapter child-process environment

The trait method `ContentAdapter::child_process_env(node) -> HashMap<String,String>`
is queried when editor and script child processes are spawned; the TUI passes
the content on opaquely via `Command::envs(...)`. The Postgres adapter supplies
`PGHOST`, `PGPORT`, `PGUSER`, `PGPASSWORD`, `PGDATABASE` and `PGSSLMODE`.

**Setup**: an active Postgres adapter with `auto_connect: startup` (auto
warm-up); `transport.mode: ssh_tunnel` is the interesting case. In nvim,
`postgres_lsp` active according to `:LspInfo`. The jsonc next to the scripts
may only contain the schema line — remove all `db` keys.

- [ ] Press `e` (edit) on a `postgres:db_script` node — nvim opens the buffer
      in `db_scripts/<db>/` and `postgres-language-server` starts cleanly (in
      the logs: no "pool timed out").
- [ ] Type `SELECT * FROM ` in the buffer → a completion popup with the real
      tables and columns of the NodeRef's database. Compare before and after:
      without the adapter env (e.g. `disableConnection: true` in the jsonc)
      v0.25 returns 0 items.
- [ ] In a shell terminal inside the editor: `env | grep ^PG` shows the five
      or more variables (do not paste the value of `PGPASSWORD` into the
      repo!).
- [ ] Adapter offline (status bar `Disconnected`): `e` still opens, the LSP
      starts without a DB connection and offers 0 completions (gracefully, no
      error message).
- [ ] Run `:script` on a table node in the Postgres tab: a `python3` heredoc
      can read `os.environ["PGPASSWORD"]` (e.g. the script writes a list of the
      `PG*` variables to `$NYD_OUTPUT_FILE`). The adapter env must not
      overwrite `NYD_OUTPUT_FILE` — the adapter's `PG*` variables are applied
      before the `NYD_*` ones.
- [ ] `:script` in the Tasks tab: empty env, no `PG*` (no regression — task
      scripts still see only the old variables).
- [ ] `:script` in the Trackings tab: the same, empty env.
- [ ] Kill the tunnel by hand (on the SSH bastion) and issue a query in the TUI
      → tear-down plus reconnect; the next editor start on a DB script has
      `PGPORT` set to the new local port, not the old one.

## Confluence adapter (CF-3 … CF-16)

Background: the Confluence Server/DC adapter mirrors the Jira and Taiga
architecture — `confluence:space` as the root, `confluence:page` recursively
below it, plus `confluence:attachment` and `confluence:comment` as leaf
branches on every page. All actions are bound through the adapter-side
`actions_for_type`; the view YAML entries are documentation.

### Setup

- `~/.config/not_yet_done/views/confluence-adapter.yaml` with the path to the
  cookie script (`auth.bindings[].provider.script`). The script writes a single
  line `JSESSIONID=...; crowd.token_key=...; atlassian.xsrf.token=...` to
  stdout.
- `~/.config/not_yet_done/views/confluence.yaml` (example:
  [`docs/examples/views/confluence.yaml`](examples/views/confluence.yaml)).
- Saved-queries directory:
  `<XDG_DATA_HOME>/not_yet_done/confluence/<instance_id>/queries/`. Optional
  seed:
  [`docs/examples/views/saved/confluence/recent-pages.yaml`](examples/views/saved/confluence/recent-pages.yaml).
- Start the TUI → tab `Confluence` (default subtab `spaces`); with
  `auto_connect: never` nothing loads automatically, `r` triggers the first
  fetch.

### CF-3 — spaces

- [ ] `r` on `spaces` → a list of all spaces; the `Key`, `Name` and `Type`
      columns are filled; cursor navigation with `j`/`k` works.
- [ ] The `f` fuzzy and `/` search actions filter on `Key` and `Name`.
- [ ] `o` on a space row opens the space in the browser (webui).

### CF-4 — pages (recursive)

- [ ] Enter on a space row → the top-level pages expand inline; the tree
      markers (`▼`/`▶`) are visible.
- [ ] Enter on a page row → child pages, attachments and comments appear as
      three branches.
- [ ] Drilling deep (3+ levels) stays consistent — the same recursive
      `ChildDef` at every level.

### CF-5 — preview pane (body.storage)

- [ ] `p` on a page row → the preview pane appears in a horizontal split
      (50/50) and shows `body.storage` (XHTML).
- [ ] First `p` toggle: noticeable latency (lazy fetch of
      `GET /content/{id}?expand=body.storage,...`); second toggle: instant
      (cache).
- [ ] `p` again → the preview closes.

### CF-6 — attachments

- [ ] Drill into a page → the `attachments` branch shows file name, author,
      size, MIME type and creation date.
- [ ] `d` on an attachment → download into a temp dir plus `xdg-open` opens the
      file.
- [ ] A second `d` on the same attachment: no re-fetch (cached by id), opens
      immediately.

### CF-7 — comments (read-only)

- [ ] The `comments` branch shows author, creation date and a body excerpt.
- [ ] `p` toggles the body preview; **no** second HTTP call (the body rides
      along on `list_comments` with `expand=body.storage,version`).

### CF-8 — CQL search

- [ ] Open the `search` subtab → the default CQL from the YAML
      (`type = page AND lastModified > now("-7d") ...`) shows hits.
- [ ] `q` opens the saved-queries menu; the `recent-pages` seed shows up. Enter
      applies it; `Ctrl+f` binds a chord shortcut (persisted in
      `query_shortcut`).
- [ ] `:query new <name>` → the editor opens; type CQL, save → it appears in
      the `q` menu.
- [ ] `:query delete <name>` removes both the file and the DB shortcut.
- [ ] Drilling into a search hit opens the same three branches (pages,
      attachments, comments) as via `spaces`.

### CF-9 — edit page plus 3-way merge

- [ ] `e` on a page → `$EDITOR` opens with a `title:` header carrying the
      **real title** on line 1 (not the page id!), a blank line, then the
      pretty-printed `body.storage` (xmllint).
- [ ] Change the body trivially (e.g. insert a new paragraph), save, close the
      editor → banner `Updated page <Title> (v <n+1>)`.
- [ ] **The tree row after saving** still shows the **real title**, not the
      page id (regression guard: the post-edit row patch re-resolved the row
      via `get_by_id`, whose stub set the title to the id → the row showed the
      id until the next full tree reload; `get_by_id` now hydrates the title
      from the server).
- [ ] Confluence web: the new version appears, the body is correct, the
      **title is unchanged** (regression guard: a body-only edit used to write
      the page id back as the new title).
- [ ] **Rename**: change only the `title:` line, leave the body as it is, save
      → the page has the new name in the Confluence web UI; no no-changes
      short-circuit.
- [ ] **Disjoint merge**: before saving in the editor, change the page upstream
      in the Confluence web UI (a different spot than the edits in the buffer).
      Save → 409 → auto-merge → a banner reporting that the change was merged
      on top of version `<m>`; both changes are in the final version.
- [ ] **Conflicting merge**: change the same spot upstream before saving. Save
      → 409 → the buffer reopens with `<<<<<<< ours` / `>>>>>>> theirs` markers
      plus the banner `Merge conflict — resolve and save again`. Resolve the
      markers by hand, save → the update goes through.
- [ ] Parse error in the buffer (e.g. delete the title line) → reopen with an
      error banner, no PUT.

### CF-10 — create page

- [ ] `a` on a space row → an editor with a `title:` header and an empty
      `<p></p>` body. Set the title, save → a new top-level page in this space
      (banner with the new id); a reload shows it under the space.
- [ ] `a` on a page row → the same, a new child page below this page (a reload
      shows it as a child).
- [ ] Leave the title empty → reopen with a parse error.

### CF-11 — delete page (trash)

- [ ] `Shift+D` on a page → confirmation popup `Delete '<title>'? y/n` (or
      Enter/Esc).
- [ ] `y`/Enter → the page disappears from the list; the Confluence web trash
      contains it; restoring from the web UI works.
- [ ] `n`/Esc → the page stays; no request is fired.

### CF-12 — comments CRUD

- [ ] `c` on a page → an empty XHTML editor; enter a body, save → the new
      comment appears in the `comments` branch (force a reload with `r` on the
      page).
- [ ] `e` on a comment → a buffer with the body, modify it, save → banner
      `Updated comment`; the body is updated in the listing.
- [ ] **Comment 409**: edit the same comment in the web UI in parallel → reopen
      with an error banner (no 3-way merge — rewrite by hand and save again).
- [ ] `Shift+D` on a comment → the generic `ConfirmDeleteContentNode` popup →
      Enter deletes; the comment disappears.

### CF-13 — attachment upload

- [ ] `Shift+A` on a page → the file picker opens.
- [ ] Multi-select: pick 2–3 small test files (invented data, no real customer
      files!) → save → banner `Uploaded N attachment(s) to page <Title>`.
- [ ] The page's `attachments` branch (after `r`) lists all uploaded files; `d`
      opens them correctly.
- [ ] Select one unreadable and one readable file → the error banner names the
      failing path explicitly; the readable file is uploaded anyway
      (`uploaded 1/2; failures: ...`).

### CF-14 — clone page

- [ ] `y` on a page → the editor opens with the title set to the original
      followed by `(Clone)`, plus the pretty-printed body.
- [ ] Save without changing anything → a new page under the same parent (or as
      a top-level page, if the source was top-level) in the same space; banner
      `Cloned page <orig> → <new> (id ...)`.
- [ ] Title suffix stacking: `y` on the page just cloned → the title keeps the
      single `(Clone)` suffix (no doubled suffix).
- [ ] Edit the body before saving → the new page carries the edited body, not
      the original one.
- [ ] Parse error (delete the title line) → reopen with a banner; no POST is
      fired.

### CF bugfix 2026-06-02 — pages in a space render empty (tree_label alignment)

Symptom: expanding a space with `tree_label: name` → ~50 empty rows below the
space row, even though the `/content/page` response carries full `title`
fields. Root cause: the tree renderer
(`content_view.rs::build_tree_data_rows`) only paints the label cell for a row
when that row level's `tree_label` matches a `col.key` of the **active** column
set — and otherwise leaves every cell of a non-active-depth row empty. The
space level had `tree_label: name`, the page level `tree_label: title` → no
match → empty page rows.

Fix: the page level in `confluence.yaml` (user config and repo example) was
switched to `tree_label: name`, and the page column `key: title` became
`key: name` (the display header stays "Title" via `label:`). 115 tests green.

- [ ] Space whitelist active with two real keys (see the user config), leave
      the cursor on the first space row, `o` to expand → the pages appear with
      correct titles (not empty).
- [ ] Move the cursor onto a page row → `active_depth=1`, the header switches
      to "Title | ID", other spaces show empty (by design, see the
      convention).
- [ ] Recursive: drill into a page that has sub-pages → the sub-pages are named
      correctly as well (no regression against the recursive ChildDef).

### CF bugfix 2026-06-02 — spaces show all pages instead of the top level

Symptom: expanding a space → ~50 pages from the whole space instead of only the
direct children of the space homepage as shown in the Confluence "tree browser"
of the web UI. Root cause: `/rest/api/space/{KEY}/content/page` defaults to
`depth=all`, i.e. _every_ page in the space — not the tree-browser list.

Fix: `/rest/api/space?expand=homepage` now pulls the homepage id along per
space and `SpaceMeta::homepage_id` stores it. When a space is expanded the
adapter calls `list_child_pages(homepage_id, ...)` instead of
`list_top_pages(space_key, ...)`. The lookup path (the `get_by_id` synthesizer)
fetches `/space/{KEY}?expand=homepage` lazily on the first `list()` via a
`OnceCell`. If Confluence exposes no homepage (legacy or restricted), the
listing API returns an empty page list rather than falling back to all pages.

- [ ] Space whitelist active; open a known space with the tree-browser sidebar
      in the Confluence web UI → compare the number and titles of the pages
      visible there against the expanded list in the TUI (they should match
      1:1, ordered by `position`).
- [ ] Reload (`r`) on a space row → the pages still appear correctly (cache
      path).
- [ ] Direct lookup path: after `:focus-node` or a cross-tab link onto a space
      → the first expand does not hang (the `OnceCell` fetches the homepage
      transparently), the page list matches the web UI.

### CF bugfix 2026-06-02 — column order: the tree column comes first

YAML reorder in the `spaces` view: `name` (the tree label) now comes first,
`key` and `type` trail it. Convention for tree-mode views: the column carrying
the tree belongs at position 1, otherwise the tree indent only starts after the
narrow trailing columns.

- [ ] The spaces subtab shows the tree indent and the space name first on the
      left, with `Key` and `Type` to their right.

### CF-16 — spaces whitelist (`space_keys`)

- [ ] `confluence-adapter.yaml` without `space_keys:` → the spaces subtab lists
      every readable space as before (regression check).
- [ ] `space_keys: [BBB, AAA]` with two real keys whose desired UI order is
      **not** alphabetical → the spaces subtab shows only BBB and AAA, in
      exactly that order (BBB first).
- [ ] `space_keys: [GOOD, NOPE_TYPO]` → the spaces subtab shows only GOOD; no
      error banner, no crash (silent drop verified).
- [ ] With `space_keys:` set: `r` (reload) in the spaces subtab → no pagination
      affordances are visible (no `has_next`), the listing shows only the
      whitelisted spaces.
- [ ] Drill down into a whitelisted space → pages, attachments and comments
      work just as they do without the whitelist (the recursive ChildDef is
      unaffected).

### CT-1..CT-11 — tree find (`/` in the spaces tab)

Prerequisite: the spaces subtab loaded with ≥2 spaces holding several pages
(at least one of them nested ≥2 levels deep). With `space_keys:` set, only
whitelisted spaces may show up.

- [ ] `/` opens the input line with the prompt "Search pages" and a `?` prefix.
      Typing does not change the display (no local filter).
- [ ] Enter with a non-trivial term → toast
      `Tree find "q": N hits — n/N to navigate`. The status bar shows
      `n/N  Tree find "q": 1/N`.
- [ ] The tree expands automatically down to the first hit, with the cursor on
      it.
- [ ] `n` jumps to the next hit in tree order (the same space first, then the
      next space entry from the YAML); ancestors are loaded lazily when needed
      (a brief flicker is fine).
- [ ] `N` jumps back (wrapping around at the start and the end).
- [ ] The status-bar counter updates on every jump (`1/47`, `2/47`, …).
- [ ] When the server has more hits than the cap (100), the counter shows
      `, truncated` and the toast reports `truncated`.
- [ ] Esc on the empty input line closes it without a cache.
- [ ] Esc on a filled input line before Enter: clears the cache (n/N falls back
      to the local `/`).
- [ ] `r` (reload) clears the tree-find cache. The status-bar hint disappears,
      n/N is the local `/` again.
- [ ] Another `/` with a different query: the old cache is gone and the new
      search starts cleanly (no mix of old and new hits).
- [ ] With `space_keys:` set: tree find finds no pages from whitelisted-out
      spaces (the server filters via an injected `space in (...)`).
- [ ] While a tree find is active, manual expanding and collapsing in spaces is
      still possible; the cache survives those UI interactions.
- [ ] `f` (the local `fuzzy_filter`) is unchanged and filters only the already
      visible space rows (not pages).

### CT-12 — SpaceNode → top-level pages (instead of homepage children)

Prerequisite: the spaces subtab with ≥1 space whose homepage and/or top-level
pages are nested several levels deep.

- [ ] Drill into a space → level 1 no longer shows just the homepage children
      but all top-level pages of the space (including the homepage itself).
- [ ] Tree find (`/`) for a page ≥2 levels below the homepage → expands cleanly
      down to the page (no more complaint that the hit's ancestor at depth 1 is
      not among the loaded children).
- [ ] `o` (open in browser) on the homepage row opens the homepage URL.
- [ ] `a` on a space row still creates a top-level page; `a` on the homepage
      row creates a child below the homepage (the previous behaviour,
      unchanged).
- [ ] Spaces without recognisable top-level pages (an edge case) return an
      empty list rather than an error.

### CT-13 — the tree-find walker stays "settled" after a hit

Prerequisite: tree find active on spaces (`type: tree_find` in the view YAML).
Searched with `/` at least once beforehand and jumped to a hit.

- [ ] After jumping to a hit, move the cursor elsewhere with `j`/`k`, then
      press Enter on another, collapsed node (loading its children) → the
      cursor stays on the opened node and does NOT jump back to the last search
      result.
- [ ] `n`/`N` still moves back and forth between the hits (so settled is reset
      correctly by next/prev).
- [ ] A new search with `/` (opening the search input again) behaves as always:
      the first hit is jumped to.

### CT-14 — tree find only offers hits the query leaves visible

The search used to run against the adapter's whole universe while every tree
level was filtered by the active query, so hits the query hides stranded the
expand walk ("Tree find: Hit's ancestor '…' at depth 3 not in loaded
children") and had to be stepped past with `n` by hand.

Prerequisite (tasks tab): a task whose text also occurs in **deleted** tasks —
create one, duplicate it, delete the duplicate. The default query
`[deleted, =, false]` hides the duplicate.

- [ ] `/` for that text in the tasks tree → the status bar reports only the
      hits that are actually in the tree (the deleted duplicate is not
      counted) and the cursor lands on the live task **without** pressing `n`.
- [ ] Replace the query with `[deleted, =, true]` (`q`), search again → now
      exactly the deleted ones are found and reachable; the live task is not
      offered.
- [ ] Drop the `deleted` clause entirely → both are found, `n`/`N` steps
      through all of them.
- [ ] The scripted jump keeps working: `:tree-find "Tasks" id:<uuid>` on a
      **visible** task jumps to it; on a task the current query hides it
      reports no match instead of stalling on an unreachable hit.
- [ ] Confluence (an adapter that does not filter its levels by the view
      query) is unchanged: tree find still finds pages across the whitelisted
      spaces regardless of the active query.
- [ ] Unreachable hits are skipped, not dead ends: with a fuzzy filter (`f`)
      active that hides a hit's row, `n` walks on to the next reachable hit;
      the status bar appends "N unreachable". Only when no hit at all is
      reachable does the message appear — and it names the count
      ("none of the N hits is reachable…") rather than one id.

### RD-1 — render-loop dirty gating (CPU while idle)

See `docs/decisions/0001-render-loop-dirty-gating.md`.

- [ ] Leave the app open and do nothing (no key, no active tracking):
      `top`/`htop` shows the process CPU near 0 % (before: a constantly
      noticeable load from the 60 fps repaint).
- [ ] Typing and navigating feel as immediate as before — no input latency
      (keys trigger a redraw at once).
- [ ] Async load (e.g. loading a large content listing, a Taiga/Jira reload):
      the result appears practically instantly (≤ ~200 ms), spinners and
      banners update smoothly.
- [ ] **The busy-banner counter runs:** start a query with
      `query_timeout_secs` (Postgres), or use a slow connection — the
      "…(Ns/…)" counter in the banner ticks up every second **without** any
      further input (it does not freeze).
- [ ] **Active tracking:** start a tracking → the duration column keeps
      updating (adaptive interval), yet the app stays quiet while idle.
- [ ] Open the editor (`e`), `:w` (live apply, if active), close → returning
      renders cleanly right away; a detached script (`x`) delivers its result
      when it finishes, without stalling.

### RD-2 — render loop 1b (event-driven `select!` loop)

See `docs/decisions/0001-render-loop-dirty-gating.md` (§1b). Replaces the
200 ms polling loop; idle is now parked instead of periodically awake.
**Focus: no regression in the stdin hand-over and in the time-driven parts.**

- [ ] **Real idle:** app open, nothing happening → CPU 0 %. Tracing the process
      with `strace` for `poll` and `read` (or with `perf`) shows **no**
      periodic 200 ms wake-ups any more, as long as no tracking, banner or
      editor is alive.
- [ ] **Key latency:** typing and navigating react instantly — no regression
      against 1a.
- [ ] **An async push wakes the loop instantly:** load a large content listing
      → the result appears without perceptible delay (no longer tied to the
      200 ms grid).
- [ ] **Inline editor stdin hand-over:** `e` → the editor receives **every**
      keystroke (no swallowed first character), typing is smooth; on closing,
      the TUI returns cleanly and the first key afterwards takes effect
      immediately.
- [ ] **Detached/launch editor with `:w` live apply:** leave the editor (launch
      mode) open and save inside it → the live reload still fires;
      `.done`/closing → the commit runs and the return renders.
- [ ] **Reopen on a validation error:** provoke a commit that fails → the
      editor opens again with the error buffer, stdin belongs to the editor
      again (no swallowed keys).
- [ ] **Interactive script (`x`):** the script takes over the terminal fully
      and receives input; "Press any key" and the return work.
- [ ] **The busy-banner second** and **active tracking** tick up per second /
      adaptively as under RD-1 — and as soon as they end, the app returns to
      real idle (the ticker is disarmed).
- [ ] **Resize:** enlarge and shrink the terminal window → an immediate,
      correct repaint (not "only on the next key").
- [ ] **Quit:** `q`/`:q` exits promptly.

### Real-data sweep

- [ ] Before every commit, grep the staged diff case-insensitively for your
      company name, its short form, `atlassian.net` and real JSESSIONID
      snippets — the result must be empty. (Substitute your actual company
      name, its short form and your internal host patterns; those patterns
      themselves do **not** belong in the repo.) Example hosts are
      `wiki.example.invalid`, example cookies `JSESSIONID=synthetic`, space
      keys `DEMO`.

## Stoat adapter (phase 0 — foundation)

Connection only: login, discovery, gateway (WS) and status mirroring. **No
tree yet** — `list()` returns empty; phase 1 fills it. Tested manually against
the private test instance (credentials **outside** the repo, the `username`
field carries the email address). Prerequisite: `stoat-adapter.yaml` and
`stoat.yaml` in `~/.config/not_yet_done/views/` (templates under
`docs/examples/views/`), with the real base domain filled in.

- [ ] **Discovery and login:** open the Stoat tab → banner `Connecting…`; if
      credentials are needed, the `NeedsCreds` form appears (field `username` =
      email, plus `password`). After submitting, the login goes through.
- [ ] **Ready:** after a successful WS `Authenticate` and `Ready`, the banner
      switches to `Ready`. The tree is empty (phase 0) — that is correct.
- [ ] **Wrong credentials:** use a deliberately wrong password → the login
      fails, the banner shows `Failed{reason}` with a readable message; a
      retry (`r`, or entering the credentials again) is possible.
- [ ] **Token persistence:** quit and restart the TUI → no second credential
      prompt (the session token is reused from SQLite), the banner goes
      straight from `Connecting` to `Ready`.
- [ ] **Heartbeat and idle:** leave the tab open (no input) → the connection
      stays up (a ping every 20 s); no idle-CPU regression (cf. RD-2 — the
      gateway task sleeps between pings).
- [ ] **Reconnect:** disconnect the network briefly (e.g. toggle Wi-Fi) → the
      banner falls back to `Connecting…` and returns to `Ready` automatically
      once the network is back (backoff ≤ 30 s).
- [ ] **MFA account:** (if available) log in with an MFA account → a clear
      "MFA not supported" error message instead of a hang.
- [ ] **Clean shutdown:** `:q` exits promptly; the gateway task is aborted when
      the adapter is dropped (no lingering process or socket).
- [ ] **Real-data sweep:** no real instance domain, email address or token in
      the repo (the templates use `chat.example.org`).

## Stoat adapter (phase 1 — read-only tree)

Browsing and reading. Builds on phase 0 (login, gateway and `Ready` must be
green). The structure (servers and channels) comes from the WS `Ready`
snapshot, message bodies are pulled over REST. **No live push** — after
connecting, press `r` if necessary so that the tree shows the freshly arrived
`Ready` state.

- [ ] **Server list:** after `Ready` (and possibly `r`), the `chats` view lists
      the servers. An empty tree right after logging in means `Ready` has not
      arrived yet — press `r`.
- [ ] **Channels:** drill into a server (`Enter`/`c`) → the text channels in
      server order. Voice channels appear but cannot be expanded (no content).
- [ ] **Messages:** drill into a text channel → the last ≤ 50 messages, **the
      newest at the bottom**. Columns: author, time, message.
- [ ] **Author resolution:** the author column shows user names (not raw IDs) —
      including for authors who were not in the `Ready` snapshot (they arrive
      via `include_users`).
- [ ] **Timestamps:** the time column shows date and time (decoded from the
      message ULID), plausibly ascending downwards.
- [ ] **Preview:** `p` on a message → the preview pane shows the full message
      body (multi-line correctly; the table row stays single-line).
- [ ] **DMs (optional):** a second view with `stoat:channel` at the top level
      (a commented-out example in `stoat.yaml`) lists direct and group messages
      instead of servers.
- [ ] **A deliberate phase-1 limit:** very long channels show only the newest
      ~50 messages; backfilling older ones comes later (cursor pagination).
      Not a bug.
- [ ] **Real-data sweep:** no real channel, message or user data in the repo
      (tests and fixtures use invented IDs).

## Stoat adapter (phase 2 — live layer)

Out-of-band updates without a manual `r`. Builds on phase 1. Requires a second
client (web or mobile) or a helper to post messages into a channel while the
TUI is open.

- [ ] **Auto-populate:** open the tab fresh. As soon as the banner reaches
      `Ready`, the server tree fills **by itself** — **without** `r`. (Phase 1
      still needed a manual reload here.)
- [ ] **Live message:** keep a text channel open in the TUI (message level).
      Post a message into **exactly that** channel from outside → it appears at
      the bottom within ~1 s, without a keystroke.
- [ ] **No foreign reload:** keep channel A open and post a message into
      channel B → A does **not** reload (no flicker, no cursor jump). Only the
      open channel level reacts.
- [ ] **Edit, delete, reaction:** edit, delete or react to a visible message
      from outside → the open channel level mirrors the change after a reload.
- [ ] **Reconnect resync:** disconnect the network briefly (or provoke a
      gateway disconnect) → banner `Connecting`, then `Ready`; afterwards the
      tree shows the current state again, without `r`.
- [ ] **Known cursor reset:** on a live reload the cursor jumps back to the
      default (it does not "stay at the reading position") — expected phase-2
      behaviour, not a bug.
- [ ] **A deliberate structural limit:** a **newly created or renamed** channel
      only appears after a reconnect (a fresh `Ready`), not immediately —
      structural live events are not wired up yet. Not a bug.
- [ ] **Idle CPU:** tab open, no activity → the CPU stays around 0 % (render
      loop 1b parks; invalidations only wake it on a real event).

## Stoat adapter (phase 2.1 — categories and tree)

The flat drill-down view has been replaced by a tree view:
`server → (category | uncategorised channel) → channel → messages`.

- [ ] **Tree instead of flat:** open the tab → the servers stand there as tree
      roots (indent and glyph). Enter/→ on a server expands **inline** into its
      categories **and** its uncategorised channels (no tab switch).
- [ ] **Order:** below a server the **uncategorised channels come first**, then
      the categories.
- [ ] **Expand a category:** Enter/→ on a category shows its channels inline
      one level deeper.
- [ ] **A channel drills into messages:** Enter on a channel (whether under a
      category or directly under the server) opens the **flat message list**;
      back/← returns to the same spot in the tree.
- [ ] **Completeness:** the uncategorised channels plus all category channels
      together cover every channel of the server — nothing missing, nothing
      duplicated.
- [ ] **A server without categories:** a server that has no categories shows
      all of its channels directly as uncategorised (no empty category
      branch).
- [ ] **Voice channels stay leaves:** voice channels appear but cannot be
      drilled into or expanded.
- [ ] **`/` tree find:** `/` on the server root → searches channels across the
      whole tree (collapsed nodes included).
- [ ] **Live inside the tree:** post a message from outside into a channel that
      is **drilled open** → the message list updates live (the phase-2
      behaviour is unchanged, no matter where the channel sits in the tree).

## Stoat adapter (phase 3 — write)

Drill into a channel (the flat message list). Four actions: `a` send, `e` edit,
`d` delete, `+` react. The endpoints were verified beforehand with `curl`
against the real instance (against `SavedMessages`).

- [ ] **Send (`a`):** `a` in the message list → an empty `$EDITOR` (markdown).
      Type text, save and close → status "Message sent"; the new message
      appears at the bottom of the list (via reload **and** live event). An
      empty buffer → nothing is sent, no error message.
- [ ] **Cancel sending:** `a`, close the editor **without** a change → nothing
      is sent.
- [ ] **Markdown is preserved:** send a message that starts with `#` (e.g.
      `# Heading`) → it is sent **verbatim** (no header is swallowed).
- [ ] **Edit (`e`):** select one of your own messages, `e` → the editor shows
      the **raw body** (no `#` header). Change the body, save → status "Message
      edited"; the row shows the new text plus an "Edited" flag. Saving
      unchanged → "No changes", no round trip.
- [ ] **Edit someone else's:** `e` on another user's message, change the body,
      save → a clean server error (403), no crash.
- [ ] **Delete (`d`):** `d` on one of your own messages → confirmation, then it
      is gone; the list updates.
- [ ] **React (`+`):** `+` on a message → an emoji picker (👍 ❤️ 😂 …), pick
      one → status "Reacted 👍"; the reaction shows up in a second client, or
      in the web UI after a reload.
- [ ] **Live echo:** sending, editing and deleting also fire the live event →
      the list is up to date in a second open pane as well.

## Stoat adapter (phase 4 — structural live events)

Structural changes arrive live, without a reconnect. Trigger them in the
**official Stoat/Revolt client** (or, since the create feature, directly in the
TUI, see the next section) in a server you administer — and keep the TUI next
to it with the Stoat tab open, showing the server tree (or a channel). The wire
shapes were verified beforehand with a WS capture against the real 0.13.7
instance.

- [ ] **Create a channel:** create a text channel in the web client → it
      appears in the TUI tree under the server (uncategorised) **without** `r`.
- [ ] **Rename a channel:** rename the channel in the web client → the new name
      appears live in the tree.
- [ ] **Delete a channel:** delete the channel in the web client → it
      disappears from the tree live (and from its category, if it had one).
- [ ] **Create a category:** create a category in the web client → it appears
      live as its own branch under the server.
- [ ] **Rename a category:** → the new title appears live in the tree.
- [ ] **Move a channel into a category:** assign a channel to a category → it
      moves live out of "uncategorised" into the category branch.
- [ ] **Delete a category:** → the branch disappears live; the channels it held
      slide back into "uncategorised" (or wherever the server reassigns them).
- [ ] **Rename the server:** → the server label updates live.
- [ ] **Cursor behaviour:** a structural change resets the pane cursor (a known
      reload limitation, not a bug) — the tree stays consistent.
- [ ] **Joining and leaving a server (negative check):** it appears or
      disappears **only after a reconnect** — deliberately not live (the
      phase-4 scope boundary).

## Stoat adapter — create a channel or category (`al` / `ay`)

Creating directly from the TUI. Prerequisite: a server you administer, plus the
`al`/`ay` actions in `stoat.yaml` (the server view and the `categories`
ChildDef). `al` and `ay` are **multi-character chords** — they do not live in
`keybindings.content` but are recognised generically through the view keymap
(`ContentView::yaml_action_chord_prefix`); the `a` is stashed as the chord
prefix and the second character triggers.

- [ ] **A channel under the server:** put the cursor on the **server row**,
      type `al` → a name form (one field) opens. Enter a name, confirm → the
      channel appears live under "uncategorised" (no `r` needed; the gateway
      echoes `ChannelCreate`).
- [ ] **A category under the server:** cursor on the **server row**, type `ay`
      → form → name → the category appears live as its own branch
      (`ServerUpdate` with the full category list).
- [ ] **A channel under a category:** cursor on a **category row**, type `al` →
      form → name → the channel appears live **inside the category branch**.
      (Two steps internally: create the channel plus a server PATCH — one step
      for the user.)
- [ ] **Empty name:** confirm the form with an empty or whitespace-only name →
      a clear error message, no nameless channel or category is created.
- [ ] **`ay` on a category row:** not bound (categories only exist under the
      server) → the `a` is stashed as a prefix, but `ay` triggers nothing and
      falls through cleanly (no hang, no error).
- [ ] **Cancel the chord:** type `a`, then `esc` or a non-matching key → no
      effect, normal operation continues.

## Stoat adapter — rename a channel or category (`R`)

Renaming from the TUI. Prerequisite: a server you administer, plus the `R`
action in `stoat.yaml` (both `channels` ChildDefs and the `categories`
ChildDef). `R` is a **single character** (not a chord) and opens — like `al`
and `ay` — a name form with one field; it is not pre-filled.

- [ ] **A channel under the server:** cursor on an **uncategorised channel
      row**, type `R` → form → new name → the channel row updates live (no `r`
      needed; the gateway echoes `ChannelUpdate`).
- [ ] **A channel under a category:** cursor on a channel row **inside the
      category branch**, `R` → form → new name → the row updates live (the same
      action, `PATCH /channels/{id}`).
- [ ] **A category:** cursor on a **category row**, `R` → form → new name → the
      category header updates live (`ServerUpdate` with the full category list;
      only the title of the target category changes, the channel assignments
      stay).
- [ ] **Empty name:** confirm the form with an empty or whitespace-only name →
      a clear error message, no rename happens.
- [ ] **A server you do not administer:** `R` on a channel or category without
      the permissions → the server rejects it with a clean error message, the
      tree stays unchanged.

## Stoat adapter — channel cut and paste (`a m` / `a p`)

Moving channels between categories. `a m` (cut) **marks** the channel under the
cursor — **nothing is deleted**; only `a p` (paste) reattaches it. It reuses the
generic `mark-move`/`paste-move` actions (the same keys as in Tasks), via
`invoke_action` and `ActionContext.marked`. Prerequisite: a server you
administer. Internally the move is a full-list PATCH of the server categories
(`update_server_categories`), applied live via `ServerUpdate`.

- [ ] **A channel into a category:** cursor on a channel row, `C` (status:
      "Marked … for move") → cursor on a **category row**, `P` → the channel
      moves live into that category and disappears from its old place.
- [ ] **A channel into uncategorised:** `C` on the channel, then cursor on the
      **server row**, `P` → the channel lands in the uncategorised branch.
- [ ] **Paste next to a channel:** `C` on channel A, then cursor on channel B
      (in another category), `P` → A lands in B's category (or in
      uncategorised, if B is uncategorised).
- [ ] **Cancel with a second `C`:** `C` on a channel, then `C` again on the
      same row → "Cut cancelled", no move on a later `P`.
- [ ] **Cancel by switching tabs:** `C` on a channel, switch tabs (e.g. `1`) →
      "Cut cancelled"; back on Stoat, `P` on a category → nothing happens (no
      dangling cut).
- [ ] **`C` never deletes:** after `C` the channel is still visible and
      unchanged; only `P` alters the tree.
- [ ] **`cut` in the top bar plus highlight:** the `C cut` hint sits in the
      **top action bar** (not in the status bar). After `C` it is highlighted
      in the accent colour (bold and underlined) for as long as a cut is armed;
      after `P` or a cancellation (a second `C`, or a tab switch) the highlight
      goes away again.
- [ ] **A different server:** `C` on a channel of server A, then `P` on a
      category or server B → a clean error message ("different server"), no
      move (categories are server-local).

## Stoat adapter — chat layout (`row_layout`)

The message list renders as a chat via `row_layout`: a meta line, the body and
a spacer. Drill into a channel (the split opens the list on the right).

- [ ] **Three lines per message:** every message occupies 3 terminal lines —
      line 1 `author  time`, line 2 the message text, line 3 empty.
- [ ] **Emphasis:** the author in the accent colour, the time dimmed (from
      `style: accent` and `style: text_dim`, overridable via `tui.yaml`).
- [ ] **No column header:** in the chat layout the `Author | Time | Message`
      header row is **not** shown.
- [ ] **Selection:** navigate with `j`/`k` → the selection background covers
      the meta line and the body line, but **not** the blank line between
      messages.
- [ ] **Scrolling:** a list with more messages than screen height → `j` to the
      end scrolls cleanly block by block; the selected message stays fully
      visible (not cut in half).
- [ ] **Actions unchanged:** `e`, `d`, `+`, `p` and `n` still act on the
      selected message (the whole block selection, not individual lines).
- [ ] **Other tabs untouched:** the Jira, Taiga and Postgres tabs still render
      as ordinary single-line tables (no `row_layout` → the old behaviour).

## Stoat adapter — markdown body (`markdown: true`)

The messages' content column is `source: content` plus `markdown: true`, so the
body renders multi-line and soft-wrapped (`ratatui-markdown`). Drill into a
channel.

- [ ] **All lines visible:** a message with several hard line breaks shows
      **every** line, not collapsed into one.
- [ ] **Soft wrap:** a long paragraph wraps at the pane edge; make the pane
      narrower → the same paragraph reflows across more lines (the row height
      grows with it).
- [ ] **Inline styling:** `**bold**`, `*italic*` and `` `code` `` appear
      emphasised; a `# heading` and `- lists` render as such.
- [ ] **Colours from the theme:** body text, headings and emphasis draw their
      colours from `tui.yaml` (the theme bridge) — nothing hard-coded;
      switching the theme changes them.
- [ ] **Selection = background only:** selecting the message lays the selection
      background over the meta line and all body lines **without** flattening
      the foreground colours (author = accent, body styling).
- [ ] **Empty body:** a message without text (e.g. an attachment only) does not
      break the layout — the body line stays empty, the spacer is intact.
- [ ] **Scrolling with tall rows:** very long messages (many body lines) scroll
      cleanly; the selected message stays fully visible.
- [ ] **Other tabs untouched:** columns without `markdown:`
      (Jira/Taiga/Postgres) keep rendering on a single line.

> Known cut-offs: the `/` search does **not** highlight matches inside the
> rendered body; code blocks have no background; syntax highlighting is
> optional and separate.

## Stoat adapter — smooth scroll (`smooth_scroll: true`)

Both `messages` levels have `smooth_scroll: true`. Drill into a channel with
many (ideally multi-line) messages.

- [ ] **Line by line instead of jumping:** ↓ scrolls the content up by **one
      physical line** — a tall message at the top is cut off gradually rather
      than pushed away as a whole block.
- [ ] **No snapping:** the highlight glides along with the content (it may be
      cut off at the edge) and does **not** jump to the top edge on every step.
- [ ] **Hand-over as soon as the next message is visible:** if the next message
      is already on screen, `j` moves the highlight on by **exactly one**
      message (not to the bottom edge). Scrolling up behaves the same way
      upwards.
- [ ] **A long message:** for a message taller than the remaining space, the
      highlight stays put and `j` only scrolls — until the first line of the
      next message appears at the bottom.
- [ ] **No invisible cursor:** when scrolling up, the highlight does not move
      to the previous message while only that message's blank line is visible —
      the cursor is visible at every step.
- [ ] **Actions hit the highlighted message:** `e`, `d`, `+` and `p` operate on
      exactly the currently highlighted message (even when it is cut off at the
      edge).
- [ ] **Half and full page plus g/G:** `Ctrl+d`/`Ctrl+u` scroll by half a pane
      height (in lines); `G`/`g` jump to the end/beginning and explicitly
      select the last/first message.
- [ ] **Bottom clamp:** at the end it is impossible to scroll past the last
      line; the last message stays flush with the bottom.
- [ ] **Reload and live events:** when a new message arrives or `r` is pressed,
      the scroll position stays sensible (no jump to the beginning).
- [ ] **Other tabs untouched:** tabs without `smooth_scroll` (Jira, Taiga,
      Tasks) still scroll discretely, entry by entry.

## Stoat adapter — @-mentions (`@username` plus slug round trip)

Revolt encodes mentions in the body as `<@USERID>`. Display and editing resolve
that the same way as Jira and Taiga do, via
`not_yet_done_content::slug::SlugTable`: display → `@username`, editor →
`@uu-slug` plus a CACHE section, and back to `<@ID>` on save. The completion
list is **server-scoped** (a GET on `/api/servers/{id}/members`, cached once
per server). Prerequisite: open a server channel that holds messages with
mentions.

- [ ] **Displayed as `@username`:** a message mentioning someone shows
      **`@username`** in the list, not the raw `<@01ABC…>` code (in the label
      row **and** in the markdown body).
- [ ] **An unknown ID stays raw:** a mention of a user who is **not** in the
      server (no cache entry) → `<@ID>` stays there verbatim, no crash.
- [ ] **Editing shows slugs plus CACHE:** `e` on one of your own messages with
      a mention → the editor shows `@uu-<name>` instead of `<@ID>`, and at the
      bottom the CACHE section `#### CACHE / available @mentions … ####` with
      all `@uu-…` of the server.
- [ ] **Slug round trip (no-op):** change **nothing** in the editor and save →
      "No changes" (the `@uu-…` are translated back to `<@ID>` correctly, the
      body stays identical — no accidental edit).
- [ ] **Insert a new mention:** copy an `@uu-<name>` from the CACHE section
      into the text in the editor and save → the mentioned user really is
      notified in the web client; afterwards the TUI row shows `@<name>`.
- [ ] **An unknown slug is an error:** type `@uu-nonsense` (not in the CACHE)
      in the editor and save → a clean error "unknown mention slug
      @uu-nonsense", nothing is sent, no crash.
- [ ] **Sending (`a`) with a mention:** `a` in the channel list → an empty
      buffer plus the CACHE section; insert `@uu-<name>`, send the text → the
      mention arrives in the web client, the TUI shows `@<name>`.
- [ ] **Server scoping:** in two different servers the CACHE section holds
      **different** member lists (only the members of the respective server).
- [ ] **DM and group:** in a direct or group channel (no server) the completion
      list is fed from the recipients (the `Ready` snapshot), not from the
      members endpoint.

> Known cut-offs: the member list is cached once per server per session (no
> live refresh when someone joins or leaves); the slug source is the user name
> (the server nickname is not taken into account yet).

## Stoat adapter — type icons in the tree (`icon:`) plus the empty channel

Two things that showed up together: uncategorised channels and categories share
the server level and looked identical; and a channel whose last message had
been deleted kept its expand arrow and unread marker (Stoat does not clean up
`last_message_id`). `icon:` is set per level in `stoat.yaml` (💬 channel, 📁
category), which is why the unread marker was switched to `🔔`.

- [ ] **Expand a server:** uncategorised channels carry `💬`, categories `📁` —
      distinguishable at a glance, with the same indentation as before.
- [ ] **A channel in a category:** carries the same `💬` as in the
      uncategorised branch.
- [ ] **Unread:** a channel with new messages shows `🔔 💬 name` (the marker
      first, then the type icon), both in the unread colour; after reading,
      only `💬 name` remains.
- [ ] **Fuzzy search:** `/` plus a substring → the match highlight still sits
      exactly on the matched part of the **label** (not shifted by the glyph
      width).
- [ ] **An empty channel heals itself:** open a channel whose last message was
      deleted → the list is empty, and afterwards the expand arrow and the
      unread marker disappear from the tree **without** `r` (the message query
      corrects `last_message_id`).
- [ ] **Counter-check:** a channel with real messages keeps its arrow and list;
      a new message brings the arrow and marker straight back.

## Stoat adapter — unread marker in the tab bar

The unread state used to end at the edge of the view: a Stoat tab in the
background looked like any other. Now the tab label itself carries the marker
(`tab.unread_marker`, defaulting to the view's `unread_marker`) and is
highlighted (`tab.unread_style`, bold by default). Only what a pane **currently**
holds is counted.

- [ ] **The marker appears:** stand in another tab while a new message arrives
      in the Stoat tree → the tab turns into `🔔 💬 9 Stoat`, in bold, without a
      keystroke.
- [ ] **The marker disappears:** open the channel and scroll to the newest
      message (ack) → marker and bold type go away at the same moment, the
      label is `💬 9 Stoat` again.
- [ ] **Other tabs untouched:** Jira, Tasks and the rest never show a marker
      (their rows carry no `unread` field).
- [ ] **Width:** when the marker appears, the bar shifts by the emoji width
      without character garbage; on a narrow terminal it may wrap onto a second
      line.
- [ ] **Configuration:** set `tab.unread_style: [italic]` in `stoat.yaml` → the
      label is italic instead of bold; `tab.unread_marker: ""` → only the
      highlight, no glyph.

## Stoat adapter — cursor on the first unread message (`cursor_on_open`)

When a channel was opened, the cursor sat on the **oldest** loaded message —
you had to scroll down to the point where you had stopped reading.
`cursor_on_open: first_unread` on the `messages` level puts it on the first
unread message instead, anchored at the top edge so that the unread ones are
below it.

- [ ] **The jump:** open a channel with several unread messages → the cursor
      sits on the **oldest unread** one, at the top of the pane; the unread
      messages are below it, the history you already read is scrolled away.
- [ ] **Everything read:** open a channel with nothing unread → the cursor sits
      on the **newest** message (at the bottom), not at the top.
- [ ] **Empty channel:** open a channel without messages → no jump, no panic;
      once the first message arrives, the cursor lands on it.
- [ ] **A reload does not move it:** scroll up in the channel, then press `r`
      or have a message sent from outside → the cursor stays where it was (the
      jump belongs to opening, not to loading).
- [ ] **Interplay with the ack:** scroll from the jump target through to the
      last message → `mark_read_on_reach_end` fires as usual, the markers in
      the tree and the tab bar go away.
- [ ] **The ack does not jump away:** select a message you have read (the ack
      triggers a reload) → the cursor stays on the **same** message, even if a
      new one has arrived meanwhile and all rows have shifted up by one
      position.
- [ ] **A deleted row:** delete the selected message from outside, then reload
      → no panic, the cursor stays in the same spot in the pane.
- [ ] **The second branch:** check both for a channel **inside a category** as
      well (the `messages` level is configured twice in `stoat.yaml`).

## Stoat adapter — upload files (`A` / `attach`)

`A` on a channel row opens the file picker (multi-select), uploads every file
and posts them as a message with an empty body. Revolt allows 5 files per
message, so larger selections are spread across several messages. A few small
throwaway files from `/tmp` are enough for the tests.

- [ ] **One file:** `A` on a channel → picker → pick one file → notification
      "Attached 1 file(s) to #channel", the message shows up live in the open
      list.
- [ ] **Multi-select:** 3 files at once → **one** message with all three.
- [ ] **Above the limit:** 6+ files → **several** messages (5 plus the rest),
      no error from the server.
- [ ] **Picker cancelled:** close the picker without a selection → nothing
      happens, no empty message.
- [ ] **Unread stays clean:** after the upload the channel is **not** marked
      unread (the adapter acks its own post).
- [ ] **Add a caption afterwards:** `e` on the posted message → add text →
      `:w` → the text sits above/next to the files, the attachments are
      preserved.
- [ ] **Both branches:** check for a channel **inside a category** as well.
- [ ] **A broken path:** include a file you cannot read in the selection → the
      others are posted anyway, and the message names the one that failed.

## Stoat adapter — attachments as nodes (`stoat:attachment`)

Every uploaded file is also a node below its message. Enter on a message
**with** attachments drills into the file list.

- [ ] **Drill down:** Enter on a message with attachments → a list with file
      name, size and type.
- [ ] **Without attachments:** Enter on an ordinary message → no drill-down, no
      empty list (a message only reports `has_children` when it has files).
- [ ] **`o` opens:** select a file, `o` → the OS viewer opens. All files of the
      message live in the same temp directory, so the viewer can page through
      them.
- [ ] **A second `o` does not re-download:** close the viewer, press `o` again →
      it opens immediately (the bytes are already in the temp directory).
- [ ] **`D` saves everything:** `D` → enter a target directory (`~` allowed,
      and one that does not exist yet) → **all** files of the message end up
      there.
- [ ] **The target directory is a file:** give the path of an existing file → a
      clear error message, no partial download.
- [ ] **No deleting:** there is deliberately no delete action at this level
      (Revolt cannot remove individual files from a message).
- [ ] **Both branches:** check below a channel **inside a category** as well.

## Inline images in the terminal (`images:` in `tui.yaml`)

A markdown column (`markdown: true`) draws `![alt](url)` as a **real image**
between the text lines when the terminal can do graphics (Kitty, iTerm2, Sixel
— detected automatically at startup). In the Stoat chat the images come from
the image attachments that the adapter renders into the body as markdown
images. Test in a channel with at least one screenshot attachment.

- [ ] **The image appears:** open the channel → the image is drawn at the spot
      where the attachment sits in the body, not as `[image: …]` text.
- [ ] **Loading afterwards:** right after opening, the placeholder shows
      briefly, then the image appears by itself — without a keystroke and
      without `r`.
- [ ] **Loaded once:** scroll over the message repeatedly, or press `r` → the
      image is **not** downloaded again (no flicker, no new requests in the
      `NYD_DEBUG=1` log).
- [ ] **Scroll clipping:** scroll through the message with `j`/`k` → the image
      is cut off cleanly at the pane edge and grows back in; it does **not**
      paint over the neighbouring rows or past the edge.
- [ ] **Split:** in the coupled chat pane (80 %), check that the image ends at
      the pane boundary and does not bleed into the channel tree on the left.
- [ ] **Height cap:** a tall screenshot (phone format) occupies at most
      `max_height` lines (default 20) — the rest of the conversation stays
      visible. Change the value in `tui.yaml` → it takes effect after a
      restart.
- [ ] **Width:** make the terminal narrower → the image scales with the column
      width, the aspect ratio is preserved.
- [ ] **Resize:** resize the terminal while an image is visible → no ghost
      image, no artefacts in the text.
- [ ] **Fallback without graphics:** the same view in a terminal without
      graphics support (e.g. `xterm`) → every line stays text (`[image: …]`),
      nothing else changes.
- [ ] **Switched off:** `images: { enabled: false }` in `tui.yaml` → as above,
      and the terminal is not even probed at startup.
- [ ] **Broken URL / not an image:** an attachment that cannot be loaded or
      decoded → the placeholder text stays, no panic, and it is **not** retried
      in a loop.
- [ ] **Non-image attachments:** a PDF or ZIP in the body appears as a link
      (`📎 name`), not as an image placeholder.
- [ ] **`i` still works:** `i` on the message still opens the images in the OS
      viewer — inline and external do not exclude each other.
- [ ] **Other views untouched:** a Jira issue with a markdown body renders
      unchanged (no reserved blank lines, no offset).

## Taiga adapter — edit-hang fix (timeout plus non-blocking editor)

Two layers against the "the app freezes completely on edit (`e`) on a Taiga
row" problem. Layer 1 = HTTP timeout plus reconnect retry in the adapter;
layer 2 = the editor's `prepare` runs off-thread instead of blocking.

**Layer 1 — timeout and reconnect:**

- [ ] **The default timeout applies:** without `request_timeout_secs` in
      `taiga-adapter.yaml` everything behaves as before; a healthy but slow
      instance still answers (default 20 s).
- [ ] **A dead socket gives an error instead of a freeze:** while the app is
      running, cut the connection to the Taiga instance hard (e.g. block the
      network or the tunnel), then press `e` on a row. Expected: after roughly
      the timeout, one reconnect attempt, then a clean error message ("Failed
      to load …" or a network error) — **never** a permanent hang.
- [ ] **A short timeout for testing:** set `request_timeout_secs: 3` and block
      the tunnel → the error arrives after ~6 s (2 attempts), the app stays
      usable.
- [ ] **`connect_timeout_secs` separately:** without a value it is
      `min(request, 10)`. With an explicit `connect_timeout_secs: 30`, for
      instance, establishing a connection on a slow line may take longer than
      10 s without aborting wrongly (a healthy but slowly connecting instance).
- [ ] **Reconnect heals a transient drop:** cut the connection briefly and
      restore it right away → the second (retry) attempt goes through, no
      user-visible error.

**Layer 2 — non-blocking editor dispatch (all adapters):**

- [ ] **The UI stays responsive:** `e` on a Taiga row over a slow connection →
      the notification "⏳ Opening editor: …" appears immediately and the TUI
      keeps accepting input meanwhile (scrolling, switching tabs), so it does
      not freeze.
- [ ] **The editor opens normally:** over a healthy connection `$EDITOR` opens
      as usual; the "Opening editor…" notification disappears when it opens
      (other notifications stay).
- [ ] **No double open:** press `e` again while "Opening editor…" is running →
      "Editor is already open", no second load.
- [ ] **Error notification:** if the `prepare` fails (dead socket), the status
      line shows the error and **no** empty editor opens.
- [ ] **Other adapters unchanged:** Jira, Postgres, Confluence and Stoat — `e`
      (or their respective editor action) still opens correctly; the inline and
      pause-tui editor profiles work (the `pending_editor_request` path).

> Known cut-offs: there is no explicit "cancel" during loading — the wait is
> bounded by the adapter timeout (layer 1) anyway, and the generation token
> discards a stale session if something is reopened meanwhile. The retry only
> covers transport errors, not HTTP statuses (4xx/5xx); the multipart upload
> (`upload_attachment`) has no retry (the form is not cloneable) but is
> protected by the timeout.

## Postgres — retry after a failed first load (empty tree)

Prerequisite: make the Postgres tab reachable or unreachable so that the first
`list databases` load fails (e.g. the tunnel target is down, or a short
`query_timeout_secs`).

- [ ] Open the Postgres tab, the load fails → banner "Fetch failed: list
      databases: …".
- [ ] **Press `r` → a reload is triggered** (the banner switches to "Retrying
      …"/"Connecting …", not silently nothing). That was the bug: in an empty
      tree there was no cursor row → the view actions (including `reload`) were
      not resolved and `r` fizzled out.
- [ ] Restore the connection, `r` → the databases load, the banner goes away.
- [ ] `f` (fuzzy filter) and `/` (search) are usable in the empty tree as well
      (the same root fallback logic).

## Postgres — `auto_connect: never` (no auto-connect, only `r`)

`adapter.auto_connect: never` in `postgres.yaml` — that is the default, so
the test holds without the line as well.

- [ ] App start: the Postgres tab does **not** load automatically; banner
      "Press `r` to connect".
- [ ] Switching subtabs (databases/tables/scripts) does not trigger an auto
      load either.
- [ ] `r` establishes the connection and the SSH tunnel and loads.

## `auto_connect` — the three load modes

`adapter.auto_connect` says when an instance may connect: `never` (the
default), `on_open` (the first time its tab is opened) or `startup` (while the
app comes up, unvisited). The older boolean `manual_connect: true|false` still
reads as `never`|`startup`.

### `never` — the default, and the startup login `startup` triggers

- [ ] **The default applies**: **remove** both the `auto_connect` and the
      `manual_connect` line from a view file that has an adapter. Start the TUI
      → the tab does not load, banner "Auto-connect disabled — press `r` to
      connect".
- [ ] **Local tabs opt back in**: `views/tasks.yaml`, `trackings.yaml`,
      `projects.yaml` and `sqlite.yaml` carry `auto_connect: startup`; those
      tabs are populated immediately at startup, as before.
- [ ] **An eager tab's login arrives immediately**: set
      `auto_connect: startup` in a view that requires a login (e.g.
      `kimai.yaml`), lock the password store (`gpgconf --kill gpg-agent`) and
      start the TUI. Expected: the credential popup is there **immediately**,
      even though the Tasks tab is active; its title starts with the tab name
      (`Kimai: …`). Enter → the popup closes, Kimai loads in the background;
      Tasks stays the active tab.
- [ ] **Escape**: press `Esc` instead of Enter → the popup goes away, no second
      popup pops up behind it, the TUI is usable normally. The Kimai tab shows
      the error/connect banner when opened; `r` restarts the login.
- [ ] **Two eager logins**: set `auto_connect: startup` on two views that
      require a login. Expected: **one** popup at a time, the second does not
      overwrite it. After the first is answered or cancelled, the second
      arrives at the latest when its tab is opened.
- [ ] **A manual-connect tab stays quiet**: a tab with `auto_connect: never`
      (or without the line) shows **no** popup at app start — only `r` on that
      tab asks.

### `on_open` — connect on the first visit

Set `auto_connect: on_open` on a remote view (e.g. `taiga.yaml`) that is
**not** the tab the TUI starts on.

- [ ] **Nothing at startup**: start the TUI. The tab shows **no** banner about
      connecting, no request goes out (check `f10` / the adapter log), and no
      credential dialog appears over the tab you are on.
- [ ] **The first switch loads it**: switch to the tab. Expected: the load
      starts right away without pressing `r` — the `Connecting/Busy` banner,
      then the table. Never the "Auto-connect disabled — press `r`" banner:
      an `on_open` tab must not tell you to press a key it does not need.
- [ ] **Its login lands on its own tab**: with the password store locked, the
      credential popup appears only when you switch to the tab, and its title
      names that tab.
- [ ] **Going back does not refetch**: switch away and back several times.
      Expected: the cached table each time, **no** new request. (Only
      `auto_reload` or an explicit `r` refetches.)
- [ ] **A failed load retries on the next visit**: break the connection (VPN
      off), switch to the tab → error banner. Switch away, restore the
      connection, switch back → it tries again by itself and loads.
- [ ] **No double load**: switch to the tab and immediately away and back
      while the first fetch is still running. Expected: one load, not two —
      the banner keeps a single elapsed counter.
- [ ] **Subtabs follow along**: once the tab has loaded, switching subtabs
      loads them transparently (one adapter, one connection) — no second
      "press `r` to connect".
- [ ] **The tab the app starts on**: make an `on_open` view the _first_ tab
      (lowest `tab.order`). Start the TUI → it loads at startup, because it is
      open from the first frame.

### Compatibility with the old boolean

- [ ] `manual_connect: true` alone still behaves exactly like
      `auto_connect: never`; `manual_connect: false` alone like
      `auto_connect: startup`.
- [ ] A file carrying **both** `manual_connect: true` and
      `auto_connect: startup` connects at startup — the newer key wins.
- [ ] A typo (`auto_connect: on-open`) fails the view file at load with a
      message naming `never`, `on_open` and `startup` — it does not silently
      fall back to the default.

## Connect progress — what the login is doing, and for how long

The connect banner names the step it is in and counts its seconds
(`StatusReporter`, `docs/content-adapter-spec.md`). Best seen on a view whose
login runs an SSO script: `jira.yaml` with `auth.bindings[].provider.type:
command`.

- [ ] **The counter moves**: start the TUI with the Jira tab visible. The
      banner reads `Connecting… running the cookie script (1/3) (0s/120s)` and
      the first number counts up once a second. The limits are the ones from
      the config (`retries:` and `timeout_secs:` of the provider), not a fixed 30.
- [ ] **The steps change**: after the browser login finishes, the banner walks
      on through `signing in` and `checking the session`, each restarting the
      counter at `0s` — the clock measures the current step.
- [ ] **The load says what it loads**: once connected, the banner reads
      `Loading issues… (Ns)` (not the unnamed `Loading… (Ns)` of the frontend's
      own counter) and disappears when the rows arrive.
- [ ] **The connect banner is gone when the data is there**: with the rows on
      screen, no `Connecting…` line is left standing — neither during the load
      nor after it. (A `Connecting` that is never ended keeps counting and
      reappears under every finished request, because that is what the request
      restored to.)
- [ ] **A drill-down names itself too**: open a ticket (`o`) and switch to its
      comments → `Loading comments… (Ns)`; attachments and links likewise.
- [ ] **A second attempt is visible**: point the provider at a script that
      exits non-zero (`script: false`) with `retries: 3`. The banner counts
      `(1/3)` → `(2/3)` → `(3/3)`, each with its own restarted clock, then the
      login fails with `Connection failed: …`.
- [ ] **A tab nobody is watching**: with `auto_connect: startup` on a
      background tab, switch to that tab mid-login → the banner is there with a
      counter that matches how long the login has really been running.
- [ ] **No banner when nothing is happening**: after a successful load the
      banner is gone and the CPU stays quiet (the periodic redraw is armed only
      while a live banner exists — `App::needs_periodic_tick`).
- [ ] **CLI parity**: `nyd cli adapter jira ls` prints one `nyd: Connecting… …`
      line per step on stderr (not one per second), then the rows on stdout.

## Postgres — an `auth:` block instead of the gpg pinentry

Prerequisite: switch `postgres-adapter.yaml` to the delegating form (see
`docs/examples/views/postgres-adapter.yaml`, section "One credential script for
the whole connection"): `auth.mechanism: password` with `script:`, the bindings
`password` and `ssh_password`, and both slots (`postgres.password` and
`transport.ssh[0].auth.password`) set to `{ type: script-result }`. Run once
with the gpg agent unlocked **and** a second time with it locked
(`gpgconf --kill gpg-agent`).

- [ ] Unlocked store: open the tab → the connection comes up without any
      prompt; **no** gpg pinentry window.
- [ ] Locked store: open the tab → **our** credential popup in the middle of
      the TUI (the header comes from the script, the passphrase is masked).
      Enter → the tunnel and the DB connection come out of **one** script run,
      so exactly **one** dialog for both secrets.
- [ ] Escape in the popup → the load aborts with a message, no repeated
      dialogs; `r` asks again.
- [ ] A wrong passphrase → the script shows its form again with its own error
      message, not a fresh empty dialog.
- [ ] Set `query_timeout_secs` low (e.g. 5) and wait longer in the popup → the
      timeout does **not** run while you are typing (the credentials are
      fetched before the clock starts).
- [ ] Configuration errors are rejected while reading, not on connect: remove
      the `ssh_password` binding but keep the hop delegating → the startup
      error names `transport.ssh[0].auth.password`; set the second hop to
      `script-result` as well → the error names `ssh[1]`.

## Tab order + auto-numbering

The `tabs.order` list in `tui.yaml` (tab names in display order).

- [ ] The tab bar shows the tabs in list order, with the digits
      `1`,`2`,`3`,… as key hints.
- [ ] Digits switch tabs (including `9` for a 9th tab such as Stoat, which
      previously had no key). `0` = 10th tab; from the 11th on, no digit.
- [ ] A view not named in `order` is hidden (but not unloaded) — it comes
      back as soon as its name is added again.
- [ ] `Tab` / `Shift+Tab` cycle through the visible tabs only.
- [ ] An unassigned digit (more digits than tabs) does nothing.
- [ ] `Ctrl+X` does nothing any more (the tab-group switch was removed).
- [ ] Clearing or removing `tabs.order` → all tabs in their natural slot
      order, auto-numbered the same way.
- [ ] Two tabs with the same `tab.name` → **a hard error message** as a
      startup modal; the app shows all tabs and stays usable.
- [ ] Editing `:config` / `tui.yaml` plus a reload → the order is resolved
      again (the active tab snaps to the first visible one if it dropped
      out).

## TaskAdapter (adapterized tasks tab) — A1b + A1c-1 + A1c-2

Prerequisite: copy `docs/examples/views/tasks.yaml` to
`~/.config/not_yet_done/views/tasks.yaml`. The adapter tab runs **alongside**
the native tasks tab (for comparison), this is not the C1 cutover.

- [ ] The tab loads: the forest as a tree, top-level tasks visible, drilling
      into subtasks to any depth; `priority` right-aligned, `created`
      localized.
- [ ] `a` (add) at the root → a markdown buffer with `## Description:` and
      `## Notes:`; enter a description, `:wq` → the new top-level task shows
      up with the cursor on it.
- [ ] `a` with a `parent:` field pointing at an existing task UUID → the task
      ends up as a subtask under that parent.
- [ ] Drilled into a task, `a` → the new task hangs below it as a subtask
      (the buffer has `parent:` prefilled).
- [ ] `e` (edit) on a task → the buffer shows the current fields plus notes;
      change the description, `:wq` → the row updates. The notes file is
      written along with it.
- [ ] `e` → clear the description → `:wq` → reopens with an error banner
      (the description must not be empty).
- [ ] `e` → change `status`/`priority` → applied. An invalid `status` →
      reopens with an inline error.
- [ ] `e` → `tracking: true` → tracking starts (visible in the native
      trackings tab); with `allow_parallel=false` other active trackings are
      stopped. `tracking: false` → stops it again.
- [ ] `d` (delete) on a task with subtasks → confirm → the whole subtree is
      gone, with the message "Deleted subtree (N tasks)". Notes are
      soft-deleted.
- [ ] `u` (undelete) → the most recently deleted task(s) come back; with no
      previous deletion → "Nothing to undelete".
- [ ] `a m` (mark-move) on task A, then `a p` (paste-move) on task B → A
      becomes a subtask of B. The "marked …" indicator is visible in between.
- [ ] `a m` on A, `a p` on A itself or on a descendant of A → error (the cycle
      is rejected), nothing changes.
- [ ] A mutation in this tab → the native tasks tab (if open) repaints or
      reloads via a DomainEvent.

### `edit node` (`ctrl+n`) — the subtree-restructuring outline editor

Parity with the old native tasks tab (`ctrl+n` = "edit node"). Prerequisite:
a `tasks.yaml` carrying the `edit node` action (`key: ctrl+n`, `type: edit`,
`id: edit-tree`) on all three levels (tree root, recursive subtask level, flat
`list` view — set that way in the example config). Create a task with several
subtasks and grandchildren.

- [ ] `ctrl+n` on a task → the editor opens with that task **and its entire
      subtree** as an indented checkbox outline (`- [ ] description (p=… id=…)`),
      children indented below their parent task.
- [ ] Re-hang a line (change its indentation) → `:wq` → the task is
      re-parented and its notes move along. In the tree the node sits at its
      new place.
- [ ] Change a line's status marker (`[ ]`→`[x]`) → `:wq` → the status is
      applied. Changing the priority via `(p=N)` → applied as well.
- [ ] Add a new line (indented to fit, without an `id=`) → `:wq` → a new
      subtask is created; nested new lines inherit the right parent.
- [ ] Delete a line (remove it from the buffer) → `:wq` → the task is
      soft-deleted (recoverable via `u`/undelete); whole removed subtrees
      disappear.
- [ ] Several changes in one buffer (re-hang, add, delete, status) → `:wq` →
      all applied in a single pass; the message names the tally.
- [ ] With `tracking.allow_parallel=false`, mark two lines with the `-t` flag
      → `:wq` → reopens with the error "Only one task can be tracked at a time
      …", the edits are preserved.
- [ ] Save the buffer unchanged → nothing changes (the diff is a no-op).
- [ ] `ctrl+n` is reachable in the flat `list` view (`v`) too, and at every
      drill depth (where it edits the selected task plus its descendants as an
      outline).
- [ ] After applying: the tree is re-snapshotted (DomainEvent → full reload),
      and the native tasks tab (if open) follows suit.

### A1c-1 — the tracking-marker column + the start/stop key

- [ ] The `⏱` column is visible between task and status. Tasks with a running
      tracking show `⏱`, everything else is empty.
- [ ] `t` (toggle-tracking) on an untracked task → `⏱` appears immediately
      (reload); the tracking shows up in the native trackings tab.
- [ ] `t` again on the same task → `⏱` disappears, the tracking is stopped.
- [ ] With `tracking.allow_parallel=false`: `t` on task B while A is running →
      A's `⏱` disappears and B's appears (exclusive, the native policy).
- [ ] With `tracking.allow_parallel=true`: `t` on B leaves A's `⏱` in place
      (both run).
- [ ] Tracking started via the `e` buffer (`tracking: true`) → `⏱` appears
      without an extra `t`; `t` toggles consistently afterwards.
- [ ] `t` and the `tracking:` buffer toggle stay in sync (no stale marker):
      after every toggle the column mirrors the live state.

### The tracking marker on a collapsed node (`collapsed_source`)

Prerequisite: a `tasks.yaml` with `collapsed_source: tracking_rollup` on the
`tracking` column (on the root **and** the recursive subtask level — set that
way in the example config). Create a task with (at least) one **subtask** and
start the tracking on the **subtask** (`t`).

- [ ] Parent task **expanded**: `⏱` sits on the running subtask, the parent
      task itself is empty (it shows its own `tracking`, which is empty).
- [ ] **Collapse** the parent task (`h`/←/`zc`): the `⏱` marker now visibly
      bubbles up to the collapsed parent task — the subtree's tracking state
      stays recognizable even though the running task is hidden.
- [ ] **Expand** it again: the parent task goes empty again and `⏱` is back on
      the subtask (the marker does not get stuck).
- [ ] Multi-level: tracking on a grandchild, then collapse two levels above it
      → `⏱` appears on the collapsed grandparent node (the roll-up spans the
      whole ancestor chain).
- [ ] A collapsed parent task **without** any tracking anywhere in its subtree
      stays empty (no bogus `⏱`).
- [ ] Flat list (`v`): the column still shows the node's **own** marker —
      `collapsed_source` is inert here (there is no collapsed state).

### A1c-2 — saved queries + FilterExpr filters (a filtered tree)

Prerequisite: a `tasks.yaml` with the `query:` block (default `open tasks`:
only non-`done`, non-deleted). Create at least one `done` task deep in the
tree and one open sibling task.

- [ ] The tab loads with the default query active: `done` tasks are missing
      from the tree, open tasks are there. An open task **below** a `done`
      parent stays visible — the `done` parent appears as an ancestor, with
      only the matching open child under it.
- [ ] Drilling into a filtered node shows **only** matching children (the
      filter applies at every depth, not just at the root).
- [ ] `q` opens the query menu: the default query `open tasks` is listed.
- [ ] `:query new <name>` with your own `FilterExpr` body (for example
      `[priority, ">=", 5]`) → save → it shows up in the `q` menu; applying it
      filters the tree live.
- [ ] `:query edit <name>` → change the body, `:wq` → the tree re-filters
      immediately (the old subtree cache is discarded, no stale children).
- [ ] `:query delete <name>` → it disappears from the menu; the body file
      under `…/tasks/<id>/<view>/queries/<name>.yaml` is gone.
- [ ] A query with 0 matches → an empty tree, and the reload action (`r`)
      stays reachable (no dead end).
- [ ] Clearing the query or dropping `default` → the whole forest is visible
      again.
- [ ] A structural mutation (add/delete/reparent) → the tree is
      re-snapshotted; the filter is lost until the query is sent again (an
      accepted lifecycle edge, see plan box A1c-2).
- [ ] A saved-query shortcut (Ctrl+f in the `q` menu) bound to a query → the
      key filters the tree directly and survives a YAML reload (the
      `query_shortcut` table).

### A1c (scripts) — `:script` / `x` on the adapterized tasks tab

Prerequisite: a `tasks.yaml` with the `run script` action (key `x`).

- [ ] `x` on a selected task → the script menu opens on the directory
      `<data>/not_yet_done/scripts/tasks/task_item/` (created automatically).
      `:script` from the cmdline opens the same menu.
- [ ] At every drill depth (subtask, sub-subtask) `x` yields the **same**
      directory (the view path is stable, one shared scripts folder).
- [ ] `+name<Enter>` creates a new script from the template and opens the
      editor.
- [ ] Run a script → it receives the task as JSON in the uniform node shape,
      **not** the native `{"task": …}` shape:

  ```json
  {
    "node": {
      "id": "<uuid>",
      "label": "<description>",
      "node_type": "task:item",
      "tab": "tasks",
      "fields": {
        "status": "…",
        "priority": "…",
        "tags": "…",
        "tracking": "…",
        "created": "…",
        "ancestors": "…"
      }
    }
  }
  ```

  `fields.ancestors` is a JSON array string of `[{"id", "description"}, …]`
  from root to parent (excluding the task itself); for a top-level task it is
  `"[]"`.

- [ ] Change the selection (another task, another type mix) → the menu stays
      on the same folder (no shuffling).
- [ ] No task selected / an empty tree → the notification "No row selected",
      no crash.
- [ ] Run the ported `task_to_taiga.py` (under `scripts/tasks/task_item/`) on
      a ticket task (`#<n> - …` under `<slug>/tickets/`) → the Taiga tab
      activates the per-project query and parks the cursor on the item
      `<slug>#<n>` — identical to the behaviour on the native tasks tab.

### Task-1 — `a` (child / top level), `A` (sibling), inheritance

`a` and `A` behave as they do in the native tasks tab:

- [ ] **Tree mode, `a` on a selected task** → an editor buffer with `parent:`
      prefilled to the selected task; `:wq` → the new task hangs **as a
      child** under the selected task (not as a sibling or at top level).
- [ ] **Tree mode, `A` on a selected task** → a buffer with `parent:` set to
      the **parent** of the selected task; `:wq` → the new task is a
      **sibling** (same level). On a top-level task → a new top-level task.
- [ ] **Empty tree (no tasks):** both `a` and `A` → a new **top-level** task
      (the engine resolves the missing selection to the adapter root).
- [ ] **Flat list (`v`):** `a` → a top-level task; `A` → a sibling of the
      selected task (same parent).
- [ ] `a`/`A`, then edit the `parent:` field in the buffer → `:wq` → the
      buffer override wins over the target the key had picked.
- [ ] `U` (Shift+U) on a nested task → the task moves to the topmost level
      (parent_id = None) and appears as a top-level node.
- [ ] `U` on a task that is already top level → the notification "already at
      the top level", nothing changes.

**Action inheritance** (the recursive `subtasks` branch declares no
`actions:` of its own):

- [ ] Drill into a task (the recursive level) → `e`/`a`/`A`/`x`/`ctrl+n`/`r`
      and the node actions `d`/`u`/`s`/`m`/`p`/`U` work there exactly as they do
      on the root level (all inherited via `inherit: true`).
- [ ] `f` (fuzzy filter) and `/` (tree find) are active **only** on the root
      level (not inherited — a validator rule).

## TrackingAdapter (adapterized trackings tab) — A2a + A2b + A2c

Prerequisite: `views/trackings.yaml` (from `docs/examples/views/`) copied to
`~/.config/not_yet_done/views/`. The adapter tab runs next to the bespoke
native trackings tab (until C1).

### A2a — read path + live durations + grouping

- [ ] Open the trackings tab (the adapter one) → a flat list, newest first,
      with the columns marker (`⏱` only while running), path (styled `/a › b`),
      task, started, ended (empty while running) and duration (`H:MM:SS`,
      right-aligned).
- [ ] Start a tracking on the tasks side → the running row shows `⏱`, an empty
      ended, and the duration **ticks adaptively** (fresh: every 5 s; from
      1 min: 10 s; from 10 min: 30 s; from 1 h: 60 s — just like the native
      tab; only that row is patched, no full-reload flicker).
- [ ] Stop the tracking → `⏱` is gone, ended is filled in, the duration is
      static; the ticking stops (no more continuous CPU).
- [ ] `zg` cycles the grouping (day → week → month → year → none) with a
      per-group sum plus a footer grand total.
- [ ] `q` opens the query menu; a saved FilterExpr query (for example
      `description ~ "<word>"`) filters the list; deleting it shows everything
      again.

### A2b — mutations

- [ ] `d` on a row → a confirm dialog; confirm → "Tracking deleted", the row
      disappears (the times are kept in the database).
- [ ] If the deleted row was **active**, the tracking marker of the associated
      task disappears on the tasks tab too (cross-tab via `TrackingChanged`).
- [ ] **Deleted rows dimmed:** adjust the query so deleted trackings fall into
      the visible set (for example `[deleted, =, true]`, or drop the `deleted`
      clause) → the deleted rows appear **dimmed** (the theme's `text_dim`).
      That also applies in the flat list **grouped by day** (the `── day ──`
      headers), not just in the ungrouped or tree view.
- [ ] **`d` on an already deleted (dimmed) row** → **no** confirm dialog, just
      the notification "Already deleted" (no re-delete).
- [ ] `t` on a row → starts or stops tracking on the row's **task**; with
      `allow_parallel_tracking` disabled another running tracking is stopped
      first (the same policy as the tasks tab).
- [ ] `R` on a visible **non-deleted** row → the notification "Restore failed:
      … not deleted" (only deleted rows can be restored — and a query is what
      makes those visible in the first place, see above).
- [ ] **`A` (restore all) is ALWAYS visible in the action bar in the flat list
      view** — right after switching to the tab as well, without drilling
      anywhere (a static `on_container` hint, no `parent:` shortcut any more).
- [ ] **`A` is scoped to the active query** (not the whole database): with the
      default query (which shows only non-deleted ones) `A` finds nothing → the
      notification "No deleted trackings to restore" (no confirm popup, since
      there is nothing to do). Important: a tracking deleted elsewhere that the
      active query does **not** make visible is **not** touched by `A`.
- [ ] Adjust the query so deleted trackings fall into the visible set (for
      example remove the `deleted` clause, or use `[deleted, =, true]`), then
      `A` → a **confirm popup** "Restore N deleted tracking(s)? …" (it names
      the count; where successors exist it adds "Purges M successor intervals —
      irreversible"). `n`/Esc aborts without a change; `y` restores **only the
      ones covered by the query** and reloads the list.
- [ ] `R` on a **deleted (dimmed) row** (one the query makes visible) → a
      **confirm popup** with the same purge warning; `y` runs it, `n` aborts.
- [ ] `x` opens the `:script` menu; a script run against the selected row is
      handed that row's JSON (`{json_file}`).

### A2c — condensed (adapter-side condensing, `tracking:condensed-row`)

- [ ] `v` switches to the **condensed** subtab; `a` switches back to the flat
      list.
- [ ] Condensed shows a `── 2026-… ──` header per **day** with that day's sum,
      and below it **one row per task** with the path, the task name and that
      task's summed duration **on that day**. A task tracked on two days
      appears twice (once per day).
- [ ] Two **different** tasks with the same name do **not** merge (the inner
      grouping keys on `task_id`, not on the label).
- [ ] `zg` rotates only the **outer** (day) level (day→week→month→year→none);
      the per-task breakdown stays. On `none` → one row per task across the
      whole filtered period.
- [ ] A condensed row is **selectable**; `d`/`t` act on the row's
      representative tracking (a known limitation: the action hits a single
      interval, not the task's whole daily sum).
- [ ] The saved-query filter (`q`) applies in the condensed subtab too.

### A2c — tree (own/cumulated, M4 `tree_aggregate`)

- [ ] `T` (Shift+t) switches to the **tree** subtab; `a` switches back to the
      flat list. Lowercase `t` remains toggle-tracking on the row — the two do
      **not** collide.
- [ ] The tree shows the **task forest** (tasks, not individual intervals);
      only tasks with tracked time **somewhere in their subtree** appear
      (untracked branches are hidden, the path down to tracked leaves stays
      visible).
- [ ] The `Duration` column initially shows the **cumulated** subtree sum
      (default `cumulated`). `zt` switches all `tree_aggregate` columns to the
      task's **own** duration (and back). (`zt` is only active because the
      trackings adapter reports `supports_tree_aggregation` — the capability
      gate. A `tree_aggregate:` in the YAML alone is not enough.)
- [ ] A parent task without a tracking of its own but with tracked children
      shows cumulated > 0 (own value 0:00:00 after `zt`).
- [ ] Drilling in (Enter/→) expands the subtasks; the same `tree_aggregate`
      column applies at every depth.
- [ ] The `⏱` marker appears on tasks with a running tracking; `t` starts or
      stops tracking on the selected task (sharing the exclusivity policy with
      the tasks tab) and the tree reloads.
- [ ] **Limitation:** no live tick in the tree (durations are baked at load
      time, as in condensed); an `r` reload refreshes them.

## Saved-Query-Shortcut-Validierung (Content-Tabs)

Saved-query shortcuts claim tab-wide on the view claim level and would shadow
every key dispatched after them (navigation keys, chords, …). Test both check
paths:

- [ ] **Set time:** open the q menu, press `ctrl+s` on a query (bind a
      shortcut), then press `j` → the modal "Shortcut 'j' is already taken by
      common.list_next!" and a re-prompt; `v` → a conflict with the subtab key;
      `w` → a conflict with a window chord (the leader prefix); `z` → a
      conflict with `content.cycle_grouping` (a chord prefix); `d` → a conflict
      with the YAML node action. `esc` aborts.
- [ ] A free key (for example `M`) is accepted: "Favorite … added".
- [ ] **Load time:** write a colliding row directly into `query_shortcut` (or
      make a config change that lets an existing shortcut collide) → at startup
      the notification "<Tab>: saved-query shortcut [x] ('name') shadows … —
      rebind it via the query menu" appears; the shortcut stays active.

## Column config (`c`) on content tabs

On every non-trackings tab, `c` used to open the **native tasks** column popup
(and applying it would have overwritten that tab's settings). It is now
generic, per level:

- [x] Adapter tab (for example "Trackings"): `c` shows the columns of the
      active view (not the tasks columns); `Space` hides a column (taskpath,
      say) → the table is rebuilt without it immediately.
- [x] Persistence: restart the app → the column stays hidden (the settings row
      `content_columns:<Tab>` as a JSON map).
- [x] Reset: re-enable the column and move it back to its YAML position with
      `Ctrl+D` → the override is removed and the settings row deleted
      (`SELECT key FROM settings WHERE key LIKE 'content_columns%'` comes back
      empty).
- [x] Tree mode ("Tasks"): `c` shows the columns of the cursor's level; the
      `tree_label` column (task) is fixed (`Space` has no effect); toggling
      another column (created) takes effect immediately, and reset works as
      above.
- [x] Native tabs (tasks/trackings): the popup is unchanged (display names,
      toggling, persistence in `tree_columns`/`tracking_columns`).
- [ ] Auto-fallback level (Postgres rows): `c` → the notification "This level
      has no configurable columns" (there is a unit test, untested live — it
      needs a connected Postgres instance).

## The default query + query-menu styling

The query menu (`q`) now shares its popup chrome with the column-config popup
(SearchablePopup renders via `popup_utils`), and `ctrl+t` marks the selected
saved query as the default, which is applied automatically at app startup.

- [x] Looks: the query menu (content and native), the script menu and the tag
      menu all show the unified chrome (rounded border, wrapped hint line, the
      cursor line highlighted instead of a colour bar); saved-query shortcuts
      appear as a `[key]` suffix.
- [x] Content tab ("Trackings"): `ctrl+t` on "2 months" → the notification
      "Default query: 2 months", the settings row `default_query:<scope>` is
      created; restart the app → the query is active (the action bar shows it)
      and the menu shows `★ 2 months`.
- [x] Toggle off: `ctrl+t` on the marked query → "Default query cleared", the
      settings row is deleted.
- [x] Native tasks tab: `ctrl+t` on "All" → a restart applies "All" even
      though "2 months" was active last (the default beats the last-active
      restore); toggling it off restores the old behaviour.
- [x] Postgres script menu: no `default` hint, `ctrl+t` has no effect (scripts
      are not queries; via `open_without_default`).
- [ ] A default query with mandatory variables (`{var}`): it is applied raw at
      startup (without the variable popup) — behaviour documented, untested
      live.

## Configurable tree lines and expansion markers (`tree_lines` / `tree_markers`)

Per tree (the root `ViewDef`) the box lines (`├──`/`└──`/`│`) and the expansion
markers (`▶`/`▼`) are configurable separately: `tree_lines: false` replaces the
lines with indentation (two spaces per depth level), and `tree_markers:`
overrides (`collapsed`/`expanded`) or hides (`enabled: false`) the markers.

- [x] Postgres tab (`tree_lines: false` in the user config): expand a database
      four levels deep → the schema and table levels are merely indented,
      without `├──`/`└──` lines; the `▶`/`▼` markers are still there.
- [x] A tab without any configuration ("Tasks"): lines and markers unchanged
      (the default behaviour).
- [ ] `tree_markers.enabled: false` (set temporarily): the lines stay, the
      markers disappear; expanding with Enter still works (unit-tested, open
      live).
- [x] With `tree_lines: false` the connector colour still colours the marker
      run (in the capture: markers in the `tree_connector` colour).

## Initial expansion depth (`expand_depth`) + list view (`task:flat`)

Tasks-adapter parity with the native tab: `expand_depth: 2` on the root
`ViewDef` expands depths 0 and 1 automatically after loading (a one-shot
cascade over the normal expand path, mirroring
`tasks.tree.default_expand_depth: 2`); the second view `list` (`node_type:
task:flat`, subtab key `v`, back with `t`) shows the whole forest as a flat
table in DFS order.

- [x] Open "Tasks": three levels are visible right away (roots, children and
      grandchildren expanded), deeper levels stay closed.
- [x] Collapse a node manually, then press `r` (reload): the node stays
      collapsed — the cascade is one-shot and expands nothing against the user
      once it has finished.
- [x] Press `v`: a flat list of all tasks (every depth, no markers or
      indentation, DFS order); `t` switches back to the tree with the expansion
      state preserved.
- [x] In the list view: `e` opens the edit session of the selected row just as
      in the tree (`s` toggle-tracking uses the same invoke path; deliberately
      not pressed live — it would start or stop a real tracking).
- [ ] Apply a saved query in the list view (`q`): only the matches themselves
      appear, no ancestor rows (unit-tested, open live).

## The default query on all trackings subtabs (`query.inherit_default`)

The user default (★ in the q menu) is only stamped onto the tab's default view
at startup. `query.inherit_default: true` (condensed and tree in
trackings.yaml) stamps it onto the respective subtab as well; the tree filters
adapter-side there (the projection is re-folded from the visible trackings,
`propagates_query_to_subtree`).

- [x] Start the app with a ★ default: the normal, condensed AND tree subtabs
      all show the default query's name as the active query in the action bar
      (the limitation is unchanged: a default with a `{var}` variable is
      applied raw, i.e. effectively unfiltered — as on the default view).
- [ ] A subtab without `inherit_default` (the tasks list view, say): the
      default query still does NOT apply there (opt-in behaviour; unit-tested,
      open live).
- [x] In the tree subtab, `q` → apply a saved query: the root shows the
      filtered sum; expanding the branches stays filtered (only branches with
      visible time, and identical sums all the way up the chain when a single
      branch matches).
- [x] Back to the flat list after applying (`a`): its own query is unchanged
      (the pane state stays separate; only the startup default is shared).

## The group-by menu (`u`) on content tabs (`content.group_menu`)

Direct-jump parity with the native trackings `u`: a hotkey popup over the five
`zg` states (no grouping/day/week/month/year). Only active when the level
configures a `group_by:`; the choice is view state (not persisted, like `zg` —
the native one persisted via `SaveTrackingGrouping`).

- [x] Trackings, normal subtab: the action bar shows `u group`; `u` opens the
      "Group by" popup in the native look (standard chrome, `●` marks the
      current state (day), the hotkey letter underlined in the label, the
      keybinding legend at the bottom).
- [x] `w` jumps straight to weekly grouping (the header `── W24 2026`) with
      per-week sums; `u` → `n` removes the grouping (a flat list; the aggregate
      column and the Σ footer disappear, as with `zg` on "ungrouped").
- [x] Arrows plus Enter/Space select as well; Esc closes without a change.
- [x] Condensed subtab: `u` → `m` rotates only the day-bucket level to month
      (`── 2026-06`), the adapter-side per-task breakdown stays.
- [x] On a level without a `group_by` (tasks, say): no `u group` hint, `u`
      stays free for YAML actions.

## The trackings tree: always expanded, no markers (`expand_depth: all`)

Native parity for the trackings tree subtab: the legacy tree was always fully
open and had no expansion markers. `expand_depth: all` (a new value; the
cascade runs until nothing expandable is left) plus
`tree_markers.enabled: false` in trackings.yaml.

- [x] Trackings → `t` (tree): the whole tree is fully expanded right away —
      every level visible, without pressing Enter.
- [x] No `▶`/`▼` markers in front of the rows; the box connectors
      (`├──`/`└──`) stay.
- [x] Collapse a branch manually (Enter), then switch subtabs and come back:
      the state holds — the cascade is one-shot and expands nothing against the
      user again.
- [x] Apply a saved query (`q`): the filtered tree is fully expanded right
      away too (a new query re-arms the cascade). That holds with the cursor
      deep in the tree and after manual expanding and collapsing as well (a
      regression: an out-of-range cursor aborted the table rebuild → a stale
      display).

### Deep trees expand completely (the cascade stays armed)

Bugfix: the `expand_depth: all` cascade is pumped once per asynchronously
arriving child level. With several sibling branches loading in parallel, one
branch could run out (hit a leaf) **while** another was still in flight — the
pump for that leaf found nothing left and disarmed the cascade prematurely.
The consequence: only the top one or two levels expanded, deeper branches
stayed closed. Fix: the cascade only disarms once no already-expanded level is
still waiting for its children.

- [ ] Trackings → `t` (tree) with a **multi-level** task tree (≥3 levels,
      several siblings with branches of differing depth): after loading, the
      tree is **completely** open down to the last tracked leaf — not just the
      top two levels. Exactly as much is visible as in the native trackings
      tab.
- [ ] The grouped tree (day buckets) also expands every bucket subtree
      completely, not just the first task level.

### `s` (toggle-tracking) refreshes the view immediately

Bugfix: `s` mostly did **not** refresh the TUI in the trackings tabs (flat /
condensed / tree) right away. The reason: the toggle returned `Noop` and
relied on bridge row patches, or (in the tree) on a `PatchRow` dispatch.
Neither often hit the visible row — a _start_ creates a new, still invisible
interval (no row to patch), and `patch_row` only searches the depth-0 rows, so
deeper tree nodes were not refreshed at all. The `Noop`/`PatchRow` solution
only existed because a full `Reload` used to trigger the O(N²) expand cascade
(slow, and it blocked input).

Fix: with the eager-subtree improvement (`supports_eager_subtree`) a `Reload`
renews the entire expanded tree in **one** `list_subtree` call. The toggle
therefore simply returns `Reload` in all three views (identical to the tasks
logic) — re-folding own/cumulated values, ancestor aggregates and markers
consistently. The `PatchRow` dispatch is gone entirely.

> During the smoke test do **not** toggle a real tracking on real rows —
> create a throwaway task and track on that one.

- [ ] Trackings → tree, deeply and fully expanded: `s` on a **nested** row
      flips that row's `⏱` marker **immediately** (on when starting, off when
      stopping); the tree stays fully expanded, the reload is quick (no
      seconds-long collapse or input freeze), and the selection stays on the
      row. The ancestors' cumulated seconds are correct without an extra `r`.
- [ ] Trackings → flat list (`a`) and condensed (`v`): `s` on a running row
      stops it (`⏱` gone, the duration frozen); `s` on a stopped row starts a
      new interval that becomes visible immediately.
- [ ] Tasks → tree: `t` (toggle-tracking) flips the row's `⏱` marker
      immediately (unchanged — it already used `Reload`).

## The trackings tree: grouping via the adapter (`group_by_via_adapter`)

Native parity, point (3): the legacy tree grouped by day (one group header per
day, with the task tree below it showing only that day's durations). The
generic mechanism: the engine passes the active `group_by` through in the root
`list()`, the adapter returns `tracking:tree-group` bucket nodes with subtrees
folded per bucket; `zg`/`u` mean a reload.

- [ ] Trackings → `t` (tree): day groups as `── label` header rows (not
      selectable, header style, the label as in the grouped flat list:
      `W24 2026-06-08 Mon`), newest day first. The task rows below them start
      at indentation 0 (no extra level under the header). Fully expanded (the
      expand_depth cascade), and quick to build (no seconds-long build — folds
      and query resolution are memoized per snapshot).
- [ ] The subtree under a header: the durations are those of the respective
      day (the same task under two days shows different values). The columns
      are as in the native tab: `⏱`, task, own, cumulated; on top of that a
      **total** column closes each day on its last row (the timesheet layout).
      The cursor skips the header rows.
- [ ] `zg` rotates day → week → month → year → no grouping → day; every step
      reloads. "No grouping" shows the unbucketed task tree without headers and
      without the total column (as before this feature). The `u` menu jumps
      directly, `●` marks the active state.
- [ ] A saved query (`q`) on the grouped tree: buckets and subtrees are
      re-folded from the visible trackings; empty buckets disappear. The
      grouping state survives applying the query.
- [ ] `s` (toggle-tracking) works on a task row inside a bucket; on a bucket
      row `s` is unbound (a read-only aggregate). ⚠ during the smoke test only
      toggle on a throwaway task.

### `s` in the grouped tree refreshes only the now bucket

In the **grouped** tree (by day, for instance) every bucket is a subtree
aggregated on its own. An `s` (start/stop) only moves the totals of the bucket
that **"now"** falls into — with day grouping that is today, and generally the
bucket of the currently running or most recently touched entry. Instead of
re-folding the whole forest, the frontend therefore reloads **only that one
bucket**: the adapter sends the payload-free `Invalidation::NowAnchored`, the
frontend asks `bucket_for_now(spec)` (the most recent tracking → its bucket),
fetches that bucket's header and subtree and splices them in place; all other
buckets (including their expanded/collapsed state) are left untouched. A
_start_ that creates the period's first entry produces a brand-new bucket → the
frontend then falls back to a full pane reload (so the new bucket appears in
its sort position).

> ⚠ during the smoke test only toggle on a throwaway task, never on real rows.

- [ ] Trackings → `t` (tree), grouped by day, several days expanded: `s` on a
      task row in **today's** bucket flips that row's `⏱` marker and updates
      that bucket's daily total **immediately** — the **other** day buckets do
      not flicker, do not collapse, and their totals stay unchanged. The
      selection stays put.
- [ ] An `s` that creates the **first** entry of the current day (with no
      bucket for today visible before): the new day header appears in the
      correct sort position (the full-reload fallback), the remaining buckets
      stay expanded.
- [ ] "No grouping" (the unbucketed tree): `s` reloads the whole (single) tree
      as before — no now-bucket special case, no regression.

### The live tick in the grouped tree counts up only the now bucket

The **static** tree fold bakes all durations against the snapshot's timestamp —
merely reloading the same snapshot therefore does _not_ tick. To make the
durations in the grouped tree count up live, the adapter folds **only the now
bucket** freshly against the current time on every timer tick: the new hook
`live_group_rows(spec, query)` returns the bucket header (with the total summed
up anew) plus the **running chain** (the running task and its ancestors, whose
cumulated durations grow along) as `Invalidation::Row` patches — only rows that
actually move, everything else is left alone. The framework timer now merely
fires a payload-free `LiveTick`; the folding happens in the frontend.

**Background-tab behaviour (deliberate):** a tick from a tab that is _not_
active has **no** effect on the current tab — it is not redrawn. The tick is
only noted as a flag (`pending_live_refresh`) and evaluated **when you switch
back** to its tab, and then **coalesced**: however many ticks piled up while
you were away, switching back runs exactly one fold against the state as of
then.

> ⚠ during the smoke test only toggle on a throwaway task, never on real rows.

- [ ] Trackings → `t` (tree), grouped by day: start an `s` on a throwaway
      task. In **today's** bucket the task row, its ancestors and the daily
      total **count up live, second by second** — the **other** buckets stand
      still, do not flicker and do not collapse. The selection stays.
- [ ] While the entry is running, switch to a **different** tab and stay there
      for a few seconds: the current tab does **not** redraw because of the
      trackings tick. Switch back → the durations jump to the now-correct value
      **in one step** (no catching up on every single missed tick).
- [ ] Idle (no running entry): **no** live patches happen — the tree stays
      quiet, no unnecessary redrawing.

## Live freshness for tasks/trackings: instant markers, external starts, adaptive ticking

Three freshness fixes for the adapter tabs: (1) a root reload now renews all
**expanded** tree levels too (their cached children used to stay put → `⏱` did
not appear on nested tasks right away); (2) the new trait hook `revalidate()` —
on a tab switch the task and tracking adapters diff the running trackings
against the database and reload on drift (external starts and stops via the
CLI or waybar); (3) the live duration ticks adaptively instead of every second
(5 s → 10 s → 30 s → 60 s, native parity).

- [ ] Tasks, tree expanded: `s` on a **nested** task → `⏱` appears on the row
      immediately (no collapsing or reloading needed); `s` again → the marker
      is gone immediately. ⚠ only toggle on a throwaway task.
- [ ] Trackings flat list: a running row first ticks every 5 s, and after a
      minute noticeably less often (10 s jumps); the CPU stays quiet. After a
      stop the ticking ends.
- [ ] Start a tracking externally (via the CLI `task track …` or waybar, say)
      while another tab is active → switch to tasks: `⏱` is there; switch to
      trackings: the new running row is there and ticks. Stop it externally →
      a tab switch shows the stop.
- [ ] `r` on tasks or trackings picks up the same external change manually —
      in the **tree** with expanded levels as well (where the old state used to
      stay put).
- [ ] Trackings tree/condensed after a toggle or reload: durations are
      consistently fresh (under group headers too).

## Eager subtree (`supports_eager_subtree`, `list_subtree`)

With `expand_depth: all` or `expand_depth: N`, in-memory adapters (tasks,
trackings) return the whole expected subtree in **one** `list_subtree` call
instead of the per-node cascade. The result has to look **identical** to the
cascade — the same rows, the same order, the same expansion depth — just
without the level-by-level loading.

- [ ] Tasks tree view (`expand_depth: all`): after loading, the entire forest
      is open right away — no visible level-by-level unfolding. Test a depth of
      ≥ 3 levels (task → subtask → sub-subtask).
- [ ] Selection/collapse: put the cursor on a deep node, `zc`/collapse and
      expand again → the state is correct (the path scheme matches the
      cascade's).
- [ ] Trackings tree view: same again, fully expanded in one go.
- [ ] `:tree-find "Tasks" id:<uuid>` (via `goto_task`, say) still lands on the
      right node — the eagerly loaded tree is fully searchable.
- [ ] An `r` reload on the eager tree renews every level (a freshly started
      tracking shows `⏱` on a nested task, for instance).
- [ ] Counter-check with a remote tree (Postgres or Confluence, where
      `supports_eager_subtree` is false): it still expands **progressively**
      (level by level) and the UI does not freeze — the eager path deliberately
      does not apply there.

## Fuzzy filter — substring highlighting (tasks/trackings)

Parity with the native tasks tab: the matched substring is highlighted (the
theme's `accent`, bold). In tree mode inside the **label** of the `tree_label`
column (the box connector keeps its `tree_connector` colour), in flat mode
inside the searched columns.

- [ ] Tasks tree view: `f` plus part of a task title → in the remaining rows
      exactly the matched substring is coloured and bold; the box connectors
      (`├──`/`└──`/`▶`) keep their own colour.
- [ ] Several tokens (`foo bar`) → both matching runs in the same label are
      highlighted.
- [ ] A row that only matches via another field (a tag, say) shows **no**
      marking in its label (no bogus highlight).
- [ ] Clear the filter (`esc`) → the highlighting disappears, labels are
      normal again.
- [ ] Flat view (`v`): `f` plus text → matches in the searched columns are
      highlighted; columns that are not searched stay unmarked.
- [ ] A very narrow column or a long label: the highlight stays correctly
      clamped (no panic, and it does not paint over the connector).

## Jump mode (`J`) on content tabs (`content.jump_mode`)

Parity with the native tasks tab's jump (`p` there), here on `Shift+J`. The
default binding is `J`; configurable via `keybindings.content.jump_mode`. The
label alphabet comes from `navigation.jump_chars`.

- [ ] Tasks: press `Shift+J` → the jump overlay is active (the action bar
      shows `J jump`). Type a character that occurs in several visible rows →
      every matching row gets a label, non-matches are dimmed.
- [ ] Type a label → the cursor jumps to the corresponding row.
- [ ] A character that occurs in only **one** visible row → an immediate jump
      without the label phase.
- [ ] A character with no match → the overlay closes, the selection does not
      change.
- [ ] `esc` while the overlay is up → abort, the cursor is unchanged.
- [ ] In a split: `Shift+J` acts only on the **focused** pane; after switching
      panes the jump works there too (a freshly created pane).
- [ ] Trackings (list/condensed/tree): `Shift+J` behaves the same way.
- [ ] The native tasks tab is unchanged: `p` still opens the jump there.

## Stoat: unread highlighting (`unread_style` / `unread_marker`)

Channels and categories with unread messages, plus the headers of unread
messages themselves, are highlighted (a marker glyph plus the theme colour
`unread`, both overridable per view). The source is the Revolt read state
(`sync/unreads` plus acks); a live reload happens on every incoming message.

Prerequisite: the Stoat tab open, the gateway `Ready`, in a server with at
least one channel that holds unread messages.

- [ ] A channel with unread messages shows the marker (💬 by default)
      **before** the channel name in the tree, the name in the `unread` colour
      (`#89b4fa` by default, bold).
- [ ] The category containing such a channel is marked as well (an OR over its
      channels).
- [ ] Drill into the message list: unread messages have a highlighted header
      (the author/time line) in the same colour; read messages look normal.
- [ ] The marker width is right: the emoji (2 cells) does not shift the
      indentation or the following columns, and no connector is cut off.
- [ ] Fuzzy filter (`/`) on an unread channel: the matching runs stay in the
      fuzzy-match colour (which wins over the unread colour), the rest of the
      label is in the unread colour.
- [ ] **Send** a message in an unread channel → the channel and category
      markers disappear (ack on send), without a manual `r`.
- [ ] A new message arrives in another channel → that channel and its category
      are marked live (no manual `r`).
- [ ] With `unread_marker: ""` set in the view → no glyph, but the name and
      headers are still in the unread colour.
- [ ] With `unread_style:` set to another theme colour name → the marker and
      the name/headers use that colour.

### Ack when the cursor is on the newest message (`mark_read_on_reach_end`)

The channel marker also disappears when you move the cursor to the bottom-most
(newest) message of the list — without sending anything and without a manual
`r`. It is configured via `mark_read_on_reach_end: mark-read` on the message
level (both branches in `stoat.yaml`: channels inside and outside a category).
If the hook is missing from the view config, **nothing** happens (no ack, the
marker stays forever) — the most common cause when an older `stoat.yaml` is
installed.

The tree marker (channel plus category) is cleared **locally**: `mark_read` is
the single choke point for "read" and sends `Invalidation::All` itself as soon
as the read mark advances. That way the tree repaints immediately, regardless
of whether the server plays our own ack back as a `ChannelAck`.

- [ ] Drill into an unread channel that has **several** unread messages → the
      unread headers are highlighted and the cursor sits at the top or on one
      of the upper rows; the channel and category markers are still visible (no
      auto-ack on opening).
- [ ] Move the cursor with `j`/arrow-down down to the **bottom-most** (newest)
      row → the channel and category markers disappear in the tree (the ack),
      and the list's header highlighting fades after the reload.
- [ ] Press a key at the end of the list again, or move up and back down →
      no repeated ack flicker (idempotent: the row is read now).
- [ ] Drill into an already read channel and move to the end → nothing changes
      (there is nothing to ack).

## The shortcut menu — the keybinding editor (Ctrl+Y)

User documentation: [`keybinding-editor.md`](keybinding-editor.md). Every edit
action triggers an immediate reload; **always** check that comments and
formatting survive in the affected YAML (`git diff`).

- [ ] `Ctrl+Y` opens the menu; `Tab` switches between the context and all
      tabs; typing filters; keyless actions are listed too.
- [ ] `Ctrl+N` on a view action → record, a single key, `Return` → "Bound …";
      the key fires immediately; `views/*.yaml` gains the `key:` and the
      comments survive.
- [ ] `Ctrl+N` with a chord (`Ctrl+K` then `L`) → stored as
      `key: "ctrl+k l"`; `Backspace` discards the last step; `Esc` aborts (no
      change).
- [ ] `Ctrl+N` on a read-only row (a saved query or script) → a hint, no
      recording.
- [ ] A second binding for the same action → the list `key: [alt1, alt2]` (the
      alternative is appended, the old key stays).
- [ ] `Ctrl+D` with a single binding → gone (`[]` for a built-in in
      `tui.yaml`, and the default no longer fires).
- [ ] `Ctrl+D` with several bindings → a picker; choose one, `Return` → the
      remaining list is written; `Esc` aborts.
- [ ] `Ctrl+R` on a built-in → the `tui.yaml` override disappears and the
      default applies again; on a view action → a no-op.
- [ ] Conflict: record a binding that takes an overlapping binding (including
      a global built-in, or the prefix `k` versus `k l`) → a `y/n` prompt.
      `n`/`Esc` → nothing happens. `y` → the other binding loses the key, the
      new one applies; both files are reloaded.
- [ ] A conflict against a read-only shortcut → reported as unresolvable, the
      binding is rejected.
- [ ] Different tabs never collide (the same key in tab A and tab B → no
      prompt).

## The notification bar — display limit + the notification centre (`f10`)

`notifications.max_messages` limits how many messages the **bottom** bar shows
at once (`0` = unlimited); `notifications.history_limit` limits the running log
of both bars.

- [ ] `max_messages: 1` in `tui.yaml`: trigger two messages in a row → only
      the **newer** one is at the bottom; the older one has been displaced.
- [ ] The upper alert bar is untouched: several `prominent` messages still sit
      up there all at once.
- [ ] `f10` opens the notification centre: both bars' messages on one page,
      **newest first**, each with an `HH:MM:SS` stamp and a marker — `✖` for a
      message from `notify_error`, `▲` for one of the top bar's, `●` otherwise.
      The heading counts them (`5 messages, 1 error`).
- [ ] `j`/`k` move the cursor (a `▍` bar in front of the row); the entry under
      it is unfolded in place, wrapped over the full width, with its line
      breaks kept. A message that already fitted is **not** repeated below
      itself.
- [ ] `Ctrl+D`/`Ctrl+U` and the page keys move a screen, `g`/`G` to the ends;
      more entries than fit show a scroll bar on the right edge.
- [ ] `y` copies the message under the cursor **without** its timestamp; the
      page confirms it (`✓ message copied (N chars)`) and the notification bar
      stays out of it — nothing about the copy appears in the log.
- [ ] `Y` copies everything listed as a timestamped log, alert lines marked
      with `!`, continuation lines indented — the same text `o` opens.
- [ ] `e` narrows the page to the errors (`3 of 12 — errors only` in the
      heading) and back; the entry under the cursor keeps its place across the
      toggle. With the filter on, `Y` and `o` leave the other messages out too.
- [ ] `e` with no error in the log → "No errors in the log — press e for
      everything."
- [ ] `o` closes the page and opens the same log read-only in the editor;
      saving or closing changes nothing.
- [ ] `Esc`/`q` close the page; every other key is swallowed rather than
      closing it (unlike the `f1` overview).
- [ ] `Z` (dismiss) clears the bars, the log stays: `f10` still shows the
      dismissed messages.
- [ ] With no message at all: `f10` opens the page reading "Nothing has been
      reported yet." — not a notification.
- [ ] A log spanning two days (leave the TUI open overnight, or set the clock)
      shows a dated rule where the day turns.
- [ ] The hint in the bottom right names the actual keys
      (`[Z] dismiss  [f10] open`) and follows a rebind of the two actions.
- [ ] A `:config` reload (saving tui.yaml) loses neither the open messages nor
      the log.
- [ ] Rebind both to a **chord** (`show_notifications: z l`,
      `dismiss_notifications: z c`) and reload: `z l` opens the notification
      centre just like `f10` did, and `o` from there still opens a `pause_tui`
      editor profile. This is the regression — the chord branch used to swallow
      the editor request, so every editor-opening global action was a silent
      no-op on a chord while the same action worked on a single key.

## The builtin editor (`builtin: true`, the `vimrealm` crate)

An editor profile with `builtin: true` edits in a pane of the TUI instead of
launching an `$EDITOR` process. The profile lives in `tui.yaml`, for example:

```yaml
editors:
  compose-builtin:
    builtin: true
    height: "30%"
    line_numbers: false
```

Wired up through an action's `editor:` field (Stoat: `views/stoat.yaml`,
actions `new`/`edit`). Back to the real nvim = `editor: compose-below`.

- [ ] Stoat channel → `n`: the pane appears at the **bottom**, above the
      message bars, and its title names the action; the tab, action and status
      bars stay put.
- [ ] Normal mode works: `i a o`, `hjkl`, `w b e`, `0 ^ $`, `dd`, `cw`, `x`,
      `p`, `u` / `Ctrl+R`, counts (`3w`, `2dd`).
- [ ] `q` on its own no longer quits the **app** while the pane is open; `:q!`
      closes the pane, after which `q` quits normally again.
- [ ] `:w` sends the message (`commit_on_save: true`), the pane stays open and
      the status line shows `written`; a second `:w` without a change does
      nothing.
- [ ] `:wq` sends and closes; the new message appears in the list.
- [ ] `:q` with an unsaved change refuses with `E37`, `:q!` discards it.
- [ ] No second editor: pressing `n` again while the pane is open → "Editor is
      already open".
- [ ] Switch to `editor: compose-below` in `stoat.yaml` plus a `:config`
      reload → the real nvim in the Kitty split again, unchanged from before.
- [ ] A long message: soft wrapping in the pane, `j`/`k` move logically (not
      per screen line), and the scrolling follows the cursor.
- [ ] A small terminal: the pane takes at most two thirds of the height and
      the row list stays visible.

### Phase 3: registers, text objects, visual mode, search, dot-repeat, theming

- [ ] Registers: `"ayy` on line 1, cursor on line 2, `"ap` pastes line 1;
      `"Ayy` appends, `"add` deletes without overwriting the unnamed register
      (afterwards `p` still pastes what was yanked before).
- [ ] Text objects: `ciw` in the middle of a word replaces the whole word,
      `daw` takes the space along, `di"` empties a string, `da(` deletes the
      parentheses together with their content — across lines and with nested
      parentheses too.
- [ ] Visual charwise: `v` plus `w`/`l`/`j` selects visibly (the cursor stays
      recognizable inside the selection), `d` deletes the selection, `y` plus
      `p` pastes it, `c` deletes and lands in insert mode.
- [ ] Visual linewise: `V` selects whole lines (including the cell at the end
      of the line), `j` adds lines, `d` deletes them entirely.
- [ ] Visual: `o` jumps to the other end and extends from there; `v` ends it,
      `V` switches to linewise, `Esc` discards; `viw` selects the word.
- [ ] Search: `/word` plus Enter jumps to the next match, `n`/`N` go forwards
      and backwards, a wrap reports "search hit BOTTOM, continuing at TOP";
      `/nothinghere` reports `E486` without moving the cursor; `n` without a
      previous search → `E35`.
- [ ] Dot-repeat: `ciwfoo<Esc>` then `w` `.` replaces the next word the same
      way; `dw` `.` `.` deletes three words; `2.` repeats twice; `u` and `.`
      themselves are not recorded as the "last change".
- [ ] Cursor: in normal mode a block cursor over the character; after `i` it is
      the **real** terminal cursor (shape and blinking as in the query menu),
      no longer a block; `Esc` goes back to the block. With `A` it sits behind
      the last character.
- [ ] The insert boundary: `i` in "ab", then arrow-right twice → the cursor
      sits behind `b`; a typed character is appended (`abc`), it overwrites
      nothing. Same with `End`. In normal mode `l`/arrow-right/`$`/`End` stay
      on `b`.
- [ ] The cursor in command mode: after `:` the terminal cursor sits **in** the
      command line behind what was typed (moving along as you type), not in the
      text; `Esc` brings it back into the buffer.
- [ ] Theming: an empty or missing `vim:` block in `tui-theme.yaml` looks as it
      did before. Then set for instance
      `vim: {selection_bg: "#504945", gutter: "#928374"}`, do a `:config`
      reload and reopen → only those two roles change. Set `cursor_bg` → the
      cursor gets a fixed colour instead of reverse video (and is still visible
      inside a selection).

## Card mode (`card:` per level, toggle key plus persistence)

Setup: take the `card:` block from
[`examples/views/cards.yaml`](examples/views/cards.yaml) into a real content
level with six columns (`columns: 3`, `weights: [1, 1, 2]`, `key: C`,
`gap: 1`) — documentation: [`generic-view-spec.md`](generic-view-spec.md),
section `card:`.

- [ ] Without a `card:` block, `C` stays an ordinary key on that level
      (nothing happens, or an existing binding still applies) and the status
      bar shows no card hint.
- [ ] With a `card:` block: the level starts as a **table** (the declaration
      alone does not switch it), the status bar shows `C cards`.
- [ ] `C` → every row becomes a framed card with **two** lines of three fields
      each (the number of lines is derived, there is no `rows:` in the config);
      the hint changes to `C table`.
- [ ] The right edge: **all** card lines end on exactly the same column, the
      third (wider) slot included; make the terminal narrower and wider → it
      stays flush.
- [ ] Labels: `labels: inline` shows `Label: value`; switch to `none` → values
      only; to `above` → labels on their own line, the card twice as tall.
- [ ] `gap: 1` → a blank line between two cards, and that blank line gets
      **no** highlight when selecting (the cards read as blocks).
- [ ] Borders and labels in theme colours: change `card_border` /
      `card_label` in `tui.yaml` plus a `:config` reload → the borders
      respectively the labels change, the values do not. Then set
      `border_style:` / `label_style:` in the view → they win over the theme
      slots. `border: plain` → square corners, `border: none` → no border.
- [ ] Cursor and selection: `j`/`k` jump from card to card (not line by line),
      and the whole card gets the selection background. The `/` search still
      marks the matches **inside the value** of the card.
- [ ] Column popup (`c`): hide a card field → the field disappears from the
      card and the grid closes up (no gap, and the right edge does not move).
      Show it again → the field is back.
- [ ] `card_mode` survives a restart: `C` on, quit the TUI, start it again →
      the level opens as cards straight away. `C` off (back to the config
      default), restart → a table. (It is stored per level under
      `card_mode:<tab>`; the entry is deleted again when switching back to the
      default.)
- [ ] `default: true` in the config → the level starts as cards; `C` switches
      to the table and **that** survives a restart as well.
- [ ] Split pane: open a second pane on the same level with `wv` → both show
      the same mode, and `C` in one switches both.
- [ ] Grouping: on a level with `group_by:` → no group headers or sums in card
      mode; back to the table → the grouping is there again.
- [ ] Limitations (an expected no): tree levels and `record_detail:` followers
      do not offer `C`. A `markdown: true` field in `card.fields` → a hard
      config error at startup with a clear message (no silent fallback).
- [ ] Chord key (`key: 'v c'`, configured that way live in the Jira tab): `v`
      alone does nothing and waits, `v c` toggles. `v` plus Esc aborts without
      `c` opening the column popup afterwards. A `c` **without** a preceding
      `v` still opens the column popup normally.
- [ ] Collision check: set `card.key` to a key that is already taken on that
      level by accident (`t` in the Jira tab, say) → an error at startup naming
      `views.tickets.card.key` and the colliding action. Undo it afterwards.
- [ ] The keybinding editor (Ctrl+Y) lists the card key as "Toggle card mode"
      and writes a change to `views[*].card.key` in the view YAML.
- [ ] Leave `fields:` out → the card shows **all** columns of the level in the
      table's order (including the ones easily forgotten in an explicit list);
      the number of lines follows as `columns ÷ columns per line`. Hide a
      column in the popup (`c`) → it drops out here as well. A `markdown: true`
      column is skipped silently (no config error — that only applies to an
      **explicitly** listed markdown column).

## The creator column (Jira + Taiga)

Prerequisite: `creator` is listed as a column in the respective view YAML
(`docs/examples/views/jira.yaml` and `taiga.yaml` show the blocks; in an
existing private config it has to be added — otherwise the adapter delivers
the field and the table simply does not show it).

- [ ] Jira tickets: the column "Creator" is filled in and visibly differs from
      "Assignee" on tickets somebody else raised.
- [ ] The bookmarks subtab (`m`) shows the column as well.
- [ ] Taiga list: the same for all four item types. An item whose creator is
      **not** (or no longer) a project member shows the name from the payload,
      or `user-<id>` — not an empty cell.
- [ ] Sorting with `S` → `creator` is in the column list (Jira sorts
      server-side via JQL, Taiga client-side). Ascending, rows without a
      creator end up at the **bottom**, not at the top.
- [ ] Open a ticket with `e`: below the `---` marker there is a `creator:` line
      in the read-only block. Save and close the file without a change → no
      conflict banner (the round-trip guard ignores unknown read-only keys).
- [ ] After saving a ticket the row's creator column stays filled in (the
      post-edit row patch uses the same key).
- [ ] Anon mode: the creator appears pseudonymized, not in the clear.

## The tags column (Taiga)

Same prerequisite as the creator column: `tags` has to be listed in the
private view YAML, otherwise the adapter delivers the field and the table
simply does not show it.

- [ ] Taiga list: the column "Tags" is filled in on tagged items, comma
      separated when there are several, and **empty** on untagged ones — no
      stray comma, no `[]`, no colour codes (Taiga sends `["name", "#hex"]`
      pairs; only the name belongs in the cell).
- [ ] All four item types show it (task, issue, epic, user story).
- [ ] Sorting with `S` → `tags` is in the list. Ascending, untagged rows end
      up at the **bottom**; the order ignores case (`Zeta` after `alpha`).
      Clicking the column header sorts the same way.
- [ ] Open an item with `e`: the `# tags:` line in the buffer lists the same
      tags as the cell. Add one, save → the cell picks it up on the reload.
- [ ] Anon mode: tag names appear replaced, not in the clear (they can carry
      customer or project terms).

## The fix-versions column (Jira)

The column key is `fix_versions` (plural, like Jira's field); JQL only knows
the singular `fixVersion`, which the adapter substitutes when sorting. Here too
the column has to be present in the private view YAML — in `jira.yaml` in
**both** column lists (tickets and bookmarks) and, if used, in `card.fields`.

- [ ] Ticket list: the column "Fix Versions" is filled in on scheduled tickets
      and empty on the rest (no `-`, no `0`). A ticket with several versions
      shows them comma-separated in one cell.
- [ ] The bookmarks subtab (`m`) shows the column as well.
- [ ] Sorting with `S` → `fix_versions` is in the list; ascending, the
      scheduled tickets come first (in Jira's version order, not
      alphabetically), descending they come last. No JQL error banner — that
      would be the symptom if the plural were sent instead of `fixVersion`.
- [ ] Open a ticket with `e`: a `fix_versions:` line in the read-only block
      below the `---` marker. Save it unchanged → no conflict or change banner.
- [ ] After saving, the row's column stays filled in (the post-edit row patch).
- [ ] Card mode (`v c`): the field appears as "Fix Versions" with a label.
- [ ] Anon mode: version names appear replaced (they can contain product or
      customer terms), not verbatim.

## The story-points column (Jira)

The column key is `story_points`. Unlike every other Jira column it is a
**custom** field, so the adapter first has to find out which one: it reads the
instance's field catalogue (`/rest/api/2/field`) once per session and takes the
field named `Story Points` (Server / Data Center) or `Story point estimate`
(Cloud). The adapter file's `story_points_field` pins the id when that lookup
misses. Sorting uses the discovered `cf[<id>]` clause, not the field name. Same
prerequisite as the columns above: it has to be listed in the private view YAML
— in `jira.yaml` in **both** column lists (tickets and bookmarks) and, if used,
in `card.fields`.

- [ ] Ticket list: the column "Story Points" carries the estimate on estimated
      tickets and is empty on the rest — blank, not `-`. A ticket estimated at
      **zero** does show `0`: only an unset field is blank.
- [ ] A whole estimate renders as `5`, not `5.0`; a half one keeps its digits
      (`2.5`).
- [ ] The bookmarks subtab (`m`) shows the column as well.
- [ ] Sorting with `S` → `story_points` is in the list; descending puts the
      biggest estimate first. No JQL error banner — that would be the symptom
      of an unresolved custom-field id.
- [ ] The bookmarks subtab sorts the same column locally and compares it as a
      **number**: `10` sorts after `9`, not before it. Unestimated rows collect
      at the end ascending (the shared rule for every number column, not one of
      this column's own).
- [ ] Open a ticket with `e`: a `story_points:` line in the read-only block
      below the `---` marker. Save it unchanged → no conflict or change banner.
- [ ] After saving, the row's column stays filled in (the post-edit row patch).
- [ ] Card mode (`v c`): the field appears as "Story Points" with a label.
- [ ] Anon mode: the number stays verbatim — an estimate identifies nobody.
- [ ] Set `story_points_field: customfield_<wrong-id>` in the adapter YAML and
      restart: the column goes blank and the ticket list still loads. Remove it
      again → the values come back.

## The labels column (Jira)

The column key is `labels`; JQL knows the same name, so the sort goes to the
server like every other Jira column. Same prerequisite as above: the column
has to be listed in the private view YAML — in `jira.yaml` in **both** column
lists (tickets and bookmarks) and, if used, in `card.fields`. Unlike the other
read-only columns this one is **editable** — via the ticket buffer, not the
cell — which is what the post-edit item below is about.

- [ ] Ticket list: the column "Labels" is filled in on labelled tickets and
      **empty** on the rest (no `[]`, no stray comma). A ticket with several
      labels shows them comma-separated in one cell.
- [ ] The bookmarks subtab (`m`) shows the column as well.
- [ ] Sorting with `S` → `labels` is in the list; ascending, the unlabelled
      tickets come first, descending they come last. No JQL error banner —
      that would be the symptom of a deployment that refuses
      `ORDER BY labels`; the fix is to drop the mapping in `jql.rs`, which
      leaves the column as display-only. (Confirmed accepted against a live
      Jira Server instance; the fallback is for deployments that differ.)
- [ ] Open a ticket with `e`: the `labels:` line above the `---` marker lists
      the same labels as the cell (as `ll-…` slugs). Add one, save → the row's
      cell shows it **immediately**, without a reload (the post-edit row patch
      carries `labels`).
- [ ] Remove every label and save → the cell goes empty, not stale.
- [ ] Card mode (`v c`): the field appears as "Labels" with a label.
- [ ] Anon mode: label names appear replaced (they are project-authored and
      can carry customer or product terms), not verbatim.

## Comment preview: truncating on characters, not bytes

A regression: the preview in the comment list truncated at 80 **bytes**; if
that boundary fell inside a multi-byte character, the panic took the tokio
worker down with it.

- [ ] Select a ticket with a longer comment in a language with umlauts and
      press `C` → the list opens, no crash, and long previews end in `…`.
- [ ] A comment with a multi-byte character right around the boundary (an
      umlaut, an emoji, CJK) → the preview breaks off right after that
      character, no broken bytes in the cell.
- [ ] A server error with a non-ASCII error page (an expired session, say) →
      the error message and the HTTP log appear normally; the truncated body
      snippet does not crash while the error is being _reported_.

## Free-text search (Taiga `text_search`) — matches and the active marker

Three regressions: (1) the query template did not cover `userstory` and
contained a `ref:` block that Taiga ignores (so the response was the complete
unfiltered list); (2) the search's hint went out as soon as it took effect;
(3) the input was substituted into the template as text, so an input carrying
YAML syntax (`#294` — `#` opens a comment) turned the filter into `null`,
which Taiga again answers with the complete unfiltered list.

- [ ] Open the free-text search (the key of the `text_search` action, `s` in
      the example config) and enter a plain ref number (`112`, say) → exactly
      the items with that ref appear, across all types including user stories;
      **no** flood of non-matching rows.
- [ ] The same number with a leading `#` (`#112`) → an identical result. The
      HTTP log (`NYD_DEBUG=1`) shows `?q=%23112`, not `?q=`.
- [ ] Other YAML metacharacters in the input (a leading `-`, a `:`, a `"`) →
      still a search, never a flood and never a parse error.
- [ ] Enter a word from a subject → a full-text search across tasks, issues,
      epics and user stories.
- [ ] A view query with a valueless filter (`q:` with nothing behind it) →
      the view reports the filter as having no value instead of listing
      everything.
- [ ] While typing: the free-text search's hint is marked, the local `/`
      search's hint is **not**.
- [ ] After Enter (with the result list up): the hint stays marked for as long
      as the result list is displayed.
- [ ] Apply another query (the query menu, the default query, the query
      editor) → the marking goes out.
- [ ] Split the pane while the search is active → the new pane inherits the
      query **and** the marking.

## The SQLite tab — files, tables, scripts

Prerequisite: copy `docs/examples/views/sqlite.yaml` and
`docs/examples/views/sqlite-adapter.yaml` to
`~/.config/not_yet_done/views/` and point the `sources:` globs in the adapter
config at your own `.db` files. The tab needs no login and no server — what can
fail is the globbing and the addressing.

### Files and rows (the `sources:` globs)

- [ ] Open the tab → one row per matched file, with its size and path.
- [ ] A `**` pattern picks up files from subdirectories; a pattern without
      matches is not an error, it simply yields nothing.
- [ ] Two files with the same name in different folders appear as **two** rows
      (the key carries the path hash) and share neither their tables nor their
      script directory.
- [ ] Create a new `.db` file → `r` → it is there, without a restart.
- [ ] Drill down to `Rows`, page through (`>`/`<`), `o` opens the
      record-detail pane; a table with a BLOB column renders readably instead
      of breaking.

### Per-table scripts (`q` / `Q`)

- [ ] `Q` on a table → the editor with the template `SELECT * FROM "t";`.
      Save it, run it, page through it.
- [ ] `q` after the first save → the script is in the menu.
- [ ] `Q` one level deeper (in `Rows`) still addresses the table above, not
      the row.
- [ ] A multi-statement script → it runs, but without page information (a
      non-paginatable form).
- [ ] An `UPDATE` → fails with the hint about `read_only: false`.

### The script branch below the database (`Scripts`)

The same branch as in the Postgres tab (shared code in `sql-core`), just with
`sqlite:` types — so the point here is mainly to check that it hangs there **at
all** and points at the right file.

- [ ] `Scripts` sits next to `Tables` under every database, even when no
      script exists yet.
- [ ] `a` creates a script, `A` a folder; folders nest to any depth.
- [ ] Filesystem check:
      `<data_local>/not_yet_done/sqlite/<instance_id>/db_scripts/<key>/…` —
      `<key>` is the hashed file key, not the path.
- [ ] `e` edits in place (a temp file with the `.nyd_tmp_` prefix **inside**
      the script directory), `r` renames (keeping the extension), `M` moves.
- [ ] `d` on a non-empty folder refuses with "not empty".
- [ ] **`x` runs and paginates via LIMIT/OFFSET** — that is the deviation from
      Postgres: the result pane is set to `pagination: mode: server`. No error
      such as "sqlite has no cursor pagination"; `>`/`<` page through.
- [ ] A script under file A accesses a table that only exists in file B → an
      error from SQLite (proof that it runs against the right file, not against
      the most recently opened one).

### Table completions in the SQLite script editor

The mechanism is the same as in the Postgres tab (see TC-1 … TC-5), but with
**single-stage** tokens — a file has no schema namespace.

- [ ] `e` on a `.sql` script → the last line is
      `-- table completions: tt_<table>, …` with **all** tables _and views_ of
      the file, without the `sqlite_` internals.
- [ ] Use a token: `SELECT * FROM tt_<table>;` → `x` returns rows (it expands
      to `"<table>"`).
- [ ] Save, then `cat` the script under
      `<data_local>/not_yet_done/sqlite/<instance_id>/db_scripts/<key>/…`:
      **no** completion line on disk. Another `e` shows it again, exactly once
      (not stacked).
- [ ] `e` on a `.py` script in the same branch → no completion line.
- [ ] Two files with a table of the same name: the line under file A lists A's
      tables (proof that the key comes from the node ID).
- [ ] An unknown token (`tt_doesnotexist`) → the SQLite error names the
      literal token, no silent substitution.

## Editing database views (`E` → `edit_view`, SQLite + Postgres)

Views are a **node type of their own** (`sqlite:view` / `postgres:view`) in a
dedicated `Views` branch next to `Tables`. Reading their rows works like a
table's; on top of that comes `E`: it opens the `CREATE VIEW` statement in the
editor, and saving replaces the view. Prerequisite: the `Views` branches from
`docs/examples/views/{sqlite,postgres}.yaml` in your own config, and for SQLite
`read_only: false` in `sqlite-adapter.yaml`.

The binding deliberately carries `type: edit, id: edit_view` and **not** the
default `type: node` — which is why the first item is not a detail but the
proof that the wiring is right.

### Common to both adapters

- [ ] `Views` sits next to `Tables` (under the database in SQLite, under the
      schema in Postgres), even when the database has no view.
- [ ] Drill down to `Rows` → the view's rows, `>`/`<` page through, `o` opens
      the record-detail pane. `Q`/`q` address the view like a table.
- [ ] `E` on a view → the editor with the complete `CREATE VIEW` statement,
      with a header comment explaining what saving does. The buffer extension
      is `.sql` (syntax highlighting in the editor profile).
- [ ] Save without a change → the message "no changes", the view is **not**
      touched. Changing only the trailing semicolon or whitespace also counts
      as no change; reformatting counts as a change.
- [ ] Change the body (add a `WHERE`, say) → the message "view … replaced",
      after which `Rows` shows the new rows and another `E` the new definition
      (the buffer is re-baselined, no conflict warning).
- [ ] **Rename** the view → rejected in the editor, the text is preserved,
      with the hint "rename it back, or create the other view from a DB
      script".
- [ ] Append a second statement (`; DROP TABLE …`) → rejected, nothing runs.
- [ ] Broken SQL (`SELECT * FROM doesnotexist`) → the error from the server or
      the file appears **at the top of the buffer** as a banner, with your own
      text unchanged below it. Saving again after the correction removes the
      banner (it is not saved along).
- [ ] Change the view from outside in parallel (a second `sqlite3` or `psql`),
      then save → a conflict hint with the foreign definition in the buffer
      instead of a silent overwrite.
- [ ] Delete the view from outside, then save → a comprehensible message, no
      panic.
- [ ] The flat `views` view (`v`) → all views; `E` edits there without drilling
      down and without using the tree view.

### SQLite only

- [ ] `read_only: true` (the default) → `E` opens, saving fails with the hint
      about `read_only: false`. The view is still **there** unchanged — the
      drop happens in the same transaction as the create.
- [ ] A successful save keeps the verbatim form:
      `sqlite3 <file> "SELECT sql FROM sqlite_master WHERE name='<view>'"`
      shows exactly the saved text (SQLite does not reformat it).
- [ ] A view that uses another view: the dependent view stays usable after the
      replacement (SQLite only resolves bodies on access — the
      `SELECT … LIMIT 0` test inside the transaction is the actual check).
- [ ] The `Tables` branch lists **no** views any more (and the `kind` column is
      gone from it, since it would be constant).

### Postgres only

- [ ] Saving runs as `CREATE OR REPLACE VIEW`: a dependent view and a granted
      `GRANT` survive (compare `\dp` before and after). Nothing is dropped.
- [ ] **Change the column list** (rename or remove a column) → Postgres
      refuses and the error lands in the banner; appending a new column at the
      end works. No automatic `DROP … CASCADE`.
- [ ] Remove the schema qualifier from the header
      (`CREATE OR REPLACE VIEW v AS …`) → rejected with a hint about the
      `search_path`; nothing runs.
- [ ] Point the header at a **different** schema → rejected ("different
      schema").
- [ ] **Keep working** after a rejection: a normal query on the same tab still
      runs. (Proof that no cached session is stuck in an aborted state — there
      is deliberately no `BEGIN` block.)
- [ ] Paging through the `Rows` of a view without an `ORDER BY` of its own can
      repeat or skip rows; with an `ORDER BY` in the body it is stable. This is
      the expected behaviour: a view has no `ctid` to sort by.
- [ ] A materialized view (`relkind = 'm'`) does **not** appear in the `Views`
      branch.
- [ ] The table completions in the database script editor list views as well.

## Editing data rows (`e` → `edit_row`, SQLite + Postgres)

`e` on a row opens the configured editor with the row as a YAML mapping — one
`column: value` line per cell. Saving builds **one** `UPDATE` from the columns
that actually changed, and runs nothing else. If the statement fails, the
editor reopens with the error message **and** the generated `UPDATE` as a
banner above your own text.

Prerequisite: the `e` binding on the `Rows` levels from
`docs/examples/views/{sqlite,postgres}.yaml`, and for SQLite `read_only: false`
in `sqlite-adapter.yaml`. As with the view editor it is an `actions:` entry
with `type: edit, id: edit_row`, not the default `type: node`.

The core of the test: the row is addressed by the **key values that were read
when it was opened** — not by the offset in the tree row. The offset is only
_how_ the row was found; a page shifting underneath must not redirect the
write.

### Common to both adapters

- [ ] `e` on a table row → editor with all cells as YAML, extension `.yaml`
      (syntax highlighting from the editor profile). The header comment names
      the table, the offset and **what** the row is addressed by.
- [ ] Save without a change → "no changes", no statement runs.
- [ ] Change one cell → the message names the table and "1 column"; the table
      view shows the new value after `r`. Check from the outside (`sqlite3` /
      `psql`): **only** that column was written.
- [ ] Change two cells → "2 columns", a single `UPDATE`.
- [ ] Set a cell to `null` (YAML `null`, not the string) → the column is
      SQL NULL afterwards. Conversely, `"null"` in quotes writes the text.
- [ ] **Delete** a line from the buffer → that column stays untouched (omitting
      means "do not change", not "set to NULL").
- [ ] Mistype a column name → rejected in the buffer, the real column names are
      in the message, your own text is preserved.
- [ ] Broken YAML → rejected with a line number, text preserved.
- [ ] Write multi-line text (block scalar `|`) and special characters
      (`: `, `#`, leading spaces, emoji) → they arrive unchanged in the
      database and come back unchanged on the next `e`.
- [ ] **Change the key column itself** (e.g. `id`) → the `UPDATE` addresses the
      row by its **old** value, i.e. it renames the row instead of creating a
      second one.
- [ ] Change the row from the outside in parallel, then save → conflict notice
      with your own text unchanged in the buffer; saving **again** deliberately
      overwrites.
- [ ] Delete the row from the outside, then save → comprehensible message
      ("nothing was written"), no panic.
- [ ] The statement fails (unique violation, type error, trigger) → the banner
      contains the error message **and** below it "The statement that failed:"
      with the `UPDATE`. Save after fixing it → the banner is gone and is not
      written along.
- [ ] Put the cursor on a row, then delete a row **above** it from the outside,
      then `e` → the row the editor read is the one being edited (or it is
      cleanly rejected), never a neighbouring one.
- [ ] In the record detail pane (`o`) → `e` edits the same row (the pane is
      that same row transposed).
- [ ] Flat `tables` view → `e` works there in exactly the same way.
- [ ] `Rows` of a **view**: `e` is deliberately **not** bound there. If it is
      bound for a test, the adapter rejects it with a pointer to the underlying
      table — no editor opens.

### SQLite only

- [ ] `read_only: true` (the default) → `e` opens, saving fails with a pointer
      to `read_only: false`; the row stays unchanged.
- [ ] Table **without** a primary key → the header comment says that the
      implicit `rowid` is used for addressing; writing works.
- [ ] Table with a composite primary key → `WHERE` names all columns.
- [ ] **BLOB cell** → in the buffer only as a comment:

  ```
  #   column: <blob, N bytes>
  ```

  Uncomment the line and save → rejected ("cannot be written from here"); the
  bytes stay untouched.

- [ ] Row with `NULL` in a key column of a table without a primary key →
      addressed via `rowid`, works anyway.

### Postgres only

- [ ] Table with a primary key → the header names it; writing works.
- [ ] Table **without** a PK but with a unique index over NOT NULL columns →
      the header names the index **by name**; writing works.
- [ ] Table without a PK and without such an index → `e` rejects with the
      reason (no editor). `ctid` is deliberately **not** used as a substitute,
      because it moves on every `UPDATE`.
- [ ] A unique index over a **nullable** column does not count as a key (two
      NULLs do not collide) → the same rejection.
- [ ] Non-text types (`int`, `numeric`, `timestamptz`, `jsonb`, `bytea`,
      array) → the value arrives as text in the buffer, is written as a text
      literal and converted by the column type; a round trip changes nothing.
      `bytea` shows up as `\x…` and comes back as the same bytes.
- [ ] **Keep working** after a rejection: a normal query on the same tab still
      runs (no session in aborted state).

## Adding data rows (`e n` → `new_row`, SQLite + Postgres)

`e n` opens the same editor as `e`, but with a **template of the relation's
columns** instead of a row's cells — one `column: null` line each. Saving
builds **one** `INSERT` from the lines that are still there; a line you delete
is left to the column's `DEFAULT`. The columns the database fills itself open
**commented out**, so the common case is "save straight away and let the
database do the rest".

Prerequisite: the `new_row` binding from
`docs/examples/views/{sqlite,postgres}.yaml` (`key: n` there, `e n` in the live
config), and for SQLite `read_only: false` in `sqlite-adapter.yaml`. Like
`edit_row` it has to be an `actions:` entry with `type: edit, id: new_row` —
`type: node` cannot open an editor, and `type: create` resolves its parent from
the nav stack, which a tree pane does not fill.

The core of the test: it is bound on the **relation** as well as on its rows.
An empty table has no row to stand on, so without the first binding its first
row could never be written.

### Common to both adapters

- [ ] `e n` on a table **row** → editor with every column as `column: null`,
      extension `.yaml`. The header names the relation and lists the columns,
      marking the ones the database fills.
- [ ] `e n` on the **table** itself (one level up in the tree) → the same
      buffer. Do it on a table with **no rows at all** — this is the only way
      to write its first row.
- [ ] Fill two columns, save → notice "row added to …", the level reloads and
      the new row is visible without a manual `r`.
- [ ] Check from the outside (`sqlite3` / `psql`): exactly one row was
      inserted, and only the columns you left in the buffer were named.
- [ ] Leave a line at `null` → the column is SQL NULL. Conversely `"null"` in
      quotes writes the text.
- [ ] **Delete** a line entirely → the column's `DEFAULT` applies (not NULL, if
      it has one). Leaving the line commented out does the same.
- [ ] Delete **every** line and save → "no changes", nothing is inserted. An
      emptied buffer is a change of mind, not a request for a row of defaults.
- [ ] Mistype a column name → rejected in the buffer with the real column names
      in the message; your own text is preserved.
- [ ] Name the same column twice → rejected by name, text preserved.
- [ ] Broken YAML → rejected with a line number, text preserved.
- [ ] Violate a constraint (NOT NULL, unique, foreign key, a type) → the banner
      carries the database's message **and** below it "The statement that
      failed:" with the `INSERT`. Fix it and save → the banner is gone and is
      not written along.
- [ ] Multi-line text (block scalar `|`) and special characters (`: `, `#`,
      leading spaces, emoji) arrive unchanged and come back unchanged on `e`.
- [ ] Flat `tables` view → `e n` works there in exactly the same way, on both
      the table and its rows.
- [ ] `e n` twice in a row → two rows, not one; the second buffer is a fresh
      template, never the first one's text.

### SQLite only

- [ ] `read_only: true` (the default) → `e n` opens, saving fails with a
      pointer to `read_only: false`; nothing is inserted.
- [ ] Table with an `INTEGER PRIMARY KEY` → that column opens commented out
      ("the database fills it"); saving without it assigns the next rowid.
- [ ] Table with a `DEFAULT` on a column → same, and deleting the line applies
      the default.
- [ ] Table with a **generated** column → it is not offered in the buffer at
      all (`PRAGMA table_info` does not list it, and an `INSERT` may not name
      it).
- [ ] `Rows` of a **view**, or the view itself: `e n` is deliberately **not**
      bound. If it is bound for a test, the adapter rejects it with "SQLite
      cannot insert through one" — no editor opens.

### Postgres only

- [ ] Column with `GENERATED … AS IDENTITY` or a `nextval(…)` default → opens
      commented out; saving without it lets postgres assign the value.
- [ ] Column with `GENERATED ALWAYS AS (…) STORED` → not offered at all.
- [ ] Non-text types (`int`, `numeric`, `timestamptz`, `jsonb`, `bytea`,
      array) → written as text literals and converted by the column type; the
      inserted row reads back unchanged with `e`.
- [ ] A **simple view**: `e n` is bound there (unlike in SQLite) and inserts
      into the relation underneath — postgres auto-updatable views work.
- [ ] A view that is **not** auto-updatable (a join, an aggregate) → postgres'
      own refusal lands in the banner together with the `INSERT`. That message
      is the reason nothing is guessed up front.
- [ ] A table in a schema whose name needs quoting → the statement names
      `"schema"."table"`, and the insert works.
- [ ] **Keep working** after a rejection: a normal query on the same tab still
      runs (no session in aborted state).

## View scripts on the SQL row levels (`x`), DB scripts on `X`

Three different notions of "script" meet on the SQL tabs. Above all, the test
is meant to show that they no longer get in each other's way:

| Kind             | what it is                        | key       |
| ---------------- | --------------------------------- | --------- |
| Node scripts     | SQL, belongs to the table/view    | `Q` / `q` |
| DB scripts       | SQL, belongs to the database file | `X`/Enter |
| **View scripts** | any program, payload as `argv[1]` | `x`       |

`x` now sits on **every** row level (tree `Table`, tree `View`, and in the flat
`tables`/`views` listings). Because all of these levels share the same node
type (`sqlite:row` / `postgres:row`), they share **one** script directory:
`~/.local/share/not_yet_done/scripts/<tab>/<root>/<row-type>/`. The flat
listings need `script_source: databases` for that, otherwise their own node
type would be the root and the same scripts would have to exist three times.

- [ ] `x` on a **database** node (root level) opens the menu for
      `scripts/sqlite/sqlite_database/`. Empty directory → the notice "No
      scripts in …, Type +name then Enter to create one", not a silent
      nothing. The payload is `{"node": {…}}` with `fields.path` pointing at
      the file.
- [ ] Thanks to `inherit: true` the same `x` is also present on the levels
      **below** the database (`Tables`, a table, `Views`, a view, the
      `Scripts` branch) — and on the same directory, not one per level.
      Without `inherit` an `actions:` entry only applies to its own level;
      that was why `x` initially only fired on the topmost row.
- [ ] Put an executable test script into
      `~/.local/share/not_yet_done/scripts/sqlite/sqlite_database/sqlite_row/`
      that writes stdin to `/tmp`. `x` on a table row → the menu shows it,
      selecting it runs it.
- [ ] The payload contains the displayed page plus cursor context: `rows[]`
      (`id`, `label`, `fields`), `query`, `selected_index`, `selected_field` —
      `selected_field` follows the column cursor.
- [ ] The same script shows up without further work under `x` on a **view**
      row and in the flat `tables`/`views` listings (one directory, not
      three).
- [ ] Postgres tab: `x` on a row in the `Views` branch shows the same scripts
      as in the `Table` branch.
- [ ] In the `Scripts` branch: cursor on a DB script → the action bar shows
      `X: execute` (not `x`), `X` runs it, Enter does the same. `x` does
      **nothing** wrong there (no running the script as a side effect).
- [ ] No collision warning when the config is loaded (the `f10` log is empty
      as far as keybindings on the SQL tabs are concerned).

## CLI: connection status and credential prompt

The CLI behaves like the TUI, only without `r`: it connects right away,
reports progress on **stderr** (stdout stays pipeable) and asks for
credentials on the terminal. An empty list afterwards really does mean
"nothing there".

- [ ] `nyd adapter <chat-instance> ls` in a terminal → `nyd: Connecting…` on
      stderr, then the server rows. **Never** an empty list with exit 0 while
      the connection is still coming up.
- [ ] The same thing piped (`… ls | cat`) → the status line ends up on stderr,
      the table unchanged on stdout.
- [ ] Local adapter without a background connection (`nyd adapter tasks ls`) →
      no status line, no noticeable delay.
- [ ] Instance with `provider: { type: prompt }` and a deleted session, in a
      terminal → password prompt (masked, on stderr), then the result.
- [ ] The same instance without a TTY (`… ls < /dev/null | cat`) → an error
      message naming the missing fields and pointing at env/file/command/
      keyring, exit ≠ 0 — not an empty list.
- [ ] Backend unreachable → `Connection failed: …` and exit ≠ 0; if the
      connection hangs, the command aborts after 60 s with a timeout message.
- [ ] `nyd adapter <instance> help` works **without** a connection and without
      a credential prompt.

## CLI: auth mechanisms in the config wizard (`config auth` / `config build`)

Since the descriptor rework, only the adapter knows which mechanisms exist.
The wizard and the listing read the same table as the validation — so whatever
is offered here has to be accepted by the factory as well.

- [ ] `nyd config auth jira` → `cookie` and `basic-auth` with label, docs and
      fields; no connection, no credential prompt.
- [ ] `nyd config auth tasks` → "has no authentication", not an empty menu.
- [ ] `nyd config auth doesnotexist` → error with the list of known types.
- [ ] `nyd config build kimai` in a terminal → after the normal fields, a
      legend plus a menu for `auth.mechanism` (no free text), then a provider
      menu per field with the field name in the prompt
      (`auth.token: choose variant`).
- [ ] The generated YAML has `auth:` where the config type declares it (for
      kimai directly after `url:`), not appended at the end.
- [ ] A field whose label the name heuristic gets wrong (kimai `token` → "API
      password") gets a `label:`; `masked:` is absent where the heuristic is
      right anyway.
- [ ] Mechanism with an optional field → the question "bind the optional field
      …?" with default **no**; if declined, the field does not appear in the
      YAML.
- [ ] Escape/Ctrl-C in the mechanism selection → "aborted — no config
      written.", **nothing** on stdout (not even the fields already answered).
- [ ] Put the generated YAML into a view as `adapter.config` → the adapter
      starts without complaining about the auth section.
- [ ] `nyd config template kimai` → a hint line pointing at
      `nyd config auth kimai` on stderr, stdout stays pure YAML.

## Script-driven credentials (`auth.script` + `script-result`)

A script delivers **several** fields at once and only asks when it has to. One
fresh process per round: `{"request": […], "input": {…}}` on stdin, exactly one
of `result` / `form` / `error` on stdout; `input` accumulates the answers
across rounds. Test script (no server, just the protocol — the first round
asks, the second delivers):

```sh
#!/bin/sh
in=$(cat)
case "$in" in
  *'"pin"'*) printf '{"result":{"username":"demo","token":"t-%s"}}' "$$" ;;
  *) printf '{"form":{"header":"Demo-Login","fields":[{"name":"account","label":"Account"},{"name":"pin","masked":true,"optional":true}]}}' ;;
esac
```

- [ ] An `auth.script: <path>` plus two bindings whose provider type is
      `script-result`, in one view → connecting shows **the script's fields**
      (Account, Pin), not the mechanism's fields.
- [ ] The form `header` appears as the popup title (TUI) or above the
      questions (CLI), not the generic "Login: <tab>".
- [ ] TUI: leave the pin empty (`optional`) → Enter submits; empty account →
      "Required: Account", no submit.
- [ ] Both fields from a single script answer → the script runs **once** per
      round, not once per binding (visible from the `$$` suffix in the token
      or from the script log).
- [ ] CLI (`nyd adapter …`) in a terminal → the same questions, the pin may
      stay empty, the account may not.
- [ ] CLI in a pipe (`nyd adapter … | cat`) → "no terminal to ask on" with the
      field names, not a silent empty result.
- [ ] The script answers `{"error":"…"}` → the login fails with exactly that
      message, no form.
- [ ] The script returns a `result` that is missing a requested field → the
      login fails naming the missing field, no login with an empty value.
- [ ] The script repeats its form with `error: "…"` → the same form comes up
      again, the message is attached to it, and the previous answers are
      contained in the next round's `input`.
- [ ] A script that **always** sends a form → abort after 5 rounds with a
      pointer to the round limit, no endless loop.
- [ ] Escape in the form (TUI) or abort (CLI) → the login fails immediately
      with "cancelled"; a **second** connection attempt afterwards gets to the
      form again (the auth mutex does not stay locked).
- [ ] Reconnect after a 401 → the script runs again and the form comes up
      again (the value is deliberately short-lived), not the cached old value.
- [ ] Config check: `script-result` without `auth.script` **and** `auth.script`
      without a `script-result` binding are both rejected when the view is
      read — naming the missing half.
- [ ] `nyd config auth <type>` lists `script-result` among the providers;
      `nyd config build <type>` offers it in the provider menu and then asks
      for `auth.script` **exactly once**, no matter how many fields use it.

## Expired cookie in the middle of a session (Jira / Confluence)

`session_cache: until-rejected` means: the session is valid until the server
rejects it. A 401 (or an SSO bounce into the login flow) marks the client as
rejected; the next access throws it away and logs in again — with the
`command` provider the cookie script runs again in the process.

- [ ] Connect, load a list, then invalidate the cookie server-side (log out in
      the browser / end the session) → the next access reports the 401 once.
- [ ] Then reload (`r`) → the adapter logs in again and delivers data without
      `invalidate_session` having to be called by hand.
- [ ] Provider `command`: the script is run again on re-login (visible from
      the script log or from a password prompt), the old value is not reused.
- [ ] Open a page you are not allowed to see (403) → normal error message,
      **no** re-login, no second script or password prompt.

## Sort menu (`c s`) and column menu on `c c`

The sort menu is a second UI path to the same sorting as `S`; `c` is now the
chord leader for both table menus.

- [ ] `c c` opens the column menu (formerly `c`), `c s` the sort menu. A
      single `c` does nothing and waits for the second keystroke.
- [ ] The sort menu lists **all** sortable columns: the sorted ones at the top
      in sort order with `asc`/`desc`, the unsorted ones below.
- [ ] `j`/`k` move the cursor, `ctrl+j`/`ctrl+k` move the highlighted entry
      **within** the sorted block; on an unsorted entry nothing happens.
- [ ] `a`/`d` on an unsorted column appends it to the end of the sorted block,
      `0` takes it out again (it ends up at its natural position below).
- [ ] `Enter` applies: exactly **one** reload, the notification names the
      columns; `Esc` discards, the table stays unchanged.
- [ ] Sorting set with `S` is visible in the menu and vice versa — the sorting
      survives a tab switch (it is persisted as before).
- [ ] Level without sortable columns: `c s` reports "No sortable columns"
      instead of showing an empty popup.
- [ ] A view YAML with an `actions:` binding on `c` is reported as a conflict
      at load time (prefix collision with `c c`/`c s`); `force: true` still
      suppresses it.

## The record-detail pane shows every column (`o`)

The transposed follower is for reading one record completely, so it is no
longer limited to the columns the row view shows. Take any view with
`record_detail: true` (SQLite/Postgres `Rows`) or a level that has one.

- [ ] `o` opens the follower → the fields appear in the row view's order,
      with the row view's labels and formatting (dates, durations,
      `source: label`).
- [ ] `c c` on the source, deselect a column, `Enter` → the column leaves the
      table but the follower keeps it, **appended after** the still-selected
      ones. No cursor move needed: the follower repaints on apply.
- [ ] Re-order the selection in `c c` → the follower's leading block follows
      the new order, the deselected tail stays behind it.
- [ ] Select everything again → the follower is unchanged apart from the
      ordering (no duplicated field lines).
- [ ] A level with a `hidden: true` column (it is absent from the table by
      default) → the follower lists it last, with its YAML label.
- [ ] Postgres/SQLite `Rows` (no configured columns) behave as before: one
      line per record field, nothing doubled.
- [ ] `j`/`k` in the source still updates the follower; `X` still toggles
      wrapping, now including the appended fields.

## Unknown keys in `views/*.yaml` (a warning instead of silence)

Serde discards unknown keys without a word — the line sits in the file and
does nothing. They are now reported without breaking the tab.

- [ ] Add a `key: x` under a `children:` entry in `views/jira.yaml` → on
      startup **one** modal "View configuration warnings" appears with the
      path (`views.0.children.0.key`); the Jira tab loads normally and all
      rows are there.
- [ ] Remove the line again → startup without the modal.
- [ ] Add the same line while the TUI is running, via `:config jira`, and save
      → notification "Reloaded view jira.yaml — ignored unknown keys: …", the
      tab stays usable.
- [ ] A `hooks:` block (see `docs/examples/views/tasks.yaml`) triggers **no**
      warning — it is read by the host, not by the TUI.
- [ ] A real YAML syntax error behaves as before: the tab becomes `Broken`
      with an error panel, no warning modal.

## Custom columns as an enumeration (`set-column-options`)

Restrict a custom column to a closed set of values. Via the action menu or
from the CLI:

```sh
nyd <inst> do set-column-options <ID> --field column_key=<key> --field options=a,b,c
```

- [ ] Set a value set on a column with mixed values that does not cover all of
      them → the error names the offending row ids and the column stays
      unrestricted (nothing was written).
- [ ] Fix the values, set the same set again → it goes through, the message
      names the number of covered cells.
- [ ] Afterwards set a cell to a value **outside** the set → rejected, the old
      value is still there. A value from the set works.
- [ ] Clearing a cell stays allowed (empty = "unset", not a violation).
- [ ] In the edit form (the `edit-cells` action bound in the view) the column
      appears as a **select** with exactly those values; an unrestricted
      custom column next to it stays a text field.
- [ ] "Nothing" can be chosen in the select (the empty `(none)` state) → the
      cell is cleared.
- [ ] Set `options` to empty → the column is unrestricted again, arbitrary
      values work.
- [ ] Set a set containing a word on a `number` column → rejected (options
      have to match the `value_type`).
- [ ] Whitespace and duplicates in the set (`1 , , 2 , 1`) are trimmed,
      deduplicated and stored without blanks.

## Stoat: channel whose last message was deleted

The dead `last_message_id` used to keep the channel permanently unread.

- [ ] Open a channel whose last message was deleted: the list ends on a
      `[deleted message]` row at the chronologically correct position.
- [ ] The cursor lands right on it when opening (`cursor_on_open`) and the
      bell in the tab bar disappears **without** a keystroke.
- [ ] After a TUI restart the channel stays read (the ack went through
      server-side, not just locally).
- [ ] On the tombstone row, `edit`/`delete`/reaction/download fail with "this
      message was deleted" instead of a 404.
- [ ] Preview/detail of the same row shows the same stand-in, no error.

## View scripts on a reload hook (`ctrl+h` in the script menu)

The same script contract as `:script`, only the TUI pulls the trigger after a
load instead of the user. The point of the test is the loop protection: a
script that reloads its own view must not restart itself.

Preparation — a `commands`-mode script in the view's script directory that
appends a line to a log file and hands `reload` back to the TUI:

```python
#!/usr/bin/env python3
# mode: commands
import json, os, sys
with open("/tmp/nyd-hook.log", "a") as f:
    f.write("fired\n")
with open(os.environ["NYD_OUTPUT_FILE"], "w") as f:
    json.dump({"commands": ["reload"]}, f)
```

- [ ] `:script` → cursor on the script → `ctrl+h` opens the picker `Hook ·
<name>` with `none` (marked) and `reload`. Picking `reload` reports
      "Script '…' now runs after the view's rows (re)loaded".
- [ ] The menu entry now carries both bindings as a suffix, e.g. `[x]
[reload]`; a script without a chord shows only `[reload]`.
- [ ] `r` on the view: the log grows by **exactly one** line per reload — the
      `reload` the script itself emits does **not** fire the hook again.
      Watch it a few seconds; the count must stand still afterwards.
- [ ] Drill into a child level and back: the hook fires on the drill-down load
      too (same script directory only if the view path matches — a deeper
      level has its own scope, so it stays silent there).
- [ ] Pull the plug on the load (no connection / an adapter error): the hook
      does **not** run on a failed load.
- [ ] With a split open, reloading the **unfocused** pane fires that pane's
      hook, not the focused pane's.
- [ ] `ctrl+h` on an `interactive` or `capture` script is refused with "Cannot
      hook '…': …" — nothing is stored.
- [ ] Re-open `ctrl+h`, pick `none`: "Hook removed from script '…'", the
      suffix disappears, `r` no longer grows the log.
- [ ] Restart the TUI: the binding is still there (it lives in the DB, not in
      the YAML).
- [ ] Delete the script with `ctrl+d` and create a new one with the same name:
      it starts **without** a hook (no orphan row left behind).
- [ ] On a terminal without the kitty keyboard protocol, check whether
      `ctrl+h` arrives at all — some terminals deliver it as `backspace`. If
      so, rebind `script_menu.edit_hook` in `tui.yaml`.

## View scripts on a load hook (rows patched before they land)

The other half of `ctrl+h`: the script sees the rows on their way in and hands
back cell values instead of commands. The point of the test is that the value is
right on the **first** frame and that no extra load happens.

Preparation — a `commands`-mode script in the view's script directory that
stamps a value onto every row it is given and logs each run:

```python
#!/usr/bin/env python3
# mode: commands
import json, os, sys
with open(sys.argv[1]) as f:
    payload = json.load(f)
with open("/tmp/nyd-load-hook.log", "a") as f:
    f.write(f"{len(payload['rows'])}\n")
cells = {row["id"]: {"<a column key of this level>": "PATCHED"} for row in payload["rows"]}
with open(os.environ["NYD_OUTPUT_FILE"], "w") as f:
    json.dump({"cells": cells}, f)
```

- [ ] `ctrl+h` on the script now offers `none`, `reload` **and** `load`. Picking
      `load` reports "Script '…' now runs on the rows before they reach the
      table"; the entry gets the suffix `[load]`.
- [ ] `r` on the view: every row shows `PATCHED` **immediately** — no flicker
      from the real value to the patched one, and the log grows by exactly one
      line per reload.
- [ ] `s` (sort) on that column sorts by the patched value, and a fuzzy filter
      on it finds the patched rows — the patch is in before sorting/filtering.
- [ ] Patch a column that is configured in `columns:` but has **no** stored
      value on any row: the value appears anyway (the field is added, not
      skipped).
- [ ] Answer with a number instead of a string (`{"days": 4.5}`) on a
      `kind: number` column: it renders as `4.5`, right-aligned like a number.
- [ ] Answer with an id that is not in this load (`{"cells": {"NOPE-1": …}}`):
      one message "… row id(s) not in this load — ignored", the other rows
      still patched.
- [ ] Answer with `{"commands": ["reload"]}` from a `load` hook: refused with a
      message, and **no** reload happens (the log stands still).
- [ ] Answer with nothing / `{}` / a file the script never wrote: the rows show
      their unpatched values, no error.
- [ ] Let the script exit non-zero: an error notification, rows unpatched, the
      view still usable.
- [ ] Bind the **same** script to `reload` instead: the `cells` are ignored
      there (that hook executes commands), which is the honest way round —
      one script, one hook.
- [ ] Bind two scripts to `load` on the same level: they run in name order and
      the second sees what the first wrote (patch the same column from both and
      the alphabetically last one wins).
- [ ] Drill into a child level: the hook stays silent there (its own scope).
- [ ] Failed load (no connection / adapter error): the hook does not run.

## A load hook decides the row order (`order`)

The third thing a `load` hook may answer with: the sequence the rows are shown
in. The point of the test is that the script order shows up on the first frame
and that the user's own sort still wins.

Preparation — a `commands`-mode script on the `load` hook that reverses what the
adapter delivered:

```python
#!/usr/bin/env python3
# mode: commands
import json, os, sys
with open(sys.argv[1]) as f:
    payload = json.load(f)
order = [row["id"] for row in reversed(payload["rows"])]
with open(os.environ["NYD_OUTPUT_FILE"], "w") as f:
    json.dump({"order": order}, f)
```

- [ ] Sort menu (`c s`), take **every** column out of the sort (`0`), `Enter`.
      Then `r`: the rows come up reversed on the first frame — no flicker from
      the adapter order into the script order.
- [ ] Shorten the answer to the last row's id only (`{"order": ["<last id>"]}`):
      that one row sits at the top, everything else keeps its original relative
      order behind it.
- [ ] Name an id that is not in this load: one message "… ordered row id(s) not
      in this load — ignored", the rest still ordered.
- [ ] Name the same id twice: one message "… named twice in `order` — first
      mention kept", and the row sits at its first position.
- [ ] Answer with a malformed `order` (`{"order": {"a": 1}}`, or a list of
      numbers): one error message, the rows keep the adapter's order.
- [ ] `c s` → sort by any column → `Enter`: the message "`order` ignored — this
      view is sorted by column …" appears and the table follows the **column**,
      not the script. `r` repeats the message; the sort stays.
- [ ] `c s` → `0` on that column → `Enter`: the script order is back on the
      next load.
- [ ] Bind the same script to `reload` instead (or run it by hand from the
      menu): "Script output has `order` — ignored (only a `load` hook paints
      and orders rows)", and the order does not change.
- [ ] One script answering with `cells` **and** `order` (patch a column, then
      order by the patched value): both land in the same load, one frame.
- [ ] Two scripts on `load`, both answering with `order`: they run in name
      order, so the alphabetically last one decides the final sequence.
- [ ] On a paginated level: the order applies within the page; paging forward
      re-runs the hook on the new page and does not pull rows across pages.
- [ ] The cursor after a reload sits on the same **row** (by node id), not on
      the same index — a reordering load must not silently move the selection
      to another ticket.

## A script on the `row_change` hook (the cursor moved)

The third hook: it fires from navigation, not from a load, runs detached, and
may answer with nothing. The point of the test is that it fires **once** per
resting row, that it names both rows, and that a slow or broken script never
gets in front of the frame.

Preparation — a `background`-mode script on the `row_change` hook of any level
with more than a handful of rows, appending one line per run:

```python
#!/usr/bin/env python3
# mode: background
import json, os, sys, time
with open(sys.argv[1]) as f:
    payload = json.load(f)
rc = payload["row_change"]
with open("/tmp/nyd-row-change.log", "a") as f:
    f.write(f"{time.time():.3f} {os.environ.get('NYD_SCRIPT_HOOK')} "
            f"{rc['previous_index']} -> {rc['next_index']} {rc['next_id']}\n")
```

- [ ] `ctrl+h` in the `:script` menu offers **row_change** next to reload and
      load, described as "when the cursor lands on another row"; the entry then
      carries a `[row_change]` suffix.
- [ ] Enter the level: the first selection is a change, so one line is written
      with `previous_index` **null** and `NYD_SCRIPT_HOOK=row_change`.
- [ ] `j` once, wait: one further line, `3 -> 4` naming both rows.
- [ ] Hold `j` through twenty rows: **one** line, from the row the burst
      started at to the row it ended on — not twenty. The list scrolls at full
      speed while it is held; nothing stutters per row.
- [ ] `j` then `k` back within the settle delay: no line at all — the pane
      never reported the row in between.
- [ ] `r` on the same row (a reload that keeps the cursor): no line. Identity
      is the node id.
- [ ] `c s` → sort by a column so the selected row moves to another index: no
      line either.
- [ ] Set `script.row_change_delay_ms: 1000` in `tui.yaml`, restart: a single
      `j` writes its line a second later; holding `j` writes nothing until the
      keys stop.
- [ ] Click a row with the mouse, and jump to one with the jump mode: both
      count as row changes — the detection sits in the loop, not in the key
      handler.
- [ ] Leave the tab and come back onto the unmoved cursor: no line. Move
      inside a split's other pane and back: each pane remembers its own row.
- [ ] A level whose load comes back empty: no line, and re-entering it later
      fires again with `previous_index: null`.
- [ ] Make the script sleep 3 s and hold `j`: the TUI stays responsive
      throughout, and only one child is alive at a time — the log shows the
      runs one after another, the last one for the row the cursor rests on.
- [ ] Make the script exit non-zero with something on stderr: one notification
      quoting the tail of it. Move the cursor ten more times: the identical
      message is **not** repeated ten times.
- [ ] Fix the script: the next row change reports nothing at all (no success
      notice per cursor move).
- [ ] Try to bind a `# mode: commands` script to `row_change`: refused with "a
      row_change hook may not answer with commands". `interactive` and
      `capture` are refused as for the other hooks.
- [ ] A `# scope: table` header on the hook script: the payload carries `rows`
      **and** the `row_change` block; with `# scope: filtered_set` the block is
      the only thing naming the cursor.
- [ ] Nothing of the script's stdout reaches the terminal — no paint artefacts
      in the table while it runs.

## A script declares its own payload scope (`# scope:`)

The level's `scope:` setting says what a script is handed; a `# scope:` header
in the script itself overrides that for this one script. The point of the test
is that all three run paths agree — a script that gets a different payload
depending on how it was started is broken from the menu.

Preparation — a script in a level whose configured scope is `node`, which just
writes its payload out:

```python
#!/usr/bin/env python3
# mode: commands
# scope: table
import json, os, shutil, sys
# argv[1] is the *path* to the payload file, not the JSON itself.
shutil.copy(sys.argv[1], "/tmp/nyd-scope.json")
with open(os.environ["NYD_OUTPUT_FILE"], "w") as f:
    json.dump({"commands": []}, f)
```

- [ ] Run it from `:script`: the file holds `{"rows": [...]}` with **every**
      visible row, not the `{"node": …}` the level would have handed over.
- [ ] Bind a chord (`ctrl+s`) and run it that way: same payload.
- [ ] Bind it as a `reload` hook (`ctrl+h`) and reload: same payload again.
- [ ] Remove the header, re-run all three ways: the payload falls back to the
      level's setting (`{"node": …}`). Scripts written before the header
      existed keep working unchanged.
- [ ] `# scope: filtered_set` gives the filtered set, `# scope: node` the
      single node — also on a level configured as `table`, i.e. the header
      narrows as well as widens.
- [ ] A garbage value (`# scope: nonsense`) is ignored, the level decides.
- [ ] The script **directory** and the chord stay tied to the level: the
      header only moves the payload, the script does not migrate to another
      view's folder.

## A computed column maintained by a reload hook

The combination: a `scope: table` script on a `reload` hook computes a custom
column for the rows that are on screen and writes them back in one go with the
`set-cells` collection action. Use a throwaway custom column for the test.

- [ ] Reload the view: only the **visible** rows are computed. Rows that the
      current query does not show are untouched — check a row that scrolled
      out of the query and see that its cell keeps its old value.
- [ ] The script diffs against the cell values in its own payload: with the
      view already up to date it writes **nothing** and hands back
      `{"commands": []}` — no reload, no flicker.
- [ ] Change a source value so one row goes stale, reload: exactly that one
      cell is written, the notification names the count, and the view reloads
      once. A second reload right after is a no-op (the loop settles).
- [ ] An empty payload (a query with no hits) still writes the answer file —
      `{"commands": []}` — instead of leaving the TUI waiting.
- [ ] Switch to a query showing a different row set: the cells of the newly
      visible rows are refreshed by the very load that shows them; the
      previously visible rows keep their stored values.
- [ ] A failing write (a value that violates the column's `value_type`) leaves
      **all** cells unchanged — `set-cells` is all-or-nothing — and the error
      names the offending row.

## The load banner on every tab (`Loading… (3s)`)

The engine counts the fetches it has out per tab, so a tab reports that it is
loading even when its adapter never publishes a status of its own. Test on a
slow remote tab (Jira, Taiga, Confluence) — a local one finishes too fast to
watch.

- [ ] Reload a slow tab: the banner line appears with a **second counter that
      advances** while the fetch runs (`Loading… (1s)`, `(2s)`, …) and
      disappears the moment the rows land. It also ticks while the tab sits
      untouched — no keypress needed to move the number.
- [ ] The same on a failed load: kill the connection first, reload, and the
      banner gives way to the error rather than counting forever.
- [ ] A fast local tab (tasks, SQLite) does **not** flash a banner — it is done
      inside the grace period.
- [ ] Drill into a slow child level: the banner runs for that fetch too.
- [ ] On a tab whose adapter reports its own state (Postgres, SQLite over a
      tunnel, the calendar), the richer line still wins — a label, a timeout
      countdown (`(3s/30s)`) or a percentage, **not** the generic `Loading…`,
      and never both at once.
- [ ] `notifications.load_banner: global` moves the generic line to the shared
      bar with the tab's name in front, and two tabs loading at once collapse
      to `2 tabs loading… (4s)`. With `off`, no line appears anywhere while the
      load still runs normally.
- [ ] An `auto_connect: never` tab shows `Loading…` while connecting, not the
      "press the key to connect" hint, and reverts to the hint if the load
      fails.

## The fuzzy-input keys read the config (`common.fuzzy_filter_*`)

They used to be hardcoded in the view, which made the config look like it
worked. Test on any tab with a `fuzzy_filter` action.

- [ ] With the defaults untouched, the filter behaves as before: `enter` closes
      the input and keeps the filter, `ctrl+u` wipes the query but stays in the
      input, `esc` on a filled query wipes it, `esc` on an empty one closes the
      input and restores the tree shape.
- [ ] Set `common.fuzzy_filter_cancel: ctrl+q` in `tui.yaml`, restart, open the
      filter: `ctrl+q` cancels and **`esc` does nothing at all** (the keystroke
      is simply ignored, the input stays open).
- [ ] Type into the filter afterwards — the letters still arrive; only the
      three bound keys are intercepted.
- [ ] Bind cancel to a plain letter (`common.fuzzy_filter_cancel: x`) and
      confirm the documented consequence: `x` fires instead of being typed into
      the query. Undo afterwards.

## Mouse: selection and clicking (`mouse` cargo feature)

Build and install the default binary — the feature is on by default.

### Window-local selection

The point of this part is the **clipping**: a drag must never leave the box it
started in.

- [ ] Open any popup with enough text in it (the shortcut overview on `o s`,
      the saved-filter picker on `q`, a script menu). Drag the mouse from
      inside the popup **out over the table behind it** and release: the
      highlight stays inside the popup's frame, no row of the table behind it
      is tinted, and the copied text contains popup lines only.
- [ ] Same drag, but drag _upwards/backwards_ past the anchor: the same text
      is selected, in the same order.
- [ ] Drag inside a content pane in a split layout: the selection stops at the
      pane's focus border and does not bleed into the neighbouring pane.
- [ ] The copied text has no panel padding on it — paste it somewhere and
      check that the lines start at the first real character and carry no
      trailing spaces. Blank rows at the end of the selection are dropped.
- [ ] Hold `Alt` while dragging: the selection is a **rectangle** (a column
      block), not flowing text. Useful for taking one table column.
- [ ] A row with a wide character in it (a tree glyph, an emoji, a CJK title)
      survives the round trip — pasted, it looks like what was on screen.
- [ ] Press any key after selecting: the highlight disappears. (A highlight
      left behind over a list that has since scrolled would be a lie.)
- [ ] Click once somewhere outside every panel: the old selection is dropped
      without anything else happening.
- [ ] Over SSH (no system clipboard reachable on the remote): the selection
      still lands in the clipboard of the machine you are sitting at, via
      OSC 52. If **no** path works at all, exactly one notification appears —
      `Could not reach the clipboard` — and nothing else.

### Clicking tabs and panes

One button does two jobs, so the first thing to check is that they stay apart.

- [ ] Click a **main tab** label: the tab switches, exactly as its number key
      would — same view, same focus, same bars.
- [ ] Click the **active** tab: nothing happens, no reload, no flicker.
- [ ] Click a **sub-tab** of the active view: it switches and, if that view has
      not been loaded yet, it loads — the same thing its key does.
- [ ] Click in the **gap between two tab labels** (and in the empty stretch
      after the last one): nothing switches. The bar is not one big button.
- [ ] A tab whose label carries an icon or an emoji: clicking the label's last
      character still hits that tab and not its neighbour.
- [ ] In a split layout, click into an **unfocused pane**: it takes the focus,
      the focus border moves, and the action bar switches to that pane's
      shortcuts. The keys then act on it.
- [ ] Press the mouse inside a pane, drag a few cells, release: that is a
      **selection**, not a click — the focus does not jump and no tab changes.
- [ ] Open a popup (`o s`), then click a tab behind it: the popup keeps the
      input and nothing switches — the same refusal the number key gets.

### Clicking rows and column headers

The row a click lands on has to be the row that was drawn there — the geometry
comes from the paint, so the interesting cases are the ones where a row is not
one line tall or not flush with the top edge.

- [ ] Click a row in a plain table: the cursor moves there and the preview /
      record detail follows, exactly as walking to it with `j` would.
- [ ] Click the row the cursor is already on: nothing reloads and nothing
      flickers.
- [ ] In a **tree** view with folded and expanded nodes, click a row far down:
      the cursor lands on that row, not on a row counted off the top edge.
- [ ] In a view with **multiline rows** (a chat log, a long-text column), click
      the _second_ line of a row: it selects that row, not its neighbour.
- [ ] With **smooth scrolling** on, scroll so the top row is half cut off, then
      click the row below it: still the right row.
- [ ] Click a **group header** row (a grouped view): the cursor lands on the
      nearest selectable row, the same place `j` stops — it never sits on the
      header.
- [ ] Click into the **empty space** below the last row: nothing moves.
- [ ] **Double-click** a row: it does what `Enter` does there — drills into a
      tree node, opens a message, whatever the view binds. A slow second click
      (over ~0.4 s) does not; three fast clicks activate once, not twice.
- [ ] Click a **column header**: the table sorts by it ascending, a
      notification names the column, and the header shows the arrow. Click it
      again: descending. A third time: the sort is cleared.
- [ ] Sort by one column via header click, then by a second: the second is
      added, the first is kept — the same additive behaviour as `S` and the
      sort menu (`c s`). Open `c s` afterwards and it shows both.
- [ ] Click a header of a column the adapter cannot sort on: nothing happens —
      in particular the cursor does not jump to a row.
- [ ] In **card mode** (no header row), click anywhere in a card: the card is
      selected, nothing tries to sort.

### Word and line selection

What the terminal used to do with a double and a triple click, now done by the
app — and clipped to the window under the pointer like every other selection
here.

- [ ] Double-click a word in a preview or detail pane: the word is highlighted
      and on the clipboard, ready to paste elsewhere.
- [ ] Double-click a ticket key / a path / a URL: it comes out **whole**, not
      cut at the `-`, the `/` or the `:`.
- [ ] Double-click a word inside quotes or brackets: the quotes and brackets
      stay behind.
- [ ] Double-click a blank cell or a tree connector: nothing is selected and
      nothing is copied.
- [ ] Double-click a word near a popup's edge: the selection stops at the
      edge — it does not read on into the table behind the popup.
- [ ] **Triple-click** any line: the whole line is selected without the panel's
      padding, and copied.
- [ ] Triple-click a **table row**: the row's text is selected — the third
      click selects even where the second one opens.
- [ ] Double-click a table row: it still opens (`Enter`), it does not select a
      word. Same for a popup entry: it is still picked.
- [ ] Click once somewhere else afterwards: the highlight goes away.
- [ ] Click four times fast on the same cell: the run starts over at a single
      click instead of escalating further.

Holding a multi-click shows its unit, and dragging out of it keeps that unit:

- [ ] **Double-click and hold** in a preview or detail pane, without moving:
      the word is highlighted while the button is down.
- [ ] **Triple-click and hold** there: the whole line is highlighted.
- [ ] Double-click and hold on a **blank** cell: nothing is highlighted, no
      stray tinted cell.
- [ ] Double-click a **table row** fast: it opens as before, with no word
      flashing up first. Same for a popup entry.
- [ ] **Double-click and hold**, then drag sideways: the selection grows word by
      word — the word you pressed in stays whole, and so does the one under the
      pointer, even with the pointer in the middle of it.
- [ ] Keep dragging back past where you started: the selection shrinks the same
      way and ends up exactly where it began, still whole words.
- [ ] Drag onto a blank gap between two words: the selection ends at the last
      word, without a trailing space.
- [ ] **Triple-click and hold**, then drag down a few rows: whole lines are
      taken, edge to edge of the pane, not a ragged block.
- [ ] Release either drag: the text is on the clipboard, in the shape that was
      highlighted.
- [ ] Press once and drag as usual: still character by character — the plain
      drag is unchanged.
- [ ] Right after releasing a word drag, click once: it counts as a single
      click again (it does not escalate to a line).

### The fold marker

Only the run in front of a foldable row's label folds it: indentation, box
connectors and the `▶` / `▼` arrow. A single click there is the same toggle
`Enter` performs on that row.

- [ ] Click the `▶` of a collapsed tree row: it unfolds, and the cursor is on
      it afterwards — including when it was not the selected row before.
- [ ] Click the `▼` of the row you just unfolded: it folds again.
- [ ] Click the box connector (`└──`) or the indentation in front of a **child**
      row's arrow: it toggles too — the whole run counts, not just the arrow.
- [ ] Click the label **text** next to the arrow: the row is only selected, it
      does not fold.
- [ ] Click the indentation in front of a **leaf** (a row that cannot expand):
      the row is selected, nothing folds and nothing flickers.
- [ ] Fold a row whose subtree is long, so the rows below jump up, and click the
      marker of the row now under the pointer: it toggles that row — the
      geometry is the one of the frame you are looking at.
- [ ] Scroll horizontally (`h` / `l` with a column cursor, or a narrow window)
      so the label column moves: the marker still sits on the arrow you see,
      not at the old x.
- [ ] With **smooth scrolling** on, scroll so a tree row's top line is cut off
      by the top edge: clicking where its arrow would be selects rather than
      folds — no connector is visible there.
- [ ] In a view with **multiline rows**, click the second line of a foldable
      row at the marker's x: it selects, it does not fold.
- [ ] Double-click a fold marker fast: the row toggles **once** and stays that
      way — it does not spring back.

### The breadcrumb line

Only shown once you are drilled into a level. Each crumb goes back to that
level, running the same ascent `Backspace` runs.

- [ ] Drill two levels deep (twice `Enter` on a row that has children). Click
      the **root** crumb: you are back at the top level, with the cursor and
      the scroll position the level had when you left it — the same state the
      back key restores.
- [ ] Drill two levels again and click the **middle** crumb: exactly one level
      up, not all the way out.
- [ ] Click the **last** crumb (the level you are on) and the child-type crumb
      behind it: nothing happens, nothing reloads.
- [ ] Click a **separator** (`›`) between two crumbs: nothing happens.
- [ ] Drag across the whole path and release: the breadcrumb text is copied and
      the selection stays on that line — it is not trapped in one crumb and
      does not reach into the table below.
- [ ] Narrow the terminal until the path is cut off: the crumbs still on screen
      work, and clicking past the end of the line does nothing.
- [ ] At the top level there is no breadcrumb line at all — clicking that row
      hits whatever is drawn there instead.

### Popup modality

A popup owns the input. For the mouse that means the same refusal the keys
already get, everywhere outside the panel.

- [ ] Open a popup (`o s`, `q`, the sort menu `c s`) and click a **tab** behind
      it: nothing switches.
- [ ] Click a **row of the table** behind it: the cursor does not move, no
      preview loads, nothing drills.
- [ ] Click a **column header** behind it: nothing sorts.
- [ ] Click an **unfocused pane** behind it in a split layout: the focus stays
      where it is.
- [ ] Click a **breadcrumb** behind it: the level does not change.
- [ ] Wheel over the table behind it: the popup's own list scrolls; in
      particular the pane under the pointer does not take the focus.
- [ ] Wheel over the **tab bar** behind it: the tabs do not walk.
- [ ] Selecting still works: drag across the table behind the popup and
      release — the text is copied. Double-click a word there: it is selected
      (behind a popup even the second click is free, since nothing there acts).
- [ ] The popup itself is unaffected: its entries still take a click, a double
      click still picks.
- [ ] Close the popup (`Esc`) and click the same spot again: now it acts.
      Clicking outside is _not_ a way to close the popup.
- [ ] Hold a chord until the **which-key** panel appears and click the table:
      nothing happens there either — the chord is still waiting for a key.

### Clicking popup entries

The cursor is walked to the clicked row with the popup's own arrow keys, so
what has to hold is that it lands on the row under the pointer no matter what
sits above the list.

- [ ] Open the shortcut overview (`o s`) and click an entry well below the
      cursor: the cursor lands on that entry, not one offset by the heading.
- [ ] Type into the popup's filter first, then click a row of the narrowed
      list: it selects the row you clicked, counted in the _filtered_ list.
- [ ] Scroll a long popup list down a page, then click the top row: that row,
      not the first entry of the unscrolled list.
- [ ] **Double-click** an entry: it is picked, exactly as `Enter` would.
- [ ] Click the entry the cursor is already on: nothing happens.
- [ ] Hold a chord (`o`, `w`, …) until the **which-key** panel appears and
      click one of its rows: nothing happens — and in particular the table
      behind the panel does not move its cursor.
- [ ] Drag across two entries of a popup list: the selection spans both lines
      and stays inside the popup — it is not trapped in the single row the
      drag started on.

### The wheel

- [ ] Wheel over an **unfocused** pane in a split layout: that pane scrolls and
      takes the focus with it, so the arrow keys carry on where the wheel
      stopped.
- [ ] Wheel over the **tab bar**: it walks through the tabs, down = next, and
      wraps the way the tab keys do.
- [ ] Wheel over a popup: the popup's list scrolls, not the table behind it.

The wheel pans the view (`mouse.wheel: view`, the default) — take a table
longer than the pane for these:

- [ ] Put the cursor on a row in the middle, wheel **down** one notch: the
      content moves up by three rows and the **highlight stays on its row** —
      it slides up the screen with the row, it does not walk down the list.
- [ ] Keep wheeling down: once the row would leave the top edge the highlight
      comes along and rides the **top** edge from then on. Wheel back up and it
      rides the **bottom** edge the same way.
- [ ] Wheel down until the last row rests on the bottom edge: scrolling stops
      there (no empty space scrolled in). Keep wheeling: now the **cursor**
      walks on and reaches the very last row. Same at the top.
- [ ] After panning, press `j` once: it continues from the highlighted row, not
      from wherever the cursor was before the wheel — pane focus and cursor
      agree.
- [ ] Pan, then let a reload happen (or hit the reload key): the view stays
      where it was panned to instead of jumping back to the cursor's old
      position.
- [ ] Wheel over a table whose rows all fit in the pane: nothing scrolls, and
      the cursor walks instead of standing still.
- [ ] In a **tree** with group headers, pan so the top edge lands on a header
      row: the highlight lands on the first selectable row below it, not on the
      header.
- [ ] In the **chat** view (smooth scrolling): a notch moves three _physical
      lines_, so a long message glides rather than jumping, exactly as `j`/`k`
      do there.
- [ ] Set `mouse.wheel: cursor` in `tui.yaml`, restart: the wheel walks the
      cursor again like `j`/`k` and the view only follows at the edge.
- [ ] Set `mouse.wheel_rows: 1`, restart: one row (one line in the chat) per
      notch, in both modes.

Regressions to rule out, because mouse reporting takes things away from the
terminal:

- [ ] The wheel still scrolls the focused list, three rows per notch, up and
      down. (Without reporting the terminal turned the wheel into arrow keys;
      the app now has to do that itself.)
- [ ] Wheel **sideways** (if the mouse has it) and wheel inside the editor
      overlay: unchanged — those still go through the arrow keys.
- [ ] `Shift` + drag still gives the **terminal's own** selection — spanning
      panes, whole rows, its own colours. This is the escape hatch and needs
      no configuration.
- [ ] Launch an external editor (`e` on a row) and quit it again: back in the
      TUI, both selection and the wheel still work — the editor suspend/resume
      path has to restore mouse reporting along with the keyboard protocol.
- [ ] Run an interactive script and return: same check.
- [ ] Quit the TUI: the terminal is not left in mouse-reporting mode —
      selecting in the shell afterwards behaves normally.

Colours, settings and the off switch:

- [ ] Click anywhere once, slowly: **no coloured block** appears under the
      pointer while the button is down, and none is left behind afterwards.
      (This is `mouse.highlight_press`, off by default.)
- [ ] Set `mouse.highlight_press: true` in `tui.yaml`, restart: now the cell
      under a press is tinted while the button is down, which shows where a
      drag is anchored. A drag and a double click look the same as before in
      both settings.
- [ ] Set `theme.mouse.selection` / `theme.mouse.selection_bg` in
      `tui-theme.yaml`, restart, select: the highlight uses them. Remove them
      again and the selection falls back to background-on-accent.
- [ ] Build without the feature
      (`cargo build --release -p not-yet-done-tui --no-default-features --features clipboard`),
      install and start it: dragging selects the way the terminal always did
      (across the popup border, whole rows), no key or wheel behaviour has
      changed, and a `tui-theme.yaml` that still carries a `mouse:` block —
      or a `tui.yaml` with a `mouse.highlight_press` in it — loads without a
      warning.

## Jira: the linked tickets of an issue (`jira:link`, `o l`)

A child level under `jira:issue`, built like the comment and attachment levels.
A row is the **link**, not the ticket at its other end: `relation` is the phrase
seen from the parent issue's side and the key of the other ticket travels in the
`key` field, which is what lets the ticket-level bindings retarget themselves
with `node_id_from: key` while `d` removes only the link. Jira delivers all
links of an issue in one field and offers no server-side ordering for them, so
the level sorts locally. Prerequisite: the action and the `Links` child are in
the private `views/jira.yaml` (see `docs/examples/views/jira.yaml`); in the
example they sit on a bare `l`, in a which-key setup on `o l`.

- [ ] `o l` on a ticket with links opens the table: one row per link, columns
      Relation / Key / Type / Status / Priority / Assignee / Summary. The
      breadcrumb shows the level, `Esc` goes back to the ticket list.
- [ ] The relation is phrased from **this** ticket's side: on the ticket that
      blocks another one the row says "blocks …", and opening the level on that
      other ticket says "is blocked by …". A wrong orientation would be the
      symptom of the `outwardIssue`/`inwardIssue` inversion being dropped.
- [ ] The Key column names the ticket at the other end (never the parent), and
      Status/Priority/Assignee/Summary describe that same ticket.
- [ ] `o l` on a ticket without links → an empty table with the usual
      empty-level hint, no error banner.
- [ ] `S` sorts by every one of the seven columns, ascending and descending,
      without a JQL banner (the sort is local — nothing goes to the server).
- [ ] `f` (fuzzy filter) matches on key, summary **and** relation phrase.
- [ ] `p` opens the preview: the buffer of the **linked** ticket (body plus
      comments), not an empty pane. An empty pane means the `preview:` block
      lost its `node_id_from: key`.
- [ ] `e` opens the linked ticket in the Markdown editor; save writes back to
      **that** ticket (check its `updated`), not to the parent.
- [ ] `o` (in the example config) opens the linked ticket in the browser.
- [ ] `d` asks "Remove link '<relation>' from <KEY>? The ticket stays. (y/n)".
      On "y" the row disappears after the reload; the linked **ticket** still
      exists (look it up in the ticket list) and the link is gone on its side
      too. On "n" nothing happens.
- [ ] The bookmarks subtab (`m`) has the same level with the same bindings.
- [ ] Headless: `nyd adapter jira <KEY> ls --type jira:link` lists the same
      rows, and `nyd adapter jira <KEY>/link/<ID> show` resolves the composite
      id. Note: `nyd adapter jira:issue:link <KEY> ls` does **not** work — the
      `ls` path ignores the child-type segment and always lists comments (a
      pre-existing quirk, not specific to this level).
- [ ] Anon mode: keys, summaries and assignees appear replaced, the relation
      phrases (Jira-defined, e.g. "relates to") verbatim.

## Jira: linked tickets back into the ticket list (`apply_query`, `o a` / `o t`)

Two `type: apply_query` actions on the link level. They read the `key` cell off
the rows, render it into JQL and show the result in the **tickets** list — which
is what gets the ticket level's own bindings (edit, transition, `c c`, `S`,
preview) back for a linked ticket. `o a` takes every link of the issue, `o t`
only the row under the cursor. Prerequisite: both actions are in the private
`views/jira.yaml` (the repo example binds them to `A` / `t` on the link level).

- [ ] On a ticket with several links: `o l`, then `o a` → the pane is back at
      the tickets level (breadcrumb gone), showing exactly the linked tickets
      and nothing else. The query line reads
      `issuekey in ("KEY-1", "KEY-2", …) ORDER BY updated DESC`.
- [ ] Those rows are ordinary tickets: `e e` edits, `a t` transitions, `p`
      previews, `c c` and `S` work — the level is the ticket level, not a
      leftover child level.
- [ ] `o l`, cursor on one link, `o t` → the tickets list holds that **one**
      ticket (query `issuekey = "KEY-n"`) — the linked one, not the parent.
- [ ] After either: `q` (the query menu) picks a saved query again and the list
      returns to it — the applied query is an ordinary active query, not a mode.
- [ ] Fuzzy-filter the link level first (`f f`, e.g. by a project prefix), then
      `o a`: only the **visible** rows are in the query. This is by design (the
      level's rows are what it reads), so it is a check on the filter, not a bug.
- [ ] `o a` on a ticket **without** links → the notification says nothing was
      found on this level, and the list stays where it is.
- [ ] From the **bookmarks** subtab's link level the two keys are not bound (it
      has no query of its own). If you do bind them there, the action reports
      "this list takes no query" rather than silently listing the bookmarks.
- [ ] The jump really re-lists: the rows are the query's result from Jira, not
      the pane's previous ticket list. (It is a soft load like any other, so a
      repeated `o a` may come from the cache — `r` forces a fresh fetch.)
- [ ] Page the ticket list forward first (a `>`/next-page step), then `o l` and
      `o a`: the result starts at page 1 instead of asking for rows past the end
      of a much shorter list.

## HTML preview on `o p` — one engine, one profile per adapter (Jira + Taiga)

The preview script was split: the whole pipeline (export the workspace, build
the HTML with pandoc, place the window) lives once in
`~/.local/share/not_yet_done/scripts/_lib/nyd_html_preview.py`, and each entry
point is a three-line wrapper that names a **profile**. `_lib` is a sibling of
the `<tab>/<node_type>` folders, not a tab: script discovery only reads the one
directory a level maps to, so it is never enumerated.

Mail is not a profile of this engine. A message has no workspace and no
Markdown, and its page follows the cursor — it has its own renderer,
`_lib/nyd_mail_preview.py`, tested under "Mail: the message under the cursor in
the browser".

The profiles differ where the two trackers do:

|                                    | `jira`                                                      | `taiga`                                                                 |
| ---------------------------------- | ----------------------------------------------------------- | ----------------------------------------------------------------------- |
| Markdown source                    | wiki markup converted by the adapter                        | already Markdown                                                        |
| Jira icon filter (`(-)`, `(/)`, …) | on                                                          | off — those glyphs are literal text                                     |
| `#### CACHE` block                 | stripped                                                    | does not exist                                                          |
| Staleness check                    | remote `updated` vs. mtime, old file kept as `.bak-<stamp>` | none — always re-export                                                 |
| Node id vs. workspace key          | both the key (`KEY`)                                        | id is `task:123`, key comes from the `ref` cell (`proj#42` → `proj-42`) |

Taiga always re-exports because its `ticket.md` is a pure snapshot: no editor
writes back to it, so there is nothing to lose and no cheap way to ask Taiga
for a change stamp without a query body.

Prerequisites: the `o p` shortcut rows in `nyd.db` (`script:jira/jira:issue`
and `script:taiga/taiga:item`, both named `html_preview.py`), and the TUI
restarted after inserting one — script shortcuts are cached per scope.

- [ ] Jira `o p` on a ticket still opens the preview as before: icons rendered,
      no `#### CACHE` block, window title `pandoc-preview/nyd-jira: <KEY>`.
- [ ] Jira `o x p` still opens the **dev** ticket's preview; on a ticket with
      no `dev_ticket` cell it fails with "no dev_ticket set" and no window.
- [ ] Taiga `o p` on an item opens a window titled
      `pandoc-preview/nyd-taiga: <project>-<ref>`.
- [ ] The document shows two header tables (subject/status/assignee/tags and
      ref/type/creator/modified), then the description, then the comments under
      `### @author <when>`. No `===` markers and no completions block — those
      belong to the edit template, not to a document.
- [ ] Every run re-exports: change the subject in Taiga, hit `o p` again, the
      new subject is there without any manual refresh.
- [ ] An item with attachments: the folder
      `<data>/not_yet_done/taiga/<inst>/tickets/<key>-<slug>/attachments/`
      holds the files, and the document ends in an `## Attachments` section —
      images embedded, other files as links. A name with spaces must still be a
      working link (the target is wrapped in `<…>`); a bare `](path with
spaces)` would render as literal text.
- [ ] An attachment the description already embeds appears **once** — inline,
      not a second time in the attachment list.
- [ ] Replace an attachment in Taiga under the same name, run `o p` again: the
      new file is fetched (the sidecar compares the modification stamp, not
      just the file's existence).
- [ ] An item without comments has no `## Comments` heading; one without
      attachments no `## Attachments` heading.
- [ ] `ticket_workspace:` in `views/taiga-adapter.yaml` redirects the folder
      (`~` expanded); unset it falls back to
      `<data-local>/not_yet_done/taiga/<instance>/tickets`.
- [ ] Headless: `nyd adapter taiga:item <id> export_workspace` prints
      "<ref>: exported workspace to <dir>" — the marker the script parses. With
      `--field dir=/tmp/x` it writes there instead.
- [ ] The width picker at the top of the page works in both profiles and the
      choice survives a reload (localStorage).

## Highlighting rows, columns and cells (`highlights:`)

Colour that belongs to the view rather than to the theme. The tests walk the
two producers — the declarative rules and the `load`-hook script — and then the
four surfaces they paint on. Spec:
[`generic-view-spec.md`](generic-view-spec.md#highlights--painting-rows-columns-and-cells).

Preparation — in a view file with a level that has a `status`-like text column
and a numeric one:

```yaml
styles:
  over-budget:
    fg: "#ffffff"
    bg: "#7a1c1c"
    modifiers: [bold]
    selected: { bg: "#a52222" }
  muted: { fg: "#6c6c6c" }

views:
  - name: tickets
    highlights:
      - columns: [<numeric column>]
        style: { bg: "#1c2430", selected: { bg: "#2c3a50" } }
      - when: { field: <status column>, matches: "^(Blocked|On Hold)$" }
        style: over-budget
      - columns: [<status column>]
        when: { field: <status column>, matches: "(?i)^done$" }
        style: muted
```

### The style grammar (phase 1)

- [ ] The three style forms all load: a name (`style: over-budget`), an inline
      map (`style: { fg: "#ff5555" }`) and a bare modifier list
      (`style: [bold, italic]` — font change only, colours untouched).
- [ ] A `styles:` entry of the **view file** shadows one of the same name in
      `tui-theme.yaml`; removing it again falls back to the theme's.
- [ ] `fg: accent` (a theme role) still works next to hex; a bare `abc123`
      without `#` is treated as a role name, not found, and reported.
- [ ] `fg: auto` on a dark `bg` yields the light candidate and on a light `bg`
      the dark one; changing `auto_fg_light`/`auto_fg_dark` in
      `tui-theme.yaml` changes which colour appears.
- [ ] `fg: auto` with a `selected: { bg: … }` of a very different lightness:
      the cursor row picks the **other** candidate — the text stays legible
      when the cursor arrives.
- [ ] `fg: auto` in a style with no `bg` anywhere: nothing is repainted, the
      layer underneath keeps its foreground (no black-on-black).
- [ ] `bg: auto` is reported as ignored; `selected:` inside `selected:` and
      `modes:` inside `selected:` likewise — each a warning, the file still
      loads.

### The rules (phases 2–4)

- [ ] The column rule paints the numeric column in **every** row, and its
      `selected:` background shows while the cursor is on such a row.
- [ ] The row rule paints whole rows whose status matches, background and bold
      included; the row loses its colour again after the status changes and the
      view reloads.
- [ ] The cell rule paints only that one cell in matching rows — the rest of
      the row keeps its normal colours.
- [ ] Precedence: on a row that matches both the row rule and the cell rule,
      the cell wins in the cell and the row colour stands everywhere else.
- [ ] The cursor row keeps the highlight's **foreground and modifiers**; only
      the background falls back to the normal `RowSelected` when the rule set
      no `selected.bg`.
- [ ] Two rules that set different fields (one only `fg`, the other only `bg`)
      combine on the same row; two that set the same field: the **later** one
      in file order wins.
- [ ] `when:` in the list form works for numbers/dates
      (`when: [<numeric column>, ">", 5]`), which the short form cannot express.
- [ ] `when:` in the full form with `and:`/`or:` fires as written.
- [ ] The regex is matched against the **raw** value: a `kind: duration` column
      matches `5400`, not the rendered `1h 30m`; a narrowed column still
      matches on its full value, not on the truncated text with `…`.
- [ ] Case: `matches: "^done$"` does **not** hit `Done`; `'(?i)^done$'` does.
- [ ] A regex written double-quoted with a backslash (`"\d+"`) is a YAML error
      at load; single-quoted (`'\d+'`) it works.
- [ ] A rule with an unresolvable style name, and one with an invalid regex:
      each reported with its level path (`tickets > … .highlights[N]`), each
      inert, and every **other** rule of the file still paints.
- [ ] A rule naming a column the level does not have: reported. A rule naming a
      column that is currently hidden via `c c`: no message, nothing painted;
      unhiding it makes the colour appear.
- [ ] Highlights on a column value produced by a **custom column** or by a
      `load`-hook `cells` patch fire — by then both are ordinary fields.
- [ ] `matches` written into a saved query against the task DB fails loudly
      ("operator not supported in SQL filters") instead of matching nothing.

### The script channel (phase 5)

Preparation — a `background`-mode script bound with `ctrl+h` → `load`:

```python
#!/usr/bin/env python3
# scope: table
# mode: background
import json, os, sys
payload = json.load(open(sys.argv[1]))
out = {row["id"]: {"<numeric column>": {"bg": "#7a1c1c", "fg": "auto"}}
       for row in payload["rows"][:3]}
with open(os.environ["NYD_OUTPUT_FILE"], "w") as f:
    json.dump({"highlights": out}, f)
```

- [ ] The first three rows show the script's background on that column, on the
      **first** frame after `r` — no flicker from unpainted to painted.
- [ ] `"*"` in the column axis (`{"<id>": {"*": {…}}}`) paints the whole row;
      `"*"` in the row axis (`{"*": {"<col>": {…}}}`) the whole column;
      `{"*": {"*": {…}}}` the whole table.
- [ ] Specificity: a table-wide `*`/`*` plus a row-specific entry — the
      specific one wins on its row, the wide one everywhere else.
- [ ] A script style **wins over** a YAML rule on the same address, still per
      field: a rule setting only `fg` and a script setting only `bg` combine.
- [ ] The script may answer with a **name** from `styles:` instead of an
      object (`{"<id>": {"<col>": "over-budget"}}`).
- [ ] An unresolvable style in the answer: one warning naming
      `highlights['<id>']['<col>']`, every other entry still painted.
- [ ] A row id that is not in this load: "… highlighted row id(s) not in this
      load — ignored", the rest painted.
- [ ] `"highlights": []` (wrong shape): one error message, the load survives
      and the rows appear unpainted.
- [ ] Two scripts bound to `load` on the same level: they fold in name order,
      the alphabetically last one winning per (row, column).
- [ ] Bind the same script to **`reload`** instead: "Script output has
      `highlights` — ignored (only a `load` hook paints rows)", nothing is
      painted. Running it by hand from the menu says the same.
- [ ] A script answering with nothing but `highlights` on a `reload` hook gets
      **one** message, not additionally "missing `commands` array".
- [ ] Staleness: edit a value the script colours from, without reloading — the
      colour stays as it was until the next load. (Expected, documented.)
- [ ] A ramp script (colour computed per row from a ratio) produces a visibly
      graded column, and `fg: auto` keeps every step legible from end to end.

### The surfaces (phase 6)

- [ ] **Table** — as tested above.
- [ ] **Card** (`card:` on the level): a rule without `columns:` paints the
      whole card; with `columns: [x]` only the **value** of that field, its
      label keeping the card's own label style.
- [ ] **Details** (`o`, the record-detail split): the rules of the **source**
      level fire against the record on show; a rule with `columns: [x]` paints
      the value cell of row `x` in the split.
- [ ] A value in the split that **wraps** over several lines carries the colour
      on all of them, not only the first.
- [ ] Moving the cursor in the source pane repaints the split for the new
      record.
- [ ] **Tree**: a row wears its **own** level's rules — a rule on the parent
      level does not colour the children, and vice versa (highlights are not
      inherited, unlike `columns:`).
- [ ] Two tree levels with different rules, both expanded: each row is painted
      by its own level, with no colour bleeding between them.
- [ ] A cell rule in tree mode whose column the level does not render paints
      nothing; on `tree_label` the pre-styled segments may swallow it — the row
      form works there.
- [ ] `modes: [table]` on a rule: painted in the table, not on the card and not
      in the split. `modes: [card, details]` the other way round.
- [ ] `modes:` on the **style** alone restricts it the same way; a rule with
      its own `modes:` **replaces** the style's list rather than intersecting
      (style `[table]` + rule `[card]` paints on the card).
- [ ] The same named style used by two rules of different reach behaves
      differently per rule — no duplicate style needed.

## Repeating a request that produced no answer (`retry:` in every adapter)

Jira, Confluence, Kimai, Taiga and Stoat all send their requests through the
same seam (`not-yet-done-content/src/http_send.rs`). A transport failure — the
connection refused, reset or timed out — is repeated as far as the adapter's
`retry:` block and the HTTP method allow; an HTTP status, even 5xx, is never
repeated. Test one adapter thoroughly and the rest by spot check.

- [ ] **A brief outage heals silently:** cut the connection to the instance
      (block the tunnel/network), trigger a listing, restore the connection
      within the backoff → the list appears without an error banner.
- [ ] **A lasting outage still ends in a clean error:** leave the connection
      cut → after the attempts are used up a normal error message appears, no
      hang. With `attempts: 4` the wait is visibly longer than with the
      default 2.
- [ ] **`attempts: 1` switches repeating off:** the first failure is the error.
- [ ] **The log names the repeat:** with `NYD_DEBUG=1`, the log file carries a
      `RETRY <METHOD> <url>: attempt 1 of 2 failed: …` line before the second
      request, and the failure line names the _cause_ ("connection refused"),
      not just "error sending request".
- [ ] **A write is not doubled after a timeout:** with a very short
      `request_timeout_secs` (e.g. 3) against a healthy but slow instance,
      post a comment / transition an issue → it lands **once**. Check the
      instance: no duplicate comment, no double transition.
- [ ] **A write is repeated when nothing reached the server:** stop the
      instance (or block it before the handshake), post a comment, bring it
      back within the backoff → the comment lands once, no error.
- [ ] **A read is repeated even as a POST:** the Jira search (`POST
/rest/api/2/search`) survives a dropped connection — the very case
      "error sending request for url …" came from.
- [ ] **An unknown field in the block is rejected:** `retry:` with
      `attemps: 3` (typo) makes the adapter fail to load with a config error
      naming the field, rather than being silently ignored.
- [ ] **An absent block means the default:** an adapter YAML without `retry:`
      behaves as before (2 attempts, 250 ms).
- [ ] **Attachments are unaffected:** uploading an attachment (Taiga, Stoat)
      still works; its multipart body cannot be repeated and is sent once —
      the timeout still bounds it.

## Taiga: comments by somebody else in the `e e` buffer (`[not yours]`)

Taiga only lets a comment's **author** edit it — a foreign edit is answered
with `403 PermissionDenied` on `…/history/<type>/<id>/edit_comment`, while the
item PATCH of the same save still goes through. The buffer therefore marks
those comments and the adapter refuses the edit before a request goes out.
Deletion stays open, because Taiga also lets project admins delete a foreign
comment.

Points 1, 3 and 5 are confirmed headless: `EDITOR` is pointed at a script that
prints the prepared buffer and exits non-zero, so the CLI aborts before it
executes, and the edited buffer is fed back with
`adapter taiga <type>:<id> edit_with_comments --file <buf>`. A refusal sends no
request, and an unchanged header sends no PATCH, so the run touches nothing.

- [x] `e e` on a ticket that carries comments from two people: the header line
      of every comment written by somebody else reads
      `--- @author <ts> [not yours] (id=…) ---`, own comments are unmarked.
- [ ] Change nothing, `:wq` → no `edit_comment` request at all (with
      `NYD_DEBUG=1` the log shows none), the notification reports no comment
      operations.
- [x] Change a **foreign** comment, `:wq` → the item changes land and the
      message names the refusal locally: `edit <id>: not authored by you — …`.
      No 403 in the log, because no request was sent.
- [x] Change **only** a foreign comment and nothing else → the refusal is still
      reported (`unchanged (errors: edit <id>: …)`). A refusal is news even when
      no write happened; answering `no changes` would read as an empty buffer.
- [ ] Change an **own** comment → it is updated as before, `comments: ~1`.
- [x] An editor that strips trailing whitespace on save (`:%s/\s\+$//` before
      `:wq`, or an autocmd doing it) → no comment is counted as changed, no
      request, no refusal. This is the case that produced the 403 without
      anybody touching a comment.
- [ ] Indentation and blank lines still count: add a blank line inside an own
      comment, or indent a line by four spaces → the comment is updated.
- [ ] Delete the ` [not yours]` marker by hand and edit the body → still
      refused locally: ownership is read from the buffer as it was rendered,
      not from what the user typed.
- [ ] `del` as the sole line of a foreign comment → the request goes to Taiga:
      as a project admin it is deleted, otherwise the server's 403 appears in
      the message.

## A credential slot that may ask (`type: script`)

A provider slot that has no `auth:` block behind it — a calendar connection's
`password:`, an SSH hop's — can still run the credential script that speaks the
round protocol, and its form is put in front of the user over the adapter's
prompt stream. So a locked password store asks _inside_ the TUI instead of
letting `gpg` open a `pinentry` window the frontend knows nothing about.

Arrange the two states with the agent, not with the config: `gpg-connect-agent
reloadagent /bye` drops the cached passphrase (locked), any successful decrypt
warms it again (unlocked). `pgrep -a pinentry` while a dialog is up is the
check that nothing opened behind the TUI.

- [ ] **Warm agent**: open the tab whose connections use the script → the rows
      arrive with no dialog at all. The script is still run; it just has
      nothing to ask.
- [ ] **Locked agent**: reload the agent, then reconnect → the TUI's own
      credential form appears, titled by the script's `header`, the passphrase
      field masked. `pgrep -a pinentry` finds nothing.
- [ ] Answer it correctly → the connection completes and the following
      connections come up silently (the agent is warm now).
- [ ] **Wrong passphrase** → the same form comes back, this time with the
      script's `error` line above it. Answering correctly on the retry still
      completes.
- [ ] Keep answering wrong → after the round cap the connection fails with
      `still asked for input after 5 rounds`, and the TUI stays usable.
- [ ] **Dismiss** the form (`esc`) → the connection reports the credential as
      unavailable and names the script. No hang, no second dialog, the tab can
      be reloaded to try again.
- [ ] **Several slots, one script**: with a cold agent, reconnect a tab whose
      connections all read from the same script → **one** dialog, not one per
      connection; the others wait for it and then resolve.
- [ ] **No listener** (CLI): with a warm agent `adapter <inst> ls` works as
      before. With a cold one it fails loudly — the message names the script
      and says there is no interactive frontend — instead of hanging or
      opening a `pinentry` window in a headless run.
- [ ] A script that returns **several** values into a slot without `field:` →
      the error names the keys it got and asks for `field:`. Adding
      `field: <key>` resolves it.

## Per-adapter auto reload (`adapter.auto_reload`)

Set on the Jira instance as `auto_reload: 10m`. For a quicker round use
`auto_reload: 30s` while testing and put it back afterwards.

- [ ] **It fires**: open Jira, let it load, note the top row, change a ticket
      in the browser so the order or a cell must change, wait out the
      interval → the table refreshes by itself; the load banner appears
      briefly, the selected row is kept.
- [ ] **It refreshes, it does not connect**: restart the TUI and do _not_
      open the Jira tab. After more than one interval nothing has been
      fetched — no credential dialog, no banner on the tab bar. Only after
      the first `r` does the timer start running.
- [ ] **A manual reload resets the clock**: press `r` shortly before the
      interval runs out → the automatic reload does not come right after it,
      but a full interval later.
- [ ] **Drilled in**: drill into a ticket's comments and wait out the
      interval → the _comment_ level reloads, the pane does not jump back to
      the ticket list.
- [ ] **A background tab too**: switch to another tab, wait out the
      interval, switch back → the data is fresh (the fetch ran without the
      tab being visible), no dialog stole the focus meanwhile.
- [ ] **No auto reload configured** (any other tab): sits there unchanged
      for as long as you like; only `r` fetches, and its tab bar entry
      carries no interval.
- [ ] **The bar says so**: the Jira entry reads `3 Jira (10m)` — already
      before the tab has ever loaded, and unchanged while a load runs.
      `tabs.auto_reload_hint: false` in `tui.yaml` removes it from every tab
      and the bar keeps its layout (no second line appearing).
- [ ] **Bad interval**: write `auto_reload: 10` (no unit) into the view file
      and save it from `:config` → the write is rejected with
      `view-config parse: … has no unit (expected s/m/h/d)` and the editor
      reopens on it. `10m` saves again. Beware: a bad view file that reaches
      disk any other way is skipped silently by instance discovery — the tab
      is simply gone, and the CLI then only says it knows no such instance.
- [x] The CLI ignores it: `nyd adapter jira help --full` prints the level with
      `auto_reload: 10m` set (checked headless, 2026-08-31); a real
      `nyd adapter jira ls -q '<JQL>'` still runs one request and exits.

## Mail (IMAP): the folder tree and its messages, one account per subtab (phases 0-2a)

Prerequisite: copy
[`docs/examples/views/mail-adapter.yaml`](examples/views/mail-adapter.yaml) and
[`docs/examples/views/mail.yaml`](examples/views/mail.yaml) into
`~/.config/not_yet_done/views/`, replace the example accounts with real ones
(ids may stay `work`/`bridged`/`club` — the view's `query:` lines name them),
and add `Mail` to `tabs.order` in `tui.yaml`. Message BODIES do not exist yet:
this block ends at the envelope rows — no preview pane, no attachments.

### Headless (no TUI needed)

- [ ] **The type is registered**: `nyd config auth mail` prints the mechanisms
      `password` (fields `username`, `password`) and `xoauth2` (`username`,
      `token`) with their providers. This works before any account exists —
      it reads the mechanism table, not the config.
- [ ] **The instance is found**: `nyd adapter` lists `mail (type: mail)`. If
      it does not, the view file was rejected — instance discovery skips a bad
      view file silently, so start the TUI, which reports the parse error, and
      the CLI otherwise only says it knows no such instance.
- [ ] **The levels document themselves**: `nyd adapter mail help --full` names
      the root and its children (`Account` / `Folder`), and the folder level
      lists the columns `name`, `unread_count`, `total`, `path`. No login
      happens. KNOWN GAP: `nyd adapter mail:folder help` still reports a leaf
      level — the message children are built from a folder's id, and the
      type-level probe has none, so the CLI cannot document the level yet.
      The TUI reaches it normally.
- [ ] **Accounts without the network**: `nyd adapter mail ls --type mail:account`
      prints one row per configured account (name, address, host, id) —
      immediately, with no credential prompt. This is config, not IMAP.
- [ ] **The folder tree headlessly** (phase 1's own definition of done):
      `nyd adapter mail ls --type mail:folder -q 'account:work'` asks for the
      password once and prints the top-level folders with their unread/total
      counts, `INBOX` first.
- [ ] **A scope that names nobody**: `-q 'account:nope'` is refused with a
      message naming the ids the instance actually holds — it does not fall
      back to the first account.
- [ ] **A term nobody understands**: `-q 'is:unread account:work'` is refused
      and says what a folder query understands. A silently dropped term would
      look exactly like a filter that matches everything.
- [ ] **No scope, several accounts**: `nyd adapter mail ls --type mail:folder`
      refuses and names the `account:<id>` syntax. With a single-account
      instance the same call simply works.

### In the TUI

- [ ] **One tab, many mailboxes**: the Mail tab opens on `Work`; `b` and `a`
      switch to the other subtabs. Each shows _its own_ folders — the surest
      check is a folder name that exists in only one of the accounts.
- [ ] **Only what you look at connects**: start the TUI, open Mail, stay on
      `Work`. Only the Work account asks for its password; the other accounts
      are never contacted. Switching to `Bridged` connects that one, and now.
- [ ] **The prompt says whose password it wants**: the credential dialog's
      header carries the account label (`Work: …`). With three mailboxes
      behind one instance, an unlabelled "Password:" is unanswerable.
- [ ] **Counts**: `INBOX` shows plausible unread/total numbers — compare
      against Thunderbird on the same account. `Total` is the message count,
      not the size.
- [ ] **A container has no counts**: a `\Noselect` folder (one that only holds
      subfolders, e.g. `Archive` above `Archive/2019`) shows _empty_ count
      cells, not `0`.
- [ ] **Names are decoded, paths are not**: a folder with a non-ASCII name
      reads correctly (`Entwürfe`, not `Entw&APw-rfe`). Switch the `path`
      column on via `c c` → it shows the server's own spelling, which is what
      `exclude_folders:` matches and what a bug report should quote.
- [ ] **Expanding is free**: expand and collapse a folder with subfolders
      several times → instant, no load banner. The hierarchy came with the one
      `LIST` at the top level; only `r` re-lists.
- [ ] **Reload**: create a folder in Thunderbird, press `r` → it appears.
      Without `r` it does not (phase 6 brings `IDLE`).
- [ ] **Excluding**: add `exclude_folders: ["Trash*"]` to an account, reload
      the config, press `r` → `Trash` and everything below it are gone from
      the tree, the rest is unchanged.
- [ ] **A subtree subtab**: the `Club archive` subtab (`query: "account:club
folder:Archive"`) opens _inside_ that folder — its top row is the first
      child of `Archive`, not `INBOX`.
- [ ] **Unread emphasis**: a folder holding unread mail carries the marker and
      the `unread` colour; the tab bar prefixes `Mail` with `✉` while any
      subtab has unread. Reading the mail elsewhere and pressing `r` clears
      both.
- [ ] **A wrong password heals**: answer the prompt wrongly → a clear error,
      no crash, the tab stays usable; `r` asks again and the correct password
      connects (no restart needed).
- [ ] **A server that is not there**: point an account at a host that does not
      answer → the banner says what it is doing and for how long, the request
      is repeated per `retry:`, and the failure names the account. The other
      subtabs keep working.
- [ ] **Transport**: an account with `security: starttls` on 143 and one with
      `security: none` against a local bridge both connect. Neither is guessed
      from the port.

### Messages under a folder (phase 2a)

- [ ] **Enter opens a leaf folder's mail**: put the cursor on a folder with no
      subfolders (`INBOX` on most servers) and press Enter → the message list
      opens in a pane beside the tree, newest mail first. The arrow and Enter
      now agree: a row with no arrow no longer swallows the key.
- [ ] **`m` opens any folder's mail**: put the cursor on a folder that _does_
      have subfolders and press Enter → it expands (unchanged). Press `m` →
      its own messages open. `m` on a leaf folder does the same as Enter.
- [ ] **From a nested folder too**: expand `Archive`, put the cursor on
      `Archive/2019`, press `m` → that folder's mail, not `Archive`'s.
- [ ] **The pane is the tree's own**: with the list open, move the cursor to
      another folder and press `m` again → the same pane is REPLACED, no third
      pane stacks up. Closing the tree pane takes the list with it.
- [ ] **The cursor lands on the first unread**: open a folder with unread mail
      → the cursor sits on the oldest unread message, with the rest of the
      unread run below it. With nothing unread it lands on the newest.
- [ ] **A row is an envelope**: flags, sender, subject, date and (via `c c`)
      size, attachment count, recipient. Compare a handful against
      Thunderbird — especially a mail with an encoded subject (umlauts,
      `=?UTF-8?…?=`) and one whose sender has a display name.
- [ ] **A mail with no subject**: shows `(no subject)`, not an empty row you
      cannot aim at.
- [ ] **The flag glyphs**: `●` unread, `↩` answered, `★` flagged, `✎` draft,
      `📎` has an attachment — in that fixed order, so the column does not
      jitter from row to row. An unread mail is also painted in the `unread`
      colour.
- [ ] **Paging is server-side**: in a mailbox with more mail than
      `page_size`, `>` fetches the next window (brief load, the counter in the
      status line moves) and `<` comes back. The total is the mailbox's, not
      the page's.
- [ ] **A big mailbox opens fast**: Gmail's `All Mail` (tens of thousands of
      messages) opens in about the time `INBOX` does — the page is fetched,
      not the mailbox.
- [ ] **Sorting is honest**: `c s` on `from` sorts the rows on screen and the
      status line says the sort applies to the page. Paging on and back does
      not silently re-sort the whole mailbox.
- [ ] **The query is IMAP SEARCH**: with the message list focused, edit the
      query to `UNSEEN` → only unread mail. `FROM <someone>` and
      `SINCE 1-Sep-2026` work the same way. A syntactically wrong search is
      refused by the server with a readable message and the session survives
      (the next key still works, no reconnect).
- [ ] **Nothing is downloaded**: watch a folder of large mails scroll past —
      no delay per row, no growth in the message store. A body is a separate
      fetch, and only pressing `p` makes it.
- [ ] **A restored cursor does not connect**: leave the TUI with the cursor on
      a message, restart → the Mail tab restores without logging in (the row
      resolves from its id alone).

### Reading a message and its files (phase 2b)

- [ ] **`p` opens the body**: with the cursor on a message press `p` → the
      preview pane shows a header block (From, To, Subject, Date, and
      `Attachments:` when there are any), a blank line, then the text.
- [ ] **Reading does not mark read**: pick an unread mail, read it with `p`,
      move away, press `r` → it is STILL unread, here and in Thunderbird. The
      fetch peeks; marking read is phase 5 and deliberate.
- [ ] **HTML-only mail is readable**: a newsletter with no `text/plain` part
      shows as flowing text, not as tag soup and not as an empty pane.
- [ ] **Umlauts survive**: a mail in `quoted-printable` / ISO-8859-1 reads
      correctly — no `=FC`, no mojibake. Compare one against Thunderbird.
- [ ] **The second read is free**: read a mail, move away, read it again → it
      appears at once, with no load banner. Reconnecting (`r` after a dropped
      session) throws the cache away, so the next read fetches again.
- [ ] **The preview is not markdown**: a mail containing `*asterisks*`, `#`
      at the start of a line or `>` quoting shows those characters as written
      — nothing is reflowed or styled away.
- [ ] **Drilling into the files costs nothing**: a mail with `📎` has an arrow;
      opening it lists the attachments instantly, with no round trip (the
      structure rode along with the envelope). Name, type, size and part
      number are filled in.
- [ ] **A mail without files is a leaf**: no arrow, and Enter does not open an
      empty level.
- [ ] **`o` opens one**: put the cursor on an attachment and press `o` → the
      file opens in the desktop's handler, with its real name and its real
      content (a PDF is a PDF, not base64 text). Pressing `o` a second time
      reuses the copy instead of fetching again.
- [ ] **`D` saves them all**: press `D`, answer with a directory (`~` is
      expanded) → every attachment lands there, prefixed by its part number so
      two files of the same name do not collide. The message says how many.
      Aiming `D` at an existing FILE is refused before anything is fetched.
- [ ] **Only the part travels**: opening one attachment of a large mail is
      quick — the part is fetched, not the whole message.
- [ ] **A renumbered mailbox refuses**: with a message open, have the server
      renumber the mailbox (or use a stale restored cursor) → a clear error,
      never a different mail's body.

- [ ] **Real-data sweep**: no real mail domain, address or password anywhere in
      the repo — the examples use `example.org`/`example.net` and the
      credentials come from `pass`.

### Importing from Thunderbird (phase 3)

- [ ] **It finds the right profile**: run `nyd config import-thunderbird
--stdout` with no flags → the accounts printed are the ones Thunderbird
      shows, not an older profile's. On a machine with several profiles, check
      that the one named in `installs.ini` won.
- [ ] **Every IMAP account arrives**: the count matches Thunderbird's account
      list minus Local Folders. Host, port, security and **login name** agree
      per account — compare against Thunderbird's own server settings, which
      are the values known to work.
- [ ] **A login name that is not the address is carried**: an account logging
      in by account name rather than by e-mail keeps that name, and the run
      says so in a note.
- [ ] **The bridged pair keeps two ids**: two mailboxes on the same host and
      port get different ids and different subtabs.
- [ ] **An OAuth2 account is flagged**: the run says the store needs an app
      password for it rather than importing something that cannot log in.
- [ ] **Nothing secret moved**: `grep -i pass` over both generated files finds
      only `pass_credentials.py` invocations and store PATHS — no password,
      and Thunderbird's `logins.json` was never opened.
- [ ] **An existing config is safe**: run it a second time without `--force` →
      it refuses and names the file; the hand-written config is untouched. With
      `--force` it overwrites.
- [ ] **The output actually loads**: point `XDG_CONFIG_HOME` at a scratch
      directory holding the two generated files and start the TUI → the Mail
      tab comes up with one subtab per account, and `nyd adapter mail help
--full` prints the levels without connecting.
- [ ] **The guessed paths are printed**: every account's `<prefix>/<id>/pass`
      appears in the summary, marked as a guess. `--pass-prefix` changes all of
      them.
- [ ] **A POP3 account is named, not dropped**: if the profile has one, it is
      listed under "not imported" with the reason.

### One account's failure stays on its own subtab

Six accounts in one instance, one subtab each. The status used to travel on a
single instance-wide channel, so a `Failed` from the account you looked at for
a moment appeared on every other subtab — and stayed, because that channel
keeps its last value and a subtab that is already loaded publishes nothing new.
Each subtab now subscribes under its own `account:<id>` query.

- [ ] **The failure is where it happened**: switch to an account whose server
      is unreachable (point one at a closed port) → its subtab says
      "connection refused". Switch to a working account → its list is there
      and **no** banner. Switch back and forth: each subtab keeps saying its
      own thing.
- [ ] **A failure that heals disappears**: bring the server back, press `r` on
      the failing subtab → the banner goes and the folders load. The other
      subtabs never showed it and are unchanged.
- [ ] **The login form names the account**: an account whose store must be
      unlocked shows the form with that account's name in the title, not the
      instance's — the scoped channel keeps the label the merged one added.
- [ ] **Still one form at a time**: two accounts that both need to ask do so
      one after the other, never two dialogs at once.
- [ ] **A view over all accounts still works**: the root level (no
      `account:` in its query) shows the account rows and hears every account,
      as before.

### A command that gets no answer (`command_timeout_secs`)

IMAP runs one command at a time on one connection, so a server that neither
answers nor closes does not hold up one request — it holds up the account. That
is what "loading INBOX 50s" looked like, on a mailbox that had opened in
seconds a minute earlier. Every command now runs under a deadline
(`command_timeout_secs`, 60 s by default, per instance or per account, `0` off).

- [ ] **Normal work is untouched**: with the default, opening folders, reading
      mail and saving attachments behave exactly as in the blocks above — a
      deadline that never goes off is invisible.
- [ ] **The banner names the limit**: while a folder loads, the connect/busy
      line says what is running and how long it may take, and counts up
      towards that limit rather than counting alone.
- [ ] **A stuck server costs one command, not the account**: set
      `command_timeout_secs: 5`, then cut the connection mid-request (block
      the port with a firewall rule, or suspend the server) → within about
      five seconds the error reads "no answer in 5s — giving up on this
      connection", the banner closes, and the very next key works again on a
      fresh session. The other subtabs are unaffected.
- [ ] **A deadline is not waited out twice**: the log shows one attempt for
      that command, not a retry — repeating a command that already ran out of
      time only makes the wait twice as long. A session the **server** ended
      (`* BYE`, a dropped socket) is still retried once, silently.
- [ ] **The limit is per account too**: give one slow account its own
      `command_timeout_secs` → only that account's commands run under it; the
      instance value stands for the rest. `0` on either level means no
      deadline at all.

## Mail: the message under the cursor in the browser (`o p` + `row_change`)

Two scripts on the message level: `html_preview.py` on `o p` opens the preview,
`preview_follow.py` on the `row_change` hook keeps it on the row the cursor
rests on. Both are wrappers around `_lib/nyd_mail_preview.py` — the mail
renderer, **not** the pandoc engine the Jira and Taiga previews share. A
message is no workspace item and its body is HTML, so nothing here goes through
Markdown: the adapter's `export_html` writes the message to disk and the script
builds the page itself.

The point of the test is that there is only ever **one** window, that it never
takes the focus away from the TUI, that a hook firing on every row costs
nothing while nobody is watching, and that a mail cannot phone home.

The page keeps itself current: `preview.html` asks for `stamp.js` a few times a
second and reloads when the stamp names another message. Both come from a small
loopback server the first `o p` starts, which makes every poll proof that
somebody is still looking. That heartbeat is what the follow decision reads —
asking the window manager for a title cannot work, because a title belongs to
the window's active tab and a preview sitting behind another tab then looks
closed.

Both are bound in `nyd.db` (`query_shortcut` / `script_hook`, scope
`script:mail/mail:folder/mail:message`), and the message level carries a
`type: script` action (`x`) — without one there is no script scope at that
level, so neither the shortcut nor the hook exists. The TUI caches its
shortcuts, so a **restart** is part of the preparation.

- [ ] **`o p` opens it**: on a message row a qutebrowser window comes up on the
      current workspace, titled `pandoc-preview/nyd-mail: <subject>`, with the
      subject as the heading and From/To/Cc/Date as a field block above the
      message; attachment names are listed when there are any.
- [ ] **An HTML mail looks like the mail**: a newsletter or a reply from a web
      client keeps its headings, emphasis, lists, tables and link colours — no
      raw `<td>` markup in the text and no wall of unstyled lines. A link opens
      in a new window.
- [ ] **The sender's own images are there**: a mail carrying its logo or a
      screenshot as an inline (`cid:`) part shows it, and the page is a single
      file — those parts are embedded, not linked.
- [ ] **Remote images stay off**: a mail pulling images off a server shows
      placeholders and a bar saying how many were blocked and why. Press **Load
      images** → they appear. Before that click nothing is fetched (the
      browser's network log, or simply working offline, is the proof).
      `NYD_MAIL_REMOTE_IMAGES=1` loads them from the start.
- [ ] **Nothing in the mail runs**: an HTML mail carrying a `<script>`, an
      `onclick=`, a `<style>` block, an `<iframe>` or a form renders as text and
      layout only — no dialog, no request, no page of the sender's design.
- [ ] **A mail written in Word is not an empty box**: a message from Outlook or
      Word (`<meta name=Generator content="Microsoft Word …">`, two `<meta>`
      tags and a conditional comment in the head) shows its text. Those tags
      never close: a renderer that counts one as the start of a region to skip
      swallows the whole message and leaves a header card above a blank frame.
- [ ] **A body that cannot be shown falls back**: when the markup sanitises
      away to nothing, the text part is rendered instead — never an empty box.
      The worker log says so (`the html part sanitised to nothing`), because
      that is a fault of the renderer, not of the mail.
- [ ] **A plain-text mail reads like mail**: single line breaks stay breaks, a
      quoted passage is coloured as a quote and the reply typed under it is
      **not** part of it, a bare URL is a link — and `*asterisks*`, `#hash` and
      `_underscores_` stay exactly as typed. This is no longer Markdown.
- [ ] **The focus stays in the terminal**: the new window appears, the cursor
      keeps moving in the table without a click.
- [ ] **The cursor pulls the page along**: `j` to the next message → after the
      settle delay the page shows that message, in the same window, without a
      flash of white.
- [ ] **Fast is quiet**: hold `j` over a dozen rows → only the row you come to
      rest on is rendered (the preview never walks the whole path).
- [ ] **A second `o p` opens no second window**: press it again on another row
      → the page that is open reloads, the window count stays at one.
- [ ] **A folder row does nothing**: the hook is bound on the message level, so
      moving in the folder tree leaves the preview where it was.
- [ ] **A preview in a background tab still follows**: put the preview into a
      tab of a window whose other tab is in front, or on another workspace
      → the page still updates. Whether somebody is looking is decided by the
      page's own heartbeat, not by a window title: a title belongs to the
      window's **active** tab, so asking Sway for one left every background
      tab looking closed.
- [ ] **Closing the window ends it**: close the preview, move the cursor over
      several messages → within seconds (a heartbeat counts as stale once it
      is older than four poll intervals) nothing is rendered any more and no
      notification appears. `<preview dir>/follow.log` names the decision and
      the age of the heartbeat for every run. Re-open with `o p`.
- [ ] **A page that is still starting is not cut off**: `o p`, then move the
      cursor at once, before the browser has fetched anything → the page
      follows. `NYD_PREVIEW_FOLLOW_GRACE_S` (30 s) covers exactly the gap
      between opening a window and its first heartbeat; wait longer than that
      without a browser and the following stops.
- [ ] **A message read twice costs no login**: go back to a message you have
      already previewed → the page is back at once, from
      `~/.local/share/not_yet_done/mail/<instance>/preview/pages.v<N>/<key>.html`,
      with no CLI call and no touch of the account. A body cannot change under
      its uid, so the cache is not a guess — `NYD_MAIL_PREVIEW_REFRESH=1`
      renders again anyway.
- [ ] **A fixed renderer does not serve old pages**: the cache directory
      carries the renderer's version, so raising `RENDER_VERSION` renders every
      message again and the previous directory is removed on the first render.
      Without that, a page built by a broken renderer would outlive the fix.
- [ ] **The settle delay is the one from the config**:
      `script.row_change_delay_ms: 1000` in `tui.yaml`, restart → the preview
      visibly waits a second after the cursor stops.
- [ ] **The poll interval is a knob**: `NYD_PREVIEW_POLL_MS=250` follows more
      eagerly; `NYD_PREVIEW_POLL_MS=0` takes the poller out of the page, and
      then `o p` opens a window per invocation and the hook stops rendering —
      a page that cannot reload is not one to write into.
- [ ] **It survives a dead browser**: kill qutebrowser, move the cursor → the
      hook returns silently; `o p` opens a fresh window.
- [ ] **The preview is served, and only to this machine**: the page comes from
      a loopback server, `http://127.0.0.1:<port>/<token>/preview.html`, whose
      port and random token are noted in `<preview dir>/server.json`. Without
      the token, with a wrong one, or with a `../` in the path the answer is
      404 — the server hands out the preview directory and nothing above it,
      and it listens on loopback only. It has to be a server: a `file://` page
      cannot poll a stamp without the browser treating every render as a
      cross-origin fetch.
- [ ] **Nothing is served stale**: every answer carries `Cache-Control:
no-store`, so the reload after a render shows the new message and never
      the one the browser kept.
- [ ] **The server goes when nobody watches**: leave the preview closed for
      `NYD_PREVIEW_SERVER_IDLE_MIN` (10 minutes) → the process exits and takes
      `server.json` with it; `NYD_PREVIEW_SERVER_IDLE_MIN=0.1` makes that
      quick to test. A note left behind by a killed server is not believed
      either — the script pings it first and starts a fresh one.
- [ ] **A big image is linked, not embedded**: an inline part over
      `NYD_MAIL_INLINE_MAX_BYTES` (4 MB) is copied to
      `pages.v<N>/media/<key>/` and linked **relatively** — an absolute
      `file://` link is unreachable from a page served over http. Dropping a
      page from the cache takes its media directory with it.
- [ ] **The message on disk is the adapter's**: headless,
      `nyd adapter mail "<message id>" export_html` prints
      `exported message to <dir>`, and that directory holds `message.json`
      (written last, so a reader that waits for it never sees half a message),
      `message.html` whose `cid:` references point into `inline/`, and
      `message.txt`. A plain mail has no `message.html` and says so in the
      JSON.

## A view file that does not load: reading and copying the problems

A broken `views/*.yaml` keeps its tab and draws a configuration-error panel
instead of its content. One duplicate key in a shared YAML anchor is reported
once per subtab that uses it, so the list is routinely longer than the
terminal — these points are about getting all of it out of the app.

Set up by breaking one view file on purpose: give an action on a child level a
bare key that a global chord already claims (`key: o` under a level whose tab
also has `o s` / `o k` bound), then start the TUI.

- [ ] **The tab is still there**: the broken file's tab appears (last, without
      its icon) and opens the error panel with the file path and the count.
- [ ] **The list scrolls**: `↓`/`j`, `PageDown`, `End` walk down it, `↑`/`k`,
      `PageUp`, `Home` back up. The bottom row says how many problems are
      still below, and stops saying it at the end.
- [ ] **The wheel scrolls it too**, by `mouse.wheel_rows` a notch.
- [ ] **A drag selects**: pressing and dragging over the panel highlights the
      text and copies it on release; a double click takes the word, a triple
      the line. (Before this the panel was in no mouse region at all, so a
      drag anchored nowhere and nothing could be selected.)
- [ ] **`y` copies everything**: the clipboard holds the file path, the count
      and every problem — including the ones off the bottom of the screen —
      and the bar confirms how many.
- [ ] **The hint row names the real key**: it says whatever
      `global.show_notifications` is bound to (`[f10]` by default, `[z l]` on
      a config that moved it), never a hard-coded `f10`.
- [ ] **The bar summarises, the log carries the detail**: the bottom bar shows
      one line per broken file (`<tab>: N configuration problem(s) — [key]
lists them`), not N lines. With `notifications.max_messages: 1` and two
      broken files, only the last summary is on the bar — and both files'
      problems are still in the log.
- [ ] **The notification centre has them**: `f10` (or your binding) lists every
      problem, `e` narrows to errors, `y` copies the one under the cursor, `Y`
      the whole log, `o` hands it to `$EDITOR`.
- [ ] **The panel is not a mode**: on the broken tab the digits still switch
      tabs, `:` still opens the command line, and the `g…` link chords still
      resolve.
- [ ] **A reload re-reports**: `:config` → save a still-broken file → the
      summary and the log entries appear again.

## Refinements / deferred tasks

Points that came up during smoke tests but do not belong to the refactor in
question. They are addressed in sessions of their own.

- The validator (keymap.rs) does not know about the auto-numbering digits yet;
  in constellation mode, fixed `tab_*` bindings could show up as a phantom
  collision, or a view digit binding is not tracked as globally claimed. Low
  priority (digits as view action keys are rare).
- Persistence of the tab set switch: currently session-only (not written back
  into `tui.yaml`). Optional write-back if wanted.

## Sources

- Content actions plan: [`plan-content-actions-unification.md`](plan-content-actions-unification.md)
- EditSession refactor plan: [`plan-edit-session-refactor.md`](plan-edit-session-refactor.md)
