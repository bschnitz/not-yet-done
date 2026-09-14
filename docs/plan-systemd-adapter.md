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

**Delivers** an edit path for a unit's configuration, `systemd-analyze verify`
before anything is written, `daemon-reload` after, and the one question a write
leaves behind.

The editor-template seam fits unusually well here because **the template is the
unit file itself** — no translation, so none of the round-trip risk the Jira
wiki markup carries. Only a comment header above a cut marker, carrying the
unit's current state and, for a drop-in, the warning below. Everything above the
marker is stripped on save; a buffer whose marker the user deleted is taken as
content in full, because guessing which leading comments were ours eventually
eats one of theirs.

Write scope in this phase is `~/.config/systemd/user` only. The system manager
is phase 7 and a privilege question, not an editing question.

### The four open questions, decided

**Drop-in is the default** (`e e`), because it is what `systemctl edit` does and
what `systemctl --user revert` undoes in one word: the vendor unit keeps
receiving package updates underneath. `e f` edits the whole file, copying the
vendor one into the user's tree on first use — complete control, and no further
updates. One exception bends the default: if the unit already loads from a file
in `~/.config/systemd/user`, `e e` edits **that file**. There is nothing
underneath left to preserve, and a drop-in would only split one unit across two
files.

A drop-in is read _in addition to_ the unit file, so a list-valued directive —
`ExecStart=`, `ExecStartPre=`, `Environment=`, `After=` — **appends**. Replacing
one means clearing the list first:

```ini
[Service]
ExecStart=
ExecStart=/the/new/command
```

Without the empty line systemd answers `Service has more than one ExecStart=
setting, which is only allowed for Type=oneshot services. Refusing.` — a message
that never mentions drop-ins. Every drop-in buffer says so in its header.

**The buffer lives in a temp file**, not at the live path. `EditorPrep.file_path`
is deliberately left `None`: a rejected buffer must never exist inside
`~/.config/systemd/user`, where somebody else's `daemon-reload` would pick it up
mid-edit. The price is that a crash during editing loses the buffer, which is
the cheaper of the two failures.

**Backups live outside the systemd tree**, at
`~/.local/share/not_yet_done/systemd-backups/<unit>/<timestamp>-<filename>`, the
last ten kept. A drop-in directory reads _every_ `*.conf` in it, so a backup
filed next to the file it backs up is safe by naming convention only — and the
convention is one rename away from being configuration.

**The restart question is asked only when there is a choice.** A write changes
what the manager will load, not what is running. If the unit is inactive, the
adapter reloads the manager and says so. If it is up, a picker offers `restart`,
`reload-or-restart` or nothing — the first two being the phase 1 verbs, so the
protection list still applies to them unchanged.

### What `verify` actually says

Measured against systemd 261, not assumed:

| written                     | `systemd-analyze verify` says    | exit  |
| --------------------------- | -------------------------------- | ----- |
| `NoSuchKey=1`               | `Unknown key … ignoring.`        | 0     |
| `Restart=nonsense`          | `Failed to parse … ignoring`     | 0     |
| a line outside any section  | `Assignment outside of section.` | 0     |
| `ExecStart=/does/not/exist` | `Command … is not executable`    | **1** |

So the exit code is not the gate — it only separates "systemd would load this
anyway" from "systemd refuses the unit". The usable signal is that a clean file
prints **nothing at all**: every line is a finding. A fatal one blocks the write
and reopens the buffer with the reason on top; a non-fatal one is written and
named in the notification, because a directive that quietly never took effect is
how an edit goes unnoticed for weeks. `LC_ALL=C` on the subprocess, or systemd
answers in the caller's language inside an otherwise English buffer.

A missing `systemd-analyze` is reported as one non-fatal finding, not an error:
refusing to write because the _checker_ is absent helps nobody.

### The check runs before the file exists

`verify` needs the unit to be findable, and the pending text is not on disk yet.
So it is staged: the buffer is written to `<tmp>/<unit>.d/override.conf` and
`SYSTEMD_UNIT_PATH=<tmp>:` puts that directory ahead of the defaults (the
trailing colon appends them rather than replacing them). A drop-in of the same
name in the higher-priority directory **shadows** the real one, while the vendor
fragment and every _other_ drop-in still merge — so what is checked is the
effective unit as it would be, and a file that fails never reaches `~/.config`.
Paths in the findings are rewritten from the staging directory to the real
target before the user sees them.

