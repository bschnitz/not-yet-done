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
are rendered read-only; conflicts end up in the banner reopen.

- [ ] `Shift+e` opens a buffer with the header and all comments in
      newest→oldest order
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
- [ ] **Cut**: put a node on the move clipboard with `C` (mark-move) →
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
- [ ] **Manual connect (`adapter.manual_connect: true`, the default)**:
      in `views/postgres.yaml`, either omit `manual_connect` in the
      `adapter:` block **or** set it to `true`. Start the TUI. Expected:
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
      `manual_connect: true` but **without** a `type: reload` in
      `actions:`. The banner reads "Auto-connect disabled — no `reload`
      action configured for this view"; the tab stays empty permanently
      (a soft error, no crash).
- [ ] **Manual connect off**: `manual_connect: false` → the tab loads
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
      placeholder `… N weitere` appears as the last row (glyph `…`).
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
The per-node shortcuts are set in `postgres.yaml`:

```yaml
- name: Scripts
  node_type: "postgres:db_scripts"
  shortcuts:
    a: add
  children:
    - name: DB Script
      node_type: "postgres:db_script"
      shortcuts:
        X: execute
        e: edit
        d: delete
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

### Shortcut resolver (CP-1)

- [ ] Press `Q` in a rows pane (`postgres:row`) → it opens the Q SQL
      editor of the parent table node (via
      `shortcuts: { Q: "parent:edit_sql" }`).
- [ ] Keys that are **not** a shortcut and **not** a view action pass
      through as before (cursor movement, and so on).
- [ ] YAML with an empty action ID (`shortcuts: { x: "" }`) or with a key
      collision against the `actions:` list → a validator error on
      reload, the tab goes into the broken state.

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
- [ ] If the items view has never been loaded (`manual_connect`):
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

## Shortcut Hints (SH-1 … SH-7)

Background: YAML `shortcuts:` (e.g. `a: add`, `x: execute`,
`Q: parent:edit_sql`) are now rendered as action-bar or status-bar hints for
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

**Setup**: an active Postgres adapter with `manual_connect: false` (auto
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
  `manual_connect: true` nothing loads automatically, `r` triggers the first
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

## Stoat adapter — channel cut and paste (`C` / `P`)

Moving channels between categories. `C` (cut) **marks** the channel under the
cursor — **nothing is deleted**; only `P` (paste) reattaches it. It reuses the
generic `mark-move`/`paste-move` shortcuts (like `m`/`p` in Tasks), via
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
- [ ] **Andere Tabs unberührt:** Spalten ohne `markdown:` (Jira/Taiga/Postgres)
      rendern weiter einzeilig.

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

## Postgres — `manual_connect` (no auto-connect, only `r`)

`adapter.manual_connect: true` in `postgres.yaml` — that is the default, so
the test holds without the line as well.

- [ ] App start: the Postgres tab does **not** load automatically; banner
      "Press `r` to connect".
- [ ] Switching subtabs (databases/tables/scripts) does not trigger an auto
      load either.
- [ ] `r` establishes the connection and the SSH tunnel and loads.

## `manual_connect` — default and startup login

`adapter.manual_connect` defaults to `true`. The test covers the default itself
and what an explicit `false` triggers at startup.

- [ ] **The default applies**: **remove** the `manual_connect` line entirely
      from a view file that has an adapter. Start the TUI → the tab does not
      load, banner "Auto-connect disabled — press `r` to connect".
- [ ] **Local tabs opt back in**: `views/tasks.yaml`, `trackings.yaml`,
      `projects.yaml` and `sqlite.yaml` carry `manual_connect: false`; those
      tabs are populated immediately at startup, as before.
- [ ] **An eager tab's login arrives immediately**: set
      `manual_connect: false` in a view that requires a login (e.g.
      `kimai.yaml`), lock the password store (`gpgconf --kill gpg-agent`) and
      start the TUI. Expected: the credential popup is there **immediately**,
      even though the Tasks tab is active; its title starts with the tab name
      (`Kimai: …`). Enter → the popup closes, Kimai loads in the background;
      Tasks stays the active tab.
- [ ] **Escape**: press `Esc` instead of Enter → the popup goes away, no second
      popup pops up behind it, the TUI is usable normally. The Kimai tab shows
      the error/connect banner when opened; `r` restarts the login.
- [ ] **Two eager logins**: set `manual_connect: false` on two views that
      require a login. Expected: **one** popup at a time, the second does not
      overwrite it. After the first is answered or cancelled, the second
      arrives at the latest when its tab is opened.
- [ ] **A manual-connect tab stays quiet**: a tab with `manual_connect: true`
      (or without the line) shows **no** popup at app start — only `r` on that
      tab asks.

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
- [ ] `m` (mark-move) on task A, then `p` (paste-move) on task B → A becomes
      a subtask of B. The "marked …" indicator is visible in between.
- [ ] `m` on A, `p` on A itself or on a descendant of A → error (the cycle is
      rejected), nothing changes.
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

**Action and shortcut inheritance** (the recursive `subtasks` branch declares
no `actions:`/`shortcuts:` of its own):

- [ ] Drill into a task (the recursive level) → `e`/`a`/`A`/`x`/`ctrl+n`/`r`
      and the shortcuts `d`/`u`/`s`/`m`/`p`/`U` work there exactly as they do
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
      with the YAML `shortcuts:` entry. `esc` aborts.
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
      stays free for YAML `shortcuts:`.

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

## Eager-Subtree (`supports_eager_subtree`, `list_subtree`)

In-Memory-Adapter (Tasks, Trackings) liefern bei `expand_depth: all`
bzw. `expand_depth: N` den ganzen erwarteten Teilbaum in **einem**
`list_subtree`-Call statt der per-Knoten-Kaskade. Resultat muss optisch
**identisch** zur Kaskade sein — gleiche Zeilen, gleiche Reihenfolge, gleiche
Aufklapp-Tiefe — nur ohne das ebenenweise Nachladen.

- [ ] Tasks Tree-View (`expand_depth: all`): nach dem Laden ist der
      komplette Forest sofort offen — kein sichtbares Ebene-für-Ebene-
      Nachklappen. Tiefe ≥ 3 Ebenen testen (Task → Subtask → Sub-Subtask).
- [ ] Selektion/Collapse: Cursor auf einen tiefen Knoten, `zc`/Collapse
      und wieder aufklappen → Zustand stimmt (Pfad-Schema == Kaskade).
- [ ] Trackings Tree-View: dito, voll aufgeklappt in einem Rutsch.
- [ ] `:tree-find "Tasks" id:<uuid>` (z. B. via `goto_task`) landet
      weiterhin auf dem richtigen Knoten — der eager geladene Baum ist
      vollständig durchsuchbar.
- [ ] `r`-Reload auf dem eager Tree erneuert alle Ebenen (z. B. neu
      gestartetes Tracking zeigt `⏱` auf verschachteltem Task).
- [ ] Gegenprobe Remote (Postgres/Confluence-Tree, `supports_eager_subtree:
false`): klappt weiterhin **progressiv** auf (Ebene für Ebene), UI
      friert nicht ein — der eager Pfad greift dort bewusst nicht.

## Fuzzy-Filter — Teilstring-Highlight (Tasks/Trackings)

Parität zum nativen Tasks-Tab: der gematchte Teilstring wird hervorgehoben
(Theme-`accent`, fett). Im Tree-Mode im **Label** der `tree_label`-Spalte (der
Box-Connector behält seine `tree_connector`-Farbe), im Flat-Mode in den
durchsuchten Spalten.

- [ ] Tasks Tree-View: `f` + Teil-Text eines Task-Titels tippen → in den
      verbleibenden Zeilen ist genau der getroffene Teilstring farbig/fett
      hervorgehoben; die Box-Connectors (`├──`/`└──`/`▶`) behalten ihre
      eigene Farbe.
- [ ] Mehrere Tokens (`foo bar`) → beide Treffer-Runs im selben Label sind
      hervorgehoben.
- [ ] Eine Zeile, die nur über ein anderes Feld (z. B. Tag) matcht, zeigt im
      Label **keine** Markierung (kein falsches Highlight).
- [ ] Filter leeren (`esc`) → Highlight verschwindet, Labels normal.
- [ ] Flat-View (`v`): `f` + Text → Treffer in den durchsuchten Spalten
      hervorgehoben; nicht durchsuchte Spalten bleiben unmarkiert.
- [ ] Sehr schmale Spalte / langes Label: Highlight bleibt korrekt geclamped
      (kein Panic, kein Übermalen des Connectors).

## Jump-Mode (`J`) auf Content-Tabs (`content.jump_mode`)

Parität zum nativen Tasks-Tab-Sprung (dort `p`), hier auf `Shift+J`.
Default-Binding ist `J`; konfigurierbar über `keybindings.content.jump_mode`.
Das Label-Alphabet kommt aus `navigation.jump_chars`.

- [ ] Tasks: `Shift+J` drücken → Sprung-Overlay aktiv (Action-Bar zeigt
      `J jump`). Ein Zeichen tippen, das in mehreren sichtbaren Zeilen
      vorkommt → jede Treffer-Zeile bekommt ein Label, Nicht-Treffer sind
      gedimmt.
- [ ] Label tippen → Cursor springt in die zugehörige Zeile.
- [ ] Zeichen, das nur in **einer** sichtbaren Zeile vorkommt → sofortiger
      Sprung ohne Label-Phase.
- [ ] Zeichen ohne Treffer → Overlay schließt sich, keine Auswahländerung.
- [ ] `esc` während des Overlays → Abbruch, Cursor unverändert.
- [ ] In einem Split: `Shift+J` wirkt nur auf das **fokussierte** Pane; nach
      Pane-Wechsel funktioniert der Sprung auch dort (neu erzeugtes Pane).
- [ ] Trackings (Liste/Condensed/Tree): `Shift+J` verhält sich gleich.
- [ ] Nativer Tasks-Tab unverändert: `p` öffnet dort weiterhin den Sprung.

## Stoat: Ungelesen-Hervorhebung (`unread_style` / `unread_marker`)

Channels/Kategorien mit ungelesenen Nachrichten + ungelesene
Nachrichten-Header werden hervorgehoben (Marker-Glyph + Theme-Farbe
`unread`, beide per View überschreibbar). Quelle ist der Revolt-Read-State
(`sync/unreads` + Acks); Live-Reload bei jeder eintreffenden Nachricht.

Voraussetzung: Stoat-Tab geöffnet, Gateway `Ready`, in einem Server mit
mindestens einem Channel, der ungelesene Nachrichten enthält.

- [ ] Channel mit ungelesenen Nachrichten zeigt im Tree den Marker (Default
      💬) **vor** dem Channel-Namen, Name in der `unread`-Farbe (Default
      `#89b4fa`, fett).
