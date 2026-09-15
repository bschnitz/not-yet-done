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
engine, which had no `kind: bytes`. That is the same shape as the parked
`kind: countdown` below, and the two were decided together and built in
[phase 5](#phase-5--live).

`left` is a **countdown** — `field − now`, into the future — where the table
engine's `kind: elapsed` computes `now − field`. In this phase the adapter
formats the string itself; that is the throwaway variant, which is exactly
why it is right here. Phase 5 gave the engine a generic `kind: countdown` and
threw this one away.

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

### What it took, beyond the plan — the typed-column half

**The countdown had to pick a scale, and the scale is an argument.** The rule
is the largest non-zero unit plus the next one down, and nothing below:
`1w 2d`, `2d 13h`, `5h 24min`, `24min 13s`, `13s`. It stops at weeks because
every unit up to a week is a **fixed span** — a week is always seven days —
while a month is not: `1mo` would mean something different in February than in
July, and a column cannot carry that ambiguity silently. A yearly timer
therefore reads `52w 3d`, which is clumsy, but the `next` column beside it
already names the date, and the date is the honest answer at that distance.
Two smaller decisions fell out of the same reasoning: a zero remainder is
dropped (`5h`, not `5h 0min`), and the span is truncated rather than rounded,
so a countdown never claims more time than is left.

**A past target counts negative rather than clamping.** The adapter's
throwaway `countdown()` clamped to `0s`, on the argument that a timer whose
elapse has passed is about to fire rather than overdue. That is true of
timers and false of everything else the kind now serves: a calendar's
countdown column spends half its life on events that have already begun, and
`0s` hides exactly the thing one looks at it for. `-10min` is both, so the
generic kind counts on downwards.

**The live half was never wired up, and the countdown is what exposed it.**
The engine could already recompute a time-derived cell
(`ContentView::repaint_live_columns`, gated on `has_live_column`) — what it
lacked was anything to ask it to. The only driver was `DomainEvent::TrackingTick`,
the task tracker's own ~1 Hz heartbeat, which beats **only while a tracking
runs** and which nothing in the shipped binary emits at all. So `kind: elapsed`
had been standing still since it was built, and the systemd `Ago` column proved
it: it moved on a keypress and never on its own. A countdown that only counts
when you touch it is not a countdown, so the app now beats its own pulse — one
second, independent of every adapter, recomputing only the visible panes that
really carry such a column and leaving the frame clean when none do. An adapter
with a cadence of its own can still push `Invalidation::Repaint` into the same
recompute.

**Putting the kinds in the engine deleted adapter code rather than adding
any.** The systemd adapter lost the `left` cell, its `ColumnSchema`, the
`countdown()` helper, its test and the `now` parameter that only that helper
needed — `TimerRow::summary()` takes no clock any more. The proof that this
was the right seam is the calendar: it got a live "starts in" column from four
lines of view YAML over its existing `start` field, without the calendar
adapter changing at all.

## Phase 6 — Diagnostics

The commands that exist but that nobody runs. That is more than one phase's
worth, so it is cut into four, each of which is usable on the day it lands:

- **6a — the audit** (below): what the distribution's policy wants, how each
  unit disagrees with it, which file hides which, and what a drop-in amends —
  four columns and one verb.
- **6b — `systemd-analyze security <unit>`** as a level: one row per setting,
  with its exposure weight and status, sortable by weight — and `enter` on a
  row jumping into the drop-in edit with that directive prefilled. That turns
  an audit command into a workflow.
- **6c — the clock and the meter**: startup time per unit, sortable — what
  `systemd-analyze blame` reports, read straight off the unit's own timestamps
  rather than out of that command, plus the memory high-water mark beside the
  current one. `critical-chain` and `IOReadBytes` were measured and dropped;
  the decisions are in [the phase](#phase-6c--the-clock-and-the-meter) below.
- **6d — the ambient half**: a failed-units indicator through the existing
  Waybar CFFI module, and a `connected` hook that raises a notification when
  units are already failed at startup, so it lands in the notification centre
  without anyone going to look.

### Phase 6a — the audit

**Delivers** the four questions a unit listing raises and cannot answer: what
the distribution's preset policy wants this unit's enablement to be
(`preset`), how the unit currently disagrees with it (`drift`), which unit
file this one hides (`shadows`), and how many drop-ins amend a running service
(`dropins`). Plus the `preset` verb, which
[phase 1](#phase-1--control) deliberately left out, and the audit queries in
both levels' `template` blocks.

This is where the `preset` column that fell out of phase 0 returns, and it
comes back for the reason it was dropped: it needs a parser, and now something
else needs the same parser.

### The questions, decided

**The preset has to be parsed, not read — there is no property to ask for.**
The manager interface has no `GetUnitFilePreset`; `UnitFilePreset` exists only
on an already-**loaded** unit, which is exactly the population the unit-files
level is there to see past. A unit file nobody has started has no object to
carry the property. So the adapter walks
[`systemd.preset(5)`](https://www.freedesktop.org/software/systemd/man/systemd.preset.html)
itself, the way `systemctl` does: four user directories in priority order, a
same-named file in a higher one **replacing** the lower rather than merging
with it, all survivors then sorted by **filename across directories**, first
matching line wins, `/dev/null` symlink switches a vendor file off — and a
unit no line matches is `enable`, systemd's own default. Ten tests pin those
rules, because the rules are the feature.

**`drift` is named as the fix, not as the complaint.** The column could have
said `mismatch` or `off-policy`; it says `should-enable` and `should-disable`,
which is literally what `a p` on that row would do. A column that names the
remedy is one a user can act on without looking anything up, and the verb
beside it is then obviously the same thought. It is empty wherever the unit
agrees with its policy, and empty wherever a preset cannot apply at all — a
`static` unit has no `[Install]` section to enable, a `generated` or
`transient` one has no file anybody wrote — so on a machine in line with its
distribution the column is quiet, and sorting by it is the audit.

**The audit ships as templates, not as declared saved queries.** There is no
YAML-declared saved query in this app: `QueryConfig` carries `default`,
`template`, `editable`, `menu_key` and `inherit_default`, and saved queries are
made by the user at runtime with `★` in the `q` menu, stored in `nyd.db`. So
the four audit queries live as commented examples in the `template:` block of
each level, one keystroke from being kept. That is the seam the app already
has; inventing a second one for four queries would be the wrong trade.

**Drop-ins are a count, not a list of paths.** A number sorts and filters —
`[dropins, gt, 0]` _is_ the audit — and the paths themselves are already one
keystroke away on the property level. A column of paths would be wide, mostly
empty and unsortable.

**The vendor diff waits.** "Show me how my copy differs from the one the
package ships" is the natural next question after `shadows` answers "your copy
hides one", but it is a second surface — a diff view, not a column — and
`shadows` is useful the moment it exists. Deferred, not dropped.

### What it took, beyond the plan

**The search path is per boot, not a constant.** `ListUnitFiles` reports one
file per unit name: the winner. The loser is the interesting half — a
`~/.config` copy silently overriding a newer `/usr/lib` file after a package
update is precisely the failure that goes unnoticed — so the adapter walks the
search path itself. Hardcoding the path would have been wrong: this machine
reports eighteen directories from `systemd-analyze --user unit-paths`, several
of them under `/run/user/<uid>` and two of them **generator output for this
boot**. So the adapter asks. That it shells out at all is not a new seam —
`verify`, `calendar` and the journal level already do — and a failed call
simply yields an empty path, which costs the `shadows` column and nothing else.

**`should-enable` is loud here, and that is the policy's answer.** On this
machine 31 user units report `should-enable`, because the only user preset
file installed is systemd's own `90-systemd.preset` — two `enable` lines, no
default-off rule anywhere — and systemd's default for an unmatched unit is
`enable`. `systemctl preset-all` really would switch
all 31 on. The column is not misreading the policy; the policy is simply
permissive, and seeing that is itself worth the column. The count was checked
against `systemctl --user list-unit-files` twice — 31 against 31 for
`should-enable`, 0 against 0 for `should-disable`.

**The `preset` verb needed its own sentences, because its good outcome is
nothing happening.** Every other file verb reports symlinks changed; `preset`
on a unit already in line with its policy changes none, and "0 changes" reads
like a failure. So its arm says "already matches its preset", which is the
actual news.

**Then the message learned to say which way it went, and where.** A count is
the one thing `preset` cannot usefully report: the user pressed `a p` precisely
to let the policy decide, so the decision is what they still do not know, and
"Applied the preset (1 symlink)" withholds it. The manager hands the answer
over for free — it replies with one `(operation, filename, destination)` per
entry, and a written link means on while a removed one means off. So the
headline names the direction (`Enabled <unit> — the preset wants it on`) and
the paths follow, one per line. All five file verbs share the shape, because
`enable` and `mask` were equally quiet about _where_ they wrote; only their
headline differs, since their direction is already in the verb. The first line
is what the notification bar shows and the rest appears when the entry is
expanded, so naming every path costs nothing on screen and saves the trip to
`ls` that a count forces.

**The change list is not only symlinks, and the old count did not know that.**
systemd reports its refusals through the same array — `masked` when the unit is
masked, `dangling` when the link points at nothing — so counting entries
counted refusals as work done. `a p` on a masked unit now says "The preset
changed nothing" and prints the reason under it, where before it would have
claimed a symlink it never wrote. This is the second lie the message shape
exists to prevent; the first was "enabled" on a unit with no `[Install]`
section.

**An empty `drift` does not promise that `a p` changes nothing, and the smoke
test found that out the hard way.** A user unit can be enabled from
`/etc/systemd/user/<target>.wants/` — the system administrator's answer for
every user on the machine — and `ListUnitFiles` reports it as plainly
`enabled`, because effectively it is. `systemctl --user preset` on such a unit
nonetheless writes a _second_, redundant symlink into `~/.config/systemd/user`,
since enabling at user level is the only thing it can do. So the verb reported
"Applied the preset" on a row whose `drift` was empty, which looks like a
contradiction and is not one: the columns describe the effective state, the
verb acts at one specific level. The adapter does exactly what `systemctl`
does here and should not second-guess it — but the case belongs in the smoke
test, and it is why the "already matches" message is worth having at all: it
is the reliable way to tell the two apart, and it appears exactly when the
unit is already enabled in the user's own directory.

**Both new columns were proved against the live machine and then unproved.**
No unit name is duplicated across the search path here, so `shadows` had
nothing to show until a copy of an inert `fluidsynth.service` was laid into
`~/.config/systemd/user`; the row then named the `/usr/lib` file it hid.
`dropins` got a throwaway oneshot with a comment-only drop-in — and needed
`RemainAfterExit=yes`, since a oneshot that exits is unloaded and `ListUnits`
never sees it. Both probes were removed and `~/.config/systemd/user` compared
by hash against its pre-state: identical.

### Phase 6b — the security level

**Delivers** `systemd-analyze security` as a level under every unit: one row
per check, with its status, the exposure it contributes and the directive that
would close it; the unit's overall score as a hidden column on the Services
list; and `e h`, which prefills the unit's drop-in with the fix for the check
under the cursor and opens it for review.

### The questions, decided

**The score is a column; the checks are the level.** systemd prints this
analysis as a table with one overall score under it, and the score is the part
everyone quotes — "9.4 UNSAFE". On this machine 21 of the 27 loaded user
services score exactly 9.4, one 9.3, and four sit between 6.9 and 7.6. A number
that is identical for four rows out of five sorts nothing and suggests nothing;
what is actionable is the individual checks behind it. So the checks are the
rows, and the score is a column on the list above them — worth having, because
the handful of units that differ are the interesting ones, and hidden by
default, because on the other twenty-one it is noise.

That column costs one call for the whole listing rather than one per row:
`systemd-analyze --user security` with no unit named reports every loaded
service at once — 53 ms for 27 units here, against 8 ms for a single unit, so
the shared call pays for itself at four rows. The same form is why the Unit
files level carries no such column: it reports **loaded** services only, and a
unit the manager has never touched is precisely that level's population.

**Every check is a row, the ones that pass included.** A level that listed only
the failures would answer "what is wrong" and never "what is already covered",
and which of the two is being asked changes by the minute. Both are one query
away (`[status, =, exposed]`), which is the trade
[D2](#d2--filtering-is-a-query-not-configuration) already made for the unit
levels themselves.

**`status` is a word, because systemd answers three things.** The `set` field
in the JSON is tri-state: `true`, `false`, and **null** for a check that cannot
apply to a per-user service at all — `RemoveIPC=` is the only one on this
systemd. A boolean column would have to file that third case under one of the
other two and would misreport it either way, so the column says `ok`, `exposed`
or `no-effect`. `exposure` is empty rather than `0` where a check passes, which
is what makes `[exposure, gte, 0.3]` mean "what is still open, worst first"
without a second clause about the status.

**The fix is a curated table inside the adapter.** The plan said `enter` would
jump into the drop-in edit "with that directive prefilled", which quietly
assumed that the name of a check _is_ the directive that settles it. It is not
— see below — so the adapter keeps a table of all 81 checks mapped to the lines
that actually close them — and, for the three that have no line to write, a
comment saying why: `RootDirectory=` versus `RootImage=` and `User=` versus
`DynamicUser=` are a choice, and `RemoveIPC=` has no effect at user level at
all. This is the one place in the crate
that holds knowledge rather than reading it, and that is a real cost: a systemd
release that adds a check adds a row here. The alternative was writing text
into unit files that systemd ignores without a word of complaint, which is
worse than offering nothing.

**Hardening is an editor action, not a verb.** `e h` opens the same buffer,
under the same `systemd-analyze verify` pass and the same backup as `e e`
([phase 2](#phase-2--editing)), with the fix appended at the bottom under a
comment naming the check. Nothing is written until the buffer is saved, so the
action is also the way to _read_ what a fix would be. It appends rather than
replaces, so hardening a second check adds a second stanza to the same drop-in;
a repeated `[Service]` heading merges cleanly — measured, not assumed — which
is why the stanza brings its own heading instead of parsing for one.

**A timer is answered for by the service it triggers.** `systemd-analyze
security` refuses a `.timer` outright ("is not a service unit"), and it is
right to: a timer starts no processes of its own and has no sandbox to
describe. But the question a user has while standing on a timer row is a real
one, and its answer is one property away, so the level under a timer analyses
`Unit=`. The rows name the unit they were measured on, and `e h` writes into
_that_ unit's drop-in rather than the timer's.

### What it took, beyond the plan

**The name of a check is a display label, and the first hardening drop-in did
nothing.** A probe service at 9.4 UNSAFE, given an 80-line drop-in built by
taking every check's name as the line to write, came back at 2.2 OK with **23
checks still exposed**. The reason is that many names are shorthand for a
group: `CapabilityBoundingSet=~CAP_SET(UID|GID|PCAP)`,
`CapabilityBoundingSet=~CAP_MAC_*`, and a `RestrictAddressFamilies=~…` with a
literal ellipsis in it. None of those is valid configuration. Worse, only the
address-family lines drew a journald warning; the capability lines were
swallowed silently, and every capability that _did_ flip in that run turned out
to have been dropped as a side effect of a different directive —
`ProtectClock=yes` taking `CAP_SYS_TIME` and `CAP_WAKE_ALARM` with it,
`PrivateDevices=yes` taking `CAP_MKNOD` and `CAP_SYS_RAWIO`. Not one explicit
capability line had taken effect. So the table expands every group into the
real capability names.

**The table was measured rather than reasoned.** Each of the 78 fix lines was
applied to a throwaway unit on its own and the check re-read: 78 of 78 flipped.
Then the whole table at once, which took the probe from 9.4 UNSAFE to **0.2
SAFE**. Reasoning about this file would have been cheaper and would have
produced the 80-line drop-in above again.

**Two checks cannot both pass, and that is systemd's arithmetic.**
`ProtectClock=yes` implies `DeviceAllow=char-rtc r`, so the `DeviceAllow=`
check stays exposed whenever the clock is protected — in either order, whatever
the file says. It is documented in the level's own comments rather than worked
around, because the alternative would be a table that quietly stops protecting
the clock in order to make a column look tidy.

**`RestrictAddressFamilies=` has two checks and only one line satisfies both.**
The obvious fix for "all other address families are allowed" is
`RestrictAddressFamilies=AF_UNIX` — which re-allows `AF_UNIX` and breaks the
sibling check that had been passing. `RestrictAddressFamilies=none` satisfies
both. Found by running the table as a whole, not by reading either check.

**The action was declared as a verb, and the CLI answered "ok (no change)".**
`harden` first carried `InputSpec::None`, so nothing ever called `prepare` and
no buffer was ever built; the action reported success and did nothing. It needs
`InputSpec::Editor`, like `edit`. Only an end-to-end run through the CLI
surfaced it — the unit tests were all green, because there was nothing wrong
with the code the action never reached.

**The obvious key was taken, and the validator said so before the terminal
did.** The level wanted `S` for security; capital `S` is the app-wide sort-mode
key, and the view-config test failed with both claimants and both scopes
named. It is on `H` for hardening. That the shipped example is parsed and
validated by a test rather than trusted is what turned a key collision into a
build failure instead of a key that silently does the wrong thing.

### Phase 6c — the clock and the meter

**Delivers** how long each unit took to activate, as a sortable column on the
Services and Failed lists, plus the memory high-water mark beside the current
one. The startup column is what `systemd-analyze blame` reports, computed from
the unit's own timestamps rather than by running that command.

### The questions, decided

**Startup time is a column, not a level.** `blame` is a single number per unit,
and a level whose rows are one number each is a list with an extra keystroke in
front of it. As a column it sorts against everything else the row already
carries — the unit that is both slow and failed is one sort away, not two
levels apart — and it costs nothing to fetch, because both timestamps it is
made of are already in the property map the row is built from. There is no
subprocess and no extra D-Bus round trip.

**`critical-chain` moves to [phase 7](#phase-7--beyond-the-user-bus).** It is
the other half of `systemd-analyze`'s startup story, and on the user bus it has
nothing to say. The full chain here is six lines deep and contains no service
at all — `default.target` → `basic.target` → `sockets.target` → `dbus.socket` →
`app.slice` → `-.slice`, the whole thing inside 186 ms. That is not a defect in
the tool; a user manager starts almost everything in parallel off socket
activation, so there is no serialised chain to walk. The boot chain on the
system manager is where the command earns a level, and that is phase 7's bus.

**`IOReadBytes` was measured and dropped; `MemoryPeak` took its place.**
`IOAccounting=` is off by default on both buses, and nothing on this machine
turns it on: all 12 running user services and the system's `sshd` report
`IOReadBytes=[not set]`. A column that is empty for every row is worse than no
column, because it invites the reader to conclude there was no I/O.
`MemoryAccounting=` on the other hand is on by default, and `MemoryPeak` is
populated everywhere `MemoryCurrent` is — the high-water mark is the number
that says whether a unit's limit is anywhere near being hit, which the current
reading, sampled at whatever moment the list was loaded, cannot. It is hidden
by default beside `mem`, for the reader who is asking that question. Turning
I/O accounting on is a drop-in like any other and belongs with the other
`a p`-style verbs, not with a column that would lie until it were used.

### What it took, beyond the plan

**`blame` has no `--json`, and did not need one.** The plan assumed the number
would come out of the command. It cannot be parsed reliably —
`systemd-analyze` says verbatim that "Option --json= is only supported for
security, inspect-elf, dlopen-metadata, plot, fdstore, pcrs, nvpcrs,
architectures, capability, exit-status right now" — and it does not have to be:
the command's whole arithmetic is `ActiveEnterTimestampMonotonic` minus
`InactiveExitTimestampMonotonic`, and `GetAll` already hands the row both.
Reading the properties is both cheaper and more honest than scraping a table
whose format is not a contract.

**The second timestamp is not always `ActiveEnter`.** A `Type=oneshot` unit
without `RemainAfterExit=` never becomes active, so `ActiveEnter` stays at the
value from some previous run — or at zero — while the run that just happened is
bounded by `InactiveEnterTimestampMonotonic`. The rule the adapter implements
is: start at `InactiveExit` (which must be non-zero), end at the first of
`ActiveEnter`, then `InactiveEnter`, that is not before the start. Checked
against `systemd-analyze --user blame` row by row: 28 of 28 agree.

**Zero is a measurement, and reading it as "unknown" blanked half the list.**
The first cut required the end strictly after the start, on the reasoning that
two equal timestamps mean nothing was recorded. They do not: a `Type=simple`
unit is active the instant it execs, so both stamps are the same microsecond and
the span is a true zero. `blame` omits such units entirely, which is why the
discrepancy was not obvious — comparing the column against `blame` showed
agreement on every row `blame` printed, and 18 running services (pipewire,
gpg-agent, tidings-shell, …) blank. With `>=` the column reads `0` for them; an
empty cell now means only "never started", which on this machine is exactly 5
units, all `inactive` with a zero timestamp. The distinction is the point of the
column: `0` is a fact about a fast unit, blank is a fact about an idle one.

**`kind: duration` assumed whole seconds, and startup times live below one.**
Rendering 103 ms as `00` would have made the column useless, and the obvious
escape — declare it `kind: number` and put "ms" in the label — breaks the rule
the adapter and the renderer both state: the cell carries the canonical value
and `kind:` decides how it looks. So `duration` was widened instead. It now
parses fractional seconds, which the sort and query layers already supported
(`SortKind::Number`, `Field::Number(f64)`); only the renderer had narrowed it to
`i64`. And it takes a `format:` — the field already existed on `ColumnDef` and
was documented as being "for kinds that support one", used until now only by
`datetime`. `clock` (the default) is the existing `H:MM:SS`, so every column
that had a duration renders exactly as before; `precise` is the new one, which
names the unit its magnitude asks for: `31.0us`, `677us`, `15.3ms`, `103ms`,
`5.24s`, `1min 3s`, `5h 24min`, `1w 2d`. Three significant digits throughout,
and `us` rather than `µs` because the terminal width of `µ` is font-dependent.
Unknown format names are now a config error rather than a silent fallback, on
both the view's own columns and its children's.

**A number must never round onto the rung above it.** `59.99` rendered as
`60.0s` — arithmetically right, and wrong on the page, because a reader who
sees `60.0s` in a column that also prints `1min` will believe the two are
different units. The ladder threshold is `59.95` with rounding rather than `60`
with truncation, and the decimal bands are `99.95` and `9.995` so that nothing
ever shows a fourth significant digit (`9.997` becomes `10.0s`, not `10.00s`).
Both are regression tests now.

**One column, two lists that had to agree.** The set of columns a query may
name (`SERVICE_COLUMNS`) and the set the table declares (`service_columns()`)
are two spellings of one fact in two files, and adding the column to one and not
the other produced `unknown systemd column 'startup'` from the CLI while every
test stayed green. The fix is not just the missing entry: a test now pairs all
seven levels' constants against their schemas and fails in both directions.
Deleting the entry again to watch it fail is what makes it a test rather than a
hope.

**Absent numbers sort to the front when sorting descending.** That is
pre-existing, generic behaviour — an unparseable or empty cell goes to the end
ascending, which is the same thing — and it means "slowest first" opens with the
units that never ran. Changing it would touch every adapter and every tab, so it
is a decision of its own and not one to make inside a column. The query template
documents the working idiom instead: `[startup, gte, 0]` selects the units that
have actually run (26 of 31 rows here). Worth noting for the same reason as in
[phase 6b](#phase-6b--the-security-level): `[startup, is, null]` matches
nothing, because `is null` does not reach an absent numeric field.

## Phase 7 — Beyond the user bus

**Delivers** the system manager, and possibly remote hosts (`-H`) and
containers (`-M`).

Reading is free — unprivileged reads on the system bus are allowed, and that
includes the journal for a user in `systemd-journal` or `wheel`, and
`systemd-analyze`. Writing is not: every write is a polkit action. So the phase
splits into four stages, each of which builds, installs and is usable on its
own:

1. the read side plus a manager-aware `control.rs`, so the tab is honestly
   read-only rather than decorated with keys that always fail;
2. the polkit flag for the runtime verbs, and the system-specific protection
   defaults that come with them;
3. the unit-file verbs, which go over the same bus and through the same flag —
   but are a separate stage because whether they belong on the system tab at
   all is a question stage 2 answered with "not yet". Stage 3 answers it with
   "all but masking" (see below);
4. `critical-chain` as a level under a unit — deferred here from
   [phase 6c](#phase-6c--the-clock-and-the-meter) because the system manager is
   where the command earns a level.

### The questions, decided

**Shape: one instance per manager, and therefore one view file per manager.**
The alternative was several managers as subtabs of one instance pinned by
`query: "manager:<id>"`, the way the mail adapter holds six IMAP accounts.
It was rejected on what it would cost the rest of the adapter: `adapter:` is
declared per tab, so a single instance serving both managers means every node
id has to carry which manager it came from — `sshd.service` exists on both —
and that reaches into the live watcher, the journal calls and the protection
list. A second tab costs one tab key. The price is paid in config instead: a
second file of the same adapter type needs an explicit `adapter.id:`, and its
tab name has to be listed in `tabs.order`.

**Privilege: the polkit flag, not `pkexec` and not `sudo`.** The manager
answers an unauthorised write with

> Access denied as the requested operation requires interactive
> authentication. However, interactive authentication has not been enabled by
> the calling program.

which is an invitation, not a refusal — the caller simply never said it was
willing to be asked. zbus can say it:
`Proxy::call_with_flags(method, MethodFlags::AllowInteractiveAuth.into(), body)`
reaches the typed proxy's inner `Proxy`, so the desktop's polkit agent does the
asking and the adapter never re-execs, never spawns a privileged child and
never holds a password. Measured before designing: a non-interactive `busctl`
write drew exactly that message, which is what proves the authority exists and
is merely unasked.

That flag covers the bus, and only the bus. The editing and creating keys (`e`,
`n`) write files on disk, where there is no polkit to ask — they stay absent on
the system tab for a reason that outlasts stage 2.

### What it took, beyond the plan — stage 1

**One of the four action roads did not know which manager it was on.** A verb
reaches the user by four routes — the action list a frontend renders, the
apply-choice road, `invoke_action`, and the picker — and `control::actions_for`
was the only one that took no manager. A system tab built on that would have
offered `a s`, `a m` and `a p` and let every one of them run into polkit's
refusal. So the manager became part of the verb question itself:
`Verb::available_on(manager)` answers it once, `control::verb_on(manager, id)`
is the single chokepoint, and all four routes go through it. That shuts the
second road as well as the first — a caller naming a verb id outright, which
the CLI does, never asks what the level offers.

**The view file is smaller because the keys are fewer, not because it is a
stub.** `systemd-system.yaml` carries the same four levels (Services, Failed,
Timers, Unit files) and the same four children (Properties, Journal, Security,
Ordering), defined once as YAML anchors inside the first level and aliased into
the other three — 514 lines against 1797. The anchors have to live _inside_ a
known key: a top-level `_shared:` would be reported as an unknown view-config
key, which is to say as dead config. A test parses the committed file, asserts
it validates with no unknown keys, and asserts which verb ids appear in it — so
the promise about what the tab offers is checked by the build rather than by
reading. (Stage 1 asserted that _no_ write verb appeared; stage 2 turned that
into the runtime/unit-file split the test pins today.)

### What it took, beyond the plan — stage 2

**The flag cannot go through the typed proxy.** `zbus_systemd`'s generated
`ManagerProxy` methods take their arguments and nothing else; there is no way
to attach a message flag to `start_unit(name, mode)`. So the writes go out
through the untyped `Proxy` underneath — `proxy.inner().call_with_flags(…)` —
which means naming the D-Bus method as a string (`"StartUnit"`) and spelling
the body as a tuple. That is one helper, `Bus::write`, and the eight runtime
verbs are the only callers; everything that reads still goes through the typed
methods. `JobKind::method()` exists for the same reason and says so.

**A privileged write needs its own deadline, not no deadline.** The read
deadline is ten seconds, and it is there to catch a manager that stopped
answering: on a local bus, silence is evidence of a fault. A call waiting on a
password dialog is silent for the opposite reason — it is working exactly as
intended — and cutting it off after ten seconds would make every system verb
fail while the user is still typing. So `auth_timeout_secs` is a second budget,
five minutes by default, and the timeout message says what the wait was for.
Not unbounded: a dialog nobody ever answers has to give the pane back.

**Declining is an answer, so it is not an error.** A refused write comes back
as `ContentError::PermissionDenied`, and `control::run` turns that one variant
back into a plain sentence — the notification bar says what did not happen,
without the red of an adapter failure. There is nothing else this could
swallow: the protection list refuses _before_ any call is made, so a
`PermissionDenied` arising inside the dispatch can only have come from the bus.

Two D-Bus error names arrive there, and only two are not faults:
`InteractiveAuthorizationRequired` (nothing could ask — no polkit agent is
running for this session) and `AccessDenied`. systemd sends the second one
whether the dialog was dismissed, the password was wrong, or the policy says no
outright, so the sentence covers all three rather than guessing. This was
measured, not assumed: an attempt to provoke the refusal path with
`sudo -u nobody` failed because polkit resolves the subject through the
_session_, not the uid, and `auth_admin_keep` had already cached the earlier
authentication.

**A second protection list, and what is deliberately not on it.** The user list
protects what holds up a session; the system list is the same question asked of
a machine, not the same list with additions — `systemd-journald.service` and
`.socket`, `systemd-logind.service`, and the target chain `sysinit` → `basic` →
`multi-user` → `graphical` → `default`. What is absent carries as much weight:
`sshd.service` and `NetworkManager.service` can cut the very session a remote
user is holding, and they are still not protected, because a tab that refuses
the thing the user came to do is a tab they work around. The refusal sentence
had to learn the manager too — a protected system unit does not hold up _a
session_, it holds up the machine.

**The unit-file verbs are withheld on purpose, not pending.** polkit would
authorise `EnableUnitFiles` the same way it authorises `StartUnit`. The reason
they are not offered _in this stage_ is what they mean: enabling or masking a
system unit changes what the machine does at the _next boot_, for everyone on
it. A runtime verb is answered by looking at the same row again; a symlink
under `/etc/systemd/system` needs a reason of its own to be reachable from a
key. `Op::writes_files()` draws that line and `Verb::available_on` enforces it
— on both roads, so the CLI cannot reach what the tab will not show. Stage 3
moves the line rather than removing it: see
[stage 3](#what-it-took-beyond-the-plan--stage-3).

**Measured, at the end.** The same call, in the same second, against the same
unit: through the adapter it ran and reported `done`; through `gdbus`, which
does not set the flag, it came back
`org.freedesktop.DBus.Error.InteractiveAuthorizationRequired`. That is the
proof that the flag is what carries the write, rather than some ambient
privilege. No dialog appeared during the test, and that is correct rather than
skipped: the policy for `org.freedesktop.systemd1.manage-units` is
`auth_admin_keep` for an active session, so a session is asked once and
remembered.

---

### What it took, beyond the plan — stage 2b

**Two read-side columns were lying on the system tab, and had been since stage 1.** `preset.rs` held one hard-coded list of directories (`…/user-preset`) and
`shadow.rs` asked one hard-coded question (`systemd-analyze --user
unit-paths`); both call sites in `adapter.rs` used them whatever manager the
instance pointed at.

The preset case is the sharper one, because an empty policy is not a neutral
answer. This machine has no `/usr/lib/systemd/user-preset` file at all, so the
system tab read _nothing_, and "no policy" means `enable` — which is what
systemd itself would do. Result: 177 system units reported `should-enable`
while the real policy, `/usr/lib/systemd/system-preset/99-default.preset`, says
`disable *`. After the fix the same listing shows 51 real drifts (30
`should-disable`, 21 `should-enable`), and each one cross-checks against
`systemctl is-enabled`: `avahi-daemon.service` is enabled against a
`disable *` policy, `machines.target` is disabled while `90-systemd.preset`
names it.

The shadow case fails the other way and is therefore easier to miss: the two
scopes return disjoint directory lists, so asking the user question about
system units yields no shadow _ever_ rather than a wrong one. On a machine
with no `/etc` override of a vendor unit — this one — the column looks right
while being blind.

Both are now `load_for(manager)`. This was found while opening the stage 3
gate, and it had to be fixed first: `preset` the verb is the one those two
columns exist to make predictable, and releasing it while the column proposed
enabling 177 units against the distribution's policy would have been exactly
the trap [phase 6a](#phase-6a--what-the-preset-wants) was built to close.

---

### What it took, beyond the plan — stage 3

**The line is masking, not file-writing.** Stage 2 withheld every verb that
touched a symlink; stage 3 keeps exactly two of them back, and the criterion
changed with the reasoning. `enable`, `disable`, their `--now` pair and
`preset` all change something the row itself then reports — the `enabled`
column, the `drift` column — so the tab can be believed about its own effect.
Masking cannot be checked that way: a masked unit reads `inactive` in every
runtime column, with no failure and no journal line, exactly like a unit nobody
started, and the symlink to `/dev/null` that explains it appears on one level
only. It is withheld for being invisible, not for being powerful, and
`Op::masks_a_unit` says so where `Op::writes_files` used to.

**A prompt that is right on one manager is wrong on the other.** `disable` on
the user bus says "it will not come back on the next login", which is the whole
truth there and a misleading half of it on the system bus, where the same key
decides what the machine does at the next boot for every account on it. So
`Verb` grew a second sentence — `confirm_system`, chosen by `Verb::confirm_on(manager)`
— and `enable`, which asks nothing at all on the user manager because it is
cheap and legible, asks on the system manager because it is neither.

The reason this sentence has to exist at all, rather than leaving the question
to polkit: the password dialog knows only that _something_ wants
`manage-unit-files`. It cannot name the unit. The adapter's own question is the
only one in the chain that can, which is why it comes first, before any
password is asked for.

**Two dialogs per file verb on a cold session, accepted rather than optimised
away.** Writing the symlinks is `manage-unit-files`; making the manager notice
them is `reload-daemon`. Separate polkit actions, each `auth_admin_keep`, so
each is asked once and then remembered. The obvious saving is to skip the
`daemon-reload` when the manager's change list comes back empty — which would
hide the second dialog most of the time, because most file verbs are
idempotent. It is not taken. Since [phase 6a](#phase-6a--what-the-preset-wants)
that list has carried refusals as well as changes (`! …` entries for a masked
unit), and an empty list has meant "the symlinks were already right, the
manager's picture was not" often enough that trading a correct row for one
fewer dialog is the wrong way round.

**Opening `disable` on the system manager opened a hole in the protection
list, and closing it changed what protection means.** The list used to answer
to `Verb::disruptive` alone — the unit going down _now_. That was complete
while no file verb existed on the system manager; the moment `disable` did,
`disable dbus.socket` became a key press away and was not disruptive by that
definition, because nothing stops when you press it. It stops at the next boot,
by which time nothing on screen connects the missing unit to the key.

So protection now answers to two shapes of harm through one question,
`Verb::protected()`: `disruptive` for the immediate one and `Op::switches_off`
for the deferred one. Plain `disable` and plain `mask` join the list that was
already there by being disruptive. This tightens the **user** manager too,
where `disable dbus.socket` had the same shape and the same consequence one
login later — it was simply never noticed, because that hole was open from
phase 1 rather than opened by this stage.

---

## Safety net

This adapter writes into the running session, which none of the others do.
The obligations, distributed over the phases that introduce the risk:

- **Protection list** (phase 1, widened in phase 7) — on `init.scope`,
  `*.slice`, `dbus.socket`, `graphical-session.target` and the system list's
  own entries, a verb that takes the unit down is refused, not confirmed. Since
  phase 7 stage 3 that covers two shapes of harm rather than one: stopping it
  now (`Verb::disruptive`) and switching it off for the next boot
  (`Op::switches_off`, which is plain `disable` as well as `mask`).
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
