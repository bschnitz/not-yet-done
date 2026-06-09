# Plan: Cursor-Based Pagination + Per-Node-Action Refactor

> Two coupled feature streams that, together, generalise "execute a query
> and show its result in a pane" so that both `postgres:table` drill-down
> and `postgres:db_script` execute go through the **same** code path,
> backed by a **server-side Postgres cursor** instead of `LIMIT/OFFSET`
> wrapping.

---

## 1. Motivation

Two open issues converge on the same architectural gap:

1. **DB-Script execute (DS-1..DS-6).** The user wants to run a saved DB
   script and see the result in a split pane, just like clicking a
   table. Today there is no path for this: `postgres:db_script` has no
   `level_actions()`, and `execute_custom_query` returns the full result
   set unpaginated for multi-statement scripts (see
   `wrap_for_pagination` in `adapter/mod.rs:448` — returns `None` for
   multi-statement input).
2. **Per-node-type shortcuts.** The user wants `e` on a script node to
   open the editor, `x` to execute, `a` on a Scripts-group to add a
   new script — i.e. the **adapter declares per-node-type actions, YAML
   binds keys**. The current `LevelAction` system is a closed enum with
   a single variant (`AdapterQueryEditor`) and the keybinding is global
   (`ContentAction::EditQuery`), not configurable per node-type.

Solving (1) cleanly requires (2): the adapter exposes named actions per
node-type, and "execute" is just one of them. Solving (2) cleanly
requires the result pane to handle dynamic columns + pagination for
arbitrary SQL, which is exactly what (1) needs.

### Why cursors over LIMIT/OFFSET wrap

For free-form SQL we considered two pagination strategies:

|                                              | LIMIT/OFFSET wrap                                                                     | **Postgres cursor**                                                                    |
| -------------------------------------------- | ------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| Multi-statement scripts (DDL + final SELECT) | Hard — must locate last `;` and inject `LIMIT/OFFSET` only before the final statement | **Trivial** — all statements run inside one tx, final stmt is `DECLARE … CURSOR FOR …` |
| ORDER-BY stability across pages              | Drifts when no ORDER BY (Postgres doesn't guarantee row order across re-runs)         | **Stable** — cursor reads from one materialised snapshot                               |
| Connection model                             | Stateless, one cycle per page                                                         | **Stateful** — tx + cursor live as long as result pane is open                         |
| Complexity                                   | ~150 LOC                                                                              | ~450 LOC, plus the run_with_timeout teardown disentanglement                           |

DBeaver uses cursors (via JDBC `setFetchSize` → server-side cursor) for
exactly these reasons.

---

## 2. Design Principles

- **Separation of concerns.** Three layers, each with a clear contract:
  - **`client/cursor.rs`** — raw `tokio_postgres` cursor primitives.
    Knows nothing about TUI, panes, or YAML.
  - **`adapter/cursor_registry.rs`** — owns the `CursorId → CursorSession`
    map, hands out opaque IDs. The only place that knows a cursor exists.
  - **TUI `ContentPane`** — references a cursor by ID, treats it as
    an opaque pagination handle. Doesn't know about Postgres-isms.

- **DRY for "execute query → paginated result pane".** Both `TableNode`
  drill-down and `DbScriptNode` execute funnel through the same
  `ActionOutcome::ExecuteQuery` → `open_cursor` → pane lifecycle.
  `TableNode::list()` shrinks to "build `SELECT * FROM …` SQL, return
  Execute action" and stops being a special-case in the adapter.

- **Adapter owns intent, TUI owns presentation.** Adapter exposes
  semantic action names (`"execute"`, `"edit"`, `"add"`); YAML binds
  keys; TUI dispatches keys → action names → `Node::invoke_action(name)`.
  Replaces the closed `LevelActionKind` enum.

- **Backward-compatible YAML, opt-in cursor pagination.** Existing
  `pagination: { mode: server }` for table-rows keeps working
  unchanged. Cursor pagination is opt-in via `pagination: { mode: cursor }`
  on the result-pane child def. (We _can_ migrate TableNode's rows to
  cursor in Phase 7 — optional, depending on testing.)

- **Pane cleanup is explicit, not magic.** No async `Drop`. The pane
  lifecycle emits an explicit `ViewRequest::CloseAdapterCursor { id }`
  when destroyed; the App handles it as a fire-and-forget cleanup.

---

## 3. Architecture Overview

```
                  ┌────────────────────────────────────────────┐
                  │           TUI (ContentPane)                │
                  │ - holds CursorId (opaque)                  │
                  │ - "next page" → ViewRequest with CursorId  │
                  │ - close → CloseAdapterCursor(CursorId)     │
                  └────────────┬───────────────────────────────┘
                               │ ViewRequest::RunPostgresQuery
                               │   { ..., cursor_action: Open|Page|Close }
                               ▼
                  ┌────────────────────────────────────────────┐
                  │     adapter/cursor_registry.rs (NEW)       │
                  │ - CursorId → CursorSession map             │
                  │ - open_cursor(db, sql) -> CursorId         │
                  │ - fetch_next(id, n) -> RowsPage            │
                  │ - close_cursor(id)                         │
                  └────────────┬───────────────────────────────┘
                               │ owns
                               ▼
                  ┌────────────────────────────────────────────┐
                  │       client/cursor.rs (NEW)               │
                  │ struct CursorSession {                     │
                  │   client: Arc<tokio_postgres::Client>      │
                  │     (dedicated, not from pool!)            │
                  │   cursor_name: String                      │
                  │   columns: Vec<String>                     │
                  │ }                                          │
                  │ - BEGIN; DECLARE … CURSOR FOR <sql>; FETCH │
                  │ - FETCH FORWARD N FROM cursor              │
                  │ - ROLLBACK (closes cursor + tx)            │
                  └────────────────────────────────────────────┘
```

Per-node action dispatch:

```
keypress 'x' on a postgres:db_script row
   │
   ▼
ChildDef.shortcuts["x"] = "execute"          ← YAML
   │
   ▼
node.invoke_action("execute", ActionContext)  ← adapter trait
   │
   ▼
ActionOutcome::ExecuteQuery { db, sql, target_pane }
   │
   ▼
App opens cursor → renders Split pane → ContentPane holds CursorId
```

---

## 4. Phases

> Each phase = one self-contained commit (or small bundle) with tests +
> `cargo build --release` + `cargo install` + smoke notes. Memory gets a
> per-phase tick.

### Phase 0 — Plan + ADR + Memory anchor

- Write **this file** (`docs/plan-cursor-pagination-and-node-actions.md`).
- Add link from README's docs section.
- Memory: `project_cursor_pagination_plan.md` (this plan in short form
  - phase tracker) so post-compact resumes are clean.

### Phase 1 — `NodeAction` refactor (foundation, no behaviour change yet)

The point: replace the closed `LevelActionKind` enum with named actions,
without changing any UX. After this phase, `q` on a TableNode still
opens the SQL editor — but goes through the new dispatch path.

- **1a — Content trait.** In `not-yet-done-content/src/lib.rs`:
  - New struct `NodeAction { name: String, label: String,
placement: HintPlacement, default_key: Option<char> }`.
    `name` is the semantic identifier ("execute", "edit", "add"…).
    `default_key` lets the adapter suggest a key when YAML doesn't
    bind one (purely cosmetic — YAML wins).
  - New enum `ActionOutcome` — the _result_ of invoking an action:
    ```rust
    pub enum ActionOutcome {
        OpenEditor { kind: EditorKind, params: EditorParams },
        ExecuteQuery { database: String, sql: String, paged: bool },
        CreateChild { parent_node_id: String, child_kind: String },
        DeleteSelf,
        Reload,
        Noop,
        Error(String),
    }
    ```
  - Extend `Node` trait:
    ```rust
    fn actions(&self) -> Vec<NodeAction> { Vec::new() }
    async fn invoke_action(&self, name: &str, ctx: &ActionContext)
        -> Result<ActionOutcome>;
    ```
  - Keep `level_actions()` for now as a temporary shim that returns
    a single `AdapterQueryEditor`-mapped `NodeAction` when `actions()`
    is empty — backward compat while we migrate.

- **1b — YAML.** In `view_config.rs`:
  - `ChildDef` and `ViewDef` get a new field
    `shortcuts: HashMap<char, String>` (key → action name).
  - Validator: warn on shortcuts referencing unknown action names.
  - Smoke-test: round-trip parse of the example with `shortcuts: { x: execute, e: edit }`.

- **1c — TUI dispatch.** New module `not-yet-done-tui/src/app/node_actions.rs`:
  - `resolve_shortcut(view_def, child_def, type_chain, key) -> Option<&str>`
    walks the chain (with the same MT-1 logic) to find the matching
    `shortcuts:` entry.
  - `dispatch_action(adapter, node, name) -> ActionOutcome` + the
    handler that turns each `ActionOutcome` into the right
    `ViewRequest` / FollowUp.
  - Where it plugs in: `ContentPane::handle_key` checks the new
    shortcut map _first_, then falls back to existing keybindings.

- **1d — TableNode migration.** `TableNode::actions()` returns one
  `NodeAction { name: "edit_sql", label: "sql", default_key: Some('q'), … }`.
  `invoke_action("edit_sql", …)` returns
  `ActionOutcome::OpenEditor { kind: EditorKind::AdapterQuery, … }`,
  which the TUI then maps to the existing `OpenAdapterQueryEditor`
  request. Behavioural test: SQL editor still opens on `q`.

- **1e — Delete `LevelAction` / `LevelActionKind`.** Remove the
  closed enum, the trait method, and the dispatch hint code.
  Update `level_action_hints` to use `actions()` + resolved keys
  from the YAML map (fallback to `default_key`).

**Exit criteria:** all 388+ TUI tests green; `q` on a table still opens
the editor; no visible UX change; `LevelActionKind` gone.

### Phase 2 — `PostgresClient` cursor primitives (`client/cursor.rs`)

- **2a — `CursorSession` struct** in a new file `client/cursor.rs`:
  ```rust
  pub struct CursorSession {
      client: Arc<tokio_postgres::Client>,  // dedicated, NOT in PostgresClient.sessions
      cursor_name: String,                  // "_nyd_cur_<uuid7chars>"
      database: String,
      columns: Vec<String>,
      next_offset: u32,                     // logical, for has_prev / position display
  }
  ```
- **2b — Lifecycle methods** on `PostgresClient`:
  - `pub async fn open_cursor(&self, db: &str, sql: &str) -> Result<(CursorSession, RowsPage), String>` —
    opens a dedicated session (using `connect_session`, _not_ the
    `sessions` HashMap), issues `BEGIN; DECLARE _nyd_cur_X NO SCROLL
CURSOR FOR <sql>;`, then `FETCH FORWARD <page_size+1>` for the
    first page, captures `columns` from `RowDescription`. Returns
    session + first page.
  - `pub async fn fetch_cursor_page(&self, session: &mut CursorSession, page_size: u32) -> Result<RowsPage, String>` —
    `FETCH FORWARD <n+1>`; updates `next_offset`.
  - `pub async fn close_cursor(&self, session: CursorSession) -> Result<(), String>` —
    `ROLLBACK` (cleans cursor + tx), drops the client.
- **2c — Multi-statement support.** `open_cursor` accepts the raw
  script body. The implementation parses the trailing `SELECT/WITH`
  (using `looks_like_select_or_with` + `has_multiple_statements`
  helpers that already exist in `adapter/mod.rs`, **promoted to a
  shared `sql_shape.rs` module in client crate**), runs pre-statements
  inside the transaction first, then `DECLARE CURSOR FOR <last>`.
  If the last statement isn't a SELECT/WITH (e.g. DDL), returns
  a non-cursor path: just execute everything, return last
  `RawSqlOutcome` (rows + status).
- **2d — Unit tests** for `sql_shape.rs` (the promotion creates a
  natural test boundary): multi-statement-with-final-SELECT, only
  DDL, comment-only inputs, dollar-quoted strings.

**Exit criteria:** new client/cursor.rs module compiles; unit tests
exercise the SQL-shape splitter on edge cases; integration test
against a real local Postgres opens a cursor on `SELECT generate_series(1,
1000)`, fetches 100 rows twice, closes.

### Phase 3 — Adapter cursor registry + `run_with_timeout` disentanglement

- **3a — `cursor_registry.rs`** in `not-yet-done-postgres-adapter/src/adapter/`:
  ```rust
  pub struct CursorRegistry {
      sessions: Mutex<HashMap<CursorId, CursorSession>>,
  }
  ```
  Public API: `open(db, sql) -> Result<(CursorId, RowsPage)>`,
  `fetch(id, page_size) -> Result<RowsPage>`, `close(id)`.
  `CursorId` is a `String` (UUIDv4 hex prefix) — opaque to TUI.
- **3b — `PostgresAdapter` wires it in.** Adapter holds an
  `Arc<CursorRegistry>` next to the existing `client`.
- **3c — `run_with_timeout` revisited.** Today: on timeout
  `tear_down()` clears `sessions` + drops transport. Problem:
  cursor sessions hold dedicated clients _outside_ the pool, but
  share the **transport**. If transport drops, cursors die.
  - **Decision:** cursor sessions also die on timeout — but
    deterministically. After `tear_down`, the registry is **drained**
    and all `CursorId`s are marked dead. Next `fetch(id, …)` on a
    dead ID returns a clear error → TUI's pane shows "cursor lost,
    press r to re-execute".
  - Implementation: registry has a `live_generation: AtomicU64`
    bumped on teardown; each `CursorSession` records the generation
    it was opened under; `fetch` compares.
- **3d — Tests.** Stub `PostgresClient` (trait-extracted) so we
  can test the registry without a real DB. Test: open two cursors,
  trigger teardown, both fetches return "cursor lost".

**Exit criteria:** registry + adapter wiring compile; teardown logic
verified by unit test; integration test still passes after a
simulated timeout (manual via short `query_timeout_secs: 1` + long
query).

### Phase 4 — `CustomQueryContext` + `PaginationMode::Cursor`

- **4a — `PaginationMode::Cursor`.** In
  `not-yet-done-tui/src/config/view_config.rs`:
  ```rust
  pub enum PaginationMode {
      Server,    // existing — LIMIT/OFFSET-style
      All,       // existing — load everything
      Cursor,    // NEW — server-side cursor
  }
  ```
- **4b — `CustomQueryContext` extension.** In
  `not-yet-done-content/src/lib.rs`:
  ```rust
  pub struct CustomQueryContext {
      pub database: String,
      pub page: PageRequest,
      pub cursor: Option<CursorIntent>,     // NEW
  }
  pub enum CursorIntent {
      Open,                                 // open new cursor for this query
      Continue { cursor_id: String },       // fetch next page from existing
      Close { cursor_id: String },          // tear down (no fetch)
  }
  ```
  Returned `CustomQueryResult` grows `pub cursor_id: Option<String>`
  (set when adapter opened a new cursor).
- **4c — `PostgresAdapter::execute_custom_query`** branches:
  - `cursor: None` → existing path (LIMIT/OFFSET wrap or full load).
  - `cursor: Some(Open)` → `cursor_registry.open(db, sql)` →
    `CustomQueryResult` with `cursor_id: Some(...)`.
  - `cursor: Some(Continue { id })` → `cursor_registry.fetch(id, page_size)`.
  - `cursor: Some(Close { id })` → `cursor_registry.close(id)` →
    empty result.

**Exit criteria:** trait extension compiles cleanly across all
adapters; non-Postgres adapters return `Err("cursor not supported")`
for `Some(Open)` (acceptable — only Postgres opts in).

### Phase 5 — Pane state for cursor pagination

- **5a — `ContentPane` state.** In `content_view.rs:261` extend
  `CustomQueryRunState`:
  ```rust
  pub struct CustomQueryRunState {
      pub query: String,
      pub database: String,
      pub cursor_id: Option<String>,        // NEW
      pub mode: PaginationMode,             // NEW (for branching)
  }
  ```
- **5b — Page-navigation branch.** `try_next_page` /
  `try_prev_page` (lines 2384+) check `mode`:
  - `Server` → existing `RunPostgresQuery` with new `PageRequest`.
  - `Cursor` + `cursor_id: Some` → `RunPostgresQuery` with
    `cursor: Continue { id }`.
  - `Cursor` + `cursor_id: None` → first run, `cursor: Open`.
  - Cursor doesn't support `prev` (NO SCROLL). UX: page-prev key
    re-issues `Open` with the original query → tear-down + new
    cursor. Position memory: keep last N page snapshots in pane
    (cheap, ~100 rows × 10 pages = 1k rows).

  _Alternative for prev that we explicitly rule out:_ SCROLL CURSORs.
  They work, but Postgres' planner sometimes can't materialise
  arbitrary queries scrollably, falling back to materialising the
  whole result set — defeats the lazy-fetch advantage. We
  accept the re-issue cost for `prev`.

- **5c — `apply_custom_query_result`.** When result carries
  `cursor_id: Some(id)`, store it in `CustomQueryRunState.cursor_id`.

**Exit criteria:** TUI tests for cursor-paginated pane lifecycle
(mock adapter that returns deterministic pages).

### Phase 6 — Pane-close → cursor cleanup

- **6a — New ViewRequest variant.**
  `ViewRequest::CloseAdapterCursor { cursor_id: String }`.
  Handler calls `adapter.execute_custom_query(…, CursorIntent::Close)`.
- **6b — Emit on pane destruction.** In `PaneNode::close_leaf`
  (`content_view.rs:513`), before dropping a leaf, walk its
  `CustomQueryRunState.cursor_id`, emit a `CloseAdapterCursor`
  request. Same for coupled split-child closure (line 273).
- **6c — Emit on tab close / app exit.** App-level: shutdown
  hook walks all panes, emits close for each live cursor.
  (Acceptable to drop cleanup on hard crash — Postgres releases
  cursors when the connection closes, which happens on transport
  drop.)

**Exit criteria:** opening a script, paginating, closing the pane
results in zero hanging `idle in transaction` sessions on the DB
(manual smoke).

### Phase 7 — TableNode migration to cursor (optional, can be deferred)

Generalisation: `TableNode::list()` becomes thin — just builds the
`SELECT * FROM <q>.<t>` SQL and returns it via an
`ActionOutcome::ExecuteQuery { paged: true }`. The drill-down
mechanism then opens a cursor for the table just like for a script.

This is the **DRY win**: one pagination path, one renderer, one
cleanup. But it changes behaviour for table-rows (cursor instead of
LIMIT/OFFSET), so it gets its own phase that we can defer if Phase 6
testing reveals stability issues.

- **7a — TableNode delegates to ExecuteQuery action.** The
  `postgres:row`-Child no longer drills via the special-cased
  `TableNode::list()` path; instead the TUI uses the auto-drill
  mechanism to issue an Execute action against a synthesised
  SQL. The `Rows` ChildDef gets `pagination: { mode: cursor }`.
- **7b — Delete `TableNode::list()` SELECT path.** Keep only the
  metadata path (table name, schema, owner) needed for the table
  _node itself_.

**Exit criteria:** drilling into a table works exactly as before, but
under the hood uses the cursor path. Switch is observable only via
`pg_stat_activity` showing the open tx.

### Phase 8 — DbScript actions: `execute` + `edit`

Now that all the plumbing exists, the actual DS feature is small:

- **8a — `DbScriptNode::actions()`** returns:
  - `NodeAction { name: "execute", label: "exec", default_key: Some('x'), placement: ActionBar }`
  - `NodeAction { name: "edit", label: "edit", default_key: Some('e'), placement: ActionBar }`
- **8b — `DbScriptNode::invoke_action("execute", …)`** reads the
  script file, returns
  `ActionOutcome::ExecuteQuery { database, sql, paged: true }`.
- **8c — `DbScriptNode::invoke_action("edit", …)`** returns
  `ActionOutcome::OpenEditor { kind: EditorKind::FileBacked, … }`
  pointing at the script path.
- **8d — New `PostgresDbScriptSession`** in
  `not-yet-done-tui/src/edit_session/postgres_db_script.rs` — analog
  of `postgres_query.rs` but file path comes from
  `db_script_file_path(adapter, db, name)` (helper already exists in
  `query.rs`). `live_apply` and `commit` re-execute the script via
  the same cursor path (re-open cursor with the new SQL).
- **8e — YAML wires it.** User's `postgres.yaml`:
  ```yaml
  - node_type: "postgres:db_script"
    shortcuts:
      x: execute
      e: edit
    children:
      - node_type: "postgres:db_script_result" # NEW result-pane type
        tree_label: name # actually columns dynamic
        split: { direction: right, ratio: 0.8 }
        pagination: { mode: cursor, page_size: 100 }
        column_cursor: true
  ```
  The `postgres:db_script_result` is a synthetic node-type: the
  adapter produces _no_ such nodes via `list()`, but the YAML uses
  the type to anchor the result-pane config (split, pagination,
  keybindings). The TUI's auto-pane-open after Execute consults
  this ChildDef for pane setup.

**Exit criteria:** `x` on a script row opens a split pane with the
result. `e` opens the editor. Save the editor + `:w` re-runs the
script with the new body.

### Phase 9 — DbScript create/delete UX

- **9a — Group action on `DbScriptsGroupNode`.**
  `actions()` returns `NodeAction { name: "add", default_key: Some('a'), … }`.
- **9b — `invoke_action("add", …)`** returns
  `ActionOutcome::CreateChild { … }` which the TUI maps to a
  `:db-script-new` cmdline prompt → creates the file, opens the
  editor.
- **9c — Delete.** `DbScriptNode::actions()` adds
  `NodeAction { name: "delete", default_key: Some('d'), … }`.
  `invoke_action("delete", …)` returns `ActionOutcome::DeleteSelf`,
  TUI confirms + calls `adapter.delete_db_script(db, name)`.

### Phase 10 — Tests + docs + smoke

- **10a — README + generic-view-spec.md** updates:
  - `shortcuts:` field documented.
  - `pagination: { mode: cursor }` documented with the multi-statement
    - ORDER-BY caveats.
  - Adapter-author guide: implementing `Node::actions()`.
- **10b — `docs/smoke-tests.md`** new section "Cursor pagination &
  per-node actions" with:
  - x on db_script → split opens, columns dynamic, > pages forward.
  - Close split → no leaked tx in `pg_stat_activity`.
  - Multi-statement script (CREATE TEMP TABLE … ; SELECT) paginates.
  - DDL-only script → "N rows affected" status, no cursor.
  - Timeout during cursor session → "cursor lost" banner.
- **10c — End-to-end smoke against real DB.** Document required
  manual steps; capture pgstat snapshots before/after.

---

## 5. File Touchpoints (anchor map)

| Concern                                        | File                                                                 | Change Type                                   |
| ---------------------------------------------- | -------------------------------------------------------------------- | --------------------------------------------- |
| `NodeAction`, `ActionOutcome`, `invoke_action` | `not-yet-done-content/src/lib.rs`                                    | Add (Phase 1a, 4b)                            |
| Closed enum removal                            | `not-yet-done-content/src/lib.rs:384–406`                            | Delete (1e)                                   |
| `PaginationMode::Cursor`, `shortcuts:` field   | `not-yet-done-tui/src/config/view_config.rs:338, 297, 601`           | Extend (1b, 4a)                               |
| Shortcut resolver                              | `not-yet-done-tui/src/app/node_actions.rs` (NEW)                     | Add (1c)                                      |
| Cursor primitives                              | `not-yet-done-postgres-adapter/src/client/cursor.rs` (NEW)           | Add (Phase 2)                                 |
| SQL-shape helpers, promoted                    | `not-yet-done-postgres-adapter/src/client/sql_shape.rs` (NEW)        | Refactor out of `adapter/mod.rs:464–532` (2c) |
| Cursor registry                                | `not-yet-done-postgres-adapter/src/adapter/cursor_registry.rs` (NEW) | Add (Phase 3)                                 |
| Run-with-timeout disentangle                   | `not-yet-done-postgres-adapter/src/client/mod.rs:135–171`            | Modify (3c)                                   |
| Pane cursor state                              | `not-yet-done-tui/src/views/content_view.rs:261`                     | Extend (5a)                                   |
| Page-navigation branch                         | `not-yet-done-tui/src/views/content_view.rs:2384–2424`               | Modify (5b)                                   |
| Pane-close hook                                | `not-yet-done-tui/src/views/content_view.rs:513–540`                 | Extend (6b)                                   |
| TableNode delegation                           | `not-yet-done-postgres-adapter/src/adapter/mod.rs:1353+`             | Migrate (7a/b)                                |
| DbScript actions                               | `not-yet-done-postgres-adapter/src/adapter/mod.rs:973+`              | Extend (8a–c)                                 |
| DbScript edit session                          | `not-yet-done-tui/src/edit_session/postgres_db_script.rs` (NEW)      | Add (8d)                                      |
| Smoke + docs                                   | `docs/smoke-tests.md`, `README.md`, `docs/generic-view-spec.md`      | Extend (10a/b)                                |

---

## 6. Risks & Open Questions

1. **`run_with_timeout` teardown.** Forcing all cursors to die on
   timeout is the simple semantics (Phase 3c), but it means a slow
   listing query takes down an unrelated split-pane's cursor. If the
   user finds that disruptive, Phase 3 can be revisited with
   per-session timeout tracking (more bookkeeping; defer until felt).
2. **Connection limits.** Each open cursor pane = one extra Postgres
   connection over the SSH tunnel for as long as the pane is open.
   For users who routinely have 5-10 panes open, that's 5-10 idle
   txs. Acceptable for the workflow we know, but worth flagging in
   the smoke tests.
3. **`prev` page semantics with NO SCROLL.** Re-opening the cursor
   on `<` is the chosen trade-off (5b). If users frequently scroll
   backwards, we could revisit SCROLL CURSORs or client-side cache.
4. **Migration of TableNode (Phase 7).** Initially optional. If
   testing reveals cursor pagination is strictly better than
   LIMIT/OFFSET for tables, promote it. If not, leave TableNode on
   the LIMIT path and accept the dual implementation.
5. **`postgres:db_script_result` as a synthetic node-type.** The
   YAML references a node-type the adapter never produces. This
   pattern is new; an alternative is putting result-pane config
   on the _action_ (`NodeAction.result_pane: Option<PaneSpec>`).
   The synthetic-type approach reuses MT-1's tree-walker; the
   action-config approach is more explicit. Decision deferred to
   Phase 8 — try synthetic first, fall back to action-config if
   the walker gets twisted.

---

## 7. Phase order rationale (why this order)

The order is **bottom-up at the boundary** but **top-down by feature**:

- Phase 1 (NodeAction) first because every downstream phase consumes
  it. It also has zero risk — just refactor with the existing
  behaviour preserved.
- Phases 2–3 (cursor primitives + registry) are isolated to the
  adapter crate and don't touch the TUI. Can be developed against
  unit tests, no end-to-end yet.
- Phases 4–6 wire the cursor into the TUI. Phase 6 in particular
  must come before Phase 7/8 — without cleanup, smoke testing 7/8
  leaks tx.
- Phase 7 is **optional** and can be deferred. Phase 8/9 work
  whether or not 7 ships.
- Phase 10 is the cleanup phase; small incremental doc updates
  inside earlier phases are fine but the big doc sweep waits until
  the surface is stable.