- [ ] Die Kategorie, die einen solchen Channel enthält, ist ebenfalls
      markiert (OR über ihre Channels).
- [ ] In die Nachrichtenliste drillen: ungelesene Nachrichten haben einen
      hervorgehobenen Header (Autor/Zeit-Zeile) in derselben Farbe; gelesene
      Nachrichten normal.
- [ ] Marker-Breite stimmt: das Emoji (2 Zellen) verschiebt Einrückung/
      Folgespalten nicht, kein abgeschnittener Connector.
- [ ] Fuzzy-Filter (`/`) auf einem ungelesenen Channel: die Treffer-Runs
      bleiben in der Fuzzy-Match-Farbe (gewinnt über die Unread-Farbe), der
      Rest des Labels in der Unread-Farbe.
- [ ] In einem ungelesenen Channel eine Nachricht **senden** → Channel- und
      Kategorie-Marker verschwinden (Ack-on-send), ohne manuelles `r`.
- [ ] Eine neue Nachricht trifft in einem anderen Channel ein → dieser
      Channel + seine Kategorie werden live markiert (kein manuelles `r`).
- [ ] `unread_marker: ""` in der View gesetzt → kein Glyph, aber Name/Header
      weiterhin in der Unread-Farbe.
- [ ] `unread_style:` auf einen anderen Theme-Farbnamen gesetzt → Marker +
      Name/Header in dieser Farbe.

### Ack bei Cursor auf der neuesten Nachricht (`mark_read_on_reach_end`)

Der Channel-Marker verschwindet auch, wenn man den Cursor auf die unterste
(neueste) Nachricht der Liste bewegt — ohne zu senden und ohne manuelles `r`.
Konfiguriert über `mark_read_on_reach_end: mark-read` auf der Nachrichten-Ebene
(beide Branches in `stoat.yaml`: Channels im und außerhalb einer Kategorie).
Fehlt der Hook in der View-Config, passiert **nichts** (kein Ack, Marker bleibt
ewig stehen) — häufigste Ursache, wenn eine ältere `stoat.yaml` installiert ist.

Der Tree-Marker (Channel + Kategorie) wird **lokal** gecleart: `mark_read` ist
der einzige Choke-Point für „gelesen" und schickt selbst `Invalidation::All`,
sobald die Lese-Marke vorrückt. Damit repaintet der Tree sofort, unabhängig
davon, ob der Server das eigene Ack als `ChannelAck` zurückspielt.

- [ ] In einen ungelesenen Channel drillen, der **mehrere** ungelesene
      Nachrichten hat → die ungelesenen Header sind hervorgehoben, der Cursor
      steht oben/auf einer der oberen Zeilen; Channel-/Kategorie-Marker bleiben
      noch sichtbar (kein Auto-Ack beim Öffnen).
- [ ] Cursor mit `j`/Pfeil-runter bis auf die **unterste** (neueste) Zeile
      bewegen → Channel- und Kategorie-Marker im Tree verschwinden (Ack), die
      Header-Hervorhebung der Liste klingt nach dem Reload ab.
- [ ] Erneut am Listenende eine Taste drücken / wieder hoch- und runterfahren →
      kein erneutes Ack-Flackern (idempotent: Zeile ist nun gelesen).
- [ ] In einem bereits gelesenen Channel drillen und ans Ende fahren → keine
      Änderung (nichts zu acken).

## Shortcut-Menü — Keybinding-Editor (Ctrl+Y)

Nutzer-Doku: [`keybinding-editor.md`](keybinding-editor.md). Nach jeder
Edit-Aktion greift Sofort-Reload; Kommentar-/Format-Erhalt in der
betroffenen YAML **immer** mitprüfen (`git diff`).

- [ ] `Ctrl+Y` öffnet das Menü; `Tab` wechselt Context ↔ alle Tabs;
      Tippen filtert; keyless Actions werden mitgelistet.
- [ ] `Ctrl+N` auf einer View-Action → aufnehmen, Einzeltaste, `Return` →
      "Bound …"; Taste feuert sofort; `views/*.yaml` bekommt `key:` dazu,
      Kommentare bleiben.
- [ ] `Ctrl+N` Chord (`Ctrl+K` dann `L`) → gespeichert als
      `key: "ctrl+k l"`; `Backspace` verwirft letzten Schritt; `Esc`
      bricht ab (keine Änderung).
- [ ] `Ctrl+N` auf einer read-only-Zeile (Saved-Query/Script) → Hinweis,
      keine Aufnahme.
- [ ] Zweites Binding auf dieselbe Action → Liste `key: [alt1, alt2]`
      (Alternative angehängt, alte Taste bleibt).
- [ ] `Ctrl+D` bei einer einzigen Bindung → weg (`[]` bei Built-in in
      `tui.yaml`, Default feuert nicht mehr).
- [ ] `Ctrl+D` bei mehreren Bindungen → Picker, Auswahl, `Return` →
      Restliste geschrieben; `Esc` bricht ab.
- [ ] `Ctrl+R` auf Built-in → `tui.yaml`-Override verschwindet, Default
      gilt wieder; auf View-Action → no-op.
- [ ] Konflikt: Binding aufnehmen, das eine überlappende Bindung belegt
      (inkl. globalem Built-in / Präfix `k` vs `k l`) → `y/n`-Prompt.
      `n`/`Esc` → nichts passiert. `y` → andere Bindung verliert die
      Taste, neue greift; beide Dateien reloaded.
- [ ] Konflikt gegen read-only-Shortcut → als unauflösbar gemeldet,
      Binding abgelehnt.
- [ ] Verschiedene Tabs kollidieren nie (gleiche Taste in Tab A und B →
      kein Prompt).

## Benachrichtigungsleiste — Anzeigelimit + Log im Editor (`f10`)

`notifications.max_messages` begrenzt, wie viele Meldungen die **untere**
Leiste gleichzeitig zeigt (`0` = unbegrenzt); `notifications.history_limit`
begrenzt das mitlaufende Log beider Leisten.

- [ ] `max_messages: 1` in `tui.yaml`: zwei Meldungen nacheinander auslösen
      → unten steht nur die **neuere**; die ältere ist verdrängt.
- [ ] Obere Alert-Leiste bleibt unberührt: mehrere `prominent`-Meldungen
      stehen weiterhin alle gleichzeitig oben.
- [ ] `f10` öffnet das Log read-only im Editor: alle Meldungen beider
      Leisten chronologisch, mit Zeitstempel, Alert-Zeilen mit `!` markiert;
      Speichern/Schließen ändert nichts.
- [ ] `Z` (dismiss) leert die Leisten, das Log bleibt: `f10` zeigt die
      verworfenen Meldungen weiterhin.
