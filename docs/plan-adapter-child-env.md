# Plan: Adapter-owned child-process environment (AE)

Status: planned, not started.

## Motivation

The TUI spawns external processes in two places today:

- **Editor** (`$EDITOR` for inline / launch / detached edit-sessions)
  — see `not-yet-done-ratatui/src/utils/open_editor.rs` and the
  `EditorRequest::{Inline,Launch}` dispatch in
  `not-yet-done-tui/src/main.rs`.
- **Scripts** (`:script` menu, action chains, background scripts)
  — see `not-yet-done-tui/src/app/script.rs:612` (interactive) and
  `:673` (background).

For the Postgres LSP (`postgres-language-server`, sqlx-based) to work
against the live DB the editor's child process needs Postgres credentials
in its environment: `PGHOST`, `PGPORT`, `PGUSER`, `PGPASSWORD`,
`PGDATABASE`. Today the LSP can only see whatever the user manually puts
into `postgres-language-server.jsonc` — which means Klartext-Passwort auf
Disk, manual port-tracking (Tunnel ist dynamic), and the .jsonc would
contain Customer-Daten (HARD RULE-Violation).

The cleanest answer (architecturally and pragmatically): the **adapter**
owns the connection state and the credentials, so the **adapter** decides
which env-vars its child tools get. The TUI just pipes the
adapter-provided map into `Command::envs(...)` and looks at no individual
key.

This is the same pattern we already use for tempfile location
(`EditSession::tempfile_dir()`/`tempfile_prefix()`, see EIP feature
committed as `a2593a4`): adapter-specific knowledge surfaces through a
`Option<...>`-style hook, the TUI threads it through without
introspecting.

## Architectural decision (recap of discussion)

1. **New trait method on `ContentAdapter`** —
   `child_process_env(&self, node: &NodeRef) -> HashMap<String, String>`.
   - Sync (no async).
     Password is already resolved into RAM at adapter-connect time;
     command-substitution from `pass` does not happen here.
   - Default impl returns empty map. Jira/Taiga don't implement it;
     they fall back to default.
   - The `node: &NodeRef` parameter is required because env contents
     can be node-specific (e.g. `PGDATABASE=inventory_db` vs.
     `crm_db` depending on which db_script the user is editing).

2. **Bundle editor-spawn settings in a single struct** — replace the
   parallel `tempfile_dir() / tempfile_prefix()` EditSession methods
   with one:

   ```rust
   pub struct EditorSpawnContext {
       pub tempfile_dir: Option<PathBuf>,
       pub tempfile_prefix: Option<&'static str>,
       pub child_env: HashMap<String, String>,
   }
   ```

   `EditSession::spawn_context() -> EditorSpawnContext` is the single
   hook. `EditorRequest::Inline/Launch` carry one `EditorSpawnContext`
   field instead of two/three separate `Option<...>`s. This is the
   "Single-Point-of-Truth" consolidation mentioned in the chat (Punkt 5).

3. **Symmetric usage for scripts** — `App::run_script_background`
   and the interactive script path both flow through the same
   `child_process_env` lookup. Postgres scripts get `PG*` vars for
   free; could in future call `psql` directly. (Punkt 1 from chat.)

4. **TUI sees no Postgres-specifics**. `app/editor.rs`,
   `main.rs::dispatch_editor_request`, `app/script.rs::run_script_*`
   only know `Command::envs(spawn_ctx.child_env)`. The map's contents
   are opaque to them.

5. **Lifecycle**: child env is a snapshot at spawn time. If the
   tunnel reconnects later with a different port, that env is stale
   — but the spawned child is dead by then. No refresh mechanism.

6. **No sensitivity-marker for now**. Premature: only `PG*` vars in
   the Postgres adapter scope, and they only go to processes spawned
   in that adapter's context.

## Phases

### AE-0: Plan + memory anchor (THIS phase)

- `docs/plan-adapter-child-env.md` ← this file
- Memory: `project_adapter_child_env.md` linked from MEMORY.md
- No code.

### AE-1: `ContentAdapter::child_process_env` trait method

`not-yet-done-content/src/lib.rs` (around the existing
`actions_for_type` default impl at line ~629):

```rust
/// Environment variables to propagate to child processes
/// (editors, scripts) spawned in this adapter's context.
///
/// Default: empty. Adapters with connection state expose
/// credentials/endpoints in a form their child tools recognize
/// (e.g. libpq-style `PG*` vars for Postgres). The map is scoped
/// to one spawn and never persisted.
///
/// `node` carries which item in the adapter's tree the spawn is
/// for, so the env can be node-specific (e.g. `PGDATABASE`).
fn child_process_env(&self, _node: &NodeRef) -> std::collections::HashMap<String, String> {
    std::collections::HashMap::new()
}
```

