# 0011 — The tracking policy is one adapter setting, and it governs moves too

- **Status:** accepted, implemented
- **Date:** 2026-09-16
- **Affects:** `not-yet-done-task-core` — `TrackingService::move_tracking`,
  `find_free_slot`, `MoveOptions`; `not-yet-done-local-adapter` —
  `LocalAdapterConfig`, `tracking_policy_from_config`, `open_core_handle`,
  the `move` and `paste-move` paths; `not-yet-done-task-cli` — `track move`.
  Reverses the "where the group paths live" choice of
  [0009](0009-decorator-traits-and-action-aliases.md).

## Context

A focus watcher writes a mirror tree: every minute of work is recorded a
second time under `/Autotrack`, in parallel to the real tracking under
`/Work`. The two trees are meant to overlap — that is what a mirror is.

`toggle-tracking` already knew this: [0009](0009-decorator-traits-and-action-aliases.md)
gave the tasks and trackings views an alias that handed the action a
`group_paths` argument, so starting a `/Work` tracking stopped only `/Work`
trackings. Nothing else knew it. Moving a tracking failed on a day that
looked empty:

> Move failed: Tracking would overlap with existing trackings — use
> `--allow-overlap` to override

The day held five visible `/Work` trackings and 109 invisible `/Autotrack`
ones. The gravity search that picks an evasive slot reads the view's
snapshot, which the view query filters to `/Work`; the check inside
`move_tracking` reads the whole table. The search offered a slot it believed
free and the check refused it, and no argument on any binding could have
changed that, because the argument only ever reached `toggle-tracking`.
`equalize_trackings.py`, which moves a day's trackings flush against each
other, aborted on its first move and rolled the whole run back.

The insight the fix rests on: **"is this tracking in the way?" is the same
question as "would starting here have stopped it?"** One rule answers both,
and that rule already existed as `TrackingPolicy`.

## Options

### Where the group paths live

1. **An alias per binding** — what 0009 chose. It covers the actions someone
   thought to alias. `move` had no alias; neither does a script that invokes
   the action through the adapter CLI, where there is no binding at all to
   carry arguments. Adding an alias per action multiplies the same list of
   paths across the files again, which is the repetition 0009 set out to
   remove.
2. **A config key on the adapter (chosen)** — 0009 rejected this as "a second
   channel into the action beside its arguments". That objection holds for an
   _action argument_: something the caller decides per invocation. It does
   not hold here, because the group paths are not an input to
   `toggle-tracking`. They describe the instance's data — which subtrees are
   mirrors of which — and every action that asks whether an overlap matters
   needs the same answer. `LocalAdapterConfig` already carries
   `allow_parallel`, which is the degenerate case of exactly this setting.
3. **A shared config both binaries read.** Would have reached
   `nyd-t track move` as well. Rejected: it puts a second configuration
   surface next to the adapter block for the sake of the domain CLI, when the
   answer for scripts is to invoke the adapter (see
   [0004](0004-two-cli-binaries-adapter-vs-domain.md) and the open edge below).

### What a contradictory configuration does

`group_paths` and `allow_parallel: true` cannot both be honoured, and a
`group_paths` entry can be an invalid regex.

1. **Warn and fall back to `Exclusive`.** Every action then runs under a
   policy nobody asked for, and the warning scrolls past.
2. **Fail the first action that needs a policy.** The tab loads, the mistake
   surfaces at the first keystroke, and reads that never consult the policy
   keep working — which makes the tab look healthy.
3. **Refuse to open the adapter (chosen).** `tracking_policy_from_config` runs
   in `open_core_handle`, before the handle exists. A contradictory or
   unparseable configuration means the instance does not open and says why;
   nothing runs under a guessed policy.

### Which overlaps block a move

`find_overlapping` answers "which rows share time with this interval", and it
has to keep answering exactly that — `equalize` relies on half-open intervals,
where adjacency is not overlap. The policy is applied as a filter on its
result, in `blocking_overlaps`: `Exclusive` keeps everything, `Parallel`
keeps nothing, `Grouped` keeps the rows whose label path the policy says a
start would have stopped. The same-task check runs on the unfiltered list —
a task never overlaps itself, whatever the groups say.

## Decision

`LocalAdapterConfig` gains `group_paths: Option<Vec<String>>`.
`tracking_policy_from_config(allow_parallel, group_paths)` folds the two keys
into one `TrackingPolicy` and is called from `open_core_handle`, so the
policy is settled before the adapter is usable. `MoveOptions` gains
`policy: TrackingPolicy` (default `Exclusive`), which `move_tracking` and
`find_free_slot` push through `blocking_overlaps`. The adapter's `move` and
`paste-move` fill it from `tracking_policy_for`, the same per-invocation seam
`toggle-tracking` uses, so an individual call can still override the
instance's policy. `split_tracking` has no overlap check and needs no policy.

## Consequences

- The gravity search and the check it feeds now read the same set of
  intervals. The failing move lands; `equalize_trackings.py` completes
  instead of rolling back.
- The alias is gone from the view files: the instances configure
  `group_paths` and bind the plain `toggle-tracking` again. 0009's aliasing
  machinery stands unchanged — the documented example now shows an alias that
  _narrows_ the instance's policy (`args: { group_paths: [] }` for a
  toggle that stops everything), which is what a per-binding argument is for.
- `allow_overlap` is unaffected: it still overrides the answer, whatever the
  policy decided.
- An instance without `group_paths` behaves exactly as before, and
  `allow_parallel` keeps its meaning.
- **Open edge:** `nyd-t track move` does not read adapter configuration and
  stays `Exclusive`. Scripts therefore have to move trackings through the
  adapter CLI; `equalize_trackings.py` was migrated in this step. Whether the
  domain CLI should exist at all is 0004's question, not this one.
