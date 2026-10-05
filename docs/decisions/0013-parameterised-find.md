# 0013 — Search is one action with optional parameters, not a second query language

- **Status:** accepted, implemented
- **Date:** 2026-10-05
- **Affects:** the tasks adapter's `find` action
  (`not-yet-done-local-adapter/src/task/find.rs`), the TUI's `tree_find`
  binding (`args:`), the CLI's `--query`/`--query-name` on actions, and
  `NodeHit` (new `columns`). Doc surfaces: `README.md`
  ("Searching tasks: `find`"), `docs/architecture.md`,
  `docs/generic-view-spec.md`, `docs/examples/views/tasks.yaml`.

## Context

Searching was a convention: an adapter that can search declares an ordinary
action on its root (`find`), reads the search string from
`ActionContext::text` and answers with `ActionDispatch::Nodes`, the hits with
their ancestor path so a front-end can expand the tree to them. For tasks that
string matched descriptions, and that was all.

Plain questions about the forest had no answer through the adapter:
"which open tasks are due today", "what is overdue under Work", "what did I
work on this week". A saved query can express the conditions, but it is SQL
over single rows. It answers with rows, not with the place of each row in the
tree, and a script that wants the path has to walk the tree level by level
with one `ls` per node. That is slow and has to be written again by every
consumer.

A second gap sat underneath. Stored dates are RFC 3339 text in UTC, while a
date in a query (`today`) resolved to a local instant with its offset. The SQL
filter compared the two as text, so `+02:00` against `+00:00` sorted wrong
around midnight.

## Options

1. **A new search action** beside `find`, with its own parameters.
2. **A search form in the TUI** that assembles a saved query and runs it.
3. **`find` takes optional parameters** for the conditions people search by.
   The free text is one of them, and a front-end binding can fix any of them.

## Decision

Option 3. `find` declares its conditions as ordinary `ParamSpec`s: text and
where to look for it, status, tags, due and start dates, created, updated and
tracked ranges, subtree, priority, recurring, notes, deleted, sort and limit.
It evaluates them against the in-memory forest the adapter already holds, and
answers with paths.

- **Every parameter is optional**, and an absent one costs nothing. Only
  `text` with `in=notes|both` reads note files; every other condition is a
  comparison on rows that are already in memory. Without parameters `find`
  lists every live task, which keeps the old behaviour (text only) a special
  case and not a separate path.
- **The pane's query still scopes the result.** Hits outside the visible set
  are dropped as before, because the expand walk cannot reach them. On the
  CLI, `--query`/`--query-name` now reach actions too and play that role.
- **The TUI maps a key to a parameter set.** A `tree_find` binding takes the
  same `args:` block as any other action. The typed text travels beside the
  arguments, so `/` without `args:` behaves exactly as before, and a binding
  with `args:` is a canned search the user can still narrow. No form, no new
  binding type.
- **Errors are reported, not swallowed.** An unknown parameter, an unreadable
  date or an `under` that names no task fails with a message. An empty result
  would read as "nothing matches".
- **Dates follow the query language** (`today`, `friday`, `2 weeks ago`). A
  `_to` bound at local midnight means "through that day", because `due_to=today`
  that misses a task due tonight would be wrong in the common case.
- **The SQL filter normalises instants to UTC** before comparing, which fixes
  the offset bug for saved queries independently of `find`.

Option 1 was rejected because two search actions mean two conventions for
every front-end to learn, and the `/` key would have to choose between them.
Option 2 was rejected because it would rebuild the search in one front-end
only. The CLI and scripts would still have no path from conditions to tree
positions.

## Consequences

- One action serves the TUI (`/`, canned keys), the CLI and scripts, and every
  front-end gets new parameters without changes.
- `NodeHit` gained `columns`, extra cells a front-end may show (the CLI table
  prints them), so a search result says _why_ it matched (status, due, path)
  without a second request.
- The parameters are task-specific. Other adapters keep the text-only `find`
  until they declare their own; the convention allows that because parameters
  are per action.
- Filtering happens in memory, not in SQL. That is cheap at the forest sizes
  this tool holds, and it keeps the tree position of each hit, which SQL
  does not have. A much larger forest would need the conditions pushed into
  the query and is not addressed here.