Touch `not-yet-done-content/src/mock.rs` only if mock-adapter tests
exercise the default — otherwise leave as-is.

### AE-2: `PostgresAdapter::child_process_env` implementation

`not-yet-done-postgres-adapter/src/adapter/mod.rs:34` —
`PostgresAdapter` already holds `Arc<PostgresClient>`. The client
keeps:

- `transport_cfg: TransportConfig` (host of the _target_, not the local tunnel)
- `auth: PostgresAuth` (user + resolved password + default database)
- `transport: Mutex<Option<TransportConnection>>` — has the live
  `local_port` once the tunnel is up

Wire it like this:

1. Add `PostgresClient::tunnel_local_port(&self) -> Option<u16>` and
   `PostgresClient::pg_env_base(&self) -> HashMap<String,String>`
   that exposes everything _except_ `PGDATABASE` (which depends on
   the node).
2. In `PostgresAdapter::child_process_env`:
   - Parse `node` to derive the database name. The Postgres tree is
     `postgres:database`-rooted, so the NodeRef path will contain
     the db. Use the same parsing the cursor-registry uses.
   - If the tunnel isn't open yet, return empty map. Don't trigger
     a connect — that would deadlock the sync trait method.
   - Otherwise compose:
     ```
     PGHOST=127.0.0.1
     PGPORT=<tunnel.local_port>
     PGUSER=<auth.user>
     PGPASSWORD=<auth.resolved_password>
     PGDATABASE=<db from node>
     PGSSLMODE=disable   ← tunnel is localhost, no SSL needed
     ```

   Returning empty when transport is None is fine: the LSP just sees
   no env, and falls back to its existing (now empty) jsonc-driven
   behaviour, which is "offline mode" — what we have today.

3. Sync access to the tunnel state: `Mutex<Option<TransportConnection>>`
   is a tokio Mutex (see `client/mod.rs:93`). We need sync access from
   `child_process_env`. Two options:
   - **(preferred)** Cache `local_port` in an `Arc<AtomicU16>` on the
     client, updated whenever the tunnel opens/tears down. Sync read.
   - Refactor `child_process_env` to `async`. Rejected: forces every
     call site into async context, including the Command::envs flow.

   Go with the AtomicU16 cache.

### AE-3: `EditorSpawnContext` struct + EditSession consolidation

`not-yet-done-tui/src/edit_session/mod.rs`:

- Add struct `EditorSpawnContext` (see decision §2).
- Replace `tempfile_dir()`/`tempfile_prefix()` with:
  ```rust
  fn spawn_context(&self) -> EditorSpawnContext {
      EditorSpawnContext::default()
  }
  ```
- `PostgresDbScriptSession` overrides — pulls `tempfile_dir` from its
  `in_place: bool` + the existing path logic, and pulls `child_env`
  from `self.adapter.child_process_env(&self.node_ref)`.

  Issue: `PostgresDbScriptSession` currently holds an `Arc<dyn ContentAdapter>`
  or similar via the postgres adapter handle. Check
  `not-yet-done-tui/src/edit_session/postgres_db_script.rs` for the
  adapter handle field. If it's a concrete `Arc<PostgresAdapter>`,
  just call directly. If it's been kept generic, narrow at the
  session boundary.

### AE-4: `EditorRequest` plumbing through TUI

`not-yet-done-tui/src/app/editor.rs`:

- Replace the existing `tempfile_dir`/`tempfile_prefix` fields on
  `EditorRequest::Inline` and `EditorRequest::Launch` with one
  `spawn_context: EditorSpawnContext`.
- Detached editor path same.

`not-yet-done-tui/src/main.rs`:

- `dispatch_editor_request` destructures `spawn_context` and passes
  it to `run_inline_editor_get_content` / `run_launch_editor` /
  detached-spawn helper.
- Those helpers take `spawn_context: &EditorSpawnContext` and:
  - call `open_editor_*_in(suffix, content, tempfile_dir, tempfile_prefix)` as today
  - additionally apply `child_env` to the spawned `Command`

`not-yet-done-ratatui/src/utils/open_editor.rs`:

- `open_editor_inline_in` / `_launch_in` / `_detached_in` already
  exist (from EIP). Add a `child_env: Option<&HashMap<String,String>>`
  parameter to each — internal `Command::envs(map)` call right
  after `Command::new(...)`.

### AE-5: Script spawn parity

`not-yet-done-tui/src/app/script.rs`:

- `run_script_background` (line ~649) and `run_script_interactive`
  (line ~612) currently use `std::process::Command` with manual
  `env("NYD_OUTPUT_FILE", ...)`.
- Look up the adapter-owned child env via `ScriptContext` — context
  already carries which adapter the script runs in.
- Apply with `cmd.envs(child_env)` _before_ the existing
  `NYD_OUTPUT_FILE` env so adapter vars can't accidentally clobber
  `$NYD_*`.

