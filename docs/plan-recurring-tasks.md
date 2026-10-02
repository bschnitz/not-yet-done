# Plan: recurring tasks

A task can carry a **recurrence rule**. Completing it creates its successor:
the same task again, with a new time window computed from the rule. Two kinds
of rule, because two kinds of habit exist:

- **Fixed rhythm** — the schedule does not care when the last one was done.
  Taking a supplement every day at 08:00.
- **After done** — the next one is measured from the completion of the last.
  Sport with a recovery break: three days after the last session, whenever
  that was.

## Decisions

1. **At most one open task per series.** A successor is created when its
   predecessor is completed — never by the clock. An overdue task stays
   overdue, and nothing new piles up behind it. This is what the user asked
   for, and it removes the need for any background ticker: there is no daemon
   in nyd, and none is needed.
2. **Completion is the only trigger, for both kinds.** They differ only in how
   the successor's window is computed:
   - fixed rhythm: the first slot of the schedule after **the later of the
     completion and the task's own `not_before`**. Slots missed while the
     task was overdue are skipped, not made up; and a task completed early
     (07:50 for its 08:00 slot) does not hand its own slot to the successor.
   - after done: the completion time plus the interval.
3. **The window is the existing `not_before` / `due` pair.** A rule says when
   a successor's `not_before` lies and, optionally, how long after it `due`
   lies. Nothing new is needed to show it: the deadlines-widget deadlines corner
   already lists tasks before their `not_before`, and turns them overdue at
   `due`.
4. **Two occurrences a day are two series.** Morning and evening supplements
   are separate tasks with separate rules; one rule never yields more than one
   slot per completion.
5. **No calendar.** Occurrences do not appear in the calendar adapters, so
   there is no RRULE (RFC 5545); the rule is a small grammar of our own,
   written the way a person says it.
6. **Times are local wall-clock times.** `08:00` stays 08:00 across a DST
   change; the window is computed in the local time zone and stored in UTC
   like every other bound.

## Rule syntax

One line in the task's front matter, `recur:`; empty or absent = not
recurring.

```yaml
recur: daily at 08:00, due +4h
recur: mon,wed,fri at 18:00
recur: every 2 weeks on sat at 10:00
recur: monthly on 1 at 09:00
recur: 3d after done
recur: 3 days after done at 07:00, due +12h
```

- **Fixed rhythm**: `daily`, `weekdays`, a weekday list (`mon,wed,fri`),
  `every N days|weeks [on <weekdays>]`, `monthly on <day of month>`, always
  with `at HH:MM`. The slot is the successor's `not_before`.
- **After done**: `<N><unit> after done`, unit `h`/`d`/`w` (or spelt out).
  - **without `at`** — exact: `not_before = completed + N`. Done at 19:20,
    `3d after done` opens at 19:20 three days later.
  - **with `at HH:MM`** — by calendar day: `not_before = (completion date +
N days) at HH:MM`. Done at 19:20 on Monday, `3 days after done at 07:00`
    opens Thursday 07:00. Only day/week units are allowed with `at`.
- **`due +<offset>`** (optional, both kinds): `due = not_before + offset`.
  Without it the successor has a `not_before` and no `due`.

A rule that does not parse is a field error in the editor buffer, like any
other front-matter key.

## Data model

Two new columns on `task`:

| column      | type           | meaning                                                                                                          |
| ----------- | -------------- | ---------------------------------------------------------------------------------------------------------------- |
| `recur`     | text, nullable | the rule as written; parsed when a successor is due                                                              |
| `series_id` | uuid, nullable | shared by every occurrence; the first task's own id. History and later stats ("11 of the last 12 weeks") read it |

The completed task keeps its `recur` and `series_id`: it is history. Only the
open one matters for creating the next.

## Behaviour

- **Creating** a task with `recur:` and no window: the first window is computed
  as if it had been completed now (fixed: next slot from now; after done:
  open at once). An explicit `not_before`/`due` in the buffer wins.
- **Completing** (`done`) **or cancelling** a task with a rule: in the same
  transaction, insert the successor — same description, priority, parent,
  tags, projects, `recur`, `series_id`; status `todo`; new window. Subtasks are
  not copied (open question below).
- **Reopening** a completed occurrence removes its successor if the successor
  is still untouched (status `todo`, never edited, never tracked) — otherwise
  the series would have two open tasks. If it was touched, reopening leaves
  both and says so.
- **Editing** `recur:` on an open task changes only how its successor is
  computed; the open task's window stays as it is.
- **Ending a series**: clear `recur:` (the open task becomes an ordinary one)
  or delete the open task.

## Where it lives

- `not-yet-done-task-core`: a pure module `recur` — parse a rule, and
  `successor_window(rule, completed_at, tz) -> (not_before, due)`; tested
  without a database, `completed_at` and the zone passed in (no clock read
  inside).
- The successor is created where status changes are written, so every path
  gets it: `TaskRepository::update_task` (editor, CLI, deadlines-widget' done click)
  and `apply_batch` (tree edit). One helper, called from both.
- `not-yet-done-local-adapter`: `recur:` in the new/edit buffers
  (`editor_templates.rs`), parse errors as `FieldError`, a `recur` column in
  the views.
- Docs: README (tasks section), view spec / example config if the column is
  shown by default, `docs/smoke-tests.md`.

## deadlines-widget

The deadlines source (`deadlines-widget/sources/nyd-deadlines.sh`) selects `due set OR
tag todo`. A successor with a `due` shows up as-is. A rule without `due +…`
gives a task with only a `not_before`, which the source does not select —
decide when implementing whether `recur` set joins that selection.

## Open questions

- Copy subtasks into the successor (a checklist per occurrence)? Proposal: not
  in the first version.
