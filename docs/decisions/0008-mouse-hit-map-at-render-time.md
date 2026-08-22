# 0008 — Mouse input: a hit map recorded while painting

- **Status:** accepted, phase 1 implemented
- **Date:** 2026-08-22
- **Affects:** `not-yet-done-tui` — `mouse/`, `render.rs`, `events.rs`,
  `ui/panel_chrome.rs`, `views/content_view.rs`, `main.rs::run_loop`

## Context

Dragging the mouse across a popup used to select whole **terminal rows**: the
popup's text plus whatever the table behind it happened to show at the same
height. That is not a bug in the app — the app never read the mouse at all, so
the terminal did what it always does with a grid it has no structure for. Only
an application that reads the mouse itself can know that a box was painted
there and clip the selection to it.

Reading the mouse is a one-way door for everything else that follows (clicking
a tab, focusing a pane, pressing a hint, sorting by a column header): all of it
needs the same missing piece, namely a way to turn a cell coordinate back into
"the sort menu" or "the left pane".

The UI is immediate-mode. A widget receives a `Rect`, paints into it and
forgets it; there is no retained tree to walk when an event arrives. Layout is
also not a pure function of the terminal size — it depends on which popup is
open, how wide the hints came out, how the panes were split.

## Options

### Where the geometry comes from

1. **Recompute the layout when the event arrives.** Rejected: it duplicates
   every layout rule in a second place, and the copy is wrong the moment the
   original changes. Nothing would tell us it had drifted.
2. **Record what was painted, while painting it (chosen).** The render pass
   pushes one `(Rect, Region)` entry per visible surface; the lookup walks the
   list backwards, so the topmost surface wins for free — the overlays are
   already drawn in Z-order. The map is ~20 entries, rebuilt every frame, and
   cannot go stale because it _is_ what is on screen.

### How the map is reached

1. **A `HitMap` on `App`, threaded through the render tree.** Rejected, and not
   on taste: a popup draws itself through `PanelChrome` from inside
   `&mut app.some_popup` and therefore cannot also borrow the map off `App`.
   Working around that means an extra argument on all seventeen panel sites —
   plumbing that carries no meaning.
2. **Frame-scoped ambient state (chosen).** A `thread_local!` `Vec`, cleared by
   `begin_frame()`, written by the free function `push()`. There is exactly one
   terminal, one render pass and one map — the same reasoning that already
   makes the hint-width cap and the graphics-protocol probe process-wide.

### Which reporting mode to enable

1. **`crossterm::EnableMouseCapture`.** Rejected: it also sets DECSET 1003
   (report _any_ motion), which means one event per cell the pointer crosses.
   The render loop is event-driven and dirty-gated (see
   [ADR 0001](0001-render-loop-dirty-gating.md)) and idles at ~0 % CPU; 1003
   would wake it continuously for pointer movement nobody asked about.
2. **Write the subset by hand (chosen).** `1000h` (buttons) + `1002h` (motion
   only while a button is held, i.e. drags) + `1006h` (SGR encoding, so
   coordinates past column 223 survive). Events arrive only when something
   actually happened.

## Decision

Record the map at render time, reach it ambiently, enable the three modes by
hand. Two consequences were folded into the same change:

- **One chokepoint for terminal modes.** `events::resume_input_modes()` /
  `suspend_input_modes()` switch the kitty keyboard protocol and mouse
  reporting together. Twelve call sites suspend the terminal (editor launches,
  scripts); a pair that can be enabled independently is a pair one of them will
  eventually forget.
- **The wheel is part of phase 1, not a later phase.** Without reporting, the
  terminal translates the wheel into arrow keys on the alternate screen, so
  scrolling works today. Turning reporting on takes that away. The wheel
  therefore feeds synthetic arrow keys through the normal key path — same
  behaviour as before, and no second scrolling implementation to keep in sync.

Copying is snapshot-based: ratatui resets the drawn buffer on swap, so the
selection's own cells are captured at the end of the render pass while a
selection exists, and read back on release. The write goes to the system
clipboard where one is reachable and to OSC 52 otherwise, which is what makes
copying work over SSH.

## Consequences

- **The escape hatch is the terminal's, not ours.** Terminals keep their native
  selection on `Shift` + drag even while an application reads the mouse, so a
  selection spanning several panes is always one modifier away. Nothing had to
  be built or configured for that, and it is why the app's own selection can
  afford to be strict about its box.
- **Everything is behind the `mouse` cargo feature**, on by default. With the
  feature off the types remain and every entry point is an empty `#[inline]`
  no-op, so neither the render pass nor the event loop carries a `cfg`
  attribute. Not one escape sequence is emitted and the terminal's own
  selection is untouched — the pre-feature behaviour, exactly.
- **`theme.mouse:` is deliberately _not_ feature-gated**, so a
  `tui-theme.yaml` shared across machines parses against either build.
- Later routing (tab clicks, pane focus, table rows, column-header sort, hints
  as buttons) needs no new mechanism — only more `Region` variants and more
  arms in the dispatcher. The payload on `Region::ContentPane` exists for
  exactly that.
- The map describes one frame. Anything that wants to answer a click has to be
  drawn, which is a limitation worth naming: an off-screen row cannot be
  clicked, and scrolling is what brings it into reach.
