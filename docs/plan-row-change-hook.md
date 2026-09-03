# Plan: a script hook that fires when the cursor changes row

> **Status: planned.** Nothing of this is implemented yet. The two existing
> hooks (`reload`, `load`) live in `not-yet-done-tui/src/app/script_hook.rs`;
> this document adds a third one next to them and says what it may and may
> not do, because it is the first hook that fires from a **key press** rather
> than from a load, and the first one whose script must **not** block the
> render loop.

## Goal

A view script can be bound to "the cursor moved onto another row" the same
way it is bound to `load` and `reload` today (`ctrl+h` in the `:script`
menu). It receives its usual payload plus the row it came **from** and the
row it went **to**, and it runs without the TUI waiting for it.

The motivating case: an HTML preview of the selected mail that follows the
cursor. `o p` opens the preview in a browser window, and from then on every
`j` / `k` re-renders it for the newly selected message — the browser turns
into a reading pane the TUI itself does not have to draw.

## What is different about this hook

Everything the two existing hooks do is _load-shaped_: a fetch finishes, the
rows land, a script runs once, synchronously, while nothing else can happen
anyway. A row-change hook breaks all three of those assumptions.

- **It fires from navigation, not from a fetch.** `j` held down produces
  dozens of row changes per second.
- **It must not be synchronous.** `run_script_background` waits for the child
  (`wait_with_output`), which is fine after a load and unusable per keypress:
  a 300 ms render would make the list feel broken.
- **Its answer cannot be control flow.** A `reload` hook may answer with
  `commands`; a command that reloads moves the cursor, which fires the hook
  again. The existing depth guard rides on the _load_, so it would not catch
  it.

The three rules below follow directly from that.

## The contract

### When it fires

The hook fires when the **focused pane of the active tab** shows a different
selected row than the last time this pane reported one, and the selection has
then been still for a settle delay.

- **Identity is the row's node id, not its index.** A reload that keeps the
  cursor on the same mail changes nothing; a sort that moves the same row to
  another index changes nothing either. Both would otherwise re-render a
  preview that is already correct.
- **Arriving counts as a change.** When a pane gets a selection it did not
  have before — a level opens, a tab is entered for the first time — the hook
  fires with `previous_index: null`. A script that only wants to _update_
  something already on screen can see that from the payload; a script that
  wants to open something on arrival has no other way to be told.
- **Per pane.** The last reported row is remembered per `(view, pane)`, so
  switching tabs and coming back to an unmoved cursor is not a row change.
- **Nothing selected is not a row change.** An empty pane reports nothing and
  forgets its last row, so re-entering it fires again.

### The settle delay

`script.row_change_delay_ms` in `tui.yaml` (default **250**). The hook fires
once the selection has been unchanged for that long, so holding `j` through
forty rows runs the script **once**, for row forty — and `previous_index` is
the row the burst started from, because the rows in between were never
reported to anybody.

The delay is a config option rather than a constant because the right value
depends on what the script does: a preview that costs a pandoc run wants
250 ms or more, a script that only writes a file could sit at 50.

The timer is a `tokio::select!` branch in the main loop next to
`which_key_deadline` and `auto_reload_deadline` — the same shape, so an
unarmed hook costs no wakeups at all.

### How the script runs

**Detached and fire-and-forget.** The child is spawned and _not_ waited on;
the render loop goes straight back to drawing. The child is reaped on the
existing 200 ms ticker.

- **At most one run per script at a time.** If the cursor moves again while a
  run is still alive, the new change re-arms the timer instead of starting a
  second child. When the running one exits, the pane is looked at again — so
  the script converges on the row the cursor actually rests on, and two
  renders never race for the same output file.
- **`mode: background` only.** `interactive` and `capture` are refused when
  binding, exactly as for the other hooks — and `commands` is refused too,
  for this hook only: see below.
- **stdout is ignored, a failure is reported once.** A non-zero exit (with
  its stderr) becomes a notification, but an identical message repeating
  back-to-back is suppressed: a broken script must not be able to fill the
  notification log one line per cursor move.