### The question-after-the-write gap

The commit path could say "done", "open this again" or "cancelled" — it had no
way to say "done, and now one thing needs deciding". Phase 2 adds
`ActionOutcome::OpenPicker { action_id, message }` to the content protocol,
additively. It is `OpenEditor` one step later: instead of a menu choosing which
editor to open, it is a menu that follows a write, opened on the _same_ node
through the ordinary `picker_options` → `execute` road. No new prompt plumbing,
and the frontend that cannot prompt (the CLI) reports the message and says which
action was left undone rather than turning a successful write into an error.

### Protection covers editing too

The phase 1 list was approved for the disruptive verbs. It refuses the editor as
well, at `prepare` time, before the buffer opens: the unit file decides what the
unit comes back as, so a broken override on `dbus-broker.service` is the same
mistake as `stop`, only deferred to the next start.

## Phase 3 — Creating

**Delivers** the timer/service pair from one form, the empty unit file in the
editor, and `OnCalendar` input that does not require knowing systemd's calendar
grammar by heart.

**The timer/service pair is the point of this phase.** A timer alone is
useless; one always writes two files and forgets half of one. One form, one
result: name, what it runs (a new `ExecStart` _or_ an `option_menu` over
existing services), schedule, `Persistent=`, `RandomizedDelaySec=`,
`AccuracySec=`, enable now. The adapter writes `foo.service` and `foo.timer`
together.

### The scope, and what waits

The plan named three ways to make a unit. They coexist rather than compete, but
they do not all arrive at once:

1. **The pair form** and **the empty editor template** — this phase, and with
   them a plain **service form**, which fell out of the pair for free: writing
   `foo.service` alone is the pair minus the timer. The editor template is
   nearly free for the same reason — phase 2 already renders a buffer, checks it
   in a staging directory and writes it, so creating is the same road starting
   from a skeleton instead of a file.
2. **Skeletons from a directory** —
   `~/.config/not_yet_done/systemd-templates/*.service`, picked via
   `create_child`: oneshot script, long-running daemon, path watcher pair,
   socket activated, resource-limited. Files, not Rust constants, so the set
   grows without a rebuild. **After** the form, because the form is what shows
   which fields a skeleton actually has to cover.

Creating sits on its own `n` leader — `n s` service, `n t` the timer/service
pair, `n f` an empty file in the editor. Not under `a`: that leader means "a
verb on the row under the cursor", and creating has no row. Not `a` by itself
either, though that is what every other tab uses for adding, because here `a` is
already the verb leader and `e` the editor leader.

### `OnCalendar` is normalised, not translated

`natural-date` cannot do this job, and that is not a gap in it: its whole API
(`resolve_datetime`, `resolve_date`, `resolve_offset`) resolves to _one_
instant, while `OnCalendar` is a recurrence. Using it would mean writing a
recurrence grammar.

That turns out not to be needed, because systemd already accepts nearly
everything a person would type. Measured against `systemd-analyze calendar` on
systemd 261:

| accepted                                           | rejected            |
| -------------------------------------------------- | ------------------- |
| `daily`, `weekly`, `hourly`, `monthly`, `yearly`   | `every monday at 9` |
| `monday`, `Mon 09:00`, `mon 9:00`, `friday 18:00`  | `each monday 09:00` |
| `Mon,Fri 09:00`, `Mon..Fri 09:00`, `sat,sun 10:00` | `monday 9`, `mon 9` |
| `9:00`, `09:00`, `*:0/15`, `2026-09-14 09:00`      | `15 minutes`        |

What is missing is filler words and an hour without minutes. So the adapter
**normalises** — drops `every`, `each`, `at`, `on`, completes a bare hour to
`H:00` — and hands the rest to systemd unchanged, writing systemd's own
normalised form into the file. A translator would make the field and the file
two different languages, of which the file only speaks one.

`natural-date` still has a place here, for the other kind of timer: the
**one-shot** (`tomorrow at 9`), where a single instant is exactly right and
`OnCalendar=2026-09-14 09:00:00` is what the file wants.

Unlike `systemd-analyze verify`, **the exit code is the gate here**: an
unparseable expression exits 1, a good one exits 0 and prints the normalised
form plus the next elapses (`--iterations=N` for more than one).

