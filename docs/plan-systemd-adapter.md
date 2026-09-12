# Plan: a systemd adapter

> **Status: planned.** Nothing of this is implemented yet. The document is
> written to be read phase by phase: each phase names what it builds, and —
> deliberately — which questions are left open until the phase before it is
> done. Decisions already taken are recorded under
> [Decisions](#decisions-taken), with the reasoning, so a later reader does
> not have to reconstruct why the shape is what it is.

## Goal

A tab that browses and operates the systemd manager as a content tree:
services, timers and unit files as tables; a unit's properties, journal,
dependencies, drop-ins and processes as levels below it; start/stop/enable
and the rest as node actions; and — the part `systemctl` has no answer for —
creating and editing units comfortably, including the timer/service pair that
nobody gets right by hand.

User manager first. The system manager is a later phase and a different
problem (privileges), not a switch to flip.

## Working method: phases with a decision gate

Every phase opens with a short round of questions and closes in a state that
builds, installs and is usable on its own — no half a tab waiting for the
next phase. What a phase does not decide, it does not anticipate either: the
code stays additively extensible at those seams instead of casting an
assumption in concrete.

The phase table is the index; the per-phase sections say what is open and why
it is open _there_ rather than now.

| Phase                                                    | Delivers                                                                       | Decided in the round before it                                            |
| -------------------------------------------------------- | ------------------------------------------------------------------------------ | ------------------------------------------------------------------------- |
| [0 — Skeleton & reading](#phase-0--skeleton--reading)    | crate, factory, config, root + services/timers/unit files read-only, view YAML | node type names, level set, config shape, columns — **done**              |
| [1 — Control](#phase-1--control)                         | start/stop/restart/enable/…, job tracking, protection list, properties level   | key layout, confirmation policy, protection list                          |
| [2 — Editing](#phase-2--editing)                         | `cat` as content, drop-in and full edit, `verify`, `daemon-reload`             | drop-in vs. full as the default, backup strategy, restart prompt          |
| [3 — Creating](#phase-3--creating)                       | form wizard, skeleton templates, **timer + service as a pair**                 | form vs. editor default, `natural-date` → `OnCalendar`, templates         |
| [4 — Journal](#phase-4--journal)                         | `systemd:log` level, cursor pagination, `highlights:`, `--follow`              | level or external terminal, query syntax, follow yes/no                   |
| [5 — Live](#phase-5--live)                               | D-Bus signals → `Invalidation::Row`, dependency tree, timer countdown          | `kind: countdown` and `kind: bytes` generic or adapter-side, signal scope |
| [6 — Diagnostics](#phase-6--diagnostics)                 | `analyze security` / `blame`, drift audit, resource columns, waybar            | which of the four, in which order                                         |
| [7 — Beyond the user bus](#phase-7--beyond-the-user-bus) | system bus + polkit, remote hosts, containers                                  | polkit route, subtabs vs. instances, whether at all                       |

After phase 2 this is a tab one uses daily. After phase 4 it replaces
`systemctl` and `journalctl` for a handful of user units. From phase 5 it
becomes something a command-line tool cannot be.

---

## Decisions taken

### D1 — D-Bus, not a `systemctl` subprocess

The adapter talks to `org.freedesktop.systemd1` over D-Bus via **zbus**
([z-galaxy/zbus](https://github.com/z-galaxy/zbus), 748★, last push
2026-09-11, v5.19, 81 M downloads), with **zbus_systemd**
([lucab/zbus_systemd](https://github.com/lucab/zbus_systemd), 56★, last push
2026-06-26) for the generated proxies — its versioning tracks the systemd
release (`0.26100.x` = systemd 261), which is the version on this machine.
zbus is already the entry in the curated `rust-lib-reference.md`; note that
the repository moved from `dbus2/zbus` to `z-galaxy/zbus`.

Rejected: `systemctl --output=json` as the primary route (one process spawn
per refresh, no push, text parsing), and `dbus-rs` (sync-leaning C binding to
libdbus).

**Why it matters beyond taste.** `Subscribe()` plus the `JobNew` /
`JobRemoved` / `UnitNew` / `PropertiesChanged` signals deliver state changes
_pushed_, and the frontend seam for that already exists:
`Invalidation::Row(NodeSummary)` replaces a single row in place, keeping
selection and scroll. A restart is then visible as `active → deactivating →
activating → active` without a single reload. Polling a subprocess could
never show that.

**Where subprocesses stay**, via `tokio::process`, because these have no D-Bus
interface:

- `journalctl --output=json` (phase 4). The sd-journal C API is the
  alternative and is not worth the FFI.
- `systemd-analyze verify | security | calendar | blame | critical-chain`
  (phases 2, 3, 6).

`systemctl cat` is _not_ in that list: the assembled unit text is
`FragmentPath` + `DropInPaths` read from disk, which the adapter can do
itself.

### D2 — Filtering is a query, not configuration

The adapter config carries **no** `include:` / `exclude:` globs. What is
shown is a saved query, switchable at runtime.

The distinction is worth stating because the SQLite adapter looks like a
counter-example. Its `sources:` globs exist for a **cost** reason: without
them the adapter would have to crawl the filesystem to learn which databases
exist at all. systemd has no such cost — `ListUnits` and `ListUnitFiles` are
one D-Bus call each for everything, milliseconds for the ~170 unit files of a
user session. A static `include:` would therefore be nothing but a worse
query: the same filtering, minus the ability to change it without a config
reload.

So the native query language is the **`rowsieve` `FilterExpr`** that the tasks
and trackings adapters already use — `and`/`or` over `[field, op, value]`
triples, evaluated in memory over the rows the adapter has just built. That
buys, without writing any of it:

- the `q` menu over saved queries, `:query new|edit|delete`, the query editor
- `ctrl+f` keyboard shortcuts onto individual saved queries
- `${var}` placeholders via the shared `query_vars` helper — the frontend
  opens the input popup, the adapter delegates
- extended queries on top (set operations across several queries)

Examples of what saved queries then are:

```yaml
# failed.yaml
name: Failed
query: [active, =, failed]

# mine.yaml — only what lives in ~/.config, not the distribution's units
name: My units
query: [fragment, like, "/home/%"]

# flapping.yaml — running, but restarting over and over
name: Flapping
query:
  and:
    - [active, =, active]
    - [restarts, ">", 0]

# by-name.yaml — prompts for the pattern when invoked
name: By name
query: [name, has, "${pattern}"]
```

No second syntax next to it. A journalctl-style short form (`--state=failed`)
would be its own parser and its own error surface; if it turns out to be
missed, it can be an alias layer above the `FilterExpr` later.

**Deferred optimisation.** systemd offers `ListUnitsByPatterns(states,
patterns)`, so name and state clauses could be pushed down into the D-Bus call
instead of being filtered locally. At ~170 units that is measurably pointless.
It is noted here so it is not rediscovered, and ignored until a system bus
with several hundred units makes it hurt.

### D3 — One node type per unit kind

`systemd:manager`, `systemd:service`, `systemd:timer`, `systemd:socket`,
`systemd:path`, `systemd:target`, `systemd:unitfile`, `systemd:property` —
rather than one generic `systemd:unit` with a kind column.

These names end up in the user's view YAML, in `node_scripts` paths and in
script scopes, so renaming them later costs configuration, not just code.
Separate types mean a level can carry its own columns and its own actions: a
timer wants `next` / `left` / `last`, a service wants `pid` / `mem` /
`restarts`, and neither wants the other's empty cells.

### D4 — No hiding default query

The services, timers and unit-files levels load unfiltered. A default query
that hides rows is the classic twenty-minutes-of-searching bug — "the unit is
not there" when it is only filtered out. At the size of a user session there
is nothing to gain by hiding.

---

## Phase 0 — Skeleton & reading

**Delivers** a new crate `not-yet-done-systemd-adapter`, its factory
registered in `not-yet-done-host`, and a read-only tab.

Levels in this phase: **Services**, **Timers**, **Unit files** — and
**Failed**, which turned out not to be a level at all. It is the Services
level with a default query (`[active, =, failed]`), which is D2 applied to
this adapter's own tab: a question that narrows a level is a view, not a node
type. Its one limitation is worth stating, because it is invisible from the
tab: a failed _timer_ does not appear there, since the level lists services.

Sockets, paths, targets, mounts and slices are deliberately left out — once
the mechanism stands they are a view-YAML addition, not code, which is the
cheapest possible way to add them later.

The adapter config is what is left after D2:

```yaml
manager: user # user | system (system is phase 7)
timeout_secs: 10 # deadline per D-Bus call
```

Journal settings join it in phase 4. `manager:` is a field from day one even
though it only accepts one value, so a second manager is an additive change
rather than a schema break — that is the only concession this phase makes to
a later one.

Columns, as a starting point to be cut down once real data is on screen:

- **Services** — `name`, `description`, `load`, `active`, `sub`, `enabled`,
  `since` (the `ActiveEnterTimestamp` itself, `kind: datetime` — a live-ticking
  `kind: elapsed` column is a view-YAML addition over the same value, not a
  second cell), `pid`, `mem`, `tasks`, `cpu`, `restarts`, `needs_reload`,
  `fragment`
- **Timers** — `name`, `next`, `left`, `last`, `passed`, `unit`, `result`,
  `enabled`, `persistent`
- **Unit files** — `name`, `state`, `path`, `vendor`

`restarts` (`NRestarts`) earns its place: a unit with `Restart=always` that
crashes in a loop reads as "active" in `systemctl status`. A column plus a
`highlights:` rule on `> 0` shows it at a glance.

**`preset` and `drift` fell out of the Unit files level.** They were planned
on the assumption that systemd reports a unit file's preset the way it reports
its state; it does not. There is no `GetUnitFilePreset` on the manager
interface — `UnitFilePreset` exists only as a property of an already-_loaded_
unit, which is precisely the population this level exists to look past — and
`systemctl` computes the preset locally by walking
`/etc/systemd/user-preset/`, `/usr/lib/systemd/user-preset/` and friends.
That parser is work of its own with its own rules (first match wins, glob
patterns, the `enable`/`disable` verbs), not a free column, so it belongs with
the **drift audit** in [phase 6](#phase-6--diagnostics) where the same parser
pays for both. What the level does carry instead is `vendor` — where the file
comes from (`vendor` / `admin` / `runtime` / `user`), which falls straight out
of its path and answers the cheap half of the same question.

**`mem` carries raw bytes, not a formatted string.** A pre-rendered
"95.4 MiB" sorts as text (the sort compares cell strings) and cannot be
compared in a query, so the column would look right and behave wrong. The
adapter therefore emits the number and the display gap moves to the table
engine, which has no `kind: bytes`. That is the same shape as the parked
`kind: countdown` below, and the two are decided together in
[phase 5](#phase-5--live).

`left` is a **countdown** — `field − now`, into the future — where the table
engine's `kind: elapsed` computes `now − field`. In this phase the adapter
formats the string itself; that is the throwaway variant, which is exactly
why it is right here. Whether the engine gets a generic `kind: countdown` is
decided in phase 5.

**Anonymisation** is a footnote, not work: the default `StandardAnonymizer`
already covers the adapter. Unit names are harmless, but `FragmentPath` and
`ExecStart` carry usernames and customer names, and the default masks those.
A `scrub_label` override that keeps unit names unit-shaped is cosmetic and
can wait.

## Phase 1 — Control

**Delivers** the actions that make the tab more than a viewer, plus the
`systemd:property` level (all of `show`, as a filterable table — which is
where a 200-property wall becomes usable).

Fourteen actions on the `a` leader, one level of chord. Related letters sit
together, and a capital means "and now":

| Key   | Verb              | Key   | Verb          |
| ----- | ----------------- | ----- | ------------- |
| `a s` | start             | `a e` | enable        |
| `a x` | stop              | `a E` | enable --now  |
| `a r` | restart           | `a d` | disable       |
| `a l` | reload            | `a D` | disable --now |
| `a L` | reload-or-restart | `a m` | mask          |
| `a f` | reset-failed      | `a u` | unmask        |
| `a k` | kill (signal)     | `a z` | freeze / thaw |

`preset` is **not** here. Without knowing what the vendor preset says, the key
does something you cannot predict from the row in front of you — so it moves to
phase 6, where the drift audit shows the preset next to the current state and
the verb finally has a visible meaning.

Which verbs a level offers is the adapter's decision, not the view's: each verb
declares the scope it works in. A timer has no processes and so is never offered
`reload`, `kill` or `freeze`; a unit file is not loaded and so is never offered
`reset-failed` — but it _is_ offered `start`, because `StartUnit` takes a name
whether the manager has loaded it or not, and the unit-files level is the only
one that can see a unit the manager has never touched.

**Job tracking is the substance of this phase.** `StartUnit` returns a job
path; the adapter waits for `JobRemoved` and reports the real outcome
(`done` / `failed` / `timeout` / `dependency`) instead of "command sent".
While the job runs, `StatusReporter::busy("Starting backup-photos.service")`. On
failure, the unit's last journal lines go straight into the notification —
that is the moment they are wanted.

### Confirmation and protection are different instruments

**Confirmation** asks "did you mean it". The adapter writes the question,
because only it knows the consequence — that stopping a unit stops whatever
depends on it, that a mask survives a reboot. It is returned as
`ActionDispatch::Confirm { prompt }` on the first invocation and the frontend
re-invokes the same action with `confirmed: true`.

Asked: `stop`, `disable`, `disable --now`, `mask`. Not asked: `start`,
`restart`, `reload`, `reload-or-restart`, `enable`, `enable --now`, `unmask`,
`reset-failed`, `freeze` — each is either additive or plainly reversible by the
key next to it. `kill` is asked in a different currency: it opens the signal
menu, and picking SIGKILL out of a list is the deliberate act a y/n would
otherwise stand in for.

**Protection** is not a question. A prompt is no answer to "this would take the
session down", because the muscle memory that pressed the key presses `y` too.
A protected unit **refuses** the disruptive verbs outright and still accepts the
additive ones.

The built-in list covers the session plumbing — `*.slice`, `*.scope`,
`dbus.socket`, `dbus.service`, `dbus-broker.service`, `default.target`, `basic.target`,
`sockets.target`, `timers.target`, `paths.target`, `graphical-session.target`,
`graphical-session-pre.target`. Config adds to it with `protect:` and lifts an
entry with `unprotect:`, which must spell the entry exactly; an `unprotect:`
that matches nothing is refused when the config loads, because otherwise a
misspelling looks like success while the protection is still in place.

### The reload-plus-message gap

Every verb here both changes the row and has a verdict worth reading, and
`ActionDispatch` had no variant for that: `Reload` refreshes without speaking,
`Notify` speaks but explicitly does not reload. A `start` that reports "started"
while the row still says `inactive` tells the user half the truth. Phase 1 adds
`ActionDispatch::Done { message: Option<String> }` to the content protocol —
additively, so no existing adapter changes.

## Phase 2 — Editing

**Delivers** the assembled unit as `Content` (syntax `systemd`/`ini`), an
edit path, `systemd-analyze verify`, and `daemon-reload`.

The editor-template seam fits unusually well here because **the template is
the unit file itself** — no translation, so none of the round-trip risk the
Jira wiki markup carries. Only a comment header with the current state and
`# available:` hints for enumerated directives.

Two variants to choose between as the default:

- **drop-in** (what `systemctl edit` does) — writes
  `~/.config/systemd/user/<unit>.d/override.conf`, leaves the vendor unit
  alone
- **full** (`--full`) — copies the vendor unit into `~/.config` on first edit

Write scope in this phase is `~/.config/systemd/user` only. The system
manager is phase 7 and a privilege question, not an editing question.

**Order matters:** `verify` runs _before_ `daemon-reload`, never after. A
rejected file is not activated, the message goes to the notification centre,
and the buffer stays open. Afterwards the user is asked what to do —
restart, reload-or-restart, or just the daemon-reload.

**Open until the round before this phase:** drop-in or full as the default;
the backup strategy before overwriting a unit file (the local adapter's
backup-on-hook pattern is the obvious template); and how insistent the
restart prompt should be.

## Phase 3 — Creating

**Delivers** three ways to make a unit, which coexist rather than compete:

1. **A form** (`InputSpec::Form` on the existing spec-driven form driver) —
   name, description, type, `ExecStart`, working directory, restart policy,
   `After=`, environment, plus "enable now" / "start now" toggles. The
   adapter writes the file, reloads, optionally enables and starts.
2. **The editor template** from phase 2, for the full file.
3. **Skeletons from a directory** —
   `~/.config/not_yet_done/systemd-templates/*.service`, picked via
   `create_child`: oneshot script, long-running daemon, path watcher pair,
   socket activated, resource-limited service. Files, not Rust constants, so
   the set grows without a rebuild.

**The timer/service pair is the point of this phase.** A timer alone is
useless; one always writes two files and forgets half of one. One form, one
result: name, what it runs (new `ExecStart` _or_ an `option_menu` over
existing services), schedule, `Persistent=`, `RandomizedDelaySec=`,
`AccuracySec=`, enable now. The adapter writes `foo.service` and `foo.timer`
together.

Two candidates to decide on:

- **`natural-date` → `OnCalendar`.** The crate is in this workspace already.
  "every monday at 9" → `Mon *-*-* 09:00:00`, validated through
  `systemd-analyze calendar`, which also returns the next elapses.
- **A live preview inside the form** — a derived field the adapter recomputes
  on every change (the counterpart to the existing `FormNotice`), showing the
  normalised expression and the next firing while the user types. Generic,
  not systemd-specific. The cheap alternative is a `validate` action that
  writes the same information into a notification: ~90 % of the value for
  ~10 % of the work.

## Phase 4 — Journal

**Delivers** `systemd:log` as a level under any unit, from
`journalctl --output=json --user-unit=<unit>`.

- Columns `time`, `prio`, `pid`, `message`; multi-line rows for stack traces
  are already available (`row_layout`).
- Severity colouring is **pure configuration** via `highlights:` — no code.
- Pagination rides on journalctl's `--after-cursor`, which maps directly onto
  the existing cursor pagination rather than onto offsets.
- `--follow` as a background task pushing `Invalidation::Row` per line — the
  same pattern the Stoat adapter uses for incoming messages, including the
  `first_unread` behaviour.

**Open until the round before this phase:** whether the journal is a level at
all or just a key that opens `journalctl -fu <unit>` in a terminal on the
current workspace (much cheaper, much less); the level's query syntax; and
whether follow is in scope.

## Phase 5 — Live

**Delivers** the payoff of D1: `Subscribe()` plus signal handling, so unit
state changes arrive as `Invalidation::Row` and rows update in place. Plus
the recursive `systemd:dep` dependency tree — which `systemctl
list-dependencies` can only print as text, and which here gets filtering,
sorting, drilling and per-row actions.

**Open until the round before this phase:** whether the table engine gets a
generic `kind: countdown` (useful well beyond systemd — calendar events want
it too) or the adapter keeps formatting the string; whether it also gets a
`kind: bytes`, which phase 0 walked into with `mem` (see above) and which is
the same question one step over — a column whose value must stay a number to
sort and compare, but whose display is not the number; and how wide the signal
subscription should be, since `PropertiesChanged` on every unit is a lot of
traffic for rows nobody is looking at.

## Phase 6 — Diagnostics

**Delivers**, in an order to be decided, the commands that exist but that
nobody runs:

- **`systemd-analyze security <unit>`** as a level: one row per setting, with
  its exposure weight and status, sortable by weight — and `enter` on a row
  jumping into the drop-in edit with that directive prefilled. That turns an
  audit command into a workflow.
- **`systemd-analyze blame` / `critical-chain`** under the manager: startup
  time per unit, sortable.
- **Drift audit** as saved queries: `is-enabled` vs. preset, units needing a
  daemon-reload, units with drop-ins, units in `~/.config` shadowing
  `/usr/lib`, and a vendor diff for the last of those — the question one
  actually has after a package update. This is also where the `preset` and
  `drift` columns that fell out of [phase 0](#phase-0--skeleton--reading)
  return: they need a parser for the preset files, and so does this audit, so
  one parser serves both.
- **Resource view**: `MemoryCurrent`, `CPUUsageNSec`, `TasksCurrent`,
  `IOReadBytes` as sortable columns — a `top` over units, free, because the
  properties are already being read.
- **Waybar**: a failed-units indicator through the existing CFFI module.
- **A `connected` hook** that raises a notification when units are failed at
  startup, so it lands in the notification centre without anyone looking.

## Phase 7 — Beyond the user bus

**Delivers** the system manager, and possibly remote hosts (`-H`) and
containers (`-M`).

The hard part is not reading — unprivileged reads on the system bus are
allowed — but writing, which goes through polkit and therefore needs an
agent, or `pkexec`, or the `SUDO_ASKPASS` route. That is its own decision and
deliberately sits behind a finished user-level tab.

The second question here is shape: several managers as subtabs of one
instance pinned by `query: "manager:<id>"`, the way the mail adapter holds
six IMAP accounts (with `subscribe_status_for` per subtab), or simply one
instance per manager. D1's `manager:` config field keeps both open.

---

## Safety net

This adapter writes into the running session, which none of the others do.
The obligations, distributed over the phases that introduce the risk:

- **Protection list** (phase 1) — stop/mask on `init.scope`, `*.slice`,
  `dbus.socket`, `graphical-session.target` is refused, not confirmed.
- **`confirm: true`** (phase 1) on stop, disable, mask, delete; mask with an
  explicit warning about what masking means.
- **Backup before every unit-file write** (phase 2), adapter-side, following
  the local adapter's pattern.
- **`verify` before `daemon-reload`** (phase 2), never after.
- **Anonymisation** (phase 0) — the default already covers it; an override is
  legibility, not safety.

---

## Implementation notes for phase 0

Collected while surveying the existing adapters, so the phase does not start
by rediscovering them.

**Dependencies.** `zbus_systemd` is feature-gated per systemd interface, so
only what is needed gets generated:

```toml
zbus_systemd = { version = "0.26100", default-features = false, features = [
    "systemd1",
    "zbus-async-tokio",
] }
```

It pulls only `serde` and `zbus` (≥ 5.3), the published crate is 67 KB, and
it requires Rust 1.87. `zbus` itself needs its `tokio` feature, which the
`zbus-async-tokio` feature above selects.

**What the traits actually require.** The spec in `content-adapter-spec.md`
predates the `childs` refactor and still describes `children_types` / `list`
on `Node`; the code does not. The real surface is much smaller:

- `ContentAdapter` — only `adapter_type`, `root`, `get_by_id` and `childs`
  have no default. Everything else (capabilities, status, invalidations,
  actions, query handling, stores) is defaulted.
- `Node` — only `id`, `label`, `node_type` and `metadata`.

`childs<'a>(&'a self, node: &'a dyn Node) -> Vec<children::Child<'a>>` is the
single source of truth for a level: each `Child` carries its `NodeType`, its
`ColumnSchema` list and a **lazy** `list` callback ("list without the await").
`child_types`, `columns_for` and `list` are derived from it as free functions,
so a declared type without a fetcher is not expressible. The workflow adapter
(`not-yet-done-workflow/src/adapter.rs`, `fn childs`) is the shortest example
to copy the shape from.

**The `in_rows` promise.** Every `ColumnSchema` with `in_rows: true` must
appear as a metadata field on _every_ row that child's `list` returns.
`children::check_rows` makes that testable — worth a unit test per level from
the start rather than after the first level renders blank cells.

**The view YAML need not be hand-written.** `not_yet_done_content::scaffold`
(the CLI's `config generate`) projects the adapter's own type tree into a
loadable view-config skeleton, with every action emitted commented out and a
`# TODO key`. Generate it, then prune and tune — do not start from a blank
file or from a copy of `sqlite.yaml`.

**Property reads.** `ListUnits` already delivers name, description, load
state, active state and sub state. The remaining columns come from
`org.freedesktop.DBus.Properties.GetAll` per unit — split across two
interfaces: `ActiveEnterTimestamp`, `UnitFileState`, `FragmentPath` and
`NeedDaemonReload` on `…systemd1.Unit`, and `MainPID`, `MemoryCurrent`,
`TasksCurrent`, `CPUUsageNSec`, `NRestarts` on `…systemd1.Service`. That is
two calls per loaded unit; issue them concurrently rather than in sequence.