- [ ] Ohne jede Meldung: `f10` → "No notifications yet", kein Editor.
- [ ] Hint rechts unten nennt die echten Tasten (`[Z] dismiss  [f10] open`)
      und folgt einem Rebind der beiden Actions.
- [ ] `:config`-Reload (tui.yaml speichern) verliert weder offene Meldungen
      noch das Log.

## Builtin-Editor (`builtin: true`, Crate `vimrealm`)

Ein Editor-Profil mit `builtin: true` editiert in einem Pane der TUI statt
einen `$EDITOR`-Prozess zu starten. Profil in `tui.yaml`, z.B.:

```yaml
editors:
  compose-builtin:
    builtin: true
    height: "30%"
    line_numbers: false
```

Verdrahtet über das `editor:`-Feld einer Action (Stoat: `views/stoat.yaml`,
Actions `new`/`edit`). Zurück auf den echten nvim = `editor: compose-below`.

- [ ] Stoat-Kanal → `n`: Pane erscheint **unten** über den Meldungsleisten,
      Titel nennt die Action; Tab-/Action-/Statusleiste bleiben stehen.
- [ ] Normal-Mode funktioniert: `i a o`, `hjkl`, `w b e`, `0 ^ $`, `dd`, `cw`,
      `x`, `p`, `u` / `Ctrl+R`, Counts (`3w`, `2dd`).
- [ ] `q` allein beendet die **App nicht** mehr, solange das Pane offen ist;
      `:q!` schließt das Pane, danach quittet `q` wieder normal.
- [ ] `:w` sendet die Nachricht (`commit_on_save: true`), Pane bleibt offen,
      Statuszeile zeigt `written`; ein zweites `:w` ohne Änderung tut nichts.
- [ ] `:wq` sendet und schließt; die neue Nachricht erscheint in der Liste.
- [ ] `:q` bei ungespeicherter Änderung verweigert mit `E37`, `:q!` verwirft.
- [ ] Kein zweiter Editor: bei offenem Pane erneut `n` → "Editor is already
      open".
- [ ] Umschalten auf `editor: compose-below` in `stoat.yaml` + `:config`-Reload
      → wieder echter nvim im Kitty-Split, unverändert wie vorher.
- [ ] Lange Nachricht: Softwrap im Pane, `j`/`k` bewegen sich logisch
      (nicht pro Bildschirmzeile), Scrollen folgt dem Cursor.
- [ ] Kleines Terminal: Pane nimmt höchstens zwei Drittel der Höhe, die
      Zeilenliste bleibt sichtbar.

### Phase 3: Register, Text-Objekte, Visual, Suche, Dot-Repeat, Theming

- [ ] Register: `"ayy` in Zeile 1, Cursor in Zeile 2, `"ap` fügt Zeile 1 ein;
      `"Ayy` hängt an, `"add` löscht ohne den unnamed Register zu überschreiben
      (danach `p` fügt weiterhin das vorher Gejankte ein).
- [ ] Text-Objekte: `ciw` mitten im Wort ersetzt das ganze Wort, `daw` nimmt
      auch das Leerzeichen mit, `di"` leert einen String, `da(` löscht Klammern
      samt Inhalt — auch mehrzeilig und bei geschachtelten Klammern.
- [ ] Visual charwise: `v` + `w`/`l`/`j` markiert sichtbar (Cursor bleibt im
      markierten Bereich erkennbar), `d` löscht die Markierung, `y` + `p` fügt
      sie ein, `c` löscht und landet in Insert.
- [ ] Visual linewise: `V` markiert ganze Zeilen (inkl. der Zelle am
      Zeilenende), `j` nimmt Zeilen dazu, `d` löscht sie ganz.
- [ ] Visual: `o` springt an das andere Ende und erweitert von dort; `v` beendet,
      `V` schaltet auf linewise um, `Esc` verwirft; `viw` markiert das Wort.
- [ ] Suche: `/wort` + Enter springt zum nächsten Treffer, `n`/`N` weiter und
      zurück, Wrap meldet „search hit BOTTOM, continuing at TOP"; `/nichtsda`
      meldet `E486`, ohne den Cursor zu bewegen; `n` ohne vorherige Suche → `E35`.
- [ ] Dot-Repeat: `ciwfoo<Esc>` dann `w` `.` ersetzt das nächste Wort ebenso;
      `dw` `.` `.` löscht drei Wörter; `2.` wiederholt zweimal; `u` und `.`
      selbst werden nicht als „letzte Änderung" aufgenommen.
- [ ] Cursor: im Normal-Mode Blockcursor über dem Zeichen; nach `i` ist es der
      **echte** Terminal-Cursor (Form/Blinken wie im Query-Menü), kein Block
      mehr; `Esc` zurück zum Block. Bei `A` steht er hinter dem letzten Zeichen.
- [ ] Insert-Grenze: `i` in „ab", zweimal Pfeil rechts → Cursor steht hinter
      `b`; ein getipptes Zeichen hängt an (`abc`), überschreibt nichts. Ebenso
      mit `End`. Im Normal-Mode bleiben `l`/`Pfeil rechts`/`$`/`End` auf `b`.
- [ ] Cursor im Command-Mode: nach `:` sitzt der Terminal-Cursor **in** der
      Kommandozeile hinter dem Getippten (wandert beim Tippen mit), nicht im
      Text; `Esc` bringt ihn zurück in den Puffer.
- [ ] Theming: leerer/fehlender `vim:`-Block in `tui-theme.yaml` sieht aus wie
      bisher. Dann z.B. `vim: {selection_bg: "#504945", gutter: "#928374"}`
      setzen + `:config`-Reload und neu öffnen → nur diese beiden Rollen ändern
      sich. `cursor_bg` setzen → Cursor hat feste Farbe statt Reverse (und ist
      innerhalb einer Markierung immer noch zu sehen).

## Card-Modus (`card:` pro Ebene, Umschalttaste + Persistenz)

Setup: den `card:`-Block aus
[`examples/views/cards.yaml`](examples/views/cards.yaml) in eine echte
Content-Ebene mit sechs Spalten übernehmen (`columns: 3`, `weights: [1, 1, 2]`,
`key: C`, `gap: 1`) — Doku: [`generic-view-spec.md`](generic-view-spec.md),
Abschnitt `card:`.

- [ ] Ohne `card:`-Block bleibt `C` auf der Ebene eine gewöhnliche Taste (nichts
      passiert / bestehende Bindung greift weiter), und die Status-Leiste zeigt
      keinen Card-Hinweis.
- [ ] Mit `card:`-Block: Ebene startet als **Tabelle** (Deklaration allein
      schaltet nicht um), Status-Leiste zeigt `C cards`.
- [ ] `C` → jede Zeile wird eine gerahmte Card mit **zwei** Zeilen à drei
      Feldern (Zeilenzahl abgeleitet, kein `rows:` in der Config); Hinweis
      wechselt auf `C table`.
- [ ] Rechte Kante: **alle** Card-Zeilen enden exakt auf derselben Spalte, auch
      der dritte (breitere) Slot; Terminal schmaler/breiter ziehen → bleibt
      bündig.
- [ ] Labels: `labels: inline` zeigt `Label: Wert`; auf `none` umstellen →
      nur Werte; auf `above` → Labels auf eigener Zeile, Card doppelt so hoch.
- [ ] `gap: 1` → Leerzeile zwischen zwei Cards, und diese Leerzeile bekommt beim
      Auswählen **keinen** Highlight (die Cards lesen sich als Blöcke).
- [ ] Rahmen/Labels in Theme-Farben: `card_border` / `card_label` in `tui.yaml`
      ändern + `:config`-Reload → Rahmen bzw. Labels ändern sich, Werte nicht.
      Danach `border_style:` / `label_style:` in der View setzen → gewinnt über
      die Theme-Slots. `border: plain` → eckige Ecken, `border: none` → kein
      Rahmen.
- [ ] Cursor/Selektion: `j`/`k` springen von Card zu Card (nicht Zeile für
      Zeile), die ganze Card bekommt den Auswahl-Hintergrund. `/`-Suche
      markiert die Treffer weiterhin **im Wert** der Card.
- [ ] Spalten-Popup (`c`): ein Card-Feld ausblenden → das Feld verschwindet aus
      der Card, das Raster rückt nach (keine Lücke, keine Verschiebung der
      rechten Kante). Wieder einblenden → Feld ist zurück.
- [ ] `card_mode` überlebt den Neustart: `C` an, TUI beenden, neu starten →
      Ebene öffnet direkt als Cards. `C` aus (= zurück auf den Config-Default),
      neu starten → Tabelle. (Gespeichert wird pro Ebene unter
      `card_mode:<tab>`; der Eintrag wird beim Zurückschalten auf den Default
      wieder gelöscht.)
