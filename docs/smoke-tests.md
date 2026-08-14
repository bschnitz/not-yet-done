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

Pfad-basiertes Lookup über den **generischen** `--path`-Resolver (D3b-1) auf
der Adapter-Instanz `tasks` (das frühere hartkodierte `task show` ist
entfernt). Ein Segment pro Ebene, gegen Child-Labels per Substring (mit `-i`
case-gefoldet) oder `re:`-Regex; jedes Segment muss genau ein Kind treffen.
Erfolg → Node-Felder auf stdout (`-o json` für JSON), exit 0.

> ⚠ Der generische Resolver hat andere Fehlermeldungen/Exit-Codes als das alte
> `task show`. Diese Items beim nächsten Smoke gegen das Ist-Verhalten von
> `adapter_cli` (substring/`re:`, „ambiguous … list the candidates") neu
> verifizieren und die erwarteten Strings hier nachziehen.

- [ ] `nyd tasks show --path /Work/Clients/Acme/Tickets` →
      stdout mit den Node-Feldern, exit 0.
- [ ] `nyd tasks show -i --path /work/clients/acme/tickets` →
      gleicher Hit wie oben, exit 0.
- [ ] `nyd tasks show --path '/Inbox/re:^Week \d+$'` → Regex-Segment matched.
- [ ] `nyd tasks show --path /nope` → Fehler (unmatched segment), exit ≠ 0.
- [ ] `nyd tasks show --path /<ambig>` wenn mehrere Kinder matchen →
      Fehler mit Kandidatenliste, exit ≠ 0.
- [ ] Pipe-bar: `nyd tasks show --path … -o json | jq -r '.id'` liefert
      eine valide UUID (smoke fürs JSON-Schema).

## Script `mode: commands` (script → TUI command relay)

`# mode: commands` Skripte schreiben in `$NYD_OUTPUT_FILE` JSON
`{"commands": [...]}` und steuern darüber die TUI (z.B. `jump`,
`focus-task`, `tag`, ...). `interactive+commands` macht dasselbe,
nur dass das Skript zusätzlich das Terminal bekommt.

- [ ] Background-Variante: Skript schreibt
      `{"commands": ["jump Tasks:tree"]}` in `$NYD_OUTPUT_FILE`,
      Tab wechselt nach Tasks:tree.
- [ ] Mehrere Commands hintereinander: `["jump Tasks:tree",
"focus-task /work/foo"]` → Jump + Path-Expand + Focus
      laufen der Reihe nach durch.
- [ ] Skript schreibt nichts → Notification "Script finished", keine
      Commands ausgeführt (no-op statt Fehler).
- [ ] JSON ist nicht parsebar → Modal _"Script output is not valid JSON: …"_.
- [ ] JSON ohne `commands`-Array → Modal _"missing `commands` array"_.
- [ ] Eintrag in `commands` ist kein String → Modal _"command entry is not a string"_, weitere Einträge laufen trotzdem.
- [ ] Eintrag mit führendem `:` (`":jump Tasks:tree"`) wird genauso
      akzeptiert wie ohne.
- [ ] Skript exitet mit non-zero status → Modal _"Script exited with …"_,
      Commands werden NICHT ausgeführt.
- [ ] Vorwärtskompatibilität: JSON `{"commands": [...], "version": 1,
"metadata": {…}}` wird akzeptiert, unbekannte Keys ignoriert.
- [ ] Stderr aus dem Skript landet weiterhin als Notification.
- [ ] `interactive+commands`: Skript läuft interaktiv (TUI yielded
      Terminal), nach Beendung werden Commands aus dem detached-
      output_file ausgeführt.
- [ ] Das output-file in `/tmp` ist nach dem Lauf weg (cleanup).

## SQ-8 — Postgres-Script-Shortcuts via `query_shortcut` (DB)

Postgres-pro-Tabelle-Skripte hängen ihren Hotkey nicht mehr in einer
`.shortcuts.yaml` neben dem Skript-`.sql`, sondern in der
`query_shortcut`-Tabelle (Scope = NodeRef-Pfad
`postgres/<inst>/<db>/schemas/<schema>/tables/<table>`, Name =
Skript-Datei-Stem, Shortcut = Chord). Apply-on-Chord ist symmetrisch zu
Jira/Taiga: globaler Auslöser solange das fokussierte Pane auf genau
dieser Tabelle steht.

- [ ] **Bestand:** Migration der 6 jira/taiga-Zeilen mit `:`-Separator auf
      NodeRef-`/`-Form:
      `sh
sqlite3 ~/.local/share/not_yet_done/nyd.db \
    "UPDATE query_shortcut SET scope = REPLACE(scope, ':', '/') WHERE scope LIKE '%:%';"
`
      Danach erscheinen Jira/Taiga-Saved-Query-Hotkeys (`ctrl+i`, `ctrl+m`,
      `ctrl+w` etc.) im Tab wieder als highlighted Apply-Hint und feuern
      die zugehörige Query.
- [ ] Postgres-Tab → eine konkrete Table fokussieren (Drilldown auf Schema
      `public` → Cursor auf `users`) → `q queries` → ein existierendes
      Skript hat `[chord]` in der Liste, wenn vorher gebunden.
- [ ] `Ctrl+e` in der Skript-Liste → Modal „Press a shortcut key" → eine
      freie Taste drücken → Notification „Bound shortcut" oder still.
      Anschließend `sqlite3 ~/.local/share/not_yet_done/nyd.db
"SELECT scope, name, shortcut FROM query_shortcut WHERE scope LIKE
'postgres/%';"` zeigt den Eintrag mit NodeRef-Pfad.
- [ ] Popup schliessen, Cursor bleibt auf derselben Tabelle → den
      gebundenen Chord drücken → der Skript-Result öffnet sich im
      Rows-Split (gleiches Verhalten wie Enter-on-Apply aus dem Menü).
- [ ] Selber Chord, Cursor auf ANDERER Tabelle → der Chord ist NICHT
      claimed (`PostgresTableScriptShortcut` ist auf den Table-NodeRef
      konditioniert), Default-Verhalten der Taste bleibt aktiv.
- [ ] Im Q-Menü `d` (delete) auf ein Skript mit Shortcut → Datei UND
      `query_shortcut`-Zeile sind weg (sqlite3-Check).
- [ ] Restart TUI → die Bindings überleben (DB-persistent).
- [ ] Filesystem-Check: Es gibt nach Bind/Delete KEINE
      `.shortcuts.yaml`-Datei mehr in
      `~/.local/share/not_yet_done/postgres/*/queries/**`.

## Jira Multi-Hop Workflow-Transitions (TR-1 … TR-7)

Hintergrund: Der Transition-Picker schreibt jede beobachtete Workflow-Kante
in den Cache (`jira_workflow_edge`) und enumeriert daraus Mehrschritt-Ketten,
sodass User von `Ready → In Progress → Done` in einem Schritt durchgehen
können, ohne die Zwischenstati einzeln klicken zu müssen.

### Setup

- Backing-DB ist `~/.local/share/not_yet_done/jira-cache.sqlite`. Tabelle
  `jira_workflow_edge` wird beim ersten Adapter-Start automatisch angelegt
  (SeaORM auto-sync).
- Cache nie manuell zurückgesetzt — Hop-Limit ist 4, Self-Loops werden
  aufgezeichnet aber nicht traversiert.

### Smoke (Cold-Start, leerer Cache)

- [ ] DB-Tabelle leeren: `sqlite3 ~/.local/share/not_yet_done/jira-cache.sqlite "DELETE FROM jira_workflow_edge;"`
      vor TUI-Start.
- [ ] Auf einem Jira-Ticket Transition-Picker öffnen → Optionen entsprechen
      genau den direkten Transitions. Label = nur Ziel-Status, kein `*`
      (alle direkt). Kein doppelter Eintrag wenn zwei Transitions zum
      selben Status führen — erste gewinnt.
- [ ] Picker schließen → in der DB existieren Edges für aktuellen Status:
      `sqlite3 ... "SELECT from_status_name, transition_name, to_status_name FROM jira_workflow_edge;"`

### Smoke (Snowball wächst, Multi-Hop erscheint)

- [ ] Ticket in Status `Ready` öffnen → Picker → direkte Transitionen
      (keine `*`).
- [ ] Andere Tickets in `In Progress` und in `Review` öffnen, Picker je
      einmal aufrufen, dann Esc (kein Transition!).
- [ ] Zurück zum ersten Ticket → Picker → erscheint jetzt zusätzlich
      Eintrag `Done*` (oder ähnlich), erreichbar via Multi-Hop. Das `*`
      markiert "nicht direkt"; die Zwischenstati erscheinen NICHT mehr
      im Label.
- [ ] Gibt es sowohl eine direkte Transition nach `Done` als auch eine
      Multi-Hop-Kette zu `Done`, erscheint nur ein Eintrag `Done` (ohne
      `*`) — der direkte Pfad gewinnt.

### Smoke (Chain-Erfolg)

- [ ] Picker → Multi-Hop-Eintrag wählen (z.B. 2-Hop) → Enter.
- [ ] Status-Bar: `<KEY> → <Endstatus>`.
- [ ] Ticket-Detail zeigt Endstatus; Jira-Web bestätigt dass beide
      Transitionen durchliefen.

### Smoke (Chain-Fehler mit Refresh)

- [ ] Workflow mit `required field` an zweitem Hop konstruieren (z.B.
      Transition fordert `resolution` als pflicht). Test-Ticket auf
      Startstatus zurücksetzen.
- [ ] Picker → Multi-Hop wählen, dass den Pflicht-Feld-Hop enthält → Enter.
- [ ] Status-Bar: `Chain stopped at step 2/3 (now in <Zwischenstatus>):
<Jira-Errorbody>`.
- [ ] Ticket-Detail in TUI zeigt aktuellen Zwischenstatus (Hop 1 erfolgreich
      persistiert, Hop 2 abgebrochen) — nicht den ursprünglichen Startstatus.

### Smoke (Picker-Hint-Bar)

- [ ] Beliebige Picker-Action öffnen (`transition`, oder auch andere wie
      Postgres-Cell-Picker falls existent) → Hint-Bar zeigt `Enter apply
| Esc close` am Popup-Footer.
- [ ] Im Detail-Pane: Style passt zu anderen Menü-Hints (Query-Menü,
      Tag-Menü). Kein doppelter Style, kein Layout-Bruch.

### Edge Cases

- [ ] Ticket-Key ohne `-` (sollte nicht passieren, aber falls
      Test-Konfig kaputt ist): Recording bricht still ab, direkter
      Picker funktioniert weiter.
- [ ] `db.url` in der Jira-Adapter-YAML auf `none`/leer: Recording
      No-op, Picker zeigt nur direkte Transitionen, kein Crash.
- [ ] Self-Loop-Transition in Workflow (z.B. `To Do → To Do` zum Anhängen
      eines Attachments): Wird als Edge geschrieben, aber NICHT als
      Pfad-Option vorgeschlagen.

## SearchablePopup — intrinsische Navigation (SP-1 … SP-7)

Hintergrund: `SearchablePopup` trägt jetzt sein eigenes Set an Bindings
für `next` / `prev` / `backspace` / `cursor_left` / `cursor_right`
(`PopupAction`-Enum, konfigurierbar unter `popup:` in `tui.yaml`,
Defaults `ctrl+j`/`ctrl+k`/`backspace`/`left`/`right` plus Pfeiltasten
als Sekundär-Binding für `Next`/`Prev`). Die intrinsischen Hints
erscheinen automatisch in der Hint-Bar — der Transition-Picker stimmt
damit visuell und funktional zu QueryMenu/TagMenu/ScriptMenu.

### Smoke (Transition-Picker bekommt jetzt sichtbare Navigation)

- [ ] Jira-Ticket mit ≥3 verfügbaren Transitions auswählen, Picker
      öffnen (`Action`-Trigger des Adapters, z.B. `t`).
- [ ] Hint-Bar am unteren Popup-Rand zeigt: `↓ next  ↑ prev  ⏎ apply
␛ close` (Icons via `key_icons`-Map; `↓/↑` für Pfeil + ggf. `ctrl+j`
      mit Slash davor).
- [ ] Pfeil-Hoch/Runter UND `Ctrl+J`/`Ctrl+K` navigieren beide.
- [ ] Tippen filtert die Liste; `Backspace` zeigt zusätzlichen Hint
      `⌫ erase` (nur wenn die Query nicht leer ist).

### Smoke (andere Picker unverändert)

- [ ] QueryMenu (`q` auf Trackings/Tasks/Content) — Hint-Bar zeigt
      jetzt zusätzlich `next`/`prev` vor den bisherigen Hints
      (`apply`/`edit`/`shortcut`/`delete`/`close`).
- [ ] TagMenu (`:tag`), ScriptMenu (`:script`) und `gl`-Link-Popup
      analog: Pfeile + `Ctrl+J/K` funktionieren, Hint-Bar zeigt sie.
- [ ] `:config` Picker funktioniert weiterhin: tippen filtert,
      Pfeil-Navigation, Enter öffnet die Datei.

### Smoke (Custom-Bindings via tui.yaml)

- [ ] In `~/.config/not-yet-done/tui.yaml` unter `keybindings.popup:`
      `next: ctrl+n` und `prev: ctrl+p` setzen.
- [ ] TUI neu starten (oder `:config` → tui.yaml → save → granularer
      Reload).
- [ ] Transition-Picker zeigt jetzt `^N next  ^P prev …` und genau
      diese Tasten navigieren.

## Shortcut Hints (SH-1 … SH-7)

Hintergrund: YAML-`shortcuts:` (z.B. `a: add`, `x: execute`,
`Q: parent:edit_sql`) werden jetzt als Action-Bar- oder Status-Bar-Hints
auf der gerade selektierten Zeile gerendert. Die Hints sind row-spezifisch
und kommen aus dem `Node::actions()`-Lookup pro `node_id`, asynchron
gefetcht und gecached. Race-frei via Cache-Key = `node_id`.

### Smoke (Postgres — Datenbank-Subtab Tree-Mode)

- [ ] Postgres-Tab → Subtab `5` (Datenbank) → in Tree-Mode hoch- und
      runter-cursorn. Cursor auf einer DB-Scripts-Gruppen-Row →
      Action-Bar zeigt `a: add`.
- [ ] Tree-expand der Scripts-Gruppe (`l`/`Enter`) → Cursor auf einer
      einzelnen DB-Script-Leaf → Action-Bar zeigt `X: execute`,
      `e: edit`, `d: delete` — `a: add` ist NICHT mehr sichtbar
      (Leaf hat kein `add` in `actions()`).
- [ ] Erster Cursor-Move auf neuer Zeile: Hints können kurz fehlen,
      erscheinen sobald die Adapter-Antwort eintrifft (max. einige
      ms). Beim zweiten Besuch derselben Zeile sofort da (Cache-Hit).

### Smoke (Rows-View mit `parent:`-Shortcut)

- [ ] Postgres-Tab → Tabelle wählen → Rows-View öffnen. Action-Bar
      zeigt `Q: edit sql` (aus dem ViewDef-Shortcut
      `Q: parent:edit_sql`), aufgelöst über den Eltern-Table-Node.
      Cursor in der Zeilen-Liste bewegen → der Hint bleibt stabil
      (Target ist das Parent, nicht die aktuelle Row).

### Smoke (Cache-Invalidation auf Reload)

- [ ] Auf einer Row mit existierenden Hints (z.B. DB-Script-Leaf,
      `x e d` sichtbar) → `r` (reload) → Liste wird neu geladen, Hints
      werden für die nun selektierte Zeile neu gefetched und
      erscheinen wieder.

### Regression-Bait

- [ ] Mehrfaches schnelles Hoch-/Runter-Cursorn auf verschiedenen
      Rows: keine Duplikat-Fetches, keine stale Hints aus einer alten
      Row (Cache key=node_id, Pending-Dedup).
- [ ] Jira-Ticket-Liste: bewegt sich der Cursor zwischen Tickets, sind
      die Shortcut-Hints der jeweiligen Row stets ihrer eigenen
      `actions()` zugeordnet (kein Ticket zeigt die Hints des
      Nachbar-Tickets).

## EIP — Edit-in-Place für DB Scripts

ChildDef-Flag `editor_in_place: true` legt das Editor-Tempfile im
Zielverzeichnis statt in `$TMPDIR` ab, damit LSPs (z. B.
`postgres-language-server`) den Projektkontext finden.

**Setup**: ein leeres oder beliebiges `postgres-language-server.jsonc`
unter `<instance_data_dir>/db_scripts/<db>/` ablegen.

- [ ] DB Scripts-Tree öffnen, mit `e` ein Skript editieren →
      vim/$EDITOR-Statuszeile zeigt einen Pfad **innerhalb** des
      `db_scripts/<db>/`-Verzeichnisses, prefix `.nyd_tmp_…`, suffix
      `.sql` (kein `/tmp/…`).
- [ ] Nach `:w` + `:q` ist das `.nyd_tmp_…`-Tempfile im Verzeichnis
      gelöscht, das echte Skript trägt den geschriebenen Inhalt.
- [ ] `postgres-language-server.jsonc` neben den Skripten wirkt auf
      die Edit-Session (LSP-Diagnose / Hover, je nach Server-Setup).
- [ ] Mit `editor_in_place: false` (oder Default) liegt das Tempfile
      wieder in `/tmp/…`.
- [ ] Ein `.py`/`.md`-Skript editieren: Tempfile-Suffix übernimmt die
      reale Endung (`.nyd_tmp_xyz.py`), kein SQL-Template eingefügt.
- [ ] TUI hart killen mitten in einer Edit-Session →
      `.nyd_tmp_…`-Datei verbleibt im Verzeichnis (Prefix ist klar
      als Junk erkennbar; manuell löschbar).

## AE — Adapter Child-Process Environment

Trait `ContentAdapter::child_process_env(node) -> HashMap<String,String>`
wird beim Spawn von Editor- und Skript-Kindprozessen abgefragt; die TUI
gibt den Inhalt opak per `Command::envs(...)` weiter. Postgres-Adapter
liefert `PGHOST`/`PGPORT`/`PGUSER`/`PGPASSWORD`/`PGDATABASE`/`PGSSLMODE`.

**Setup**: aktiver Postgres-Adapter mit `manual_connect: false` (auto-
warmup), `transport.mode: ssh_tunnel` ist der interessante Fall. Im
nvim `postgres_lsp` aktiv via `:LspInfo`. Die jsonc neben den Skripten
darf nur die Schema-Zeile enthalten — alle `db`-Keys raus.

- [ ] Auf einem `postgres:db_script` Node `e` (Edit) — nvim öffnet den
      Buffer in `db_scripts/<db>/`, `postgres-language-server` startet
      sauber (Logs: kein „pool timed out").
- [ ] Im Buffer `SELECT * FROM ` tippen → Completion-Popup mit den
      realen Tabellen/Spalten der DB der NodeRef. Vorher/nachher
      vergleichen: ohne Adapter-Env (z. B. `disableConnection: true`
      in der jsonc) liefert v0.25 0 Items.
- [ ] In einem Shell-Term im editor: `env | grep ^PG` zeigt die fünf+
      Variablen (Wert von `PGPASSWORD` nicht in den Repo paste!).
- [ ] Adapter offline (Status-Bar `Disconnected`): `e` öffnet trotzdem,
      LSP startet ohne DB-Connection und zeigt 0 Completions (graceful,
      keine Fehlermeldung).
- [ ] `:script` im Postgres-Tab auf einem Tabellen-Knoten ausführen:
      `python3 - <<'PY' …` kann `os.environ["PGPASSWORD"]` lesen
      (z. B. Skript schreibt nach `$NYD_OUTPUT_FILE` eine Liste der
      `PG*`-Variablen). `NYD_OUTPUT_FILE` darf von Adapter-Env nicht
      überschrieben werden — Adapter-`PGFOO` kommt vor `NYD_*`.
- [ ] `:script` im Tasks-Tab: empty env, kein `PG*` (no regression —
      Tasks-Skripte sehen weiterhin nur die alten Variablen).
- [ ] `:script` im Trackings-Tab: gleich, empty env.
- [ ] Tunnel manuell killen (auf der SSH-Bastion), in der TUI eine
      Query absetzen → tear_down + reconnect; nächster Editor-Start
      auf einem DB-Script hat `PGPORT` mit dem neuen lokalen Port,
      nicht dem alten.

## Confluence-Adapter (CF-3 … CF-16)

Hintergrund: Confluence-Server/DC-Adapter spiegelt die Jira-/Taiga-
Architektur — `confluence:space` als Root, `confluence:page` rekursiv
darunter, plus `confluence:attachment` und `confluence:comment` als
Leaf-Branches an jeder Seite. Alle Aktionen werden über das
adapterseitige `actions_for_type` gebunden, die View-YAML-Einträge
sind Dokumentation.

### Setup

- `~/.config/not_yet_done/views/confluence-adapter.yaml` mit Cookie-
  Skript-Pfad (`auth.bindings[].provider.script`). Skript schreibt
  eine Zeile `JSESSIONID=...; crowd.token_key=...; atlassian.xsrf.token=...`
  auf stdout.
- `~/.config/not_yet_done/views/confluence.yaml` (Beispiel:
  [`docs/examples/views/confluence.yaml`](examples/views/confluence.yaml)).
- Saved-Queries-Verzeichnis:
  `<XDG_DATA_HOME>/not_yet_done/confluence/<instance_id>/queries/`.
  Optionaler Seed:
  [`docs/examples/views/saved/confluence/recent-pages.yaml`](examples/views/saved/confluence/recent-pages.yaml).
- TUI starten → Tab `Confluence` (Default-Sub-Tab `spaces`); per
  `manual_connect: true` lädt nichts automatisch, `r` triggert den
  ersten Fetch.

### CF-3 — Spaces

- [ ] `r` auf `spaces` → Liste aller Spaces; `Key`/`Name`/`Type`
      Spalten gefüllt; Cursor-Navigation `j`/`k` funktioniert.
- [ ] `f`-Fuzzy- und `/`-Search-Aktionen filtern auf `Key`+`Name`.
- [ ] `o` auf einem Space-Row öffnet den Space im Browser (webui).

### CF-4 — Pages (rekursiv)

- [ ] Auf einer Space-Row Enter → Top-Level-Pages werden inline
      expandiert; Tree-Marker (`▼`/`▶`) sichtbar.
- [ ] Auf einer Page-Row Enter → Kindseiten + Attachments + Comments
      als drei Branches erscheinen.
- [ ] Tief drillen (3+ Ebenen) bleibt konsistent — gleicher
      rekursiver `ChildDef` an jedem Level.

### CF-5 — Preview Pane (body.storage)

- [ ] Auf einer Page-Row `p` → Preview-Pane erscheint horizontal
      gesplittet (50/50), zeigt `body.storage` (XHTML).
- [ ] Erste `p`-Toggle: spürbare Latenz (lazy-fetch
      `GET /content/{id}?expand=body.storage,...`); zweite Toggle:
      sofort (Cache).
- [ ] `p` erneut → Preview schließt.

### CF-6 — Attachments

- [ ] Page drillen → `attachments`-Branch zeigt Dateiname, Author,
      Size, Mime-Type, Created.
- [ ] `d` auf einem Attachment → Download in Tempdir + `xdg-open`
      öffnet die Datei.
- [ ] Zweiter `d` auf demselben Attachment: kein erneuter Fetch
      (cached-by-id), öffnet sofort.

### CF-7 — Comments (read-only)

- [ ] `comments`-Branch zeigt Author, Created, Body-Auszug.
- [ ] `p` toggelt Body-Preview; **kein** zweiter HTTP-Call (Body
      ridet auf `list_comments` mit `expand=body.storage,version`).

### CF-8 — CQL Search

- [ ] Sub-Tab `search` öffnen → Default-CQL aus YAML
      (`type = page AND lastModified > now("-7d") ...`) zeigt
      Treffer.
- [ ] `q` öffnet Saved-Queries-Menu; Seed `recent-pages` taucht auf.
      Enter applied; `Ctrl+f` bindet Chord-Shortcut (persistiert in
      `query_shortcut`).
- [ ] `:query new <name>` → Editor öffnet; CQL eintippen, speichern
      → erscheint im `q`-Menu.
- [ ] `:query delete <name>` entfernt Datei + DB-Shortcut.
- [ ] Drilldown auf einem Search-Treffer öffnet die gleichen drei
      Branches (pages / attachments / comments) wie via `spaces`.

### CF-9 — Edit Page + 3-Way Merge

- [ ] `e` auf einer Page → `$EDITOR` öffnet mit `title: <echter
Titel>` auf Zeile 1 (nicht die Page-Id!), Leerzeile, dann
      pretty-printed `body.storage` (xmllint).
- [ ] Body trivial ändern (z.B. neuen Absatz einfügen), speichern,
      Editor schließen → `Updated page <Title> (v <n+1>)` Banner.
- [ ] **Tree-Zeile nach Save** zeigt weiterhin den **echten Titel**,
      nicht die Page-Id (Regression-Guard: der Post-Edit-Row-Patch
      re-resolved die Zeile via `get_by_id`, dessen Stub den Titel auf
      die Id setzte → Zeile zeigte die Id bis zum nächsten vollen
      Tree-Reload; `get_by_id` hydriert den Titel jetzt vom Server).
- [ ] Confluence-Web: neue Version erscheint, Body korrekt, **Titel
      unverändert** (Regression-Guard: früher schrieb ein body-only
      Edit die Page-Id als neuen Titel zurück).
- [ ] **Rename**: nur die `title:`-Zeile ändern, Body gleich lassen,
      speichern → Page heißt im Confluence-Web neu; keine NoChanges-
      Kurzschluss.
- [ ] **Disjoint-Merge**: vor Editor-Save in Confluence-Web die
      Page upstream ändern (eine andere Stelle als die Edits im
      Buffer). Speichern → 409 → auto-merge → `Merged on top of
v<m>` Banner; beide Änderungen sind in der Final-Version.
- [ ] **Conflict-Merge**: vor Save dieselbe Stelle upstream
      ändern. Speichern → 409 → Buffer reopent mit
      `<<<<<<< ours` / `>>>>>>> theirs` Markern + Banner
      `Merge conflict — resolve and save again`. Marker manuell
      auflösen, Save → Update geht durch.
- [ ] Parse-Error im Buffer (z.B. Title-Zeile löschen) → Reopen
      mit Error-Banner, kein PUT.

### CF-10 — Create Page

- [ ] `a` auf einer Space-Row → Editor mit `title:`-Header + leerem
      `<p></p>`-Body. Title setzen, Save → neue Top-Level-Page in
      diesem Space (Banner mit neuer ID); Reload zeigt sie unter
      dem Space.
- [ ] `a` auf einer Page-Row → analog, neue Kindseite unter dieser
      Page (Reload zeigt sie als Child).
- [ ] Title leer lassen → Reopen mit Parse-Error.

### CF-11 — Delete Page (Trash)

- [ ] `Shift+D` auf einer Page → Confirm-Popup
      `Delete '<title>'? y/n` (oder Enter/Esc).
- [ ] `y`/Enter → Page verschwindet aus Liste; Confluence-Web Trash
      enthält sie; Restore aus Web-UI funktioniert.
- [ ] `n`/Esc → Page bleibt; kein Request gefeuert.

### CF-12 — Comments CRUD

- [ ] `c` auf einer Page → leerer XHTML-Editor; Body eingeben, Save
      → neuer Comment erscheint im `comments`-Branch (Reload
      erzwingen via `r` auf der Page).
- [ ] `e` auf einem Comment → Buffer mit Body, modifizieren,
      speichern → Banner `Updated comment`; Body neu im Listing.
- [ ] **Comment-409**: parallel auf dem Web denselben Comment
      editieren → Reopen mit Error-Banner (kein 3-way merge — manuell
      neu schreiben + speichern).
- [ ] `Shift+D` auf einem Comment → generisches `ConfirmDeleteContentNode`
      Popup → Enter löscht; Comment verschwindet.

### CF-13 — Attachment-Upload

- [ ] `Shift+A` auf einer Page → FilePicker öffnet sich.
- [ ] Multi-Select: 2–3 kleine Test-Dateien wählen (invented data,
      keine echten Kunden-Files!) → Save → Banner
      `Uploaded N attachment(s) to page <Title>`.
- [ ] `attachments`-Branch der Page (nach `r`) listet alle
      hochgeladenen Files; `d` öffnet sie korrekt.
- [ ] Eine unlesbare Datei + eine lesbare auswählen → Error-Banner
      benennt explizit den fehlgeschlagenen Pfad; lesbare Datei
      trotzdem hochgeladen (`uploaded 1/2; failures: ...`).

### CF-14 — Clone Page

- [ ] `y` auf einer Page → Editor öffnet mit `title: <Original> (Clone)` + pretty-printed Body.
- [ ] Save ohne Änderung → neue Page unter demselben Parent
      (oder als Top-Level, wenn Quelle Top-Level war) im selben
      Space; Banner `Cloned page <orig> → <new> (id ...)`.
- [ ] Title-Suffix-Stacking: `y` auf der gerade geklonten Page →
      Title bleibt `<Original> (Clone)` (kein doppeltes Suffix).
- [ ] Body editieren vor Save → neue Page hat den editierten Body,
      nicht den Original-Body.
- [ ] Parse-Error (Title-Zeile löschen) → Reopen mit Banner; kein
      POST gefeuert.

### CF-Bugfix 2026-06-02 — Pages-in-Space rendern leer (tree_label-Alignment)

Symptom: eine `tree_label: name`-Space aufgeklappt → ~50 leere Zeilen unter der Space-
Zeile, obwohl der `/content/page`-Response volle `title`-Felder
liefert. Root-Cause: der Tree-Renderer (`content_view.rs::build_tree_data_rows`)
malt für jede Zeile nur dann die Label-Zelle, wenn die `tree_label`
des Zeilen-Levels mit einem `col.key` der **aktiven** Spaltenmenge
übereinstimmt — und zeigt für non-active-depth Zeilen sonst alle
Zellen leer an. Space-Level hatte `tree_label: name`, Page-Level
`tree_label: title` → keine Übereinstimmung → Page-Zeilen leer.

Fix: Page-Level in `confluence.yaml` (User-Config + Repo-Example) auf
`tree_label: name` umgestellt; Page-Column `key: title` → `key: name`
(Display-Header bleibt "Title" via `label:`). 115 Tests grün.

- [ ] Space-Whitelist aktiv mit zwei realen Keys (s. user-config),
      Cursor auf der ersten Space-Zeile lassen, `o` zum Expand →
      Pages erscheinen mit korrekten Titeln (nicht leer).
- [ ] Cursor auf eine Page-Zeile bewegen → active_depth=1, Header
      wechselt auf "Title | ID", andere Spaces zeigen leer (by design,
      siehe Konvention).
- [ ] Recursive: in eine Page mit Sub-Pages drillen → Sub-Pages
      ebenfalls korrekt benannt (kein Regress gegen die rekursive
      ChildDef).

### CF-Bugfix 2026-06-02 — Spaces zeigen alle Pages statt Top-Level

Symptom: eine Space aufgeklappt → ~50 Pages der gesamten Space, statt
nur der direkten Children des Space-Homepages wie im Confluence
"Tree browser" der Web-UI sichtbar. Root-Cause:
`/rest/api/space/{KEY}/content/page` liefert per Default `depth=all`,
also _jede_ Page der Space — nicht die Tree-Browser-Liste.

Fix: `/rest/api/space?expand=homepage` zieht jetzt die Homepage-Id
pro Space mit, `SpaceMeta::homepage_id` speichert sie. Beim Expand
einer Space ruft der Adapter `list_child_pages(homepage_id, ...)`
statt `list_top_pages(space_key, ...)`. Lookup-Pfad
(`get_by_id`-Synthesizer) fetcht `/space/{KEY}?expand=homepage`
lazy beim ersten `list()` via `OnceCell`. Wenn Confluence keine
Homepage exponiert (legacy/restricted), liefert die Listing-API
eine leere Page-Liste statt zurück auf alle-Pages-Fallback.

- [ ] Space-Whitelist aktiv, eine bekannte Space mit Tree-Browser-
      Seitenleiste im Web-Confluence öffnen → Anzahl + Titel der
      sichtbaren Pages dort gegen die Expand-Liste in der TUI
      vergleichen (sollten 1:1 matchen, Reihenfolge `position`).
- [ ] Reload (`r`) auf eine Space-Zeile → Pages erscheinen weiter
      korrekt (Cache-Pfad).
- [ ] Direkter Lookup-Pfad: nach `:focus-node` oder Cross-Tab-Link
      auf eine Space → erste Expand hängt nicht (OnceCell fetcht
      Homepage transparent), Pages-Liste matcht Web-UI.

### CF-Bugfix 2026-06-02 — Column-Reihenfolge: Tree-Spalte zuerst

YAML-Reorder in `spaces`-View: `name` (tree-label) jetzt vorne,
`key` und `type` trailing. Konvention für tree-mode Views: die
Tree-tragende Spalte gehört an Position 1, sonst kommt der
Tree-Indent erst nach den schmalen Trailing-Spalten.

- [ ] Spaces-Subtab zeigt links zuerst den Tree-Indent + Space-Name,
      rechts daneben `Key` und `Type`.

### CF-16 — Spaces-Whitelist (`space_keys`)

- [ ] `confluence-adapter.yaml` ohne `space_keys:` → spaces sub-tab
      listet wie früher alle lesbaren Spaces (Regression-Check).
- [ ] `space_keys: [BBB, AAA]` mit zwei realen Keys, die _nicht_
      alphabetisch sortiert die gewünschte UI-Reihenfolge sind →
      spaces sub-tab zeigt nur BBB und AAA, in genau dieser
      Reihenfolge (BBB zuerst).
- [ ] `space_keys: [GOOD, NOPE_TYPO]` → spaces sub-tab zeigt nur
      GOOD; kein Error-Banner, kein Crash. (silent-drop verifiziert)
- [ ] Mit `space_keys:` gesetzt: `r` (reload) im spaces sub-tab →
      keine Pagination-Affordanzen sichtbar (kein `has_next`),
      Listing zeigt nur die whitelist-Spaces.
- [ ] Drill-down in eine whitelisted Space → Pages/Attachments/
      Comments funktionieren wie ohne whitelist (Recursive ChildDef
      ist unbetroffen).

### CT-1..CT-11 — Tree-Find (spaces-tab `/`)

Voraussetzung: Spaces sub-tab geladen mit ≥2 Spaces, in denen
mehrere Pages liegen (mindestens eine mit ≥2 Verschachtelungstiefe).
Wenn `space_keys:` gesetzt: nur whitelisted Spaces dürfen auftauchen.

- [ ] `/` öffnet die Eingabezeile mit Prompt "Search pages" und
      `?`-Prefix. Tippen ändert die Anzeige nicht (kein
      Local-Filter).
- [ ] Enter mit nicht-trivialem Begriff → Toast
      `Tree find "q": N hits — n/N to navigate`. Status-Bar zeigt
      `n/N  Tree find "q": 1/N`.
- [ ] Tree expandiert automatisch bis zum ersten Hit, Cursor sitzt
      darauf.
- [ ] `n` springt zum nächsten Hit in Baum-Reihenfolge (gleicher
      Space zuerst, dann nächster YAML-Space-Eintrag); Vorfahren
      werden lazy nachgeladen wenn nötig (kurzes Flackern okay).
- [ ] `N` springt zurück (wrap-around am Anfang/Ende).
- [ ] Status-Bar-Counter aktualisiert sich bei jedem Sprung
      (`1/47`, `2/47`, …).
- [ ] Wenn der Server mehr Treffer hat als der Cap (100), zeigt der
      Counter `, truncated` an und das Toast meldet `truncated`.
- [ ] Esc auf der leeren Eingabezeile schließt sie ohne Cache.
- [ ] Esc auf gefüllter Eingabezeile vor Enter: löscht den Cache
      (n/N fällt zurück auf Local-/).
- [ ] `r` (reload) löscht den Tree-Find-Cache. Status-Bar-Hint
      verschwindet, n/N wieder Local-/.
- [ ] Erneutes `/` mit anderer Query: alter Cache weg, neuer Such-
      vorgang startet sauber (kein Mix von alten + neuen Hits).
- [ ] Mit `space_keys:` gesetzt: tree_find findet keine Pages aus
      whitelisted-out Spaces (Server filtert via `space in (...)`
      injection).
- [ ] Während ein Tree-Find aktiv ist, weiterhin manuelles
      Expandieren/Kollabieren in Spaces möglich; Cache überlebt
      diese UI-Interaktionen.
- [ ] `f` (lokaler fuzzy_filter) ist unverändert und filtert nur
      die bereits sichtbaren Space-Zeilen (nicht Pages).

### CT-12 — SpaceNode → top-level pages (statt Homepage-Kinder)

Voraussetzung: Spaces sub-tab mit ≥1 Space, dessen Homepage und/oder
Top-Level-Pages mehrere Verschachtelungsebenen haben.

- [ ] Drill in einen Space → Level 1 zeigt nicht mehr nur die
      Homepage-Kinder, sondern alle Top-Level-Pages des Space
      (inklusive der Homepage selbst).
- [ ] Tree-Find (`/`) auf eine Page, die ≥2 Ebenen unter der
      Homepage liegt → expandiert sauber bis zur Page (kein
      `Tree find: Hit's ancestor '…' at depth 1 not in loaded
children` mehr).
- [ ] Page mit `o` (open-in-browser) auf Homepage-Zeile öffnet die
      Homepage-URL.
- [ ] `a` auf Space-Zeile erstellt weiterhin eine top-level Page;
      `a` auf Homepage-Zeile erstellt ein Child unter der Homepage
      (= bisheriges Verhalten, unverändert).
- [ ] Spaces ohne erkennbare Top-Level-Pages (Edge-Case) liefern
      eine leere Liste statt eines Errors.

### CT-13 — Tree-Find Walker bleibt nach Hit "settled"

Voraussetzung: tree_find auf spaces aktiv (`type: tree_find` in
view-YAML). Vorher mindestens einmal `/` gesucht und auf einen Hit
gesprungen.

- [ ] Nach Treffer-Sprung den Cursor mit `j`/`k` woandershin bewegen,
      dann auf einem anderen, eingeklappten Knoten Enter drücken
      (lädt dessen Kinder) → Cursor bleibt auf dem geöffneten Knoten,
      springt NICHT zurück zum letzten Suchergebnis.
- [ ] `n`/`N` springt weiterhin zwischen den Hits hin und her (also
      settled wird durch next/prev korrekt zurückgesetzt).
- [ ] Neue Suche mit `/` (Search-Input erneut öffnen) verhält sich
      wie immer: erster Hit wird angesprungen.

### RD-1 — Render-Loop Dirty-Gating (CPU im Leerlauf)

Siehe `docs/decisions/0001-render-loop-dirty-gating.md`.

- [ ] App offen lassen, nichts tun (kein Key, kein Tracking aktiv):
      `top`/`htop` zeigt die CPU des Prozesses nahe 0 % (vorher: dauerhaft
      spürbare Last durch 60-fps-Repaint).
- [ ] Tippen/Navigieren fühlt sich unverändert direkt an — keine
      Eingabe-Latenz (Keys lösen sofort Redraw aus).
- [ ] Async-Last (z. B. großes Content-Listing laden, Taiga/Jira-Reload):
      Ergebnis erscheint praktisch sofort (≤ ~200 ms), Spinner/Banner
      aktualisieren sich flüssig.
- [ ] **Busy-Banner-Zähler läuft:** eine Query mit `query_timeout_secs`
      starten (Postgres) bzw. langsame Verbindung — der „…(Ns/…)"-Zähler
      im Banner zählt **ohne** weitere Eingabe sekündlich hoch (friert
      nicht ein).
- [ ] **Aktives Tracking:** ein Tracking starten → die Dauer-Spalte
      aktualisiert sich weiterhin (adaptives Intervall), App bleibt im
      Leerlauf trotzdem ruhig.
- [ ] Editor öffnen (`e`), `:w` (live-apply, falls aktiv), schließen →
      Rückkehr rendert sofort sauber; detached-Script (`x`) liefert nach
      Ende sein Ergebnis ohne Hänger.

### RD-2 — Render-Loop 1b (event-getriebener `select!`-Loop)

Siehe `docs/decisions/0001-render-loop-dirty-gating.md` (§1b). Ersetzt den
200-ms-Poll-Loop; Idle ist jetzt geparkt statt periodisch wach. **Fokus:
keine Regression an stdin-Übergabe und Zeitgetriebenem.**

- [ ] **Echtes Idle:** App offen, nichts tun → CPU 0 %. Mit `strace -fp
<pid> -e trace=poll,read` (oder `perf`) sieht man **keine**
      periodischen 200-ms-Wakeups mehr, solange kein Tracking/Banner/Editor
      lebt.
- [ ] **Key-Latenz:** Tippen/Navigieren reagiert sofort — kein Regress
      gegenüber 1a.
- [ ] **Async-Push weckt sofort:** großes Content-Listing laden → Ergebnis
      erscheint ohne wahrnehmbare Verzögerung (nicht mehr an 200-ms-Raster
      gebunden).
- [ ] **Inline-Editor stdin-Übergabe:** `e` → der Editor bekommt **jeden**
      Tastendruck (kein verschlucktes erstes Zeichen), Tippen flüssig;
      Schließen → TUI kehrt sauber zurück, erste Taste danach wirkt sofort.
- [ ] **Detached-/Launch-Editor + `:w` live-apply:** Editor (Launch-Modus)
      offen lassen, im Editor speichern → live-reload greift weiterhin;
      `.done`/Schließen → Commit läuft, Rückkehr rendert.
- [ ] **Validierungsfehler-Reopen:** einen Commit mit Fehler provozieren →
      Editor öffnet erneut mit Fehlerpuffer, stdin gehört wieder dem Editor
      (keine verschluckten Keys).
- [ ] **Interaktives Script (`x`):** Script übernimmt das Terminal voll,
      bekommt Eingaben; „Press any key" + Rückkehr funktionieren.
- [ ] **Busy-Banner-Sekunde** und **aktives Tracking** zählen wie unter
      RD-1 sekündlich/adaptiv hoch — und sobald sie enden, kehrt die App in
      echtes Idle zurück (Ticker disarmt).
- [ ] **Resize:** Terminal-Fenster vergrößern/verkleinern → sofortiger,
      korrekter Repaint (kein „erst bei nächster Taste").
- [ ] **Quit:** `q`/`:q` beendet prompt.

### Real-Data-Sweep

- [ ] Vor jedem Commit: `git diff --staged | grep -iE '<euer-firmenname>|<firmen-kurzform>|atlassian\.net|<echte JSESSIONID-Snippets>'`
      — muss leer sein. (Die Platzhalter `<…>` durch euren echten
      Firmennamen + Kurzform + interne Host-Muster ersetzen; diese
      Muster selbst gehören **nicht** ins Repo.) Beispiel-Hosts sind
      `wiki.example.invalid`, Beispiel-Cookies `JSESSIONID=synthetic`,
      Space-Keys `DEMO`.

## Stoat-Adapter (Phase 0 — Fundament)

Verbindungs-only: Login + Discovery + Gateway (WS) + Status-Spiegelung.
**Noch kein Baum** — `list()` liefert leer; Phase 1 füllt ihn. Manueller
Test gegen die private Test-Instanz (Credentials **außerhalb** des Repos,
`username`-Feld trägt die E-Mail). Voraussetzung: `stoat-adapter.yaml` +
`stoat.yaml` in `~/.config/not_yet_done/views/` (Vorlagen unter
`docs/examples/views/`), echte Basis-Domain eingetragen.

- [ ] **Discovery + Login:** Stoat-Tab öffnen → Banner `Connecting…`; falls
      Credentials nötig, erscheint das `NeedsCreds`-Formular (Feld
      `username` = E-Mail, `password`). Nach Submit läuft der Login durch.
- [ ] **Ready:** Nach erfolgreichem WS-`Authenticate`+`Ready` springt das
      Banner auf `Ready`. Der Baum ist leer (Phase 0) — das ist korrekt.
- [ ] **Falsche Credentials:** absichtlich falsches Passwort → Login
      schlägt fehl, Banner `Failed{reason}` mit lesbarer Meldung; erneuter
      Versuch (`r` / Credentials neu) möglich.
- [ ] **Token-Persistenz:** TUI beenden + neu starten → kein erneuter
      Credential-Prompt (Session-Token aus SQLite wiederverwendet), Banner
      geht direkt über `Connecting` nach `Ready`.
- [ ] **Heartbeat/Idle:** Tab offen lassen (kein Input) → Verbindung bleibt
      bestehen (Ping alle 20 s); keine Idle-CPU-Regression (vgl. RD-2 — der
      Gateway-Task schläft zwischen Pings).
- [ ] **Reconnect:** Netz kurz trennen (z. B. WLAN aus/an) → Banner fällt
      auf `Connecting…`, nach Wiederkehr automatisch zurück auf `Ready`
      (Backoff ≤ 30 s).
- [ ] **MFA-Konto:** (falls verfügbar) Login mit MFA-Konto → klare
      „MFA not supported"-Fehlermeldung statt Hänger.
- [ ] **Sauberes Beenden:** `:q` beendet prompt; der Gateway-Task wird beim
      Adapter-Drop abgebrochen (kein hängender Prozess/Socket).
- [ ] **Real-Data-Sweep:** keine echte Instanz-Domain/E-Mail/Token im Repo
      (Vorlagen nutzen `chat.example.org`).

## Stoat-Adapter (Phase 1 — Read-only Baum)

Browsen + Lesen. Baut auf Phase 0 auf (Login/Gateway/Ready müssen grün
sein). Struktur (Server/Channels) kommt aus dem WS-`Ready`-Snapshot,
Message-Bodies per REST-Pull. **Kein Live-Push** — nach Connect ggf. `r`
drücken, damit der Baum den frisch eingetroffenen `Ready`-Stand zeigt.

- [ ] **Server-Liste:** Nach `Ready` (ggf. `r`) listet der `chats`-View die
      Server. Leerer Baum direkt nach Login = `Ready` kam noch nicht / `r`
      drücken.
- [ ] **Channels:** In einen Server drillen (`Enter`/`c`) → Text-Channels in
      der Server-Reihenfolge. Voice-Channels erscheinen, sind aber nicht
      aufklappbar (kein Inhalt).
- [ ] **Messages:** In einen Text-Channel drillen → die letzten ≤ 50
      Nachrichten, **neueste unten**. Spalten: Author, Time, Message.
- [ ] **Autor-Auflösung:** Author-Spalte zeigt Benutzernamen (nicht die
      rohe ID) — auch für Autoren, die nicht im `Ready`-Snapshot waren
      (kommt über `include_users`).
- [ ] **Zeitstempel:** Time-Spalte zeigt Datum/Uhrzeit (aus der Message-
      ULID dekodiert), plausibel aufsteigend nach unten.
- [ ] **Preview:** Auf einer Message `p` → Preview-Pane zeigt den vollen
      Message-Body (mehrzeilig korrekt; Tabellen-Zeile bleibt einzeilig).
- [ ] **DMs (optional):** Eine zweite View mit Top-Level `stoat:channel`
      (auskommentiertes Beispiel in `stoat.yaml`) listet Direkt-/Gruppen-
      Nachrichten statt Server.
- [ ] **Phase-1-Grenze bewusst:** sehr lange Channels zeigen nur die
      neuesten ~50 Messages; älteres Backfill kommt später (Cursor-
      Pagination). Kein Bug.
- [ ] **Real-Data-Sweep:** keine echten Channel-/Message-/User-Daten im
      Repo (Tests/Fixtures nutzen erfundene IDs).

## Stoat-Adapter (Phase 2 — Live-Layer)

Out-of-band-Updates ohne manuelles `r`. Baut auf Phase 1 auf. Braucht
einen zweiten Client (Web/Mobile) oder einen Helfer, um Nachrichten in
einen Channel zu schicken, während die TUI offen ist.

- [ ] **Auto-Populate:** Tab frisch öffnen. Sobald der Banner `Ready`
      erreicht, füllt sich der Server-Baum **von selbst** — **ohne** `r`.
      (Phase 1 brauchte hier noch ein manuelles Reload.)
- [ ] **Live-Message:** In der TUI einen Text-Channel offen halten
      (Message-Level). Von außen eine Nachricht in **genau diesen** Channel
      posten → sie erscheint innerhalb ~1 s unten, ohne Tastendruck.
- [ ] **Kein Fremd-Reload:** Channel A offen halten, Nachricht in Channel B
      posten → A lädt **nicht** neu (kein Flackern/Cursor-Sprung). Nur das
      offene Channel-Level reagiert.
- [ ] **Edit/Delete/Reaction:** Eine sichtbare Nachricht von außen
      editieren / löschen / mit einer Reaction versehen → das offene
      Channel-Level spiegelt die Änderung nach Reload.
- [ ] **Reconnect-Resync:** Netz kurz trennen (oder Gateway-Disconnect
      provozieren) → Banner `Connecting`, dann `Ready`; danach zeigt der
      Baum wieder den aktuellen Stand, ohne `r`.
- [ ] **Cursor-Reset bekannt:** Beim Live-Reload springt der Cursor auf
      Standard (kein „an der Leseposition bleiben") — erwartetes
      Phase-2-Verhalten, kein Bug.
- [ ] **Strukturgrenze bewusst:** Ein **neu angelegter/umbenannter** Channel
      erscheint erst nach einem Reconnect (frisches `Ready`), nicht sofort —
      strukturelle Live-Events sind noch nicht verdrahtet. Kein Bug.
- [ ] **Idle-CPU:** Tab offen, keine Aktivität → CPU bleibt bei ~0 %
      (Render-Loop 1b parkt; Invalidations wecken nur bei echtem Event).

## Stoat-Adapter (Phase 2.1 — Kategorien + Tree)

Die flache Drill-Down-View ist durch eine Tree-View ersetzt:
`Server → (Kategorie | uncategorized Channel) → Channel → Messages`.

- [ ] **Tree statt flach:** Tab öffnen → Server stehen als Tree-Wurzeln da
      (Indent/Glyph). Enter/→ auf einem Server expandiert **inline** zu
      seinen Kategorien **und** uncategorized Channels (kein Tab-Wechsel).
- [ ] **Reihenfolge:** Unter einem Server kommen die **uncategorized
      Channels zuerst**, danach die Kategorien.
- [ ] **Kategorie expandieren:** Enter/→ auf einer Kategorie zeigt ihre
      Channels inline eine Ebene tiefer.
- [ ] **Channel drillt in Messages:** Enter auf einem Channel (egal ob
      unter Kategorie oder direkt unter Server) öffnet die **flache
      Message-Liste**; Back/← kehrt in den Baum an dieselbe Stelle zurück.
- [ ] **Vollständigkeit:** Summe aus „uncategorized + alle Kategorie-
      Channels" deckt alle Channels des Servers ab — nichts fehlt, nichts
      doppelt.
- [ ] **Server ohne Kategorien:** Ein Server, der keine Kategorien hat,
      zeigt alle seine Channels direkt als uncategorized (kein leerer
      Kategorie-Zweig).
- [ ] **Voice-Channel bleibt Leaf:** Voice-Channels erscheinen, sind aber
      nicht drill-/expandierbar.
- [ ] **`/` tree find:** Auf der Server-Wurzel `/` → durchsucht Channels
      quer durch den Baum (eingeklappte Knoten inklusive).
- [ ] **Live im Tree:** Eine Message in einem **drillgeöffneten** Channel
      von außen posten → die Message-Liste aktualisiert sich live (Phase-2-
      Verhalten gilt unverändert, egal wo der Channel im Baum hängt).

## Stoat-Adapter (Phase 3 — Write)

In einen Channel drillen (flache Message-Liste). Vier Aktionen:
`a` send, `e` edit, `d` delete, `+` react. Endpoints sind vorab per `curl`
gegen die echte Instanz verifiziert (gegen `SavedMessages`).

- [ ] **Senden (`a`):** In der Message-Liste `a` → leerer `$EDITOR`
      (Markdown). Text tippen, speichern/schließen → Status „Message sent";
      die neue Nachricht erscheint unten in der Liste (Reload **und** Live-
      Event). Leerer Buffer → kein Versand, keine Fehlermeldung.
- [ ] **Senden bricht ab:** `a`, Editor **ohne** Änderung schließen →
      nichts wird gesendet.
- [ ] **Markdown bleibt:** Eine Nachricht senden, die mit `#` beginnt
      (z.B. `# Überschrift`) → wird **wörtlich** gesendet (kein Header wird
      gefressen).
- [ ] **Editieren (`e`):** Eigene Nachricht auswählen, `e` → Editor zeigt
      den **rohen Body** (kein `#`-Header). Body ändern, speichern → Status
      „Message edited"; die Zeile zeigt den neuen Text + „Edited"-Flag.
      Unverändert speichern → „No changes", kein Roundtrip.
- [ ] **Editieren fremd:** Fremde Nachricht `e`, Body ändern, speichern →
      sauberer Server-Fehler (403), kein Crash.
- [ ] **Löschen (`d`):** Eigene Nachricht `d` → Bestätigung, dann weg;
      Liste aktualisiert sich.
- [ ] **Reagieren (`+`):** Nachricht `+` → Emoji-Picker (👍 ❤️ 😂 …),
      eines wählen → Status „Reacted 👍"; Reaktion erscheint in einem
      zweiten Client / nach Reload im Web-UI.
- [ ] **Live-Echo:** Senden/Editieren/Löschen löst zusätzlich das Live-
      Event aus → die Liste ist auch in einem zweiten geöffneten Pane
      aktuell.

## Stoat-Adapter (Phase 4 — Strukturelle Live-Events)

Strukturänderungen werden live, ohne Reconnect. Auslösen im **offiziellen
Stoat/Revolt-Client** (oder seit dem Create-Feature direkt in der TUI, siehe
nächster Abschnitt), in einem Server, den du administrierst — die TUI mit
dem Stoat-Tab offen und auf dem Server-Baum (oder in einem Channel) daneben
halten. Wire-Shapes vorab per WS-Capture gegen die echte 0.13.7-Instanz
verifiziert.

- [ ] **Channel anlegen:** Im Web-Client einen Text-Channel anlegen → er
      erscheint **ohne** `r` im TUI-Baum unter dem Server (uncategorized).
- [ ] **Channel umbenennen:** Channel im Web-Client umbenennen → der neue
      Name erscheint live im Baum.
- [ ] **Channel löschen:** Channel im Web-Client löschen → verschwindet
      live aus dem Baum (und aus seiner Kategorie, falls zugeordnet).
- [ ] **Kategorie anlegen:** Kategorie im Web-Client anlegen → erscheint
      live als eigener Branch unter dem Server.
- [ ] **Kategorie umbenennen:** → neuer Titel live im Baum.
- [ ] **Channel in Kategorie ziehen:** Channel einer Kategorie zuordnen →
      wandert live aus „uncategorized" in den Kategorie-Branch.
- [ ] **Kategorie löschen:** → Branch verschwindet live; die enthaltenen
      Channels rutschen zurück nach „uncategorized" (bzw. wohin der Server
      sie umhängt).
- [ ] **Server umbenennen:** → Server-Label aktualisiert sich live.
- [ ] **Cursor-Verhalten:** Eine Strukturänderung setzt den Pane-Cursor
      zurück (bekannte Reload-Grenze, kein Bug) — Baum bleibt konsistent.
- [ ] **Server beitreten/verlassen (Negativ-Check):** Erscheint/verschwindet
      **erst nach Reconnect** — bewusst nicht live (Phase-4-Scope-Grenze).

## Stoat-Adapter — Channel/Kategorie anlegen (`al` / `ay`)

Anlegen direkt aus der TUI. Voraussetzung: ein Server, den du
administrierst, und die `al`/`ay`-Actions in `stoat.yaml` (Server-View +
`categories`-ChildDef). `al`/`ay` sind **Mehrzeichen-Chords** — sie liegen
nicht in `keybindings.content`, sondern werden generisch über die
View-Keymap erkannt (`ContentView::yaml_action_chord_prefix`); das `a`
wird als Chord-Präfix gestasht, das zweite Zeichen löst aus.

- [ ] **Channel unter Server:** Cursor auf die **Server-Zeile**, `al` tippen
      → Namens-Formular (ein Feld) öffnet. Name eingeben, bestätigen →
      Channel erscheint live unter „uncategorized" (kein `r` nötig; der
      Gateway echot `ChannelCreate`).
- [ ] **Kategorie unter Server:** Cursor auf die **Server-Zeile**, `ay`
      tippen → Formular → Name → Kategorie erscheint live als eigener
      Branch (`ServerUpdate` mit voller Kategorie-Liste).
- [ ] **Channel unter Kategorie:** Cursor auf eine **Kategorie-Zeile**, `al`
      tippen → Formular → Name → Channel erscheint live **im
      Kategorie-Branch**. (Zweistufig intern: Channel anlegen + Server-PATCH
      — für den Nutzer ein Schritt.)
- [ ] **Leerer Name:** Formular mit leerem/Whitespace-Namen bestätigen →
      klare Fehlermeldung, kein namenloser Channel/Kategorie entsteht.
- [ ] **`ay` auf Kategorie-Zeile:** nicht gebunden (Kategorien gibt es nur
      unter dem Server) → `a` wird zwar als Präfix gestasht, `ay` löst
      nichts aus und fällt sauber durch (kein Hänger, kein Fehler).
- [ ] **Chord-Abbruch:** `a` tippen, dann `esc`/eine nicht-passende Taste →
      kein Effekt, normale Bedienung läuft weiter.

## Stoat-Adapter — Channel/Kategorie umbenennen (`R`)

Umbenennen aus der TUI. Voraussetzung: ein Server, den du administrierst,
und die `R`-Action in `stoat.yaml` (beide `channels`-ChildDefs +
`categories`-ChildDef). `R` ist ein **Einzelzeichen** (kein Chord) und
öffnet — wie `al`/`ay` — ein Namens-Formular mit einem Feld, vorgefüllt
ist es nicht.

- [ ] **Channel unter Server:** Cursor auf eine **uncategorized Channel-Zeile**,
      `R` tippen → Formular → neuer Name → Channel-Zeile aktualisiert sich
      live (kein `r` nötig; der Gateway echot `ChannelUpdate`).
- [ ] **Channel unter Kategorie:** Cursor auf eine Channel-Zeile **im
      Kategorie-Branch**, `R` → Formular → neuer Name → Zeile aktualisiert
      sich live (gleiche Action, `PATCH /channels/{id}`).
- [ ] **Kategorie:** Cursor auf eine **Kategorie-Zeile**, `R` → Formular →
      neuer Name → Kategorie-Header aktualisiert sich live (`ServerUpdate`
      mit voller Kategorie-Liste; nur der Titel der Zielkategorie ändert
      sich, Channel-Zuordnungen bleiben).
- [ ] **Leerer Name:** Formular mit leerem/Whitespace-Namen bestätigen →
      klare Fehlermeldung, kein Umbenennen findet statt.
- [ ] **Fremder Server (kein Admin):** `R` auf einem Channel/einer Kategorie
      ohne Rechte → der Server lehnt mit sauberer Fehlermeldung ab, der Baum
      bleibt unverändert.

## Stoat-Adapter — Channel cut/paste (`C` / `P`)

Channels zwischen Kategorien verschieben. `C` (cut) **markiert** den Channel
unter dem Cursor — es wird **nichts gelöscht**; erst `P` (paste) hängt ihn um.
Reuse der generischen `mark-move`/`paste-move`-Shortcuts (wie Tasks `m`/`p`),
über `invoke_action` + `ActionContext.marked`. Voraussetzung: ein Server, den
du administrierst. Der Move ist intern ein Voll-Listen-PATCH der Server-
Kategorien (`update_server_categories`), live via `ServerUpdate`.

- [ ] **Channel in Kategorie:** Cursor auf eine Channel-Zeile, `C` (Status:
      „Marked … for move") → Cursor auf eine **Kategorie-Zeile**, `P` → Channel
      wandert live in diese Kategorie, verschwindet aus der alten Stelle.
- [ ] **Channel → uncategorized:** Channel `C`, dann Cursor auf die
      **Server-Zeile**, `P` → Channel landet im uncategorized-Branch.
- [ ] **Paste neben Channel:** Channel A `C`, dann Cursor auf Channel B (in
      einer anderen Kategorie), `P` → A landet in B's Kategorie (bzw.
      uncategorized, wenn B uncategorized ist).
- [ ] **Abbruch per zweimal `C`:** Channel `C`, dann auf derselben Zeile noch
      einmal `C` → „Cut cancelled", kein Move bei späterem `P`.
- [ ] **Abbruch per Tab-Wechsel:** Channel `C`, Tab wechseln (z. B. `1`) →
      „Cut cancelled"; zurück auf Stoat, `P` auf einer Kategorie → nichts
      passiert (kein hängender Cut).
- [ ] **`C` löscht nie:** nach `C` ist der Channel unverändert sichtbar; nur
      `P` verändert den Baum.
- [ ] **`cut` in der oberen Leiste + Highlight:** Der `C cut`-Hint steht in der
      **oberen Action-Bar** (nicht in der Status-Leiste). Nach `C` wird er in der
      Akzentfarbe (fett + unterstrichen) hervorgehoben, solange ein Cut armiert
      ist; nach `P` oder Abbruch (zweimal `C` / Tab-Wechsel) erlischt das
      Highlight wieder.
- [ ] **Fremder Server:** Channel von Server A `C`, dann `P` auf eine
      Kategorie/Server B → saubere Fehlermeldung („different server"), kein
      Move (Kategorien sind serverlokal).

## Stoat-Adapter — Chat-Layout (`row_layout`)

Die Message-Liste rendert per `row_layout` als Chat: Meta-Zeile + Body +
Spacer. In einen Channel drillen (split öffnet die Liste rechts).

- [ ] **Drei Zeilen je Nachricht:** Jede Nachricht belegt 3 Terminalzeilen —
      Zeile 1 `author  time`, Zeile 2 der Nachrichtentext, Zeile 3 leer.
- [ ] **Hervorhebung:** Author in Akzentfarbe, Time gedimmt (kommt aus
      `style: accent` / `style: text_dim`, über `tui.yaml` überschreibbar).
- [ ] **Kein Spaltenkopf:** Im Chat-Layout wird die `Author | Time | Message`-
      Kopfzeile **nicht** angezeigt.
- [ ] **Selektion:** Mit `j`/`k` navigieren → Auswahl-Hintergrund deckt die
      Meta- und die Body-Zeile ab, **nicht** die Leerzeile dazwischen.
- [ ] **Scrollen:** Liste mit mehr Nachrichten als Bildschirmhöhe → `j` ans
      Ende scrollt sauber Block für Block; ausgewählte Nachricht bleibt
      vollständig sichtbar (nicht halb abgeschnitten).
- [ ] **Aktionen unverändert:** `e`/`d`/`+`/`p`/`n` wirken weiter auf die
      ausgewählte Nachricht (die ganze Block-Auswahl, nicht einzelne Zeilen).
- [ ] **Andere Tabs unberührt:** Jira/Taiga/Postgres-Tabs rendern weiter als
      normale einzeilige Tabellen (kein `row_layout` → altes Verhalten).

## Stoat-Adapter — Markdown-Body (`markdown: true`)

Die Content-Spalte der Messages ist `source: content` + `markdown: true`, der
Body wird mehrzeilig und soft-gewrappt gerendert (`ratatui-markdown`). In einen
Channel drillen.

- [ ] **Alle Zeilen sichtbar:** Eine Nachricht mit mehreren harten
      Zeilenumbrüchen zeigt **jede** Zeile, nicht zu einer Zeile kollabiert.
- [ ] **Soft-Wrap:** Ein langer Absatz bricht am Pane-Rand um; Pane schmaler
      ziehen → derselbe Absatz reflowt über mehr Zeilen (Row-Höhe wächst mit).
- [ ] **Inline-Styling:** `**fett**`, `*kursiv*`, `` `code` `` erscheinen
      hervorgehoben; eine `# Überschrift` und `- Listen` werden als solche
      gerendert.
- [ ] **Farben aus Theme:** Body-Text/Headings/Emphasis ziehen ihre Farben aus
      `tui.yaml` (Theme-Bridge) — kein Hardcode; Theme wechseln verändert sie.
- [ ] **Selektion = nur Hintergrund:** Auswahl der Nachricht legt den
      Auswahl-Hintergrund über Meta + alle Body-Zeilen, **ohne** die
      Vordergrundfarben (author=accent, Body-Styling) plattzumachen.
- [ ] **Leerer Body:** Eine Nachricht ohne Text (z. B. nur Attachment) bricht
      das Layout nicht — die Body-Zeile bleibt leer, Spacer intakt.
- [ ] **Scrollen mit hohen Rows:** Sehr lange Nachrichten (viele Body-Zeilen)
      scrollen sauber; ausgewählte Nachricht bleibt vollständig sichtbar.
- [ ] **Andere Tabs unberührt:** Spalten ohne `markdown:` (Jira/Taiga/Postgres)
      rendern weiter einzeilig.

> Bekannte Schnitte: `/`-Suche markiert Treffer **nicht** im gerenderten Body;
> Code-Blöcke ohne Hintergrund; Syntax-Highlighting ist optional/separat.

## Stoat-Adapter — Smooth-Scroll (`smooth_scroll: true`)

Beide `messages`-Level haben `smooth_scroll: true`. In einen Channel mit vielen
(idealerweise mehrzeiligen) Nachrichten drillen.

- [ ] **Zeilenweise statt sprunghaft:** ↓ scrollt den Inhalt um **eine
      physische Zeile** nach oben — eine hohe Nachricht oben wird dabei
      sukzessive angeschnitten, nicht als ganzer Block weggeschoben.
- [ ] **Kein Snapping:** Der Highlight gleitet mit dem Inhalt mit (darf am Rand
      angeschnitten sein) und springt **nicht** bei jedem Schritt an den oberen
      Rand.
- [ ] **Übergabe, sobald die nächste Nachricht sichtbar ist:** Ist die nächste
      Nachricht bereits auf dem Schirm, rückt der Highlight bei `j` sofort um
      **genau eine** Nachricht weiter (nicht an den unteren Rand). Beim
      Hochscrollen analog nach oben.
- [ ] **Lange Nachricht:** Bei einer Nachricht, die höher ist als der
      verbleibende Platz, bleibt der Highlight stehen und `j` scrollt nur —
      bis die erste Zeile der nächsten Nachricht unten auftaucht.
- [ ] **Kein unsichtbarer Cursor:** Beim Hochscrollen springt der Highlight
      nicht schon dann auf die vorige Nachricht, wenn nur deren Leerzeile
      sichtbar ist — der Cursor ist bei jedem Schritt zu sehen.
- [ ] **Aktionen treffen die hervorgehobene Nachricht:** `e`/`d`/`+`/`p`
      operieren auf genau der aktuell hervorgehobenen Nachricht (auch wenn sie
      gerade angeschnitten am Rand sitzt).
- [ ] **Halbe/ganze Seite + g/G:** `Ctrl+d`/`Ctrl+u` scrollen um eine halbe
      Pane-Höhe (in Zeilen); `G`/`g` springen ans Ende/an den Anfang und
      wählen dabei explizit die letzte/erste Nachricht.
- [ ] **Bottom-Clamp:** Am Ende lässt sich nicht über die letzte Zeile hinaus
      scrollen; die letzte Nachricht bleibt unten bündig stehen.
- [ ] **Reload/Live-Event:** Kommt eine neue Nachricht an oder wird `r`
      gedrückt, bleibt die Scroll-Position sinnvoll (kein Sprung an den Anfang).
- [ ] **Andere Tabs unberührt:** Tabs ohne `smooth_scroll` (Jira/Taiga/Tasks)
      scrollen weiterhin diskret Eintrag-für-Eintrag.

## Stoat-Adapter — @-Mentions (`@username` + Slug-Roundtrip)

Revolt kodiert Erwähnungen im Body als `<@USERID>`. Anzeige und Editieren
lösen das wie bei Jira/Taiga über `not_yet_done_content::slug::SlugTable`:
Anzeige → `@username`, Editor → `@uu-slug` + CACHE-Section, beim Speichern
zurück nach `<@ID>`. Die Completion-Liste ist **server-scoped** (`GET
/api/servers/{id}/members`, einmal pro Server gecacht). Voraussetzung: einen
Server-Channel öffnen, in dem Nachrichten mit Erwähnungen existieren.

- [ ] **Anzeige `@username`:** Eine Nachricht, die jemanden erwähnt, zeigt in
      der Liste **`@Benutzername`**, nicht den rohen `<@01ABC…>`-Code (Label-
      Zeile **und** Markdown-Body).
- [ ] **Unbekannte ID bleibt roh:** Erwähnung eines Users, der **nicht** im
      Server ist (kein Cache-Eintrag) → `<@ID>` bleibt wörtlich stehen, kein
      Crash.
- [ ] **Edit zeigt Slugs + CACHE:** Eigene Nachricht mit Erwähnung `e` →
      Editor zeigt `@uu-<name>` statt `<@ID>`, und unten die CACHE-Section
      `#### CACHE / available @mentions … ####` mit allen `@uu-…` des Servers.
- [ ] **Slug-Roundtrip (No-op):** Im Editor **nichts** ändern, speichern →
      „No changes" (die `@uu-…` werden korrekt zurück nach `<@ID>` übersetzt,
      Body bleibt identisch — kein versehentlicher Edit).
- [ ] **Neue Erwähnung einfügen:** Im Editor einen `@uu-<name>` aus der CACHE-
      Section in den Text kopieren, speichern → der Erwähnte wird im Web-Client
      tatsächlich benachrichtigt; die TUI-Zeile zeigt danach `@<name>`.
- [ ] **Unbekannter Slug → Fehler:** Im Editor `@uu-quatsch` (nicht im CACHE)
      eintippen, speichern → sauberer Fehler „unknown mention slug @uu-quatsch",
      kein Versand, kein Crash.
- [ ] **Senden (`a`) mit Mention:** In der Channel-Liste `a` → leerer Buffer +
      CACHE-Section; `@uu-<name>` einfügen, Text senden → Erwähnung kommt im
      Web-Client an, TUI zeigt `@<name>`.
- [ ] **Server-Scoping:** In zwei verschiedenen Servern hat die CACHE-Section
      **unterschiedliche** Mitgliederlisten (nur Mitglieder des jeweiligen
      Servers).
- [ ] **DM/Gruppe:** In einem Direkt-/Gruppen-Channel (kein Server) speist sich
      die Completion-Liste aus den Recipients (Ready-Snapshot), nicht aus dem
      Members-Endpoint.

> Bekannte Schnitte: Member-Liste wird einmal pro Server pro Session gecacht
> (kein Live-Refresh bei Beitritt/Austritt); Slug-Source ist der Username
> (Server-Nickname noch nicht berücksichtigt).

## Stoat-Adapter — Typ-Symbole im Tree (`icon:`) + leerer Channel

Zwei Dinge, die zusammen aufgefallen sind: unkategorisierte Channels und
Kategorien teilen sich die Server-Ebene und sahen identisch aus; und ein
Channel, dessen letzte Nachricht gelöscht wurde, behielt Aufklapp-Pfeil und
Ungelesen-Marker (Stoat räumt `last_message_id` nicht auf). `icon:` steht pro
Ebene in `stoat.yaml` (💬 Channel, 📁 Kategorie), der Ungelesen-Marker ist
deshalb auf `🔔` umgestellt.

- [ ] **Server aufklappen:** Unkategorisierte Channels tragen `💬`, Kategorien
      `📁` — auf einen Blick unterscheidbar, gleiche Einrückung wie vorher.
- [ ] **Channel in Kategorie:** trägt dasselbe `💬` wie im uncategorized-Zweig.
- [ ] **Ungelesen:** Ein Channel mit neuen Nachrichten zeigt `🔔 💬 name`
      (Marker zuerst, dann Typ-Symbol), beides in der Ungelesen-Farbe; nach
      dem Lesen bleibt nur `💬 name`.
- [ ] **Fuzzy-Suche:** `/` + Teilstring → der Treffer-Highlight sitzt weiterhin
      exakt auf dem gematchten Teil des **Labels** (nicht um die Glyph-Breite
      verschoben).
- [ ] **Leerer Channel heilt sich:** Einen Channel öffnen, dessen letzte
      Nachricht gelöscht wurde → die Liste ist leer, und im Baum verschwindet
      danach **ohne** `r` der Aufklapp-Pfeil samt Ungelesen-Marker (die
      Nachrichten-Abfrage korrigiert `last_message_id`).
- [ ] **Gegenprobe:** Ein Channel mit echten Nachrichten behält Pfeil und
      Liste; eine neue Nachricht bringt Pfeil/Marker sofort zurück.

## Stoat-Adapter — Ungelesen-Marker in der Tab-Leiste

Der Ungelesen-Zustand endet bisher am Rand der View: ein Stoat-Tab im
Hintergrund sah aus wie jeder andere. Jetzt trägt das Tab-Label selbst den
Marker (`tab.unread_marker`, Default = `unread_marker` der View) und wird
hervorgehoben (`tab.unread_style`, Default fett). Gezählt wird nur, was eine
Pane **gerade** hält.

- [ ] **Marker erscheint:** In einem anderen Tab stehen, während im Stoat-Baum
      eine neue Nachricht eintrifft → das Tab wird zu `🔔 💬 9 Stoat`, fett,
      ohne Tastendruck.
- [ ] **Marker verschwindet:** Channel öffnen und bis zur neuesten Nachricht
      scrollen (Ack) → Marker und Fettschrift gehen im selben Moment weg, das
      Label ist wieder `💬 9 Stoat`.
- [ ] **Andere Tabs unberührt:** Jira/Tasks/… zeigen nie einen Marker (ihre
      Zeilen tragen kein `unread`-Feld).
- [ ] **Breite:** Die Leiste rutscht beim Auftauchen des Markers um die
      Emoji-Breite weiter, ohne Zeichenmüll; bei schmalem Terminal darf sie in
      die zweite Zeile umbrechen.
- [ ] **Konfiguration:** In `stoat.yaml` `tab.unread_style: [italic]` setzen →
      Label kursiv statt fett; `tab.unread_marker: ""` → nur die Hervorhebung,
      kein Glyph.

## Stoat-Adapter — Cursor auf der ersten ungelesenen Nachricht (`cursor_on_open`)

Beim Öffnen eines Channels stand der Cursor auf der **ältesten** geladenen
Nachricht — man musste sich erst zu der Stelle runterscrollen, an der man
aufgehört hatte. `cursor_on_open: first_unread` auf der `messages`-Ebene setzt
ihn stattdessen auf die erste ungelesene Nachricht, oben am Rand verankert, so
dass die ungelesenen darunter stehen.

- [ ] **Sprung:** Channel mit mehreren ungelesenen Nachrichten öffnen → der
      Cursor steht auf der **ältesten ungelesenen**, und zwar oben im Pane; die
      ungelesenen Nachrichten stehen darunter, der gelesene Verlauf ist
      weggescrollt.
- [ ] **Alles gelesen:** Channel ohne Ungelesenes öffnen → Cursor auf der
      **neuesten** Nachricht (unten), nicht oben.
- [ ] **Leerer Channel:** Channel ohne Nachrichten öffnen → kein Sprung, kein
      Panic; trifft danach die erste Nachricht ein, landet der Cursor auf ihr.
- [ ] **Reload verschiebt nicht:** Im Channel nach oben scrollen, dann `r`
      drücken bzw. eine Nachricht von außen schicken → der Cursor bleibt, wo er
      war (der Sprung gehört zum Öffnen, nicht zum Laden).
- [ ] **Zusammenspiel mit dem Ack:** Vom Sprungziel bis zur letzten Nachricht
      durchscrollen → `mark_read_on_reach_end` greift wie gehabt, Marker in Baum
      und Tab-Leiste gehen weg.
- [ ] **Ack springt nicht weg:** Eine gelesene Nachricht anwählen (der Ack löst
      einen Reload aus) → der Cursor bleibt auf **derselben** Nachricht, auch
      wenn zwischenzeitlich eine neue eingetroffen ist und alle Zeilen um eine
      Position hochgerutscht sind.
- [ ] **Gelöschte Zeile:** Die angewählte Nachricht von außen löschen, dann
      reloaden → kein Panic, der Cursor bleibt an derselben Stelle im Pane.
- [ ] **Zweiter Zweig:** Beides auch für einen Channel **in einer Kategorie**
      prüfen (die `messages`-Ebene ist in `stoat.yaml` zweimal konfiguriert).

## Stoat-Adapter — Dateien hochladen (`A` / `attach`)

`A` auf einer Channel-Zeile öffnet den File-Picker (Mehrfachauswahl), lädt jede
Datei hoch und postet sie als Nachricht mit leerem Body. Revolt erlaubt 5
Dateien pro Nachricht, größere Auswahlen werden auf mehrere Nachrichten
verteilt. Für die Tests reichen ein paar kleine Wegwerf-Dateien aus `/tmp`.

- [ ] **Eine Datei:** `A` auf einem Channel → Picker → eine Datei wählen →
      Notification „Attached 1 file(s) to #channel", die Nachricht taucht live
      in der offenen Liste auf.
- [ ] **Mehrfachauswahl:** 3 Dateien auf einmal → **eine** Nachricht mit allen
      dreien.
- [ ] **Über dem Limit:** 6+ Dateien → **mehrere** Nachrichten (5 + Rest), keine
      Fehlermeldung vom Server.
- [ ] **Picker abgebrochen:** Picker ohne Auswahl schließen → nichts passiert,
      keine leere Nachricht.
- [ ] **Ungelesen bleibt sauber:** Nach dem Upload ist der Channel **nicht**
      als ungelesen markiert (der Adapter ackt den eigenen Post).
- [ ] **Caption nachtragen:** `e` auf der geposteten Nachricht → Text ergänzen →
      `:w` → Text steht über/bei den Dateien, die Anhänge bleiben erhalten.
- [ ] **Beide Zweige:** Auch für einen Channel **in einer Kategorie** prüfen.
- [ ] **Kaputter Pfad:** Eine Datei ohne Leserecht mitauswählen → die anderen
      werden trotzdem gepostet, die Meldung nennt die fehlgeschlagene.

## Stoat-Adapter — Anhänge als Knoten (`stoat:attachment`)

Jede hochgeladene Datei ist auch ein Knoten unter ihrer Nachricht. Enter auf
einer Nachricht **mit** Anhängen drillt in die Dateiliste.

- [ ] **Drilldown:** Enter auf einer Nachricht mit Anhängen → Liste mit
      Dateiname, Größe und Typ.
- [ ] **Ohne Anhänge:** Enter auf einer normalen Nachricht → kein Drilldown,
      keine leere Liste (die Nachricht meldet `has_children` nur mit Dateien).
- [ ] **`o` öffnet:** Datei anwählen, `o` → OS-Viewer geht auf. Alle Dateien der
      Nachricht liegen im selben Temp-Verzeichnis, der Viewer kann durchblättern.
- [ ] **Zweites `o` lädt nicht neu:** Viewer schließen, `o` erneut → geht sofort
      auf (die Bytes liegen schon im Temp-Verzeichnis).
- [ ] **`D` speichert alles:** `D` → Zielverzeichnis eingeben (auch mit `~`, auch
      ein noch nicht existierendes) → **alle** Dateien der Nachricht liegen dort.
- [ ] **Zielverzeichnis ist eine Datei:** Pfad einer existierenden Datei angeben
      → klare Fehlermeldung, kein Teil-Download.
- [ ] **Kein Löschen:** Es gibt bewusst keine Delete-Action auf dieser Ebene
      (Revolt kann einzelne Dateien nicht aus einer Nachricht entfernen).
- [ ] **Beide Zweige:** Auch unter einem Channel **in einer Kategorie** prüfen.

## Inline-Bilder im Terminal (`images:` in `tui.yaml`)

Eine Markdown-Spalte (`markdown: true`) zeichnet `![alt](url)` als **echtes
Bild** zwischen den Textzeilen, wenn das Terminal Grafik kann (Kitty, iTerm2,
Sixel — beim Start automatisch erkannt). Im Stoat-Chat kommen die Bilder aus
den Bild-Anhängen, die der Adapter als Markdown-Bilder in den Body rendert.
Getestet wird in einem Channel mit mindestens einem Screenshot-Anhang.

- [ ] **Bild erscheint:** Channel öffnen → das Bild wird an der Stelle
      gezeichnet, an der der Anhang im Body steht, nicht als `[image: …]`-Text.
- [ ] **Nachladen:** Direkt nach dem Öffnen steht kurz der Platzhalter, dann
      erscheint das Bild von selbst — ohne Tastendruck, ohne `r`.
- [ ] **Einmal laden:** Mehrfach über die Nachricht scrollen bzw. `r` drücken →
      das Bild wird **nicht** erneut heruntergeladen (kein Flackern, keine
      neuen Requests im `NYD_DEBUG=1`-Log).
- [ ] **Scroll-Clipping:** Mit `j`/`k` durch die Nachricht scrollen → das Bild
      wird an der Pane-Kante sauber abgeschnitten und wächst wieder herein; es
      malt **nicht** über die Nachbarzeilen oder über den Rand hinaus.
- [ ] **Split:** Im gekoppelten Chat-Pane (80 %) prüfen, dass das Bild an der
      Pane-Grenze endet und nicht in den Channel-Baum links läuft.
- [ ] **Höhen-Cap:** Ein hoher Screenshot (Handy-Format) belegt höchstens
      `max_height` Zeilen (Default 20) — der Rest der Unterhaltung bleibt
      sichtbar. Wert in `tui.yaml` ändern → nach Neustart greift er.
- [ ] **Breite:** Terminal schmaler ziehen → das Bild skaliert mit der
      Spaltenbreite, das Seitenverhältnis bleibt.
- [ ] **Resize:** Während ein Bild sichtbar ist, das Terminal umgroßziehen →
      kein Geisterbild, keine Artefakte im Text.
- [ ] **Fallback ohne Grafik:** Dieselbe Ansicht in einem Terminal ohne
      Grafik-Support (z. B. `xterm`) → jede Zeile bleibt Text
      (`[image: …]`), sonst ändert sich nichts.
- [ ] **Abgeschaltet:** `images: { enabled: false }` in `tui.yaml` → wie oben,
      und beim Start wird das Terminal gar nicht erst abgefragt.
- [ ] **Kaputte URL / kein Bild:** Ein Anhang, der nicht ladbar oder nicht
      dekodierbar ist → Platzhalter-Text bleibt stehen, kein Panic, und es wird
      **nicht** in einer Schleife erneut versucht.
- [ ] **Nicht-Bild-Anhänge:** Eine PDF/ZIP im Body erscheint als Link
      (`📎 name`), nicht als Bildplatzhalter.
- [ ] **`i` bleibt:** `i` auf der Nachricht öffnet die Bilder weiterhin im
      OS-Viewer — inline und extern schließen sich nicht aus.
- [ ] **Andere Views unberührt:** Ein Jira-Ticket mit Markdown-Body rendert
      unverändert (keine reservierten Leerzeilen, kein Versatz).

## Taiga-Adapter — Edit-Hang-Fix (Timeout + nicht-blockierender Editor)

Zwei Ebenen gegen das „App friert beim Edit (`e`) auf einer Taiga-Zeile
komplett ein"-Problem. Ebene 1 = HTTP-Timeout + Reconnect-Retry im Adapter;
Ebene 2 = Editor-`prepare` läuft off-thread statt blockierend.

**Ebene 1 — Timeout/Reconnect:**

- [ ] **Default-Timeout greift:** Ohne `request_timeout_secs` in
      `taiga-adapter.yaml` verhält sich alles wie bisher; eine gesunde,
      langsame Instanz antwortet weiterhin (Default 20 s).
- [ ] **Toter Socket → Fehler statt Freeze:** Während die App läuft, die
      Verbindung zur Taiga-Instanz hart kappen (z. B. Netzwerk/Tunnel
      blockieren), dann `e` auf einer Zeile. Erwartet: nach ~Timeout ein
      Reconnect-Versuch, dann saubere Fehlermeldung
      („Failed to load …" / Netzwerkfehler) — **nie** dauerhaftes Hängen.
- [ ] **Kurzer Timeout zum Testen:** `request_timeout_secs: 3` setzen, Tunnel
      blockieren → Fehler kommt nach ~6 s (2 Versuche), App bleibt bedienbar.
- [ ] **`connect_timeout_secs` separat:** Ohne Angabe = `min(request, 10)`. Mit
      explizit z. B. `connect_timeout_secs: 30` darf der Verbindungsaufbau auf
      einer langsamen Leitung länger als 10 s dauern, ohne fälschlich
      abzubrechen (gesunde, langsam verbindende Instanz).
- [ ] **Reconnect heilt transienten Abriss:** Verbindung kurz kappen und sofort
      wieder freigeben → der zweite (Retry-)Versuch geht durch, kein
      Nutzer-sichtbarer Fehler.

**Ebene 2 — nicht-blockierender Editor-Dispatch (alle Adapter):**

- [ ] **UI bleibt responsiv:** `e` auf einer Taiga-Zeile bei langsamer
      Verbindung → die Notification „⏳ Opening editor: …" erscheint sofort,
      und die TUI nimmt währenddessen weiter Input an (scrollen, Tab wechseln),
      friert also nicht ein.
- [ ] **Editor öffnet normal:** Bei gesunder Verbindung öffnet `$EDITOR` wie
      gewohnt; die „Opening editor…"-Notification verschwindet beim Öffnen
      (andere Notifications bleiben stehen).
- [ ] **Kein Doppel-Open:** Während „Opening editor…" läuft, erneut `e` →
      „Editor is already open", kein zweiter Ladevorgang.
- [ ] **Fehler-Notification:** Schlägt das `prepare` fehl (toter Socket), zeigt
      die Statuszeile den Fehler und es öffnet sich **kein** leerer Editor.
- [ ] **Andere Adapter unverändert:** Jira/Postgres/Confluence/Stoat — `e`
      (bzw. die jeweilige Editor-Aktion) öffnet weiterhin korrekt; inline- und
      pause-tui-Editor-Profile funktionieren (der `pending_editor_request`-Pfad).

> Bekannte Schnitte: kein explizites „Abbrechen" während des Ladens — der
> Wartebalken ist durch den Adapter-Timeout (Ebene 1) ohnehin begrenzt, und der
> Generation-Token verwirft eine veraltete Session, falls zwischenzeitlich neu
> geöffnet wird. Retry trifft nur Transport-Fehler, nicht HTTP-Status (4xx/5xx);
> der multipart-Upload (`upload_attachment`) hat keinen Retry (Form nicht
> klonbar), wird aber vom Timeout geschützt.

## Postgres — Retry nach fehlgeschlagenem Erst-Load (leerer Tree)

Voraussetzung: Postgres-Tab erreichbar machen/kappen, sodass der erste
`list databases`-Load fehlschlägt (z. B. Tunnel-Ziel down, oder kurzes
`query_timeout_secs`).

- [ ] Postgres-Tab öffnen, Load schlägt fehl → Banner „Fetch failed:
      list databases: …".
- [ ] **`r` drücken → Reload wird ausgelöst** (Banner wechselt auf
      „Retrying …"/„Connecting …", nicht stummes Nichts). Das war der
      Bug: im leeren Tree war keine Cursor-Zeile → die View-Actions
      (inkl. `reload`) wurden nicht aufgelöst, `r` verpuffte.
- [ ] Verbindung wieder herstellen, `r` → Datenbanken laden, Banner weg.
- [ ] Auch `f` (fuzzy filter) und `/` (search) sind im leeren Tree
      ansprechbar (gleiche Wurzel-Fallback-Logik).

## Postgres — `manual_connect` (kein Auto-Connect, nur `r`)

`adapter.manual_connect: true` in `postgres.yaml` — das ist der Default,
der Test gilt also auch ohne die Zeile.

- [ ] App-Start: Postgres-Tab lädt **nicht** automatisch; Banner
      „Press `r` to connect".
- [ ] Subtab-Wechsel (databases/tables/scripts) löst ebenfalls keinen
      Auto-Load aus.
- [ ] `r` baut Verbindung + SSH-Tunnel auf und lädt.

## `manual_connect` — Default und Startup-Login

`adapter.manual_connect` steht per Default auf `true`. Getestet wird der
Default selbst und das, was ein explizites `false` beim Start auslöst.

- [ ] **Default greift**: In einer View-Datei mit Adapter die Zeile
      `manual_connect` **ganz entfernen**. TUI starten → der Tab lädt
      nicht, Banner „Auto-connect disabled — press `r` to connect".
- [ ] **Lokale Tabs opten zurück**: `views/tasks.yaml`,
      `trackings.yaml`, `projects.yaml`, `sqlite.yaml` tragen
      `manual_connect: false`; diese Tabs sind beim Start wie bisher
      sofort gefüllt.
- [ ] **Login eines eager Tabs kommt sofort**: In einer View mit
      Anmeldung (z. B. `kimai.yaml`) `manual_connect: false` setzen,
      Passwortspeicher sperren (`gpgconf --kill gpg-agent`), TUI
      starten. Erwartet: das Credential-Popup steht **sofort** da,
      obwohl der Tasks-Tab aktiv ist; der Titel beginnt mit dem
      Tab-Namen (`Kimai: …`). Enter → Popup schließt, Kimai lädt im
      Hintergrund; Tasks bleibt der aktive Tab.
- [ ] **Escape**: Statt Enter `Esc` → Popup weg, kein zweites Popup
      poppt nach, die TUI ist normal bedienbar. Der Kimai-Tab zeigt beim
      Öffnen den Fehler-/Connect-Banner; `r` startet den Login neu.
- [ ] **Zwei eager Logins**: Zwei Views mit Anmeldung auf
      `manual_connect: false`. Erwartet: **ein** Popup zur Zeit, das
      zweite überschreibt es nicht. Nach Beantworten/Abbrechen des
      ersten kommt das zweite spätestens beim Öffnen seines Tabs.
- [ ] **Manual-Connect-Tab bleibt still**: Ein Tab mit
      `manual_connect: true` (oder ohne die Zeile) zeigt beim App-Start
      **kein** Popup — erst `r` auf diesem Tab fragt nach.

## Postgres — `auth:`-Block statt gpg-Pinentry

Voraussetzung: `postgres-adapter.yaml` auf die delegierende Form umstellen
(siehe `docs/examples/views/postgres-adapter.yaml`, Abschnitt „One
credential script for the whole connection"): `auth.mechanism: password`
mit `script:`, Bindings `password` + `ssh_password`, und beide Slots
(`postgres.password`, `transport.ssh[0].auth.password`) auf
`{ type: script-result }`. gpg-Agent vorher entsperrt **und** in einem
zweiten Durchgang gesperrt (`gpgconf --kill gpg-agent`).

- [ ] Entsperrter Store: Tab öffnen → Verbindung kommt ohne jede
      Rückfrage; **kein** gpg-Pinentry-Fenster.
- [ ] Gesperrter Store: Tab öffnen → **unser** Credential-Popup mitten in
      der TUI (Header aus dem Skript, Passphrase maskiert). Enter →
      Tunnel und DB-Verbindung kommen aus **einem** Skript-Lauf, also
      genau **ein** Dialog für beide Secrets.
- [ ] Escape im Popup → Load bricht mit Meldung ab, keine
      Wiederholungs-Dialoge; `r` fragt erneut.
- [ ] Falsche Passphrase → Skript zeigt das Formular erneut mit seiner
      Fehlermeldung, nicht ein neuer leerer Dialog.
- [ ] `query_timeout_secs` klein setzen (z. B. 5) und im Popup länger
      warten → Timeout läuft **nicht** während der Eingabe (die
      Credentials werden vor der Uhr geholt).
- [ ] Config-Fehler werden beim Lesen abgewiesen, nicht beim Connect:
      Binding `ssh_password` entfernen, aber den Hop delegieren lassen →
      Start-Fehler nennt `transport.ssh[0].auth.password`; zweiten Hop
      ebenfalls auf `script-result` setzen → Fehler nennt `ssh[1]`.

## Tab-Reihenfolge + Autonummerierung

`tabs.order`-Liste in `tui.yaml` (Tab-Namen in Anzeigereihenfolge).

- [ ] Tab-Bar zeigt die Tabs in Listenreihenfolge, mit Ziffern
      `1`,`2`,`3`,… als Key-Hint.
- [ ] Ziffern wechseln den Tab (auch `9` für einen 9. Tab wie Stoat, der
      vorher keine Taste hatte). `0` = 10. Tab; ab dem 11. keine Ziffer.
- [ ] Eine nicht in `order` genannte View ist verborgen (aber nicht
      entladen) — taucht wieder auf, sobald ihr Name ergänzt wird.
- [ ] `Tab` / `Shift+Tab` zykeln nur durch die sichtbaren Tabs.
- [ ] Nicht belegte Ziffer (mehr Ziffern als Tabs) tut nichts.
- [ ] `Ctrl+X` tut nichts mehr (Tab-Gruppen-Umschalter wurde entfernt).
- [ ] `tabs.order` leeren/entfernen → alle Tabs in natürlicher
      Slot-Reihenfolge, gleich autonummeriert.
- [ ] Zwei Tabs mit gleichem `tab.name` → **harte Fehlermeldung** als
      Start-Modal; App zeigt alle Tabs und bleibt bedienbar.
- [ ] `:config` / `tui.yaml` editieren + Reload → Reihenfolge wird neu
      aufgelöst (aktiver Tab snappt auf den ersten sichtbaren, falls er
      rausfiel).

## TaskAdapter (adapterisierter Tasks-Tab) — A1b + A1c-1 + A1c-2

Voraussetzung: `docs/examples/views/tasks.yaml` nach
`~/.config/not_yet_done/views/tasks.yaml` kopieren. Der Adapter-Tab läuft
**parallel** zum nativen Tasks-Tab (Vergleich), kein C1-Cutover.

- [ ] Tab lädt: Forest als Tree, Top-Level-Tasks sichtbar, Drill in
      Subtasks beliebig tief; `priority` rechtsbündig, `created` lokalisiert.
- [ ] `a` (add) am Root → Markdown-Buffer mit `## Description:` /
      `## Notes:`; Beschreibung eintragen, `:wq` → neuer Top-Level-Task
      erscheint, Cursor darauf.
- [ ] `a` mit `parent:`-Feld auf eine bestehende Task-UUID → Task landet
      als Subtask unter dem Parent.
- [ ] In einen Task gedrillt, `a` → neuer Task hängt als Subtask darunter
      (Buffer hat `parent:` vorbefüllt).
- [ ] `e` (edit) auf Task → Buffer zeigt aktuelle Felder + Notes;
      Beschreibung ändern, `:wq` → Zeile aktualisiert. Notes-Datei
      mitgeschrieben.
- [ ] `e` → Beschreibung leeren → `:wq` → Reopen mit Error-Banner
      (Description darf nicht leer sein).
- [ ] `e` → `status`/`priority` ändern → übernommen. Ungültiger `status`
      → Reopen mit Inline-Fehler.
- [ ] `e` → `tracking: true` → Tracking startet (im nativen Trackings-Tab
      sichtbar); bei `allow_parallel=false` werden andere aktive Trackings
      gestoppt. `tracking: false` → stoppt wieder.
- [ ] `d` (delete) auf Task mit Subtasks → Confirm → ganzer Teilbaum weg;
      Meldung „Deleted subtree (N tasks)". Notes soft-deleted.
- [ ] `u` (undelete) → zuletzt gelöschte(r) Task(s) zurück; ohne
      vorherige Löschung → „Nothing to undelete".
- [ ] `m` (mark-move) auf Task A, dann `p` (paste-move) auf Task B → A
      wird Subtask von B. „marked …"-Indikator währenddessen sichtbar.
- [ ] `m` auf A, `p` auf A selbst oder auf einen Nachfahren von A →
      Fehler (Zyklus abgelehnt), keine Änderung.
- [ ] Mutation in diesem Tab → nativer Tasks-Tab (falls offen) repaint/
      reload via DomainEvent.

### `edit node` (`ctrl+n`) — Subtree-Restructure-Outline-Editor

Parität zum alten nativen Tasks-Tab (`ctrl+n` = „edit node"). Voraussetzung:
`tasks.yaml` mit der `edit node`-Action (`key: ctrl+n`, `type: edit`,
`id: edit-tree`) auf allen drei Ebenen (Tree-Wurzel, rekursive Subtask-Ebene,
flache `list`-View — im Beispiel-Config gesetzt). Einen Task mit mehreren
Subtasks/Enkeln anlegen.

- [ ] `ctrl+n` auf einem Task → Editor öffnet mit dem Task **und seinem ganzen
      Teilbaum** als eingerückte Checkbox-Outline (`- [ ] Beschreibung (p=… id=…)`),
      Kinder unter dem Eltern-Task eingerückt.
- [ ] Eine Zeile umhängen (Einrückung ändern) → `:wq` → Task wird re-parented;
      Notes wandern mit. Im Baum steht der Knoten an der neuen Stelle.
- [ ] Status-Marker einer Zeile ändern (`[ ]`→`[x]`) → `:wq` → Status
      übernommen. Priorität via `(p=N)` ändern → übernommen.
- [ ] Neue Zeile hinzufügen (passend eingerückt, ohne `id=`) → `:wq` → neuer
      Subtask angelegt; verschachtelte neue Zeilen erben den richtigen Parent.
- [ ] Eine Zeile löschen (aus dem Buffer entfernen) → `:wq` → Task soft-deleted
      (per `u`/undelete rückholbar); ganze entfernte Teilbäume verschwinden.
- [ ] Mehrere Änderungen in einem Buffer (umhängen + neu + löschen + Status) →
      `:wq` → alle in einem Durchgang angewandt; Meldung nennt die Bilanz.
- [ ] Bei `tracking.allow_parallel=false` zwei Zeilen mit `-t`-Flag markieren →
      `:wq` → Reopen mit Fehler („Only one task can be tracked at a time …"),
      Edits bleiben erhalten.
- [ ] Buffer unverändert speichern → keine Änderung (Diff ist No-op).
- [ ] `ctrl+n` auch in der flachen `list`-View (`v`) und auf jeder Drill-Tiefe
      erreichbar (editiert dort den selektierten Task + Nachfahren als Outline).
- [ ] Nach Anwenden: Baum re-snapshottet (DomainEvent → voller Reload),
      nativer Tasks-Tab (falls offen) zieht nach.

### A1c-1 — Tracking-Marker-Spalte + Start/Stop-Taste

- [ ] `⏱`-Spalte zwischen Task und Status sichtbar. Tasks mit laufendem
      Tracking zeigen `⏱`, alle anderen leer.
- [ ] `t` (toggle-tracking) auf untracktem Task → `⏱` erscheint sofort
      (Reload); im nativen Trackings-Tab taucht das Tracking auf.
- [ ] `t` erneut auf demselben Task → `⏱` verschwindet, Tracking gestoppt.
- [ ] Bei `tracking.allow_parallel=false`: `t` auf Task B während A läuft →
      A's `⏱` verschwindet, B's erscheint (exklusiv, native Policy).
- [ ] Bei `tracking.allow_parallel=true`: `t` auf B lässt A's `⏱` stehen
      (beide laufen).
- [ ] Tracking via `e`-Buffer (`tracking: true`) gestartet → `⏱` erscheint
      ohne extra `t`; `t` togglet danach konsistent.
- [ ] `t` und der `tracking:`-Buffer-Toggle bleiben synchron (kein
      Stale-Marker): nach jedem Toggle spiegelt die Spalte den Live-Stand.

### Tracking-Marker am eingeklappten Knoten (`collapsed_source`)

Voraussetzung: `tasks.yaml` mit `collapsed_source: tracking_rollup` auf der
`tracking`-Spalte (Root- **und** rekursive Subtask-Ebene — im Beispiel-Config
gesetzt). Einen Task mit (mindestens) einem **Subtask** anlegen und das
Tracking auf dem **Subtask** starten (`t`).

- [ ] Eltern-Task **aufgeklappt**: `⏱` steht beim laufenden Subtask, der
      Eltern-Task selbst ist leer (zeigt seinen eigenen `tracking` = leer).
- [ ] Eltern-Task **einklappen** (`h`/←/`zc`): Der `⏱`-Marker „bubbelt" jetzt
      sichtbar auf den eingeklappten Eltern-Task — der Subtree-Tracking-Stand
      bleibt erkennbar, obwohl der laufende Task verborgen ist.
- [ ] Wieder **aufklappen**: Eltern-Task wird wieder leer, `⏱` steht wieder am
      Subtask (Marker springt nicht „hängen").
- [ ] Mehrstufig: Tracking auf einem Enkel; zwei Ebenen darüber einklappen →
      `⏱` erscheint am eingeklappten Großeltern-Knoten (Roll-up über die ganze
      Vorfahrenkette).
- [ ] Eingeklappter Eltern-Task **ohne** Tracking irgendwo im Teilbaum bleibt
      leer (kein falscher `⏱`).
- [ ] Flat-Liste (`v`): Die Spalte zeigt unverändert den **eigenen** Marker —
      `collapsed_source` ist hier inert (es gibt keinen Einklapp-Zustand).

### A1c-2 — Saved Queries + FilterExpr-Filter (gefilterter Baum)

Voraussetzung: `tasks.yaml` mit dem `query:`-Block (Default `open tasks`:
nur nicht-`done`, nicht gelöscht). Mindestens ein `done`-Task tief im Baum
und ein offener Geschwister-Task anlegen.

- [ ] Tab lädt mit aktivem Default-Query: `done`-Tasks fehlen im Tree,
      offene Tasks da. Ein offener Task **unter** einem `done`-Parent bleibt
      sichtbar — der `done`-Parent erscheint als Vorfahr mit (nur dem
      passenden offenen Kind).
- [ ] Drill in einen gefilterten Knoten zeigt **nur** matchende Kinder
      (Filter greift auf jeder Tiefe, nicht nur an der Wurzel).
- [ ] `q` öffnet das Query-Menü: Default-Query `open tasks` gelistet.
- [ ] `:query new <name>` mit eigenem `FilterExpr`-Body (z. B.
      `[priority, ">=", 5]`) → speichern → erscheint im `q`-Menü; Apply
      filtert den Baum live.
- [ ] `:query edit <name>` → Body ändern, `:wq` → Baum re-filtert sofort
      (alter Subtree-Cache verworfen, keine Stale-Kinder).
- [ ] `:query delete <name>` → verschwindet aus dem Menü; Body-Datei unter
      `…/tasks/<id>/<view>/queries/<name>.yaml` weg.
- [ ] Query mit 0 Treffern → leerer Baum, Reload-Action (`r`) bleibt
      erreichbar (kein Dead-End).
- [ ] Query leeren / `default` droppen → ganzer Forest wieder sichtbar.
- [ ] Strukturelle Mutation (add/delete/reparent) → Baum re-snapshottet;
      Filter geht bis zum nächsten erneuten Query-Send verloren (akzeptierte
      Lifecycle-Kante, s. Plan-Box A1c-2).
- [ ] Saved-Query-Shortcut (Ctrl+f im `q`-Menü) auf eine Query → Taste
      filtert den Baum direkt; übersteht YAML-Reload (`query_shortcut`-Tabelle).

### A1c (scripts) — `:script` / `x` auf dem adapterisierten Tasks-Tab

Voraussetzung: `tasks.yaml` mit der `run script`-Action (Key `x`).

- [ ] `x` auf einem selektierten Task → Script-Menü öffnet, Verzeichnis
      `<data>/not_yet_done/scripts/tasks/task_item/` (auto-angelegt). Auch
      `:script` über die Cmdline öffnet dasselbe Menü.
- [ ] Auf jeder Drill-Tiefe (Subtask, Sub-Subtask) liefert `x` **dasselbe**
      Verzeichnis (View-Pfad stabil, ein gemeinsamer Scripts-Ordner).
- [ ] `+name<Enter>` legt ein neues Script aus dem Template an; Editor öffnet.
- [ ] Script ausführen → bekommt den Task als JSON
      `{"node": {"id": <uuid>, "label": <description>, "node_type":
"task:item", "tab": "tasks", "fields": {status/priority/tags/tracking/
created/…/ancestors}}}` (uniforme Node-Form, NICHT die native
      `{"task": …}`-Form). `fields.ancestors` ist ein JSON-Array-String
      `[{"id", "description"}, …]` Root→Parent (exklusive des Tasks selbst);
      bei einem Top-Level-Task `"[]"`.
- [ ] Selektion wechseln (anderer Task, anderer Typ-Mix) → Menü bleibt am
      selben Ordner (kein Shuffle).
- [ ] Kein Task selektiert / leerer Baum → Notification „No row selected",
      kein Crash.
- [ ] Portiertes `task_to_taiga.py` (unter `scripts/tasks/task_item/`) auf
      einem Ticket-Task (`#<n> - …` unter `<slug>/tickets/`) ausführen →
      Taiga-Tab aktiviert die Per-Project-Query und parkt den Cursor auf
      dem Item `<slug>#<n>` — identisch zum Verhalten auf dem nativen
      Tasks-Tab.

### Task-1 — `a` (Kind / Top-Level), `A` (Sibling), Vererbung

`a` und `A` verhalten sich wie im nativen Tasks-Tab:

- [ ] **Tree-Mode, `a` auf selektiertem Task** → Editor-Buffer mit `parent:`
      auf den selektierten Task vorbefüllt; `:wq` → neuer Task hängt **als
      Kind** unter dem selektierten Task (nicht als Sibling/Top-Level).
- [ ] **Tree-Mode, `A` auf selektiertem Task** → Buffer mit `parent:` auf
      den **Eltern** des selektierten Tasks; `:wq` → neuer Task ist ein
      **Sibling** (gleiche Ebene). Auf einem Top-Level-Task → neuer
      Top-Level-Task.
- [ ] **Leerer Baum (keine Tasks):** sowohl `a` als auch `A` → neuer
      **Top-Level**-Task (Engine löst fehlende Selektion auf den Adapter-Root
      auf).
- [ ] **Flache Liste (`v`):** `a` → Top-Level-Task; `A` → Sibling des
      selektierten Tasks (gleiche Eltern).
- [ ] `a`/`A`, dann im Buffer das `parent:`-Feld editieren → `:wq` →
      Buffer-Override gewinnt über das von der Taste gewählte Ziel.
- [ ] `U` (Shift+U) auf einem verschachtelten Task → Task wandert auf die
      oberste Ebene (parent_id = None), erscheint als Top-Level-Knoten.
- [ ] `U` auf einem bereits Top-Level-Task → Notification „already at the
      top level", keine Änderung.

**Action-/Shortcut-Vererbung** (rekursiver `subtasks`-Branch deklariert
keine eigenen `actions:`/`shortcuts:`):

- [ ] In einen Task drillen (rekursive Ebene) → `e`/`a`/`A`/`x`/`ctrl+n`/`r`
      und die Shortcuts `d`/`u`/`s`/`m`/`p`/`U` funktionieren dort genauso wie
      auf der Wurzel-Ebene (alle via `inherit: true` vererbt).
- [ ] `f` (fuzzy filter) und `/` (tree find) sind **nur** auf der
      Wurzel-Ebene aktiv (nicht vererbt — Validator-Regel).

## TrackingAdapter (adapterisierter Trackings-Tab) — A2a + A2b + A2c

Voraussetzung: `views/trackings.yaml` (aus `docs/examples/views/`) nach
`~/.config/not_yet_done/views/` kopiert. Der Adapter-Tab läuft neben dem
bespoke nativen Trackings-Tab (bis C1).

### A2a — Read-Path + Live-Dauern + Grouping

- [ ] Trackings-Tab (Adapter) öffnen → flache Liste, neueste zuerst; Spalten
      Marker (`⏱` nur bei laufenden), Path (gestylt `/a › b`), Task, Started,
      Ended (leer bei laufend), Duration (`H:MM:SS`, rechtsbündig).
- [ ] Auf der Tasks-Seite ein Tracking starten → die laufende Zeile zeigt
      `⏱`, leeres Ended, und die Duration **tickt adaptiv** (frisch: alle
      5 s; ab 1 min: 10 s; ab 10 min: 30 s; ab 1 h: 60 s — wie der native
      Tab; nur diese Zeile wird gepatcht, kein Voll-Reload-Flackern).
- [ ] Tracking stoppen → `⏱` weg, Ended gefüllt, Duration statisch; das
      Ticken stoppt (kein Dauer-CPU mehr).
- [ ] `zg` zykliert die Gruppierung (Day → Week → Month → Year → None) mit
      Pro-Gruppe-Summe + Footer-Gesamtsumme.
- [ ] `q` öffnet das Query-Menü; eine gespeicherte FilterExpr-Query
      (z. B. `description ~ "<wort>"`) filtert die Liste; löschen zeigt
      wieder alles.

### A2b — Mutationen

- [ ] `d` auf einer Zeile → Confirm-Dialog; bestätigen → „Tracking deleted",
      Zeile verschwindet (Zeiten bleiben in der DB erhalten).
- [ ] War die gelöschte Zeile **aktiv**, verschwindet auch der Tracking-Marker
      des zugehörigen Tasks auf dem Tasks-Tab (Cross-Tab via `TrackingChanged`).
- [ ] **Gelöschte Zeilen ausgegraut:** Query so anpassen, dass gelöschte
      Trackings im sichtbaren Satz liegen (z. B. `[deleted, =, true]` oder die
      `deleted`-Klausel droppen) → die gelöschten Zeilen erscheinen **dimmed**
      (Theme-`text_dim`). Greift auch in der **nach Tag gruppierten** flachen
      Liste (`── Tag ──`-Header), nicht nur in der ungruppierten/Tree-Ansicht.
- [ ] **`d` auf einer bereits gelöschten (ausgegrauten) Zeile** → **kein**
      Confirm-Dialog, nur Notification „Already deleted" (kein Re-Delete).
- [ ] `t` auf einer Zeile → startet/stoppt Tracking auf dem **Task** der Zeile;
      bei deaktiviertem `allow_parallel_tracking` wird ein anderes laufendes
      Tracking zuerst gestoppt (gleiche Politik wie Tasks-Tab).
- [ ] `R` auf einer sichtbaren **nicht-gelöschten** Zeile → Notification
      „Restore failed: … not deleted" (nur gelöschte Zeilen lassen sich
      restoren — und die macht erst ein Query sichtbar, s. o.).
- [ ] **`A` (restore all) ist im flachen Listen-View IMMER in der Action-Bar
      sichtbar** — auch direkt nach dem Tab-Wechsel, ohne irgendwohin zu
      drillen (statischer `on_container`-Hint, kein `parent:`-Shortcut mehr).
- [ ] **`A` ist auf den aktiven Query gescoped** (nicht die ganze DB): Mit dem
      Default-Query (zeigt nur nicht-gelöschte) findet `A` nichts →
      Notification „No deleted trackings to restore" (kein Confirm-Popup, da
      nichts zu tun ist). Wichtig: ein an anderer Stelle gelöschtes Tracking,
      das der aktive Query **nicht** sichtbar macht, wird von `A` **nicht**
      angefasst.
- [ ] Query so anpassen, dass gelöschte Trackings im sichtbaren Satz liegen
      (z. B. `deleted`-Klausel entfernen oder `[deleted, =, true]`), dann `A`
      → **Confirm-Popup** „Restore N deleted tracking(s)? …" (nennt die Anzahl;
      bei vorhandenen Nachfolgern zusätzlich „Purges M successor intervals —
      irreversible"). `n`/Esc bricht ab ohne Änderung; `y` stellt **nur die
      vom Query erfassten** wieder her und lädt die Liste neu.
- [ ] `R` auf einer **gelöschten (ausgegrauten) Zeile** (die der Query
      sichtbar macht) → **Confirm-Popup** mit derselben Purge-Warnung; `y`
      führt aus, `n` bricht ab.
- [ ] `x` öffnet das `:script`-Menü; ein Script gegen die selektierte Zeile
      bekommt deren JSON (`{json_file}`) übergeben.

### A2c — Condensed (adapter-seitiges Condensing, `tracking:condensed-row`)

- [ ] `v` schaltet auf den **Condensed**-Subtab; `a` schaltet zurück zur
      flachen Liste.
- [ ] Condensed zeigt pro **Tag** einen `── 2026-… ──`-Header mit Tages-Summe,
      darunter **je Task eine Zeile** mit Pfad, Task-Name und der summierten
      Dauer dieses Tasks **an diesem Tag**. Ein Task, der an zwei Tagen
      getrackt wurde, erscheint zweimal (einmal pro Tag).
- [ ] Zwei **verschiedene** Tasks mit gleichem Namen verschmelzen **nicht**
      (innere Gruppierung keyt auf `task_id`, nicht aufs Label).
- [ ] `zg` rotiert nur die **äußere** (Tag-)Ebene (Day→Week→Month→Year→None);
      die Pro-Task-Aufschlüsselung bleibt. Auf `None` → eine Zeile pro Task
      über den ganzen gefilterten Zeitraum.
- [ ] Eine Condensed-Zeile ist **selektierbar**; `d`/`t` wirken auf das
      repräsentative Tracking der Zeile (bekannte Grenze: Aktion trifft ein
      einzelnes Intervall, nicht die ganze Task-Tagessumme).
- [ ] Der Saved-Query-Filter (`q`) wirkt auch im Condensed-Subtab.

### A2c — Tree (own/cumulated, M4 `tree_aggregate`)

- [ ] `T` (Shift+t) schaltet auf den **Tree**-Subtab; `a` schaltet zurück zur
      flachen Liste. `t` (klein) bleibt toggle-tracking auf der Zeile — die
      beiden kollidieren **nicht**.
- [ ] Der Tree zeigt den **Task-Forest** (Tasks, nicht einzelne Intervalle);
      nur Tasks mit getrackter Zeit **irgendwo im Teilbaum** erscheinen
      (untracked Branches sind ausgeblendet, der Pfad zu getrackten Blättern
      bleibt sichtbar).
- [ ] Die `Duration`-Spalte zeigt zunächst die **kumulierte** Teilbaum-Summe
      (Default `cumulated`). `zt` schaltet alle `tree_aggregate`-Spalten auf
      die **eigene** Dauer des Tasks um (und zurück). (`zt` ist nur aktiv, weil
      der Trackings-Adapter `supports_tree_aggregation` meldet — das
      Capability-Gate. Ein `tree_aggregate:` in der YAML allein reicht nicht.)
- [ ] Ein Eltern-Task ohne eigenes Tracking, aber mit getrackten Kindern, zeigt
      cumulated > 0 (Eigenwert 0:00:00 nach `zt`).
- [ ] Drill-in (Enter/→) klappt die Subtasks auf; auf jeder Tiefe gilt die
      gleiche `tree_aggregate`-Spalte.
- [ ] `⏱`-Marker erscheint auf Tasks mit laufendem Tracking; `t` startet/stoppt
      Tracking auf dem selektierten Task (gemeinsame Exklusiv-Policy mit dem
      Tasks-Tab) und der Tree lädt neu.
- [ ] **Grenze:** kein Live-Tick im Tree (Dauern backen beim Load wie
      Condensed); ein `r`-Reload aktualisiert sie.

## Saved-Query-Shortcut-Validierung (Content-Tabs)

Saved-Query-Shortcuts claimen tab-weit auf der View-Claim-Ebene und würden
jede danach dispatchte Taste überschatten (Navigations-Keys, Chords, …).
Beide Prüfpfade testen:

- [ ] **Set-Time:** q-Menü öffnen, auf einer Query `ctrl+s` (Shortcut
      binden), dann `j` drücken → Modal „Shortcut 'j' is already taken by
      common.list_next!" und Re-Prompt; `v` → Konflikt mit dem Subtab-Key;
      `w` → Konflikt mit einem Window-Chord (Leader-Präfix); `z` →
      Konflikt mit `content.cycle_grouping` (Chord-Präfix); `d` →
      Konflikt mit dem YAML-`shortcuts:`-Eintrag. `esc` bricht ab.
- [ ] Ein freier Key (z. B. `M`) wird akzeptiert: „Favorite … added".
- [ ] **Load-Time:** eine kollidierende Row direkt in `query_shortcut`
      schreiben (oder eine Config-Änderung, die einen bestehenden
      Shortcut kollidieren lässt) → beim Start erscheint eine
      Notification „<Tab>: saved-query shortcut [x] ('name') shadows … —
      rebind it via the query menu"; der Shortcut bleibt aktiv.

## Column-Config (`c`) auf Content-Tabs

`c` öffnete früher auf jedem Nicht-Trackings-Tab das **native
Tasks**-Spalten-Popup (und hätte beim Anwenden dessen Settings
überschrieben). Jetzt generisch pro Level:

- [x] Adapter-Tab (z. B. „Trackings"): `c` zeigt die Spalten der
      aktiven View (nicht die Tasks-Spalten); `Space` blendet eine
      Spalte aus (z. B. Taskpath) → Tabelle baut sofort ohne sie neu.
- [x] Persistenz: App neu starten → die Spalte bleibt ausgeblendet
      (Settings-Row `content_columns:<Tab>` als JSON-Map).
- [x] Reset: Spalte wieder aktivieren und per `Ctrl+D` an die
      YAML-Position schieben → Override entfernt, Settings-Row gelöscht
      (`SELECT key FROM settings WHERE key LIKE 'content_columns%'`
      ist leer).
- [x] Tree-Mode („Tasks"): `c` zeigt die Spalten der Cursor-Ebene;
      die `tree_label`-Spalte (Task) ist fix (`Space` ohne Wirkung);
      andere Spalte (Created) togglen wirkt sofort + Reset wie oben.
- [x] Native Tabs (Tasks/Trackings): Popup unverändert (Display-Namen,
      Toggle, Persistenz in `tree_columns`/`tracking_columns`).
- [ ] Auto-Fallback-Level (Postgres-Rows): `c` → Notification „This
      level has no configurable columns" (Unit-Test vorhanden, live
      ungetestet — braucht verbundene Postgres-Instanz).

## Default-Query + Query-Menü-Styling

Das Query-Menü (`q`) teilt sich jetzt das Popup-Chrome mit dem
Column-Config-Popup (SearchablePopup rendert über `popup_utils`), und
`ctrl+t` markiert die selektierte Saved Query als Default, die beim
App-Start automatisch angewendet wird.

- [x] Optik: Query-Menü (Content + nativ), Script- und Tag-Menü zeigen
      das einheitliche Chrome (abgerundeter Rahmen, gewrappte
      Hint-Zeile, Cursor-Zeile hinterlegt statt Farbbalken);
      Saved-Query-Shortcuts erscheinen als `[key]`-Suffix.
- [x] Content-Tab („Trackings"): `ctrl+t` auf „2 months" →
      Notification „Default query: 2 months", Settings-Row
      `default_query:<scope>` angelegt; App-Neustart → Query ist aktiv
      (Action-Bar zeigt sie), Menü zeigt `★ 2 months`.
- [x] Toggle-Off: `ctrl+t` auf der markierten Query → „Default query
      cleared", Settings-Row gelöscht.
- [x] Nativer Tasks-Tab: `ctrl+t` auf „Alle" → Neustart wendet „Alle"
      an, obwohl zuletzt „2 months" aktiv war (Default schlägt
      Last-Active-Restore); Toggle-Off stellt das alte Verhalten
      wieder her.
- [x] Postgres-Script-Menü: kein `default`-Hint, `ctrl+t` ohne Wirkung
      (Scripts sind keine Queries; via `open_without_default`).
- [ ] Default-Query mit Pflicht-Variablen (`{var}`): wird beim Start
      roh (ohne Variablen-Popup) angewendet — Verhalten dokumentiert,
      live ungetestet.

## Tree-Linien + Aufklappmarker konfigurierbar (`tree_lines` / `tree_markers`)

Pro Tree (Wurzel-`ViewDef`) sind die Box-Linien (`├──`/`└──`/`│`) und die
Aufklappmarker (`▶`/`▼`) getrennt konfigurierbar: `tree_lines: false` ersetzt
die Linien durch Einrückung (zwei Leerzeichen pro Tiefe), `tree_markers:`
überschreibt (`collapsed`/`expanded`) oder versteckt (`enabled: false`) die
Marker.

- [x] Postgres-Tab (`tree_lines: false` in der User-Config): Datenbank
      vier Ebenen tief aufklappen → Schema/Tabellen-Ebenen sind nur
      eingerückt, ohne `├──`/`└──`-Linien; die `▶`/`▼`-Marker
      erscheinen weiterhin.
- [x] Tab ohne Konfiguration („Tasks"): unverändert Linien +
      Marker wie bisher (Default-Verhalten).
- [ ] `tree_markers.enabled: false` (temporär setzen): Linien bleiben,
      Marker verschwinden; Aufklappen per Enter funktioniert weiter
      (unit-getestet, live offen).
- [x] Connector-Farbe färbt bei `tree_lines: false` weiterhin den
      Marker-Lauf (im Capture: Marker in `tree_connector`-Farbe).

## Initiale Aufklapptiefe (`expand_depth`) + Listenansicht (`task:flat`)

Tasks-Adapter-Parität mit dem nativen Tab: `expand_depth: 2` auf dem
Wurzel-`ViewDef` klappt nach dem Laden Tiefe 0 und 1 automatisch auf
(One-Shot-Kaskade über den normalen Expand-Pfad, spiegelt
`tasks.tree.default_expand_depth: 2`); die zweite View `list`
(`node_type: task:flat`, Subtab-Key `v`, zurück `t`) zeigt den ganzen
Forest als flache Tabelle in DFS-Reihenfolge.

- [x] „Tasks" öffnen: drei Ebenen sind direkt sichtbar (Wurzeln +
      Kinder + Enkel aufgeklappt), tiefere Ebenen bleiben zu.
- [x] Einen Knoten manuell zuklappen, dann `r` (Reload): der Knoten
      bleibt zu — die Kaskade ist one-shot und klappt nach Abschluss
      nichts mehr gegen den User auf.
- [x] `v` drücken: flache Liste aller Tasks (alle Tiefen, keine
      Marker/Einrückung, DFS-Reihenfolge); `t` wechselt zurück zum Tree,
      Aufklappstand bleibt erhalten.
- [x] In der Listenansicht: `e` öffnet die Edit-Session der selektierten
      Zeile wie im Tree (`s` toggle-tracking nutzt denselben
      invoke-Pfad; bewusst nicht live gedrückt — würde ein echtes
      Tracking starten/stoppen).
- [ ] Saved Query in der Listenansicht anwenden (`q`): nur die Treffer
      selbst erscheinen, keine Vorfahren-Zeilen (unit-getestet, live
      offen).

## Default-Query auf allen Trackings-Subtabs (`query.inherit_default`)

Der User-Default (★ im q-Menü) wird beim Start nur auf die Default-View
des Tabs gestempelt. `query.inherit_default: true` (condensed + tree in
trackings.yaml) stempelt ihn zusätzlich auf den jeweiligen Subtab; der
Tree filtert dabei adapter-seitig (Projektion wird aus den sichtbaren
Trackings neu gefaltet, `propagates_query_to_subtree`).

- [x] App mit ★-Default starten: Normal-, Condensed- UND Tree-Subtab
      zeigen den Default-Query-Namen als aktive Query in der Action-Bar
      (Grenze unverändert: ein Default mit `{var}`-Variable wird roh,
      d. h. effektiv ungefiltert, angewendet — wie auf der Default-View).
- [ ] Subtab ohne `inherit_default` (z. B. Tasks Listenansicht):
      Default-Query greift dort weiterhin NICHT (Opt-in-Verhalten;
      unit-getestet, live offen).
- [x] Im Tree-Subtab `q` → Saved Query anwenden: Wurzel zeigt die
      gefilterte Summe; Expand der Äste bleibt gefiltert (nur Äste mit
      sichtbarer Zeit, identische Summen die Kette hoch bei
      Einzel-Ast-Treffern).
- [x] Nach Anwendung zurück zur flachen Liste (`a`): deren eigene Query
      unverändert (Pane-State bleibt getrennt; geteilt ist nur der
      Start-Default).

## Group-by-Menü (`u`) auf Content-Tabs (`content.group_menu`)

Direktsprung-Parität zum nativen Trackings-`u`: ein Hotkey-Popup über die
fünf `zg`-Zustände (No grouping/Day/Week/Month/Year). Nur aktiv, wenn die
Ebene ein `group_by:` konfiguriert; Wahl ist View-State (nicht
persistiert, wie `zg` — nativ persistierte via `SaveTrackingGrouping`).

- [x] Trackings, Normal-Subtab: Action-Bar zeigt `u group`; `u`
      öffnet das Popup „Group by" in der nativen Optik (Standard-Chrome,
      `●` markiert den aktuellen Zustand (Day), Hotkey-Buchstabe im Label
      unterstrichen, Keybinding-Legende unten).
- [x] `w` springt direkt auf Wochen-Gruppierung (Header `── W24 2026`),
      Summen pro Woche; `u` → `n` entfernt die Gruppierung (flache
      Liste; Aggregat-Spalte + Σ-Footer verschwinden, wie bei `zg` auf
      „ungruppiert").
- [x] Pfeile + Enter/Space wählen ebenfalls; Esc schließt ohne Änderung.
- [x] Condensed-Subtab: `u` → `m` rotiert nur die Tag-Bucket-Ebene auf
      Monat (`── 2026-06`), die adapter-seitige Pro-Task-Aufschlüsselung
      bleibt.
- [x] Auf einer Ebene ohne `group_by` (z. B. Tasks): kein
      `u group`-Hint, `u` bleibt frei für YAML-`shortcuts:`.

## Trackings-Tree: immer ausgeklappt + ohne Marker (`expand_depth: all`)

Native Parität für den Tree-Subtab von Trackings: der Legacy-Tree war
immer komplett offen und hatte keine Aufklappmarker. `expand_depth: all`
(neuer Wert, Kaskade läuft bis nichts Aufklappbares übrig ist) +
`tree_markers.enabled: false` in trackings.yaml.

- [x] Trackings → `t` (Tree): der gesamte Baum ist sofort komplett
      ausgeklappt — alle Ebenen sichtbar, ohne manuelles Enter.
- [x] Keine `▶`/`▼`-Marker vor den Zeilen; die Box-Connectors
      (`├──`/`└──`) bleiben.
- [x] Manuell einen Ast zuklappen (Enter), dann Subtab wechseln und
      zurück: Zustand bleibt — die Kaskade ist one-shot und klappt nichts
      gegen den User wieder auf.
- [x] Saved Query anwenden (`q`): der gefilterte Baum ist ebenfalls
      sofort voll ausgeklappt (neue Query re-armiert die Kaskade).
      Auch mit Cursor tief im Baum + vorherigem manuellen Auf-/Zuklappen
      (Regression: Out-of-Range-Cursor brach den Tabellen-Rebuild ab →
      stale Anzeige).

### Tiefe Bäume klappen vollständig auf (Kaskade bleibt scharf)

Bugfix: die `expand_depth: all`-Kaskade wird pro asynchron eintreffender
Kind-Ebene einmal gepumpt. Bei mehreren Geschwister-Ästen, die parallel
laden, konnte ein Ast „auslaufen" (ein Blatt landet) **während** ein anderer
noch in der Luft war — der Pump für das Blatt fand nichts mehr und
ent-schärfte die Kaskade voreilig. Folge: nur die obersten ein/zwei Ebenen
klappten auf, tiefere Äste blieben zu. Fix: die Kaskade ent­schärft erst,
wenn keine bereits-expandierte Ebene mehr auf ihre Kinder wartet.

- [ ] Trackings → `t` (Tree) mit einem **mehrstufigen** Task-Baum
      (≥3 Ebenen, mehrere Geschwister mit unterschiedlich tiefen Ästen):
      der Baum ist nach dem Laden **komplett** offen bis zum letzten
      getrackten Blatt — nicht nur die obersten beiden Ebenen. Gleichviel
      sichtbar wie im nativen Trackings-Tab.
- [ ] Auch der gruppierte Tree (Tages-Buckets) klappt jeden Bucket-Teilbaum
      vollständig auf, nicht nur die erste Task-Ebene.

### `s` (toggle-tracking) aktualisiert die Ansicht sofort

Bugfix: `s` aktualisierte die TUI in den Trackings-Tabs (flach / condensed /
Tree) meist **nicht** sofort. Grund: der Toggle gab `Noop` zurück und
verließ sich auf Bridge-Row-Patches bzw. (im Tree) auf einen
`PatchRow`-Dispatch. Beide trafen die sichtbare Zeile oft nicht — ein
_Start_ erzeugt ein neues, noch unsichtbares Intervall (keine Zeile zum
Patchen), und `patch_row` durchsucht nur die Tiefe-0-Zeilen, sodass tiefere
Tree-Knoten gar nicht aktualisiert wurden. Die `Noop`/`PatchRow`-Lösung
existierte nur, weil ein voller `Reload` früher die O(N²)-Expand-Kaskade
auslöste (langsam, blockierte Eingabe).

Fix: Mit der Eager-Subtree-Verbesserung (`supports_eager_subtree`) erneuert
ein `Reload` den ganzen aufgeklappten Baum in **einem** `list_subtree`-Call.
Der Toggle gibt deshalb in allen drei Views schlicht `Reload` zurück
(identisch zur Tasks-Logik) — re-foldet Own/Cumulated, Vorfahren-Aggregate
und Marker konsistent. Der `PatchRow`-Dispatch entfällt ganz.

> Beim Smoke-Test **kein** echtes Tracking auf echten Zeilen togglen —
> eine Wegwerf-Aufgabe anlegen und auf der tracken.

- [ ] Trackings → Tree, tiefer/voll aufgeklappter Baum: `s` auf einer
      **verschachtelten** Zeile flippt deren `⏱`-Marker **sofort** (an beim
      Start, weg beim Stopp); der Baum bleibt voll aufgeklappt, der Reload
      ist flott (kein sekundenlanges Zusammenklappen/Eingabe-Freeze), die
      Selektion bleibt auf der Zeile stehen. Kumulierte Sekunden der
      Vorfahren stimmen ohne extra `r`.
- [ ] Trackings → flache Liste (`a`) und condensed (`v`): `s` auf einer
      laufenden Zeile stoppt sie (`⏱` weg, Dauer eingefroren); `s` auf einer
      gestoppten Zeile startet ein neues Intervall, das sofort sichtbar wird.
- [ ] Tasks → Tree: `t` (toggle-tracking) flippt den `⏱`-Marker der
      Zeile sofort (unverändert — nutzte schon `Reload`).

## Trackings-Tree: Gruppierung via Adapter (`group_by_via_adapter`)

Native Parität, Punkt (3): der Legacy-Tree gruppierte nach Tag (ein
Gruppenkopf pro Tag, darunter der Task-Baum mit den Durations nur dieses
Tages). Generischer Mechanismus: Engine reicht das aktive `group_by` im
Root-`list()` durch, Adapter liefert `tracking:tree-group`-Bucket-Knoten
mit per-Bucket gefalteten Teilbäumen; `zg`/`u` = Reload.

- [ ] Trackings → `t` (Tree): Tages-Gruppen als `── label`-Header-Zeilen
      (nicht selektierbar, Header-Style, Label wie in der gruppierten
      Flat-List: `W24 2026-06-08 Mon`), neuester Tag zuerst. Die Task-Zeilen
      darunter starten bei Einrückung 0 (keine Extra-Ebene unter dem
      Header). Voll aufgeklappt (expand_depth-Kaskade), Aufbau flott (kein
      sekundenlanger Aufbau — Folds + Query-Auflösung pro Snapshot
      memoisiert).
- [ ] Teilbaum unter einem Header: Durations sind die des jeweiligen
      Tages (derselbe Task unter zwei Tagen zeigt unterschiedliche
      Werte). Spalten wie nativ: `⏱`, Task, Own, Cumulated; zusätzlich
      schließt eine **Total**-Spalte jeden Tag auf seiner letzten Zeile
      (Stundenzettel-Layout). Cursor überspringt die Header-Zeilen.
- [ ] `zg` rotiert Day → Week → Month → Year → No grouping → Day; jeder
      Schritt lädt neu. „No grouping" zeigt den ungebucketeten Task-Baum
      ohne Header und ohne Total-Spalte (wie vor diesem Feature). `u`-Menü
      springt direkt, `●` markiert den aktiven Zustand.
- [ ] Saved Query (`q`) auf gruppiertem Tree: Buckets + Teilbäume
      re-falten aus den sichtbaren Trackings; leere Buckets verschwinden.
      Gruppierungszustand überlebt das Query-Apply.
- [ ] `s` (toggle-tracking) auf einer Task-Zeile im Bucket funktioniert;
      auf einer Bucket-Zeile ist `s` nicht belegt (read-only Aggregat).
      ⚠ im Smoke-Test nur auf einem Wegwerf-Task togglen.

### `s` im gruppierten Tree aktualisiert nur den Now-Bucket

Im **gruppierten** Tree (z. B. nach Tag) ist jeder Bucket ein eigenständig
aggregierter Teilbaum. Ein `s` (Start/Stopp) verschiebt nur die Totals des
Buckets, in den **„jetzt"** fällt — bei Tages-Gruppierung der heutige Tag,
generell der Bucket der gerade laufenden/zuletzt berührten Buchung. Statt
den ganzen Forst neu zu falten lädt das Frontend deshalb **nur diesen einen
Bucket** neu: Der Adapter sendet das payload-freie `Invalidation::NowAnchored`,
das Frontend fragt `bucket_for_now(spec)` (jüngstes Tracking → dessen Bucket),
holt Header + Teilbaum dieses Buckets und spleißt sie in-place ein; alle
anderen Buckets (inkl. deren Auf-/Zugeklappt-Zustand) bleiben unangetastet.
Ein _Start_, der den ersten Eintrag der Periode anlegt, erzeugt einen
brandneuen Bucket → das Frontend fällt dann auf einen vollen Pane-Reload
zurück (damit der neue Bucket in Sortier-Position erscheint).

> ⚠ im Smoke-Test nur auf einem Wegwerf-Task togglen, nie auf echten Zeilen.

- [ ] Trackings → `t` (Tree), nach Tag gruppiert, mehrere Tage
      aufgeklappt: `s` auf einer Task-Zeile im **heutigen** Bucket flippt
      deren `⏱`-Marker und aktualisiert das Tages-Total dieses Buckets
      **sofort** — die **anderen** Tages-Buckets flackern nicht, klappen
      nicht zu und ihre Totals bleiben unverändert. Selektion bleibt stehen.
- [ ] Ein `s`, das die **erste** Buchung des heutigen Tages anlegt (vorher
      kein heutiger Bucket sichtbar): der neue Tages-Header erscheint in
      korrekter Sortier-Position (Fallback voller Reload), restliche Buckets
      bleiben aufgeklappt.
- [ ] „No grouping" (ungebucketeter Tree): `s` lädt wie gehabt den ganzen
      (einen) Baum neu — kein Now-Bucket-Spezialfall, keine Regression.

### Live-Tick im gruppierten Tree zählt nur den Now-Bucket hoch

Der **statische** Tree-Fold bäckt alle Dauern gegen den Snapshot-Zeitpunkt —
ein bloßes Neuladen desselben Snapshots tickt also _nicht_. Damit die Dauern
im gruppierten Tree live hochzählen, faltet der Adapter pro Timer-Tick **nur
den Now-Bucket** frisch gegen die aktuelle Uhrzeit: der neue Hook
`live_group_rows(spec, query)` liefert den Bucket-Header (Total neu aufsummiert)
plus die **laufende Kette** (laufende Task + ihre Vorfahren, deren kumulierte
Dauer mitwächst) als `Invalidation::Row`-Patches — nur Zeilen, die sich
tatsächlich bewegen, alle übrigen bleiben unberührt. Der Framework-Timer
feuert dabei nur noch ein payload-freies `LiveTick`; die Faltung passiert erst
im Frontend.

**Hintergrund-Tab-Verhalten (bewusst):** Ein Tick eines _nicht aktiven_ Tabs
hat **keine** Auswirkung auf den aktuellen Tab — er wird nicht neu gezeichnet.
Der Tick wird nur als Flag (`pending_live_refresh`) vermerkt und **erst beim
Zurückschalten** auf seinen Tab ausgewertet, und zwar **coalesced**: egal wie
viele Ticks in der Abwesenheit anfielen, beim Zurückschalten läuft genau eine
Faltung gegen den dann aktuellen Stand.

> ⚠ im Smoke-Test nur auf einem Wegwerf-Task togglen, nie auf echten Zeilen.

- [ ] Trackings → `t` (Tree), nach Tag gruppiert: auf einem Wegwerf-Task
      `s` starten. Im **heutigen** Bucket zählen Task-Zeile, deren Vorfahren
      und das Tages-Total **sekündlich/live hoch** — die **anderen** Buckets
      stehen still, flackern nicht und klappen nicht zu. Selektion bleibt.
- [ ] Während die Buchung läuft, auf einen **anderen** Tab wechseln und ein
      paar Sekunden bleiben: der aktuelle Tab zeichnet **nicht** wegen des
      Trackings-Ticks neu. Zurückschalten → die Dauern springen **in einem
      Schritt** auf den jetzt korrekten Wert (kein Nachholen jedes einzelnen
      verpassten Ticks).
- [ ] Idle (keine laufende Buchung): es passieren **keine** Live-Patches —
      der Tree bleibt ruhig, kein unnötiges Neuzeichnen.

## Live-Frische Tasks/Trackings: Marker sofort, externe Starts, adaptives Ticken

Drei Frische-Fixes für die Adapter-Tabs: (1) ein Root-Reload erneuert jetzt
auch alle **aufgeklappten** Tree-Ebenen (vorher blieben deren gecachte
Children stehen → `⏱` erschien auf verschachtelten Tasks nicht sofort);
(2) neuer Trait-Hook `revalidate()` — beim Tab-Wechsel diffen Task-/
Tracking-Adapter die laufenden Trackings gegen die DB und laden bei Drift
neu (externe Starts/Stops via CLI/waybar); (3) die Live-Dauer tickt
adaptiv statt sekündlich (5 s → 10 s → 30 s → 60 s, native Parität).

- [ ] Tasks, Baum aufgeklappt: `s` auf einem **verschachtelten** Task
      → `⏱` erscheint sofort auf der Zeile (kein Zuklappen/Neuladen
      nötig); nochmal `s` → Marker sofort weg.
      ⚠ nur auf einem Wegwerf-Task togglen.
- [ ] Trackings Flat-List: laufende Zeile tickt erst alle 5 s, nach
      einer Minute spürbar seltener (10 s-Sprünge); CPU bleibt ruhig.
      Nach Stop hört das Ticken auf.
- [ ] Extern ein Tracking starten (z. B. CLI `task track …` / waybar),
      während ein anderer Tab aktiv ist → auf Tasks wechseln: `⏱`
      ist da; auf Trackings wechseln: neue laufende Zeile da und
      tickt. Extern stoppen → Tab-Wechsel zeigt den Stop.
- [ ] `r` auf Tasks bzw. Trackings holt dieselbe externe Änderung
      manuell — auch im **Tree** mit aufgeklappten Ebenen (vorher blieb
      dort alter Stand stehen).
- [ ] Trackings Tree/Condensed nach Toggle/Reload: Durations
      konsistent frisch (auch unter Gruppen-Headern).

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
