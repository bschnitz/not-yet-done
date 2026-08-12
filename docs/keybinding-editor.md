# Editing keybindings from the TUI

The shortcut menu is not just a reference list — it is also where you
rebind, add, delete and reset keyboard shortcuts. Every change is written
back to the config file that owns it (comment-preservingly) and reloaded
in-process, so a new binding is live immediately without a restart.

## Opening the menu

Press `Ctrl+Y` (the `shortcut_menu` action) to open it. It lists the
shortcuts active in the current view; press `Tab` to toggle between this
view and every tab. Type to fuzzy-filter, `↑↓` or `Ctrl+J`/`Ctrl+K` to
move, `Esc` to close (the first `Esc` clears an active filter).

Rows without a key are shown too — those are actions you can give a
binding to.

## What can be edited

| Origin                                                                | Editable | Notes                                   |
| --------------------------------------------------------------------- | -------- | --------------------------------------- |
| View actions (`actions:` in a `views/*.yaml`)                         | yes      | written to that view file               |
| Built-ins (`global` / `common` / `content` / `window` in `tui.yaml`)  | yes      |                                         |
| Tab switches (`Switch to …`)                                          | yes      | written to that view's `tab.key`        |
| Per-node `shortcuts:` (e.g. `s: toggle-tracking`)                     | yes      | the map key is the binding              |
| Subtab keys (`views[*].key`)                                          | yes      | written to that view's `key`            |
| Query menu key, `preview.keybinding`, child `keybindings`             | yes      | written to the owning block in the view |
| `action_chains`, search-jump keys                                     | yes      | written to the owning block in the view |
| Saved-query / `:script` / Postgres-table script shortcuts (DB-stored) | yes      | key lives in the adapter DB, not YAML   |

Every shortcut is editable. YAML-backed bindings are written to their
config file; DB-stored shortcuts (saved queries, `:script`-menu scripts and
Postgres-table scripts, whose key lives in the adapter's `query_shortcut`
table, not any YAML file) are written straight to the database — the same
store the query menus already use.

A DB-stored shortcut holds a **single** chord (there is no alternatives
list), so `Ctrl+N` on such a row _replaces_ its key rather than adding one.

## Recording a binding (`Ctrl+N`)

With a row selected, press `Ctrl+N` to start recording. The heading shows
a live `● rec` prompt:

- Press the keys of the shortcut. A single key press (`a`, `Shift+A`,
  `Ctrl+Shift+A`) records a one-step binding; press several keys in
  sequence (`Ctrl+K` then `L`) to record a chord.
- `Backspace` drops the last recorded step.
- `Return` saves. **Return is never itself part of a binding.**
- `Esc` cancels without changing anything.

The recorded binding is **added as an alternative** — the action keeps any
keys it already had. To _replace_ a binding, delete the old one first
(`Ctrl+D`) and then record the new one.

## Deleting a binding (`Ctrl+D`)

With a row selected, press `Ctrl+D`:

- If the action has a single binding, it is removed straight away.
- If it has several, a picker opens; choose which one to remove with
  `↑↓` and confirm with `Return` (`Esc` cancels).

Removing the last binding disables the shortcut — for a built-in this
writes the disable form (`quit: []`) so the compiled-in default no longer
fires. To the user, disabled and deleted are the same: the key is gone.

## Restoring a default (`Ctrl+R`)

Press `Ctrl+R` on a **built-in** row to drop its `tui.yaml` override and
fall back to the compiled-in default. It also works on a **tab-switch**
row, whose default is the tab's positional autonumber digit (`1`..`9`,
then `0`): restoring removes the `tab.key` override so the digit takes
over again. View actions have no compiled default, so `Ctrl+R` does
nothing for them.

## Tab-switch keys

By default a tab is selected by its position digit — the first tab is
`1`, the second `2`, and so on (`0` for a tenth). Record a binding on a
`Switch to …` row (`Ctrl+N`) to give a tab an explicit key or chord
instead; it is written to that tab's view file under `tab.key`:

```yaml
tab:
  name: Tasks
  key: ctrl+1 # or [j, "ctrl+k t"], or [] to disable the switch key
```

Because a tab-switch key is global, rebinding it conflict-checks against
every other tab's key and every global shortcut. `Ctrl+R` drops the
override and returns the tab to its position digit.

## Conflicts

A binding conflicts when another shortcut in an overlapping scope already
claims the same keys — including built-in globals, and prefix collisions
(`k` shadows `k l`). Shortcuts in different tabs never conflict.

When a recorded binding collides, the menu raises a `y`/`n` prompt that
**lists every colliding binding** — the key and the shortcut that owns it,
one per line. Confirm with `y` to remove all of them from their shortcuts
and bind the key here; decline with `n` (or `Esc`) to leave everything
unchanged — the new binding is _not_ applied.

If any listed conflict is itself read-only, it is tagged `(read-only)` and
cannot be removed; the prompt then only offers `n`/`Esc`, because the key
cannot be freed.

## Config surface

Bindings are written in one of three forms:

- **single** — a scalar: `key: ctrl+shift+a`
- **chord** — a string with steps separated by spaces: `key: "ctrl+k l"`
  (the legacy concatenated form `zr` is still read)
- **alternatives** — a list, any of which triggers the action:
  `key: [a, "ctrl+k l"]`

A space always separates the steps of one chord; a list always means
alternatives. Disable a binding with the empty list: `key: []`. A literal
space bar is written as the word `space` (e.g. `ctrl+space`).