### Validation on submit, not a live preview

A field that recomputes while the user types would need a form event on every
keystroke plus an adapter round-trip from it — and `FormEvent` has only
`Submitted` / `Cancelled` / `Consumed`, with `FormNotice` set from outside. The
round-trip would cross a sync key path into an async adapter and fork
`systemd-analyze` per keystroke; `picker_options` blocking the event loop is the
same shape of mistake, already made once.

So: validate on submit. A rejected expression keeps the form open with systemd's
own message as `FormNotice::Alert`; an accepted one puts the normalised form and
the next three elapses into the success notification. That is the ~90 % for
~10 %, and a live preview stays a generic form-protocol project rather than a
by-product of this phase.

### What it took, beyond the plan

**The form had to be able to come back.** The decision above says a rejected
expression keeps the form open; the TUI could not do that. `execute_content_action_form`
took the popup on submit and turned any `Err` into a notification, so a mistyped
schedule cost the whole form. A submitted form now stays on screen, frozen,
until its answer arrives — Esc abandons it, every other key is swallowed so a
slow write cannot be sent twice — and a failure re-arms it with the adapter's
own sentence under the fields. That is not systemd-specific: every form action
in the app now keeps what was typed when it is refused. It is also why the
failure messages lost their `Action failed: <id>:` prefix — inside a form the
heading already names the action, and the adapter's sentence reads better
everywhere else too.

**The editor road has no channel but the buffer.** `ActionOutcome::Reopen`
carries content and nothing else. So for `n f` the reason a save did not take
has to be _in_ the file that comes back, the way a rejected drop-in already does
it in phase 2 — which means the checks have to run, and be able to refuse,
before anything is touched. Hence the split: `stage` answers "may this be
written, and why not", `commit` puts it on disk. The form road runs both in one
call; the editor road stops between them, because a form can be handed back with
its fields filled in while an editor buffer holds a whole file that nothing else
has a copy of. The header is rebuilt rather than patched, so a second refusal
replaces the first notice instead of stacking, and the `# unit:` line keeps the
name the user typed.

**The normaliser is wider than "filler words".** Measured against
`systemd-analyze calendar` on systemd 261, these are what a person types and
systemd refuses, with what the adapter hands over instead:

| typed              | handed to systemd | why                                    |
| ------------------ | ----------------- | -------------------------------------- |
| `every day at 9`   | `*-*-* 9:00`      | filler out, bare hour completed        |
| `weekdays 9:00`    | `Mon..Fri 9:00`   | English period word                    |
| `weekend 10:00`    | `Sat,Sun 10:00`   | English period word                    |
| `mondays 9:00`     | `monday 9:00`     | the plural is ours, not systemd's      |
| `9am`, `9 pm`      | `09:00`, `21:00`  | the 12-hour clock                      |
| `noon`, `midnight` | `12:00`, `00:00`  | English word for a time                |
| `daily` _(alone)_  | `daily`           | already systemd's own word — untouched |

The last row is the rule the rest obey: a word that systemd has its own meaning
for is only rewritten when something else stands next to it. `daily` is a
complete expression; `every day at 9` is not.

**What `verify` calls fatal drew the same line as in phase 2.** Measured while
creating: `Type=nonsense` and an unrecognised directive are warnings — the file
is written and the complaint goes into the notification. A `[Service]` with no
`ExecStart=` is fatal and nothing is written at all; for a pair that is the
whole point, since a rejected timer must not leave its service behind. Creating
needed no policy of its own.

**One caveat about the `n` leader.** `n` is also "next search match", but that
claim is only made while a search is live with hits — the keymap leaves `n` free
otherwise, by design. So the three creating keys work at all times except with a
search open, where Esc comes first. The view YAML says so.

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

### The three open questions, decided — and a fourth from measuring

**A level _and_ a terminal key**, not one or the other. The level is the
substance of the phase: sorting, the query language, `highlights:`, multi-line
rows, the details pane, copying — all of it already exists and none of it
exists in a pager. The terminal key costs ten lines, opens
`journalctl -fu <unit>` on the current workspace, and covers colour and follow
the day the phase lands.