- [ ] `default: true` in der Config → Ebene startet als Cards; `C` schaltet auf
      Tabelle und **das** überlebt ebenfalls den Neustart.
- [ ] Split-Pane: mit `wv` eine zweite Pane derselben Ebene öffnen → beide
      zeigen denselben Modus, `C` in einer schaltet beide um.
- [ ] Gruppierung: auf einer Ebene mit `group_by:` → im Card-Modus keine
      Gruppen-Kopfzeilen/Summen; zurück auf Tabelle → Gruppierung ist wieder da.
- [ ] Grenzen (erwartetes Nein): Tree-Ebene und `record_detail:`-Follower bieten
      `C` nicht an. Ein `markdown: true`-Feld in `card.fields` → harter
      Config-Fehler beim Start mit klarer Meldung (kein stiller Fallback).
- [ ] Chord-Taste (`key: 'v c'`, so live im Jira-Tab konfiguriert): `v` allein
      tut nichts und wartet, `v c` schaltet um. `v` + Esc bricht ab, ohne dass
      `c` danach das Spalten-Popup öffnet. Ein `c` **ohne** vorheriges `v`
      öffnet weiterhin normal das Spalten-Popup.
- [ ] Kollisionsprüfung: `card.key` versehentlich auf eine belegte Taste der
      Ebene setzen (im Jira-Tab z. B. `t`) → Fehler beim Start, der
      `views.tickets.card.key` und die kollidierende Action nennt. Danach
      zurücksetzen.
- [ ] Keybinding-Editor (Ctrl+Y) listet die Card-Taste als „Toggle card mode"
      und schreibt eine Änderung nach `views[*].card.key` in die View-YAML.
- [ ] `fields:` weglassen → die Card zeigt **alle** Spalten der Ebene in der
      Reihenfolge der Tabelle (auch die, die man in einer expliziten Liste
      leicht vergisst); Zeilenzahl entsprechend `Spalten ÷ columns`. Eine
      Spalte im Popup (`c`) ausblenden → sie fällt auch hier raus. Eine
      `markdown: true`-Spalte wird still übersprungen (kein Config-Fehler —
      der gilt nur für eine **explizit** aufgeführte Markdown-Spalte).

## Creator-Spalte (Jira + Taiga)

Voraussetzung: `creator` steht als Spalte in der jeweiligen View-YAML
(`docs/examples/views/jira.yaml` / `taiga.yaml` zeigen die Blöcke; in einer
bestehenden privaten Config muss sie nachgetragen werden — sonst liefert der
Adapter das Feld, die Tabelle zeigt es nur nicht).

- [ ] Jira-Tickets: Spalte „Creator" ist gefüllt und weicht bei Tickets, die
      jemand anders gestellt hat, sichtbar von „Assignee" ab.
- [ ] Bookmarks-Subtab (`m`) zeigt die Spalte ebenfalls.
- [ ] Taiga-Liste: dasselbe für alle vier Item-Typen. Ein Item, dessen Creator
      **nicht** (mehr) Projektmitglied ist, zeigt den Namen aus dem Payload
      bzw. `user-<id>` — nicht leer.
- [ ] Sortieren mit `S` → `creator` steht in der Spaltenliste (Jira sortiert
      per JQL server-seitig, Taiga client-seitig). Aufsteigend landen Zeilen
      ohne Creator am **Ende**, nicht oben.
- [ ] Ticket mit `e` öffnen: unterhalb des `---`-Markers steht eine
      `creator:`-Zeile im Read-only-Block. Datei ohne Änderung speichern und
      schließen → kein Konflikt-Banner (der Round-Trip-Guard ignoriert
      unbekannte Read-only-Keys).
- [ ] Nach dem Speichern eines Tickets bleibt die Creator-Spalte der Zeile
      gefüllt (Post-Edit-Row-Patch mit demselben Key).
- [ ] Anon-Modus: Creator erscheint pseudonymisiert, nicht im Klartext.

## Fix-Versions-Spalte (Jira)

Spaltenschlüssel ist `fix_versions` (Plural wie Jiras Feld); JQL kennt nur den
Singular `fixVersion`, den der Adapter beim Sortieren einsetzt. Auch hier muss
die Spalte in der privaten View-YAML stehen — in `jira.yaml` in **beiden**
Spaltenlisten (tickets + bookmarks) und, falls genutzt, in `card.fields`.

- [ ] Ticketliste: Spalte „Fix Versions" ist bei eingeplanten Tickets gefüllt
      und bei den übrigen leer (kein `-`, kein `0`). Ein Ticket mit mehreren
      Versionen zeigt sie komma-getrennt in einer Zelle.
- [ ] Bookmarks-Subtab (`m`) zeigt die Spalte ebenfalls.
- [ ] Sortieren mit `S` → `fix_versions` steht in der Liste; aufsteigend
      kommen die eingeplanten Tickets zuerst (Jiras Versions-Reihenfolge, nicht
      alphabetisch), absteigend zuletzt. Kein JQL-Fehler-Banner — das wäre das
      Symptom, wenn der Plural statt `fixVersion` gesendet würde.
- [ ] Ticket mit `e` öffnen: `fix_versions:`-Zeile im Read-only-Block unter dem
      `---`-Marker. Unverändert speichern → kein Konflikt-/Änderungs-Banner.
- [ ] Nach dem Speichern bleibt die Spalte der Zeile gefüllt
      (Post-Edit-Row-Patch).
- [ ] Card-Modus (`v c`): das Feld erscheint als „Fix Versions" mit Label.
- [ ] Anon-Modus: Versionsnamen erscheinen ersetzt (sie können Produkt-/
      Kundenbegriffe enthalten), nicht verbatim.

## Kommentar-Vorschau: Kürzung auf Zeichen, nicht Bytes

Regression: die Vorschau in der Kommentar-Liste kürzte auf 80 **Bytes**; lag
die Grenze in einem Mehrbyte-Zeichen, riss der Panic den tokio-Worker mit.

- [ ] Ticket mit einem längeren Kommentar in einer Sprache mit Umlauten
      auswählen und `C` drücken → Liste öffnet, kein Crash, lange Vorschauen
      enden auf `…`.
- [ ] Kommentar, der genau um die Grenze herum ein Mehrbyte-Zeichen hat
      (Umlaut, Emoji, CJK) → Vorschau bricht direkt hinter dem Zeichen ab,
      keine kaputten Bytes in der Zelle.
- [ ] Ein Server-Fehler mit nicht-ASCII-Fehlerseite (z. B. abgelaufene
      Session) → Fehlermeldung/HTTP-Log erscheinen normal; das gekürzte
      Body-Snippet crasht nicht beim _Melden_ des Fehlers.

## Freie Suche (Taiga `text_search`) — Treffer + Aktiv-Markierung

Zwei Regressionen: (1) das Query-Template deckte `userstory` nicht ab und
enthielt einen `ref:`-Block, den Taiga ignoriert (Antwort = komplette
ungefilterte Liste); (2) der Hint der Suche erlosch, sobald sie wirkte.

- [ ] Freie Suche öffnen (Taste der `text_search`-Action, im Beispiel-Config
      `s`) und eine reine Ref-Nummer eingeben (z. B.
      `112`) → genau die Items mit dieser Ref erscheinen, quer über alle
      Typen inkl. User Story; **keine** Flut nicht passender Zeilen.
- [ ] Dieselbe Nummer mit führendem `#` (`#112`) → identisches Ergebnis.
- [ ] Ein Wort aus einem Betreff eingeben → Volltextsuche über Task,
      Issue, Epic und User Story.
- [ ] Während des Tippens: der Hint der freien Suche ist markiert, der
      Hint der lokalen `/`-Suche **nicht**.
- [ ] Nach Enter (Ergebnisliste steht): der Hint bleibt markiert, solange
      die Ergebnisliste angezeigt wird.
- [ ] Andere Query anwenden (Query-Menü, Default-Query, Query-Editor) →
      Markierung erlischt.
- [ ] Pane splitten, während die Suche aktiv ist → das neue Pane erbt
      Query **und** Markierung.

## SQLite-Tab — Dateien, Tabellen, Skripte

Voraussetzung: `docs/examples/views/sqlite.yaml` +
`docs/examples/views/sqlite-adapter.yaml` nach
`~/.config/not_yet_done/views/` kopieren und in der Adapter-Config die
`sources:`-Globs auf eigene `.db`-Dateien zeigen lassen. Der Tab kommt ohne
Login und ohne Server — was fehlschlagen kann, ist das Globbing und die
Adressierung.

### Dateien und Zeilen (`sources:`-Globs)

