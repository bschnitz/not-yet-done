# 0001 — Render loop: dirty gating instead of a constant 60 fps redraw

- **Status:** accepted, implemented (variant 1a → **1b**)
- **Date:** 2026-06-03 (1a), 2026-06-04 (1b)
- **Affects:** `not-yet-done-tui` — `main.rs::run_loop`, `App::poll_*`/`tick_*`

## Context

The TUI burned noticeable CPU while idle. The cause: the render loop
polled with a **16 ms** timeout (`events::poll_event`) and called
`App::sync_components()` + `terminal.draw()` **unconditionally on every
iteration**.

`sync_components()` is not cheap — it rebuilds the active content table
(`rebuild_table`), the task table if there is one, collects the subtab
labels of **all** content views into fresh `Vec<(String, bool)>` and
rebuilds the status bar hints (more string allocations). That ran ~60×/s
even when no key was pressed and no background message arrived — constant
load with nothing visibly changing.

An important property of the codebase takes the sting out of the fix:
the need to redraw does **not** hang off deep components, it hangs off the
loop's few entry points. Visible changes only come from (1) a key press,
(2) an incoming async `LoadMsg`, (3) a timer (busy banner seconds, active
tracking duration), (4) a return from an editor or script.

## Options

1. **A global `redraw` flag** that any component may set.
   Rejected: it spreads mutable state across the whole tree; every new
   place that changes something has to remember the flag; hard to test.
2. **1a — bubble it up through return values (chosen).** The sources of
   change (`poll_load`, `tick_active_trackings`, `tick_animations`,
   `poll_live_editor`, `poll_editor_close`, `poll_commit_result`,
   `poll_detached_script`) return `bool`/`Option`; the loop ORs them into
   a local `dirty` and calls `sync_components()`+`draw()` only when
   `dirty`. Deep components stay untouched. No global state.
3. **1b — a fully event-driven `select!` loop.** A `tokio::select!` over
   the crossterm `EventStream`, `load_rx`, `commit_rx` and a 1 Hz
   `interval` (armed only while a banner or tracking is alive). An
   arriving message _is_ the redraw signal; idle = parked = 0 % CPU.

## Decision

**1a** first. It removes the bulk of the idle CPU (no more `sync`/`draw`
without a change), is low risk, and touches neither the terminal event
machinery nor the Kitty protocol and editor suspend/restore handling. The
poll timeout goes from 16 ms up to **200 ms**: `event::poll` returns
immediately on a key press (so no input latency), the timeout only bounds
idle wakeups, and an idle wakeup that finds nothing does no `sync`/`draw`
work. Async messages show up with ≤ 200 ms latency — imperceptible.

The one time-driven special case is the busy banner: its seconds counter
comes from `SystemTime::now()` at render time, so it would freeze between
events unless something nudges it. `App::tick_animations()` therefore
returns `true` ~1×/s for as long as `has_live_banner()` holds (= some
content view is in `AdapterStatus::Busy`). `Connecting`/`Failed`/retry are
static text and do not need it. Active tracking duration cells run through
`tick_active_trackings` on its own adaptive interval.

## Consequences

- **Positive:** idle CPU drops sharply (no more table rebuild/repaint
  60×/s). No global redraw state; the information flows upward as a return
  value and is aggregated locally in the loop — easy to test
  (`is_busy_tracks_adapter_status`).
- **Negative / open:** it is still a poll loop — while idle roughly 5
  trivial wakeups/s (no draw), not a true 0 %. Async display latency
  ≤ 200 ms.
- **Follow-up step 1b — implemented (2026-06-04):** `run_loop` is now a
  `tokio::select!` over the crossterm `EventStream`, `load_rx`, `commit_rx`
  and a conditional 200 ms `interval`. Idle = parked in the `select!`; an
  incoming `LoadMsg`/`CommitMsg` wakes the loop immediately (no 200 ms cap
  any more), which is at the same time the precondition for the planned
  adapter invalidation push (streaming adapters, see the Stoat plan). 1a
  was a genuine subset — no throwaway code; the dirty gating stays.

## 1b — how the tricky parts were solved

- **`EventStream` ↔ editor/script suspend (the main risk).** crossterm
  0.29's `EventStream` starts a background thread that blocks on stdin
  **only** while no event is pending; its `Drop` wakes that thread through
  the internal waker without consuming a byte of stdin. We use that: before
  **every** suspend point (inline editor, launched editor, interactive
  script, `Reopen` after a validation error) the reader is dropped and
  afterwards recreated via `EventStream::new()`. That way the child process
  owns stdin alone. Enabling and disabling the Kitty protocol stays where
  it was, in the editor dispatch functions (pure stdout writes, no conflict
  with the reader).
- **Poll-based sources without a waker.** `poll_live_editor`,
  `poll_editor_close`, `poll_detached_script`, `tick_animations` (busy
  banner seconds) and `tick_active_trackings` (tracking duration) have no
  channel. They run in the `interval` branch, which
  `App::needs_periodic_tick()` **arms only** while one of those sources is
  alive (editor/script pending, busy banner active, tracking running). With
  nothing time-driven alive the branch is disabled and the loop parks
  purely on events and channels → a true ~0 % idle.
- **`poll_load`/`poll_commit_result` split up.** `recv()` inside the
  `select!` consumes exactly one message; it is handled through the newly
  extracted `App::handle_load_msg` / `App::handle_commit_msg`, after which
  we drain the rest (`poll_load` with `try_recv`). That way no already
  `recv()`-ed message is lost.
- **Resize.** Terminal resize events now mark `dirty` explicitly
  (previously covered implicitly by the 200 ms repaint).

## Consequences of 1b

- **Positive:** idle is genuinely parked now (no periodic wakeup at all as
  long as nothing time-driven is pending). Async display latency without a
  200 ms cap. A clean base for out-of-band push (adapter invalidation).
- **Negative / open:** the crossterm `EventStream` reader thread is
  recreated once per suspend cycle (negligible, since that only happens on
  editor and script invocations). A very narrow race window: a key press
  landing exactly between "open the editor" and the completed
  `drop(reader)` could be swallowed by the old reader — irrelevant in
  practice, because the editor only becomes visible afterwards (the same
  class as the existing mode-switch races).