**The tab's own query language**, not journalctl's `FIELD=value` matches:
`[prio, <=, 3]`, `[message, like, foo]`, `[since, =, yesterday]`. What
journalctl can do itself is pushed down to it (`PRIORITY=`, `--since/--until`,
`--grep=`) and the rest is filtered on the rows already fetched — the same
shape the HTTP adapters use. Pass-through would be more powerful and would make
this one level speak a different language from the rest of the tab, which is
D2's point over again.

**Follow is not in this phase.** It is the same question as phase 5 — a
background task pushing `Invalidation::Row`, and one decision about repaint
pressure under a unit that logs hundreds of lines a minute. Better made once,
there, than twice. Until then the terminal key is the follow.

**`MESSAGE` is not always a string.** Measured: `journalctl --output=json`
emits it as a **JSON array of bytes** whenever it is not plainly printable
UTF-8, and that is not a corner — it is every line a service logs through
`tracing`'s colours, and was 148 of 148 rows for Chromium in a sample window.
An adapter that reads `MESSAGE` as a string shows nothing at all for those
units. So: decode the bytes lossily, and **drop the ANSI escapes on read**. The
colours carry nothing the `prio` column does not already say, and the cell is
styled by `highlights:` anyway; interpreting them instead would mean a small
terminal emulator inside a table cell.

Two more numbers from the same measurement, for whoever writes the fetch:
`--output=json` costs about 1.9 KB per row across 31 fields, `--output-fields=`
brings that to roughly 600 B, and the address fields (`__CURSOR`, both
timestamps, `_BOOT_ID`, `__SEQNUM_ID`) are emitted whether asked for or not.
`--after-cursor` behaves exactly as the cursor pagination needs: the cursor of
the third-from-last row returns precisely the two after it.

### What it took, beyond the plan

**Offsets, not `--after-cursor`.** The plan picked the cursor because
journalctl hands one out and because this application has cursor pagination.
They are two different roads, though: `CursorIntent` and
`CustomQueryResult::cursor_id` belong to `execute_query`, and a drill level is
loaded by `childs()`/`list()`, whose `ListParams.page` is an offset and a limit.
So the window is cut the way an offset-based interface wants it — `--reverse
--lines=<offset + limit>`, then drop the first `offset` rows. Stateless, exact,
newest first, and one process per page. `-r --after-cursor <c> -n <n>` was
measured and does walk backwards from an entry; that is what phase 5's follow
will want, and it is written down in the module docs for whoever gets there.

**`--grep` is deliberately not pushed down**, against the decision above. It
matches the bytes as the journal stored them, and those bytes are full of the
ANSI escapes the cell no longer has — the very escapes this phase decided to
drop on read. A pattern spanning one of them matches the cell the user is
looking at and not the row on disk, so the pushdown would drop rows the
in-memory filter keeps. Pushdown here may only ever narrow _what is read_:
severity and the time bounds qualify, the message does not. The in-memory pass
stays the authority in every case.