- [ ] Tab öffnen → eine Zeile je getroffener Datei, mit Größe und Pfad.
- [ ] Ein `**`-Pattern nimmt Dateien aus Unterverzeichnissen mit; ein
      Pattern ohne Treffer ist kein Fehler, sondern liefert nichts.
- [ ] Zwei gleichnamige Dateien in verschiedenen Ordnern erscheinen als
      **zwei** Zeilen (Key trägt den Pfad-Hash) und teilen sich weder
      Tabellen noch Skript-Verzeichnis.
- [ ] Neue `.db`-Datei anlegen → `r` → sie ist da, ohne Neustart.
- [ ] Durchdrillen bis `Rows`, blättern (`>`/`<`), `o` öffnet das
      Record-Detail-Pane; eine Tabelle mit BLOB-Spalte rendert lesbar
      statt zu brechen.

### Per-Tabelle-Skripte (`q` / `Q`)

- [ ] `Q` auf einer Tabelle → Editor mit Template `SELECT * FROM "t";`.
      Speichern, ausführen, blättern.
- [ ] `q` nach dem ersten Speichern → das Skript steht im Menü.
- [ ] `Q` eine Ebene tiefer (in `Rows`) adressiert weiterhin die Tabelle
      darüber, nicht die Zeile.
- [ ] Mehr-Statement-Skript → läuft, aber ohne Seiteninfo (nicht
      paginierbare Form).
- [ ] Ein `UPDATE` → scheitert mit dem Hinweis auf `read_only: false`.

### Skript-Ast unter der Datenbank (`Scripts`)

Derselbe Ast wie im Postgres-Tab (geteilter Code in `sql-core`), nur mit
`sqlite:`-Typen — deshalb hier vor allem prüfen, dass er **überhaupt** hängt
und auf die richtige Datei zeigt.

- [ ] `Scripts` steht neben `Tables` unter jeder Datenbank, auch wenn noch
      kein Skript existiert.
- [ ] `a` legt ein Skript an, `A` einen Ordner; Ordner nesten beliebig tief.
- [ ] Filesystem-Check:
      `<data_local>/not_yet_done/sqlite/<instance_id>/db_scripts/<key>/…` —
      `<key>` ist der gehashte Datei-Key, nicht der Pfad.
- [ ] `e` editiert in-place (Tempfile mit `.nyd_tmp_`-Prefix **im**
      Skript-Verzeichnis), `r` umbenennt (Endung bleibt), `M` verschiebt.
- [ ] `d` auf einem nicht-leeren Ordner verweigert mit "not empty".
- [ ] **`x` führt aus und paginiert per LIMIT/OFFSET** — das ist die
      Abweichung von Postgres: das Ergebnis-Pane steht auf
      `pagination: mode: server`. Kein Fehler wie "sqlite has no cursor
      pagination"; `>`/`<` blättern.
- [ ] Ein Skript unter Datei A greift auf eine Tabelle zu, die nur in
      Datei B existiert → Fehler von SQLite (belegt, dass gegen die
      richtige Datei gelaufen wird, nicht gegen die zuletzt geöffnete).

### Table-Completions im SQLite-Skript-Editor

Mechanismus wie im Postgres-Tab (siehe TC-1 … TC-5), aber **einstufige**
Tokens — eine Datei hat keinen Schema-Namensraum.

- [ ] `e` auf einem `.sql`-Skript → letzte Zeile ist
      `-- table completions: tt_<tabelle>, …` mit **allen** Tabellen _und
      Views_ der Datei, ohne `sqlite_`-Interna.
- [ ] Token benutzen: `SELECT * FROM tt_<tabelle>;` → `x` liefert Zeilen
      (expandiert zu `"<tabelle>"`).
- [ ] Speichern, dann `cat` des Skripts unter
      `<data_local>/not_yet_done/sqlite/<instance_id>/db_scripts/<key>/…`:
      **keine** Completion-Zeile auf Platte. Erneutes `e` zeigt sie wieder,
      genau einmal (nicht gestapelt).
- [ ] `e` auf einem `.py`-Skript im selben Ast → keine Completion-Zeile.
- [ ] Zwei Dateien mit gleichnamiger Tabelle: die Zeile unter Datei A listet
      A's Tabellen (Beleg, dass der Key aus der Node-ID kommt).
- [ ] Unbekannter Token (`tt_gibtsnicht`) → SQLite-Fehler nennt den
      literalen Token, kein stiller Ersatz.

## DB-Views editieren (`E` → `edit_view`, SQLite + Postgres)

Views sind ein **eigener Node-Type** (`sqlite:view` / `postgres:view`) in einem
eigenen `Views`-Ast neben `Tables`. Zeilen lesen sie wie eine Tabelle; dazu
kommt `E`: das öffnet die `CREATE VIEW`-Anweisung im Editor, Speichern ersetzt
die View. Voraussetzung: die `Views`-Äste aus
`docs/examples/views/{sqlite,postgres}.yaml` in der eigenen Config, und für
SQLite `read_only: false` in `sqlite-adapter.yaml`.

Die Bindung ist bewusst eine `actions:`-Zeile mit `type: edit, id: edit_view`
und **keine** `shortcuts:`-Zeile — deshalb ist der erste Punkt kein Detail,
sondern der Beleg, dass die Verdrahtung stimmt.

### Gemeinsam (beide Adapter)

- [ ] `Views` steht neben `Tables` (SQLite unter der Datenbank, Postgres unter
      dem Schema), auch wenn die DB keine View hat.
- [ ] Durchdrillen bis `Rows` → Zeilen der View, `>`/`<` blättert, `o` öffnet
      das Record-Detail-Pane. `Q`/`q` adressieren die View wie eine Tabelle.
- [ ] `E` auf einer View → Editor mit der kompletten `CREATE VIEW`-Anweisung,
      Kopfkommentar erklärt, was Speichern tut. Puffer-Endung `.sql`
      (Syntax-Highlighting im Editor-Profil).
- [ ] Ohne Änderung speichern → Meldung "no changes", die View wird **nicht**
      angefasst. Nur Semikolon/Whitespace am Ende ändern zählt ebenfalls als
      keine Änderung; Umformatieren zählt als Änderung.
- [ ] Rumpf ändern (z. B. `WHERE` ergänzen) → Meldung "view … replaced", danach
      zeigt `Rows` die neuen Zeilen und ein erneutes `E` die neue Definition
      (der Puffer ist re-baselined, kein Konflikt-Warnhinweis).
- [ ] View **umbenennen** → Ablehnung im Editor, Text bleibt erhalten, Hinweis
      "rename it back, or create the other view from a DB script".
- [ ] Zweites Statement anhängen (`; DROP TABLE …`) → Ablehnung, nichts läuft.
- [ ] Kaputtes SQL (`SELECT * FROM gibtsnicht`) → Fehler des Servers/der Datei
      **oben im Puffer** als Banner, der eigene Text darunter unverändert.
      Erneutes Speichern nach der Korrektur entfernt das Banner (es wird nicht
      mit gespeichert).
- [ ] Die View parallel von außen ändern (zweites `sqlite3` / `psql`), dann
      speichern → Konflikt-Hinweis mit der fremden Definition im Puffer statt
      stillem Überschreiben.
- [ ] View von außen löschen, dann speichern → nachvollziehbare Meldung, kein
      Panic.
- [ ] Flache `views`-Ansicht (`v`) → alle Views; `E` dort editiert ohne
      Durchdrillen und ohne die Baumansicht zu benutzen.

### Nur SQLite

- [ ] `read_only: true` (Default) → `E` öffnet, Speichern scheitert mit dem
      Hinweis auf `read_only: false`. Die View bleibt unverändert **da** —
      der Drop passiert in derselben Transaktion wie das Create.
- [ ] Erfolgreiches Speichern behält die verbatime Form:
      `sqlite3 <datei> "SELECT sql FROM sqlite_master WHERE name='<view>'"`
      zeigt genau den gespeicherten Text (SQLite formatiert nicht nach).
- [ ] Eine View, die eine andere View benutzt: die abhängige View bleibt nach
      dem Ersetzen benutzbar (SQLite löst Rümpfe erst beim Zugriff auf — der
      `SELECT … LIMIT 0`-Test in der Transaktion ist die eigentliche Prüfung).
- [ ] Der `Tables`-Ast listet **keine** Views mehr (und die `kind`-Spalte ist
      dort weg, weil sie konstant wäre).

### Nur Postgres

- [ ] Speichern läuft als `CREATE OR REPLACE VIEW`: eine abhängige View und
      ein gesetztes `GRANT` überleben (`\dp` vor/nach vergleichen). Nichts
      wird gedroppt.
- [ ] **Spaltenliste ändern** (Spalte umbenennen oder entfernen) → Postgres
      lehnt ab, Fehler landet im Banner; anhängen einer neuen Spalte am Ende
      funktioniert. Kein automatisches `DROP … CASCADE`.
