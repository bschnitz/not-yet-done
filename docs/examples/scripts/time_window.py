#!/usr/bin/env python3
# mode: commands
# scope: table
# The header describes the manual run from the script menu. On the load hook
# the mode is irrelevant (the answer file is always offered) and the scope is
# fixed to `table` anyway -- there is no cursor before the load.
"""Colour tasks by their time window: too early, in time, too late.

A task can carry a window -- `not_before` (work must not start earlier) and
`due` (work must be finished by then). Bound to the tasks tree's **load** hook
(script menu, ctrl+h), this paints every row whose window says something about
*now*:

  - not startable yet (`not_before` lies in the future)  -> `window-pending`
  - overdue and still open (`due` lies in the past)      -> `window-overdue`
  - everything else                                      -> no colour

Done and cancelled tasks get no colour: an overdue task that got finished is not
a problem any more, and one that will not be done is not a problem at all.
Deleted rows are left alone too -- they are only on screen as greyed-out context.

The two styles are named, not spelled out: they are defined in tui.yaml under
`theme.styles`, where they can be re-coloured without touching this script. A
name that resolves to nothing is reported by the TUI on every load.

The answer is *complete*: the highlight map is rebuilt on every load, so a row
left out loses its colour. That is exactly what should happen when a window
opens or a task gets done -- the next load takes the colour away.

Run by hand from the script menu there is nothing to do: only a load hook may
paint, and the menu reloads the pane after a `scope: table` script anyway, which
runs this very hook.

The window fields arrive as RFC 3339 instants (empty = open on that side); the
status as `status_key`, the stored value, because the visible `status` column is
a nerd-font glyph.
"""
import json
import os
import sys
from datetime import datetime, timezone

PENDING_STYLE = "window-pending"
OVERDUE_STYLE = "window-overdue"
CLOSED = {"done", "cancelled"}

AS_LOAD_HOOK = os.environ.get("NYD_SCRIPT_HOOK") == "load"


def instant(value):
    """An RFC 3339 cell as an aware datetime, or None for an open bound (or
    one this script cannot read -- a bad cell must not cost the other rows
    their colour)."""
    if not value:
        return None
    try:
        dt = datetime.fromisoformat(value)
    except ValueError:
        return None
    return dt if dt.tzinfo else dt.replace(tzinfo=timezone.utc)


def style_for(fields, now):
    if fields.get("deleted") == "true":
        return None
    if fields.get("status_key") in CLOSED:
        return None
    not_before = instant(fields.get("not_before"))
    if not_before and now < not_before:
        return PENDING_STYLE
    due = instant(fields.get("due"))
    if due and now > due:
        return OVERDUE_STYLE
    return None


def highlights(payload, now):
    out = {}
    for row in payload.get("rows") or []:
        style = style_for(row.get("fields") or {}, now)
        if style:
            out[row["id"]] = {"*": style}
    return out


def report(answer):
    """Always written, including the empty answer -- a script in `mode:
    commands` that leaves its output file untouched says nothing at all."""
    out = os.environ.get("NYD_OUTPUT_FILE")
    if not out:
        return
    with open(out, "w") as f:
        json.dump(answer, f)


def load_payload():
    """The TUI hands over the *path* to the payload file, not the JSON."""
    if len(sys.argv) < 2:
        return {}
    with open(sys.argv[1]) as f:
        return json.load(f)


def main():
    if not AS_LOAD_HOOK:
        report({"commands": []})
        return
    payload = load_payload()
    report({"highlights": highlights(payload, datetime.now(timezone.utc))})


if __name__ == "__main__":
    main()
