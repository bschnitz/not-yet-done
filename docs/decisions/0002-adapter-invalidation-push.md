# 0002 — Adapter invalidation push (streaming adapters)

- **Status:** accepted, implemented (Stoat phase 2)
- **Date:** 2026-06-04
- **Affects:** `not-yet-done-content` (`Invalidation`,
  `ContentAdapter::subscribe_invalidations`), `not-yet-done-tui`
  (`LoadMsg::AdapterInvalidation`, `spawn_content_invalidation_watcher`,
  `handle_adapter_invalidation`), `not-yet-done-stoat-adapter` (gateway)

## Context

Every adapter so far is **pull-only**: the frontend asks via
`list()`/`get_by_id()`, the adapter answers. Chat (Stoat) breaks that
model — new messages, edits, deletes and reactions arrive as WebSocket
events and have to trigger a reload of the affected view **out of band**,
without the user doing anything.

Important prior work: since ADR `0001` (variant 1b) the render loop is
event-driven (`tokio::select!` over `load_rx` among others). An incoming
push signal therefore wakes the loop immediately — no poll interval in the
way. And an adapter→TUI push precedent already exists: `subscribe_status()`
(`watch<AdapterStatus>`), whose updates a background watcher pumps into the
`load_tx` channel.

## Decision

Live updates are **the same machinery as `subscribe_status`, generalized**:
instead of "the status changed" → "node X is stale".

- A new adapter-neutral type `Invalidation` plus the trait method
  `ContentAdapter::subscribe_invalidations()` in `not-yet-done-content`,
  with a **no-op default** (a receiver that never fires, whose sender lives
  for the lifetime of the process). Pull-only adapters have to touch
  **nothing** (open/closed).
- The gateway pushes `Invalidation::Node{id: <channel>}` on message and
  reaction events, and `Invalidation::All` on every `Ready` (both the first
  connect **and** a reconnect resync).
- Per view the TUI spawns an invalidation watcher next to the status
  watcher, which pumps the receiver into the **existing** `load_tx` channel
  (`LoadMsg::AdapterInvalidation`). `poll_load` reloads the affected panes
  at their current level.

## Options (and why they were rejected)

1. **A dedicated new push channel per adapter, all the way into the loop.**
   Rejected: `load_tx` plus the 1b `select!` loop already carry async
   results and wake the loop. A second channel doubles the wiring for no
   gain.
2. **`watch` instead of `broadcast` for the invalidations** (like
   `subscribe_status`). Rejected: `watch` only holds the **latest** value —
   two invalidations for different channels in quick succession would
   coalesce into one and the intermediate state would be lost.
   Invalidations are discrete **events**, not latest-value state.
3. **`mpsc` instead of `broadcast`** (as the original plan sketch had it).
   Rejected: `mpsc` is single-consumer. One adapter instance can feed
   **several** views (two tabs or splits on the same adapter), each of
   which has to call `subscribe_invalidations()` independently —
   `broadcast` fans out, `mpsc` does not.
4. **An app-wide `NodeRef` as the payload** (instead of the
   adapter-internal node ID). Rejected: the watcher is already bound to one
   concrete view (`view_index`), so it does not need to route. All it needs
   is "which level inside the view" — that is, the raw parent node ID the
   pane holds in `parent_node_id()` anyway. A `NodeRef`
   (`<type>/<instance>/<id>`) would have to be built by the adapter and
   taken apart again by the watcher — coupling to the frontend's path
   encoding for no benefit.
5. **Push `All` on every event** (reload everything). Rejected: a message
   in a channel that is not open would pointlessly refetch every view of
   that adapter over REST. Matching `Node{id}` against `parent_node_id`
   reloads only the channel level that is actually visible.

## Consequences

- **The first `Ready` pushes `All`** ⇒ the initially empty Stoat tree now
  fills up **without** a manual `r`. A reconnect resyncs just as
  automatically.
- A reload resets the pane cursor to the default behaviour (no "stay at the
  reading position") — accepted for phase 2.
- On a `broadcast` `Lagged` (the frontend briefly too slow) the watcher
  resyncs conservatively with `All` — no update is lost, it is just
  coarser.
- **Remaining limitation:** **structural** live events
  (`ChannelCreate`/`Delete`/`Update`, `Server*`) are not yet applied
  incrementally to `StoatState` — a newly created or renamed channel only
  shows up after a reconnect. The reason: that would need incremental event
  application (a concern of its own), and unlike the message events the
  exact wire shapes of those events have not been verified with `curl` yet.
  Follow-up work.
- The pattern is **reusable** for future push backends (a streaming adapter
  only implements `subscribe_invalidations` and feeds its
  `broadcast::Sender`).