- [ ] Schema-Qualifier aus dem Kopf entfernen (`CREATE OR REPLACE VIEW v AS …`)
      → Ablehnung mit dem Hinweis auf den `search_path`; nichts läuft.
- [ ] Kopf auf ein **anderes** Schema zeigen lassen → Ablehnung ("different
      schema").
- [ ] Nach einer Ablehnung **weiterarbeiten**: eine normale Query auf demselben
      Tab läuft noch. (Belegt, dass keine gecachte Session in aborted state
      hängt — es gibt bewusst keinen `BEGIN`-Block.)
- [ ] Blättern in `Rows` einer View ohne eigenes `ORDER BY` kann Zeilen
      wiederholen/auslassen; mit `ORDER BY` im Rumpf ist es stabil. Erwartetes
      Verhalten: eine View hat kein `ctid`, nach dem sortiert werden könnte.
- [ ] Materialized View (`relkind = 'm'`) erscheint **nicht** im `Views`-Ast.
- [ ] Table-Completions im DB-Skript-Editor listen auch Views.

## Datenzeilen editieren (`e` → `edit_row`, SQLite + Postgres)

`e` auf einer Zeile öffnet den konfigurierten Editor mit der Zeile als
YAML-Mapping — eine `spaltenname: wert`-Zeile pro Zelle. Speichern baut aus den
Spalten, die sich tatsächlich geändert haben, **ein** `UPDATE` und führt sonst
nichts aus. Scheitert das Statement, öffnet sich der Editor wieder mit der
Fehlermeldung **und** dem gebauten `UPDATE` als Banner über dem eigenen Text.

Voraussetzung: die `e`-Bindung auf den `Rows`-Ebenen aus
`docs/examples/views/{sqlite,postgres}.yaml`, und für SQLite `read_only: false`
in `sqlite-adapter.yaml`. Wie beim View-Editor ist es eine `actions:`-Zeile mit
`type: edit, id: edit_row` und **keine** `shortcuts:`-Zeile.

Der Kern des Tests: die Zeile wird über die **Schlüsselwerte adressiert, die
beim Öffnen gelesen wurden** — nicht über den Offset in der Baumzeile. Der
Offset ist nur, _wie_ die Zeile gefunden wurde; eine Seite, die sich darunter
verschiebt, darf das Schreiben nicht umlenken.

### Gemeinsam (beide Adapter)

- [ ] `e` auf einer Tabellenzeile → Editor mit allen Zellen als YAML, Endung
      `.yaml` (Syntax-Highlighting im Editor-Profil). Kopfkommentar nennt
      Tabelle, Offset und **wodurch** die Zeile adressiert wird.
- [ ] Ohne Änderung speichern → "no changes", kein Statement läuft.
- [ ] Eine Zelle ändern → Meldung nennt die Tabelle und "1 column"; die
      Tabellenansicht zeigt den neuen Wert nach `r`. Von außen prüfen
      (`sqlite3` / `psql`): **nur** diese Spalte wurde geschrieben.
- [ ] Zwei Zellen ändern → "2 columns", ein einziges `UPDATE`.
- [ ] Eine Zelle auf `null` setzen (YAML-`null`, nicht die Zeichenkette) → die
      Spalte ist danach SQL-NULL. Umgekehrt: `"null"` in Anführungszeichen
      schreibt den Text.
- [ ] Eine Zeile aus dem Puffer **löschen** → diese Spalte bleibt unangetastet
      (Auslassen heißt "nicht ändern", nicht "auf NULL setzen").
- [ ] Spaltenname vertippen → Ablehnung im Puffer, die echten Spaltennamen
      stehen in der Meldung, der eigene Text bleibt erhalten.
- [ ] Kaputtes YAML → Ablehnung mit Zeilenangabe, Text erhalten.
- [ ] Mehrzeiligen Text schreiben (Block-Scalar `|`), Sonderzeichen
      (`: `, `#`, führende Leerzeichen, Emoji) → kommt unverändert in der
      Datenbank an und beim nächsten `e` unverändert zurück.
- [ ] **Schlüsselspalte selbst ändern** (z. B. `id`) → das `UPDATE` adressiert
      die Zeile über den **alten** Wert, benennt sie also um statt eine zweite
      anzulegen.
- [ ] Zeile parallel von außen ändern, dann speichern → Konflikt-Hinweis mit
      dem eigenen Text unverändert im Puffer; **erneutes** Speichern
      überschreibt bewusst.
- [ ] Zeile von außen löschen, dann speichern → nachvollziehbare Meldung
      ("nothing was written"), kein Panic.
- [ ] Statement scheitert (Unique-Verletzung, Typfehler, Trigger) → Banner
      enthält die Fehlermeldung **und** darunter "The statement that failed:"
      mit dem `UPDATE`. Nach der Korrektur speichern → das Banner ist weg und
      wird nicht mitgeschrieben.
- [ ] Cursor auf eine Zeile stellen, dann von außen eine Zeile **davor**
      löschen, dann `e` → es wird die Zeile editiert, die der Editor gelesen
      hat (bzw. sauber abgelehnt), nie eine benachbarte.
- [ ] Im Record-Detail-Pane (`o`) → `e` editiert dieselbe Zeile (das Pane ist
      dieselbe Zeile transponiert).
- [ ] Flache `tables`-Ansicht → `e` funktioniert dort genauso.
- [ ] `Rows` einer **View**: `e` ist dort bewusst **nicht** gebunden. Wird es
      testweise gebunden, lehnt der Adapter mit dem Hinweis auf die
      zugrundeliegende Tabelle ab — kein Editor öffnet sich.

### Nur SQLite

- [ ] `read_only: true` (Default) → `e` öffnet, Speichern scheitert mit dem
      Hinweis auf `read_only: false`; die Zeile bleibt unverändert.
- [ ] Tabelle **ohne** Primary Key → Kopfkommentar sagt, dass über den
      impliziten `rowid` adressiert wird; Schreiben funktioniert.
- [ ] Tabelle mit zusammengesetztem Primary Key → `WHERE` nennt alle Spalten.
- [ ] **BLOB-Zelle** → im Puffer nur als Kommentar (`#   spalte: <blob, N
bytes>`). Zeile einkommentieren und speichern → Ablehnung ("cannot be
      written from here"); die Bytes bleiben unangetastet.
- [ ] Zeile mit `NULL` in einer Schlüsselspalte einer PK-losen Tabelle → über
      `rowid` adressiert, funktioniert trotzdem.

### Nur Postgres

- [ ] Tabelle mit Primary Key → Kopf nennt ihn; Schreiben funktioniert.
- [ ] Tabelle **ohne** PK, aber mit Unique-Index über NOT-NULL-Spalten → Kopf
      nennt den Index **namentlich**; Schreiben funktioniert.
- [ ] Tabelle ohne PK und ohne solchen Index → `e` lehnt mit der Begründung ab
      (kein Editor). `ctid` wird bewusst **nicht** als Ersatz benutzt, weil er
      sich bei jedem `UPDATE` verschiebt.
- [ ] Unique-Index über eine **NULLable** Spalte zählt nicht als Schlüssel
      (zwei NULLs kollidieren nicht) → dieselbe Ablehnung.
- [ ] Nicht-Text-Typen (`int`, `numeric`, `timestamptz`, `jsonb`, `bytea`,
      Array) → der Wert kommt als Text im Puffer, wird als Text-Literal
      geschrieben und vom Spaltentyp konvertiert; Round-Trip verändert nichts.
      `bytea` erscheint als `\x…` und kommt als dieselben Bytes zurück.
- [ ] Nach einer Ablehnung **weiterarbeiten**: eine normale Query auf demselben
      Tab läuft noch (keine Session in aborted state).

## View-Skripte auf den SQL-Zeilenebenen (`x`), DB-Skripte auf `X`

Auf den SQL-Tabs treffen drei verschiedene "Skript"-Begriffe aufeinander. Der
Test soll vor allem belegen, dass sie sich nicht mehr in die Quere kommen:

| Sorte            | was es ist                          | Taste     |
| ---------------- | ----------------------------------- | --------- |
| Node-Skripte     | SQL, gehört der Tabelle/View        | `Q` / `q` |
| DB-Skripte       | SQL, gehört der Datenbankdatei      | `X`/Enter |
| **View-Skripte** | beliebiges Programm, JSON auf stdin | `x`       |

`x` liegt jetzt auf **jeder** Zeilenebene (Baum-`Table`, Baum-`View`, und in den
flachen `tables`/`views`-Ansichten). Weil alle diese Ebenen denselben Node-Type
haben (`sqlite:row` / `postgres:row`), teilen sie **ein** Skript-Verzeichnis:
`~/.local/share/not_yet_done/scripts/<tab>/<root>/<row-type>/`. Die flachen
Ansichten brauchen dafür `script_source: databases`, sonst wäre ihr eigener
Node-Type die Wurzel und dieselben Skripte müssten dreimal existieren.

- [ ] `x` auf einem **Datenbank**-Knoten (Wurzelebene) öffnet das Menü für
      `scripts/sqlite/sqlite_database/`. Leeres Verzeichnis → Hinweis "No
      scripts in …, Type +name then Enter to create one", kein stilles
      Nichts. Nutzlast ist `{"node": {…}}` mit `fields.path` auf die Datei.
- [ ] Dank `inherit: true` liegt dasselbe `x` auch auf den Ebenen **unter**
      der Datenbank (`Tables`, eine Tabelle, `Views`, eine View, der
      `Scripts`-Ast) — und zwar auf demselben Verzeichnis, nicht auf einem
      pro Ebene. Ohne `inherit` gilt eine `actions:`-Zeile nur für ihre
      eigene Ebene; das war der Grund, warum `x` erst nur auf der obersten
      Zeile ansprang.
- [ ] Ein ausführbares Test-Skript in
      `~/.local/share/not_yet_done/scripts/sqlite/sqlite_database/sqlite_row/`
      ablegen, das stdin nach `/tmp` schreibt. `x` auf einer Tabellenzeile →
      Menü zeigt es, Auswahl führt es aus.
- [ ] Die Nutzlast enthält die angezeigte Seite plus Cursor-Kontext:
      `rows[]` (`id`, `label`, `fields`), `query`, `selected_index`,
      `selected_field` — `selected_field` folgt dem Spalten-Cursor.
- [ ] Dasselbe Skript erscheint ohne Zutun auch unter `x` auf einer
      **View**-Zeile und in den flachen `tables`/`views`-Ansichten (ein
      Verzeichnis, nicht drei).
- [ ] Postgres-Tab: `x` auf einer Zeile im `Views`-Ast zeigt dieselben Skripte
      wie im `Table`-Ast.
- [ ] Im `Scripts`-Ast: Cursor auf einem DB-Skript → Action-Bar zeigt
      `X: execute` (nicht `x`), `X` führt aus, Enter genauso. `x` dort tut
      **nichts** Falsches (kein Ausführen des Skripts als Nebeneffekt).
- [ ] Kein Kollisions-Warnhinweis beim Config-Laden (`f10`-Log leer bzgl.
      Keybindings auf den SQL-Tabs).

## CLI: Verbindungsstatus und Credential-Prompt

Die CLI verhält sich wie die TUI, nur ohne `r`: sie verbindet sofort, meldet
den Fortschritt auf **stderr** (stdout bleibt pipebar) und fragt Credentials
auf dem Terminal ab. Eine leere Liste heißt danach wirklich „nichts da".

- [ ] `nyd adapter <chat-instanz> ls` im Terminal → `nyd: Connecting…` auf
      stderr, danach die Server-Zeilen. **Nie** eine leere Liste mit Exit 0,
      während die Verbindung noch steht.
- [ ] Dasselbe gepipet (`… ls | cat`) → die Statuszeile landet auf stderr,
      die Tabelle unverändert auf stdout.
- [ ] Lokaler Adapter ohne Hintergrund-Verbindung (`nyd adapter tasks ls`) →
      keine Statuszeile, keine spürbare Verzögerung.
- [ ] Instanz mit `provider: { type: prompt }` und gelöschter Session im
      Terminal → Passwort-Abfrage (maskiert, auf stderr), danach das Ergebnis.
- [ ] Dieselbe Instanz ohne TTY (`… ls < /dev/null | cat`) → Fehlermeldung,
      die die fehlenden Felder nennt und auf env/file/command/keyring
      verweist, Exit ≠ 0 — keine leere Liste.
- [ ] Backend nicht erreichbar → `Connection failed: …` und Exit ≠ 0; hängt
      die Verbindung, bricht der Befehl nach 60 s mit Timeout-Meldung ab.
- [ ] `nyd adapter <instanz> help` funktioniert **ohne** Verbindung und ohne
      Credential-Abfrage.

## CLI: Auth-Mechanismen im Config-Assistenten (`config auth` / `config build`)

Welche Mechanismen es gibt, weiß seit dem Deskriptor-Umbau nur noch der
Adapter. Assistent und Auflistung lesen dieselbe Tabelle wie die Validierung —
was hier angeboten wird, muss die Factory also auch akzeptieren.

- [ ] `nyd config auth jira` → `cookie` und `basic-auth` mit Label, Doku und
      Feldern; keine Verbindung, keine Credential-Abfrage.
- [ ] `nyd config auth tasks` → „has no authentication", kein leeres Menü.
- [ ] `nyd config auth gibtsnicht` → Fehler mit der Liste der bekannten Typen.
- [ ] `nyd config build kimai` im Terminal → nach den normalen Feldern eine
      Legende + Menü für `auth.mechanism` (kein Freitext), danach je Feld ein
      Provider-Menü mit dem Feldnamen im Prompt (`auth.token: choose variant`).
- [ ] Das erzeugte YAML hat `auth:` an der Stelle, an der der Config-Typ es
      deklariert (bei kimai direkt nach `url:`), nicht angehängt am Ende.
- [ ] Ein Feld, dessen Label die Namensheuristik verfehlt (kimai `token` →
      „API password"), bekommt ein `label:`; `masked:` fehlt, wo die Heuristik
      ohnehin richtig liegt.
- [ ] Mechanismus mit optionalem Feld → Abfrage „bind the optional field …?"
      mit Default **nein**; abgelehnt taucht das Feld nicht im YAML auf.
- [ ] Escape/Ctrl-C in der Mechanismus-Auswahl → „aborted — no config
      written.", **nichts** auf stdout (auch nicht die schon beantworteten
      Felder).
- [ ] Das erzeugte YAML als `adapter.config` einer View eintragen → Adapter
      startet ohne Beanstandung der Auth-Sektion.
- [ ] `nyd config template kimai` → Hinweiszeile auf `nyd config auth kimai`
      auf stderr, stdout bleibt reines YAML.

## Skriptgetriebene Credentials (`auth.script` + `script-result`)

Ein Skript liefert **mehrere** Felder auf einmal und fragt nur dann etwas,
wenn es muss. Pro Runde ein frischer Prozess: `{"request": […], "input": {…}}`
auf stdin, genau eines von `result` / `form` / `error` auf stdout; `input`
sammelt die Antworten über die Runden hinweg. Testskript (ohne Server, nur das
Protokoll — erste Runde fragt, zweite liefert):

```sh
#!/bin/sh
in=$(cat)
case "$in" in
  *'"pin"'*) printf '{"result":{"username":"demo","token":"t-%s"}}' "$$" ;;
  *) printf '{"form":{"header":"Demo-Login","fields":[{"name":"account","label":"Account"},{"name":"pin","masked":true,"optional":true}]}}' ;;
esac
```

- [ ] `auth.script: <pfad>` + zwei Bindings mit `provider: { type:
script-result }` in einer View → beim Verbinden erscheinen **die Felder
      des Skripts** (Account, Pin), nicht die Mechanismus-Felder.
- [ ] Der Formular-`header` steht als Popup-Titel da (TUI) bzw. über den
      Fragen (CLI), nicht der generische „Login: <tab>".
- [ ] TUI: Pin leer lassen (`optional`) → Enter reicht ab; Account leer →
      „Required: Account", kein Absenden.
- [ ] Beide Felder aus einer einzigen Skript-Antwort → das Skript läuft
      **einmal** pro Runde, nicht je Binding einmal (am `$$`-Suffix im Token
      bzw. am Skript-Log erkennbar).
- [ ] CLI (`nyd adapter …`) im Terminal → dieselben Fragen, Pin darf leer
      bleiben, Account nicht.
- [ ] CLI in einer Pipe (`nyd adapter … | cat`) → „no terminal to ask on" mit
      den Feldnamen, kein stilles leeres Ergebnis.
- [ ] Skript antwortet `{"error":"…"}` → Login scheitert mit genau dieser
      Meldung, kein Formular.
- [ ] Skript liefert ein `result`, in dem ein angefragtes Feld fehlt → Login
      scheitert mit dem fehlenden Namen, kein Login mit leerem Wert.
- [ ] Skript wiederholt sein Formular mit `error: "…"` → dasselbe Formular
      kommt erneut, die Meldung steht dran, und die vorherigen Antworten
      sind im `input` der nächsten Runde enthalten.
- [ ] Skript, das **immer** ein Formular schickt → nach 5 Runden Abbruch mit
      Hinweis auf das Rundenlimit, keine Endlosschleife.
- [ ] Escape im Formular (TUI) bzw. Abbruch (CLI) → Login scheitert sofort
      mit „cancelled"; ein **zweiter** Verbindungsversuch danach kommt wieder
      bis zum Formular (der Auth-Mutex hängt nicht).
- [ ] Reconnect nach 401 → das Skript läuft erneut, das Formular kommt erneut
      (der Wert ist bewusst kurzlebig), nicht der gecachte alte Wert.
- [ ] Konfig-Prüfung: `script-result` ohne `auth.script` **und** `auth.script`
      ohne `script-result`-Binding werden beide beim Lesen der View
      abgelehnt — mit Nennung der fehlenden Hälfte.
- [ ] `nyd config auth <typ>` listet `script-result` in der Provider-Liste;
      `nyd config build <typ>` bietet es im Provider-Menü und fragt danach
      **genau einmal** nach `auth.script`, egal wie viele Felder es nutzen.

## Abgelaufener Cookie mitten in der Sitzung (Jira / Confluence)

`session_cache: until-rejected` heißt: die Session gilt, bis der Server sie
ablehnt. Ein 401 (oder ein SSO-Bounce in den Login-Flow) markiert den Client
als abgelehnt; der nächste Zugriff wirft ihn weg und loggt neu ein — beim
`command`-Provider läuft dabei das Cookie-Skript erneut.

- [ ] Verbinden, Liste laden, dann den Cookie serverseitig ungültig machen
      (im Browser ausloggen / Session beenden) → nächster Zugriff meldet
      einmal den 401.
- [ ] Danach Reload (`r`) → Adapter loggt neu ein und liefert Daten, ohne
      dass `invalidate_session` von Hand aufgerufen werden muss.
- [ ] Provider `command`: das Skript wird beim Neuanmelden erneut ausgeführt
      (am Skript-Log / an einer Passwort-Abfrage erkennbar), nicht der alte
      Wert wiederverwendet.
- [ ] Seite ohne Berechtigung öffnen (403) → normale Fehlermeldung, **kein**
      Neuanmelden, kein erneuter Skript-/Passwort-Prompt.

## Sortier-Menü (`c s`) und Spalten-Menü auf `c c`

Das Sortier-Menü ist ein zweiter UI-Pfad auf dieselbe Sortierung wie `S`;
`c` ist jetzt Chord-Leader für beide Tabellen-Menüs.

- [ ] `c c` öffnet das Spalten-Menü (vorher `c`), `c s` das Sortier-Menü.
      Ein einzelnes `c` tut nichts und wartet auf den zweiten Anschlag.
- [ ] Im Sortier-Menü stehen **alle** sortierbaren Spalten: die sortierten
      oben in Sortierreihenfolge mit `asc`/`desc`, die unsortierten darunter.
- [ ] `j`/`k` bewegen den Cursor, `ctrl+j`/`ctrl+k` verschieben den markierten
      Eintrag **innerhalb** des sortierten Blocks; auf einem unsortierten
      Eintrag passiert nichts.
- [ ] `a`/`d` auf einer unsortierten Spalte hängt sie hinten an den sortierten
      Block an, `0` nimmt sie wieder heraus (sie landet an ihrer natürlichen
      Position darunter).
- [ ] `Enter` wendet an: genau **ein** Reload, Notification nennt die Spalten;
      `Esc` verwirft, die Tabelle bleibt unverändert.
- [ ] Mit `S` gesetzte Sortierung ist im Menü sichtbar und umgekehrt — die
      Sortierung überlebt einen Tab-Wechsel (wird wie bisher gespeichert).
- [ ] Ebene ohne sortierbare Spalten: `c s` meldet „No sortable columns"
      statt ein leeres Popup zu zeigen.
- [ ] View-YAML mit einer `actions:`-Bindung auf `c` wird beim Laden als
      Konflikt gemeldet (Prefix-Kollision mit `c c`/`c s`), `force: true`
      unterdrückt sie weiterhin.

## Unbekannte Keys in `views/*.yaml` (Warnung statt Stille)

Serde verwirft unbekannte Keys wortlos — die Zeile steht in der Datei und
tut nichts. Sie werden jetzt gemeldet, ohne dass der Tab kaputtgeht.

- [ ] In `views/jira.yaml` unter einem `children:`-Eintrag ein `key: x`
      ergänzen → beim Start erscheint **ein** Modal „View configuration
      warnings" mit dem Pfad (`views.0.children.0.key`); der Jira-Tab lädt
      normal und alle Zeilen sind da.
- [ ] Zeile wieder entfernen → Start ohne Modal.
- [ ] Dieselbe Zeile bei laufender TUI per `:config jira` ergänzen und
      speichern → Notification „Reloaded view jira.yaml — ignored unknown
      keys: …", der Tab bleibt bedienbar.
- [ ] Ein `hooks:`-Block (siehe `docs/examples/views/tasks.yaml`) löst
      **keine** Warnung aus — er wird vom Host gelesen, nicht vom TUI.
- [ ] Echter YAML-Syntaxfehler verhält sich unverändert: der Tab wird
      `Broken` mit Fehlerpanel, kein Warn-Modal.

## Custom-Spalten als Aufzählung (`set-column-options`)

Eine Custom-Spalte auf einen geschlossenen Wertesatz einschränken. Über das
Aktions-Menü oder per CLI (`nyd <inst> do set-column-options <ID> --field
column_key=<key> --field options=a,b,c`).

- [ ] Auf einer Spalte mit gemischten Werten einen Satz setzen, der nicht alle
      abdeckt → Fehler nennt die störenden Row-Ids, und die Spalte bleibt frei
      (nichts wurde geschrieben).
- [ ] Werte korrigieren, denselben Satz nochmal setzen → geht durch, Meldung
      nennt die Anzahl abgedeckter Zellen.
- [ ] Danach eine Zelle auf einen Wert **außerhalb** des Satzes setzen → wird
      abgelehnt, der alte Wert steht noch da. Ein Wert aus dem Satz geht.
- [ ] Zelle leeren bleibt erlaubt (leer = „unbelegt", keine Verletzung).
- [ ] In der Edit-Form (die im View gebundene `edit-cells`-Action) erscheint
      die Spalte als **Select** mit genau diesen Werten; eine unrestringierte
      Custom-Spalte daneben bleibt ein Textfeld.
- [ ] Im Select lässt sich „nichts" wählen (leerer `(none)`-Zustand) → Zelle
      wird geleert.
- [ ] `options` leer setzen → Spalte ist wieder frei, beliebige Werte gehen.
- [ ] Auf einer `number`-Spalte einen Satz mit einem Wort setzen → abgelehnt
      (Optionen müssen zum `value_type` passen).
- [ ] Leerzeichen/Dubletten im Satz (`1 , , 2 , 1`) werden getrimmt,
      entdoppelt und blank-frei gespeichert.

## Stoat: Kanal mit gelöschter letzter Nachricht

Der tote `last_message_id` hielt den Kanal früher dauerhaft ungelesen.

- [ ] Kanal öffnen, dessen letzte Nachricht gelöscht wurde: die Liste endet auf
      einer Zeile `[deleted message]` an chronologisch korrekter Stelle.
- [ ] Der Cursor landet beim Öffnen direkt darauf (`cursor_on_open`) und die
      Glocke in der Tab-Leiste verschwindet **ohne** einen Tastendruck.
- [ ] Nach TUI-Neustart bleibt der Kanal gelesen (das Ack ging serverseitig
      durch, nicht nur lokal).
- [ ] Auf der Tombstone-Zeile schlagen `edit`/`delete`/Reaktion/Download mit
      „this message was deleted" fehl statt mit einem 404.
- [ ] Vorschau/Detail derselben Zeile zeigt denselben Stand-in, kein Fehler.

## Refinements / Deferred Tasks

Punkte, die in Smoke-Tests aufkamen aber nicht zum jeweiligen Refactor
gehören. Werden in eigenen Sessions adressiert.

- Validator (keymap.rs) kennt die Autonummerierungs-Ziffern noch nicht;
  in Konstellations-Modus könnten feste `tab_*`-Bindings als
  Schein-Kollision auftauchen bzw. eine View-Ziffer-Bindung wird nicht
  als global geclaimt geführt. Niedrige Priorität (Ziffern als
  View-Action-Keys sind selten).
- Persistenz des Tab-Set-Wechsels: aktuell session-only (nicht zurück in
  `tui.yaml` geschrieben). Falls gewünscht, optionaler Write-back.

## Quellen

- Plan Content-Actions: [`plan-content-actions-unification.md`](plan-content-actions-unification.md)
- Plan EditSession-Refactor: [`plan-edit-session-refactor.md`](plan-edit-session-refactor.md)