**Five columns, not four: `level` next to `prio`.** The plan listed `time`,
`prio`, `pid`, `message`. A column of bare digits is not something anyone
reads, and a column of words cannot be asked "at least this bad" — so the
severity travels twice. `prio` is systemd's number, which is what `[prio, lte,
3]` compares and what sorts; `level` is the same value as the word systemd
prints, which is what the eye reads and what the `highlights:` rules paint.
`prio` is `hidden: true` in the shipped view: it is there to be queried, not to
be looked at. (`time` rather than the plan's `since`, too — `since` is when a
unit entered its state, and reusing the name one level down would have made
`[since, gte, …]` mean two things in one tab.)

**The terminal key needed a config line, not `OpenExternal`.** That outcome
routes through the TUI's `navigation.link_opener` — `xdg-open`, which is right
for a URL and useless for a terminal emulator. So `follow` is an ordinary
`InputSpec::None` node action that the adapter spawns itself and lets go of,
the way the Jira adapter already opens a browser. Which terminal is the
adapter's new `terminal:` setting, falling back to `$TERMINAL -e` and then to
`xterm -e`: emulators disagree about whether the command comes after `-e`,
after `--`, or bare, so it has to be a command line rather than a program name.

**`J` and `a j`, not a `j` leader.** `j` is the cursor moving down and cannot
be taken; `l`, `enter`, `<`, `>`, `p`, `G` and `gg` are all spoken for as well.
So the level opens on `J` — beside `p` for properties, the tab's other drill —
and the terminal on `a j`, where the rest of the verbs live.

**A `query:` on a drill level was dead config.** The journal's query language is
the point of this phase, and a level is where its vocabulary is taught — but
`ChildDef` had no `query:` field, so the block the Properties levels have
carried since phase 0 was parsed, dropped, and never seen. Pressing `q` one
level down offered the _parent's_ template: `[active, =, failed]` over rows that
have no `active` column. `ChildDef` now carries a `query:` block and the three
lookups that read it prefer the level on screen, falling back to the view root
so every existing view file behaves exactly as before. Only the editor is per
level; which query is _live_ still belongs to the pane and follows the user down
the drill. The shipped `systemd.yaml` is now checked for unknown keys in the
same test that parses it, so the next piece of inert configuration fails there
instead of in a user's terminal.

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

### The three open questions, decided — and what measuring changed about one

Measured first, on the user bus of a machine with 421 loaded units:

| What                           | Measured                                                       |
| ------------------------------ | -------------------------------------------------------------- |
| Idle bus, `Subscribe()` active | **0 signals in 36 s**                                          |
| One unit started and stopped   | 23 signals over 7 object paths                                 |
| …of those, for the unit itself | **5 `PropertiesChanged`**, plus `UnitNew`/`UnitRemoved`/`Job*` |
| What the payload carries       | `ActiveState`, `SubState`, `MainPID`, `Result`, the timestamps |
| What it does **not** carry     | `Description`, `LoadState`, `UnitFileState`, `MemoryCurrent`   |

**1. The subscription is global.** The worry this phase was opened with —
`PropertiesChanged` on every unit being a lot of traffic for rows nobody is
looking at — is measurably not the case: an idle manager emits nothing at all,
because the traffic is proportional to state _changes_, not to the number of
units. So there is no narrow subscription to design, and no bookkeeping about
which units are on screen. What a change does produce is a small burst — five
signals for one unit's start — so the adapter coalesces per unit over a short
window and pushes **one** `Invalidation::Row` per unit that settled. Because
the signal does not carry the static half of a row, the fire reads that one
unit's properties: one round trip per real change, none while nothing happens.

**2. `kind: countdown` is generic, in the table engine.** The `left` column is
a string the adapter formats today, which means it sorts lexically and does
not tick. A `countdown_to:` column is the exact mirror of the engine's existing
`kind: elapsed` / `elapsed_from`, rides the same repaint pulse, and is wanted
by the calendar too — so it belongs beside it, not in this adapter.

**3. `kind: bytes` comes with it**, on the same seam as `kind: duration`: the
cell's value stays the number, so `[mem, gt, 100000000]` and sorting keep
working, and only the rendering is the human form. Phase 0 walked into this
with `mem` and left it a bare byte count nobody reads.

**4. The dependency tree shows `Requires`, `Requisite`, `Wants`, `BindsTo`,
`PartOf` and `Upholds`** — what `systemctl list-dependencies` shows — and puts
the _ordering_ relations `After` / `Before` on their own level rather than
mixing them in: "needs" and "comes after" are two different questions, and a
tree that answers both at once answers neither. Cycles are cut and the depth is
bounded.

**Cut inside the phase:** live rows first and complete, the dependency tree
after. Both are phase 5, but the live half is the payoff of D1 and stands on
its own, so it ships as its own usable state.

### What it took, beyond the plan — the live half

Three things the design above did not anticipate, all found by building it.

**A `Node` invalidation cannot reach a root-level pane.** `Invalidation::Node
{ id }` is delivered to a pane whose _parent_ is that node, and the three unit
levels sit at the root of their view, where the frontend has no parent frame to
compare an id against — so the obvious `Node { id: "root" }` for a unit that
appeared or vanished would have been silently delivered to nobody. The
structural signal is therefore `Invalidation::All`, which is the narrowest
thing that actually addresses those levels. Worth knowing for every adapter
whose interesting level _is_ the root one.

**The object path is a type test, and a free one.** systemd escapes a unit name
into its object path, encoding everything outside `[A-Za-z0-9_]` as `_xx`, so
`sshd.service` lives at `…/unit/sshd_2eservice`. That makes the path suffix as
reliable as the name — and it is in the signal header, so a `.scope` or a
`.socket` (the two that churn on a desktop) is rejected before it costs the
round trip that reading `Id` would have cost.

**The watcher starts with the first level, not with the adapter.** Construction
must not open a connection — a `--user` manager may not be running, and a tab
exists long before anyone looks at it — and `Subscribe()` needs one. Tying it
to the service and timer levels rather than to `root()` also means a systemd
tab that is never opened never makes the manager emit a single signal.

**Measured after the fact, same probe as before:** one transient unit started
and stopped, which emits about five `PropertiesChanged` per transition, now
produces **2** `Invalidation::Row` — one per settled state. That is the
coalescing working, and it is what the ignored live test in `live.rs` asserts
(`cargo test -p not-yet-done-systemd-adapter -- --ignored live`).

### What it took, beyond the plan — the tree half

**The transport was already paid for.** Every relation this level needs —
`Requires`, `Requisite`, `BindsTo`, `PartOf`, `Upholds`, `Wants`, `After`,
`Before` — is an ordinary property on the generic
`org.freedesktop.systemd1.Unit` interface, so all of them arrive in the same
`GetAll` a unit row already makes. The dependency tree needed no new call, no
new interface and no second cache; what it needed was a decision about ids.

**A row is addressed by its whole path, not by its unit name.** Measured on
the user bus: `systemctl --user list-dependencies --all pipewire.service`
prints **120 rows made of 27 distinct units** — a factor of four and a half. A
unit name is therefore not an address in this tree: `dbus-broker.service`
names four or five positions at once, and the frontend addresses rows _by id_
for both the tree cache and the live row patching that phase 5a just built. So
an id carries the chain it was reached through —
`dep:pipewire.service>basic.target>sockets.target`. `>` is safe as the
separator because systemd restricts unit names to alphanumerics plus
`:-_.\@` and escapes everything else as `\xNN`.

**The ordering level is flat, and had to be.** The plan says the two questions
get two levels, which is right; what building it showed is that they cannot be
two _tree_ levels. The frontend expands **all** tree-continuing children of a
level together (`ExpandTreeNodeMulti`), so a second tree branch under a unit
row would unfold "needs" and "comes after" into one mixed list — precisely the
answer that is neither. `systemd:dep` is therefore the tree and
`systemd:order` is one flat hop, reached with `D` because `enter` on a unit
row belongs to the tree. That is also the honest shape for it: `After=` walked
recursively from `basic.target` is the whole session start, and nobody reads
that.

**Which in turn made Services and Timers tree levels.** A `tree_label` child
only continues from a tree-active parent, so the branch would have been
silently inert on a flat level. Both unit levels now carry `tree_label: name`,
and a unit row's expand arrow comes from `has_children`, which the row fills
from the six "needs" properties it already holds — free, and exactly right: a
unit that depends on nothing shows no arrow and `enter` still drills into
Properties.

**One lookahead read per child, deliberately.** `has_children` on a
_dependency_ row cannot be answered from the parent's properties, only from
the child's own — so each row in a level costs one extra `GetAll`, issued
concurrently under the same in-flight cap as everything else. The trade is
worth naming: a level is ten to twenty rows wide where the services level does
the same thing four hundred times, and the alternative is an expand arrow that
opens onto nothing, which is a lie a tree should not tell.

**A unit named by two relations shows the stronger one.** `Requires=` and
`Wants=` overlapping on the same unit is ordinary, and two rows for one unit
would be two tree positions for one thing. The row keeps the first match in
the order the adapter declares — `Requires`, `Requisite`, `BindsTo`,
`PartOf`, `Upholds`, `Wants` — because that is the one that decides what
happens when the dependency fails. Cycles are cut the same way they are shown:
a unit already present higher up its own branch gets a row marked `(cycle)`
and is not followed; depth stops at ten regardless.

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
- **Backup before every unit-file write** (phase 2), adapter-side, to
  `~/.local/share/not_yet_done/systemd-backups/` — outside the systemd tree,
  where a stray `*.conf` would be read as configuration.
- **`verify` before the write** (phase 2), not merely before `daemon-reload`:
  the buffer is staged in a temp directory and checked there, so a unit systemd
  would refuse never reaches `~/.config` at all.
- **Protection covers the editor** (phase 2) as well as the verbs: a protected
  unit refuses to open for editing, because the file decides what it comes back
  as.
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