- **`NYD_SCRIPT_HOOK=row_change`** in the environment, so a script that is
  also runnable by hand from the menu can tell the two apart — same
  convention as `load`.

### What it may answer with

**Nothing.** Neither `cells`, `highlights` and `order` (there is no load to
patch — the rows have been on screen for a while) nor `commands` (a command
that reloads or jumps moves the cursor, and the guard that stops a `reload`
hook from looping rides on the load, which this hook does not have). A script
bound here acts on the world outside the TUI; if it wants the view to catch
up it has to say so through a key the user presses.

### The payload

Whatever shape the script's `# scope:` header (or the level's action scope)
asks for — `node`, `filtered_set` or `table`, unchanged — plus one extra
top-level key:

```json
{
  "node": { "...": "..." },
  "row_change": {
    "previous_index": 3,
    "previous_id": "<node id>",
    "next_index": 4,
    "next_id": "<node id>"
  }
}
```

`previous_index` / `previous_id` are `null` on the first row a pane reports.
The `next_*` pair restates what a `node` or `table` payload already carries;
it is written out anyway so that the block is readable on its own and means
the same thing under all three shapes — a `filtered_set` payload has no
cursor in it at all.

Indices are into the **displayed** order, the same number `table` reports as
`selected_index`.

## Then: the mail preview that follows the cursor

Two scripts on the `mail:message` level, both thin, with the work in the
shared preview engine (`_lib/nyd_html_preview.py`, outside the repo):

1. `html_preview.py` — bound to `o p` as everywhere else. Renders the
   selected message and opens the browser window. Writes a small **session
   file** naming the pane it is following.
2. `preview_follow.py` — bound to the `row_change` hook. Does nothing unless
   that session file exists and is fresh; otherwise re-renders the newly
   selected message into the **same** output file. The page reloads itself
   when the stamp next to it changes, so the window is not raised, re-focused
   or replaced while the user is reading.

This is why the hook is allowed to fire on arrival with no previous row: the
follow script wants to catch up when a folder finishes loading, too.

Two things the mail side needs before that works:

- **A message must be addressable from the CLI.** `nyd adapter
mail:folder:message <id> cat` currently fails with `no child 'message' at
level 'mail:folder' (available: )` — `childs()` builds the folder's
  children out of the folder **id**, and the CLI walks the type tree with an
  id-less prototype. The child types have to be declared even when the id
  cannot be parsed (with a `list` that then errors), which is the same gap
  `mail:folder help` already reports.
- **A `mail` profile in the preview engine.** A message body is plain text
  (`mime::body_text` prefers the text part), not Markdown, so the profile
  renders it as a body block under a header table of the envelope fields
  instead of running it through the Markdown reader.

## Phases

All seven are implemented: 1-4 in `db0b2ad`, 5 in `8422b3e`, 6 in `93f5336`, 7 in the script tree outside the repo (its smoke block is `b51ba0b`). What is left is the live smoke — both blocks in `docs/smoke-tests.md` need a TUI restart, because shortcuts and hook bindings are read once at start.

| #   | what                                                                                                                           |
| --- | ------------------------------------------------------------------------------------------------------------------------------ |
| 1   | `ScriptHook::RowChange`: the vocabulary, the picker entry, the binding rules (`background` only), the round-trip test.         |
| 2   | Detection: the per-pane fingerprint, arming, the settle deadline in the main loop.                                             |
| 3   | The run path: detached spawn, one run per script, reaping on the ticker, deduped failure notice, `NYD_SCRIPT_HOOK=row_change`. |
| 4   | The payload: the `row_change` block on all three shapes, with its own test.                                                    |
| 5   | `script.row_change_delay_ms` + docs (`generic-view-spec.md` hook table and a section of its own) + a smoke block.              |
| 6   | Mail: declare the folder's child types without an id, so `mail:folder:message <id> cat` works.                                 |
| 7   | The two scripts and the `mail` profile in the preview engine (outside the repo).                                               |