If `ScriptContext` doesn't currently know its source adapter,
extend it with `adapter: Option<Arc<dyn ContentAdapter>>`. Tracking
script-context (Tasks-Tab) has no adapter; remain `None` → empty
env, no behaviour change.

### AE-6: jsonc minimization + Doku + Smoke

- User's `~/.local/share/not_yet_done/postgres/postgres/db_scripts/inventory_db/postgres-language-server.jsonc`
  — strip down to `{}` or just keep linter rules. Drop the dummy
  `db`-block.
- Verify: TUI starts, tunnel comes up, `e` on a db_script opens
  nvim with `PGPASSWORD` etc. set. LSP attaches and provides
  schema-aware completion against the live DB.
- `docs/generic-view-spec.md` — new section "Adapter child-process env"
  near the existing "Edit-in-Place" section. Note: env-map is opaque
  to the TUI; adapters document their own keys.
- `docs/smoke-tests.md` — new "AE — Adapter child-process env"
  section with:
  - Tunnel open → `e` on db_script → LSP completion works
  - Tunnel closed (manual_connect, no auto-warmup) → `e` works,
    LSP shows no completion (graceful empty env)
  - Script in postgres view sees `PGPASSWORD`
  - Tasks script (no adapter) sees empty adapter env (no regression)
- `npx prettier --write docs/*.md`

### AE-7: Build, tests, install, commit

- `cargo build --release` workspace-wide
- `cargo test --release --lib --bins` workspace-wide — expect 450+
  tests still green
- New unit tests:
  - `PostgresAdapter::child_process_env` returns empty when tunnel
    closed
  - Returns PG\* vars when tunnel open (mock the AtomicU16 + auth)
  - `EditorSpawnContext::default()` is empty
- `cargo install --path not-yet-done-tui --offline`
- Commit (one bundle covering AE feature). Ask user before push
  (`feedback_no_push`).

## File touch list (rough estimate)

| Phase | Files                                                      | LoC est. |
| ----- | ---------------------------------------------------------- | -------- |
| AE-1  | content/src/lib.rs (+test impls)                           | ~20      |
| AE-2  | postgres-adapter/src/{client,adapter}/                     | ~80      |
| AE-3  | tui/src/edit*session/{mod,postgres*\*}.rs                  | ~60      |
| AE-4  | tui/src/{app/editor,main}.rs, ratatui/utils/open_editor.rs | ~100     |
| AE-5  | tui/src/app/script.rs                                      | ~30      |
| AE-6  | docs + user jsonc                                          | ~50      |
| AE-7  | tests, build, commit                                       | --       |

Total ~340 LoC + docs. Single logical feature, one commit.

## Carry-over context (post-compact resume reference)

Local unpushed commits as of plan creation:

- `a2593a4` (EIP edit-in-place — committed today, the LSP-Path
  feature this plan extends)
- `0be2ec4` (db-script lazy nodes)
- `fc78bef` (db-script extensions)
- `b29ee3b` (editor-suffix + SQL flavor template)

User constraint reminders (CLAUDE.md):

- HARD RULE: no real customer/user/host/DB data in repo
- Never `git push` without explicit permission
- Klärungsfragen in Prosa, not AskUserQuestion-Tool
- `cargo install --path not-yet-done-tui --offline` after TUI changes
- `npx prettier --write` after markdown edits

User-Setup:

- nvim 0.12.2 with `postgres_lsp` configured via Mason
  (`postgres-language-server` v0.25.0 at
  `~/.local/share/nvim/mason/bin/postgres-language-server`)
- LSP root marker: `postgres-language-server.jsonc`
- LSP uses sqlx 0.8.6 — respects `PGHOST/PGPORT/PGUSER/PGPASSWORD/PGDATABASE/DATABASE_URL`
  env vars, and `~/.pgpass`/`PGPASSFILE`
- LSP completion is _fully DB-dependent_ — `disableConnection: true`
  yields zero completion items. The whole point of this plan is to
  give the LSP a working connection via the existing tunnel.
- `postgres-adapter.yaml` uses `transport.mode: ssh_tunnel` with a
  2-hop SSH chain → target Postgres. Tunnel listens on
  `127.0.0.1:<dynamic_port>`.

Why not other options:

- A1 (cleartext password in jsonc): violates HARD RULE for shared
  workstations; manual port tracking.
- A2 (Postgres-wire-protocol proxy in TUI): ~500 LoC with `pgwire`
  crate; correct but disproportionate.
- A3 (TUI generates jsonc on start, deletes on shutdown): file
  lifecycle conflicts with manual editing; crash-state messy.
- A4 (this plan, env-vars via adapter): minimal, correct,
  generalisable to scripts, respects architectural boundary.
