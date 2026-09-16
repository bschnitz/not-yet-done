# 0012 — One interface: scripts address our own domain through the adapter too

- **Status:** accepted, implemented
- **Date:** 2026-09-16
- **Affects:** supersedes
  [0004](0004-two-cli-binaries-adapter-vs-domain.md). `not-yet-done-task-cli`
  (`nyd-t`) stays in the workspace but is no longer installed and no longer an
  interface anything scripts against; `not-yet-done-cli` (`nyd adapter …`) is
  the single programmatic entry point to tasks and trackings. Doc surfaces:
  `README.md`, `docs/architecture.md`.

## Context

[0004](0004-two-cli-binaries-adapter-vs-domain.md) split the CLI in two. `nyd`
stayed the generic frontend over the `ContentAdapter` protocol; `nyd-t` was
added as a domain CLI on `not-yet-done-task-core`, because the user scripts
needed three things the adapter protocol was believed unable to give:

1. **Join-shaped JSON** — a tracking with its task embedded, and a nested task
   hierarchy. 0004 stated that the protocol "fundamentally cannot reproduce
   that: it returns uniform `NodeSummary`/field projections, not
   domain-specific join JSON".
2. **Graded exit codes** — `4` = path not found, `5` = path ambiguous, which
   the "goto task" scripts branch on. The generic `finish()` path collapses
   every error onto exit code `1`.
3. A conceptual argument: **adapters are interop boundaries** for _foreign_
   systems, not a domain API for our own data.

[0011](0011-tracking-policy-as-an-adapter-setting.md) made the cost of the
second door concrete. The tracking policy — which subtrees are mirrors of each
other, and therefore which overlaps are not overlaps at all — is a setting of
the adapter instance. `nyd-t` does not read it and cannot: it sits beside the
adapter, not behind it. So the same move of the same tracking succeeded through
one binary and failed through the other. Two interfaces onto one domain are two
behaviours, and the one nobody configures is the one that silently disagrees.

That prompted measuring claims 1 and 2 instead of inheriting them, by
re-implementing every script read against the adapter CLI and diffing the
result against the `nyd-t` output on the live database:

- The `trackings` level's row already carries the whole join —
  task id, description, task path, start, end, duration, deleted flag. It is
  `track export`, column for column.
- The flat task level carries an `ancestors` column: the full chain with ids.
  That is **more** than `task tree` returned, and it makes the nested shape a
  regrouping in the caller rather than a second query.
- Every migrated read matched: identical id sets for the `/Work` subtree,
  identical tree structure, identical export entries in identical order,
  identical task paths.

Claim 2 dissolved rather than being worked around. A graded exit code is what
you reach for when the channel back to the caller is one integer. The adapter's
`ls` returns the **candidate list**, so the caller reads the answer directly:
zero matches means create, one or more means navigate — to the oldest, which is
the original. The ambiguous case stops being an error condition to encode and
becomes an ordinary list with more than one element, which is also the safer
reading: an ambiguity can no longer grow another duplicate.

Claim 3 is the one worth reversing on its own merits, not just on measurement.
The adapter protocol is the project's one UI- and CLI-agnostic interface, and
the TUI already drives tasks and trackings entirely through it — every
keystroke, including every write. An interface trusted with that is not a
lowest common denominator we are squeezing our domain through; it is simply the
interface. The standing project rule that new capabilities go _into_ the
adapter interface rather than beside it was already pointing here.

## Options

1. **Keep both interfaces** — the status quo of 0004. Rejected: 0011 showed
   that a setting on the adapter cannot reach the binary that sits beside it,
   so the two interfaces drift apart exactly where behaviour matters most, and
   nothing warns anybody.
2. **Adapter only, delete the crate.** Clean, and it closes the door for good.
   But `backup list` and `backup restore <file>` have no adapter equivalent —
   the adapter exposes only `backup`, which creates one — and a restore is
   precisely the situation in which one wants a tool that does not have to come
   up through the adapter stack first.
3. **Adapter only, keep the crate uninstalled (chosen).** Nothing is installed,
   so nothing can casually reach for it and no script can grow a dependency on
   it; the door is closed by not existing on `PATH`. The crate stays in the
   workspace for the two maintenance verbs and for the cross-process test,
   reached deliberately via `cargo run -p not-yet-done-task-cli -- …`.

## Decision

**Option 3.** Scripts and every other programmatic consumer address tasks and
trackings through `nyd adapter …`, the same interface the TUI uses. `nyd-t` is
uninstalled; it is a maintenance tool built on demand, not an interface.

The scripts share one seam rather than each shelling out on its own: a small
library that wraps `nyd adapter <level> …`, indexes rows by id in the caller,
and resolves task paths itself. Two details of the adapter CLI made the seam
worth having, and both are invisible from a single call site:

- **Ids cannot be selected in a query.** `Uuid` columns are stored as
  `BLOB(16)`, so a `[id, =, …]` clause matches nothing rather than failing
  loudly. One unfiltered `ls` and an index in the caller is the way — and it is
  also faster than one `show` per row.
- **`--path` matches a segment as a substring across every child type**, so a
  path segment can be ambiguous where the tree is not. The seam resolves paths
  against the `ancestors` column instead, matching whole labels and requiring
  the segment count to equal the depth.

## Consequences

- One behaviour, one place to configure it. The tracking policy of
  [0011](0011-tracking-policy-as-an-adapter-setting.md), lifecycle hooks, and
  every future adapter-level setting now govern scripted writes as well,
  because there is no longer a path around them. 0011's "open edge" — that
  `nyd-t track move` keeps the exclusive policy — is closed by removing the
  caller rather than by teaching the binary to read adapter config.
- The join JSON and the nested tree stay available; they are assembled in the
  caller from columns the adapter already returns, not requested as a
  domain-shaped output. The generic contract did not have to grow for this.
- Errors are no longer graded by exit code. A caller that needs to distinguish
  "nothing there" from "several there" reads the list, which is the more
  informative answer anyway.
- `not-yet-done-task-cli` is no longer installed by the build instructions. Its
  integration tests still run in CI, and its cross-process visibility test —
  which needs a genuinely separate process writing to the same database —
  remains the reason the crate is a useful thing to keep compiling.
- 0004's accompanying decisions survive it: `bootstrap::default_task_dsn()` as
  the one source of truth for the task DB location, `open_module()`, and
  `create_backup_at`/`restore_backup_at` are all core-side and are what the
  adapter itself uses.
