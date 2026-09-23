#!/usr/bin/env python3
# mode: form
# scope: node
"""Log work against the selected node — a worked example of a form script.

Every round is a fresh process: the request file named by ``argv[1]`` carries
the scope payload, every value collected so far under ``input``, and whatever
this script sent as ``state`` last time. The answer goes to ``$NYD_OUTPUT_FILE``
and is exactly one of ``form``, ``result`` or ``error``.

What the three rounds of this example show:

1. **Ask.** A form with a duration, a date, a choice and a field that only
   appears for one of the choices (``visible_when``, decided in the widget —
   no round needed).
2. **Check.** The rules live here, not in the field declaration: a date in the
   future and more than a working day at once are both refused *per field*,
   with the typed values still on screen. ``fields`` is omitted, so the form
   stays as it is and only the errors change.
3. **Confirm.** Once the input is sound a second, different form asks for a
   confirmation — the case that needs a round rather than ``visible_when``,
   because only this script knows what the summary says. Replacing the fields
   drops the values of the fields that went away, so the answers ride along in
   ``state`` instead. That is what ``state`` is for.

Nothing here talks to a real tracker; ``ACTIVITIES`` stands in for the lookup a
real script would cache in ``state``.
"""

import datetime as dt
import json
import os
import sys

# Pretend this came from a backend on the first round and was cached in
# `state` afterwards — that is what `state` is for.
ACTIVITIES = ["development", "review", "meeting"]

MAX_MINUTES = 8 * 60


def answer(payload):
    """Write one round's answer and leave."""
    with open(os.environ["NYD_OUTPUT_FILE"], "w") as f:
        json.dump(payload, f)
    sys.exit(0)


def ask(state, errors=None, fields=None, header="Log work"):
    form = {"header": header}
    if fields is not None:
        form["fields"] = fields
    if errors:
        form["errors"] = errors
    answer({"form": form, "state": state})


def first_form():
    return [
        # `duration` accepts `1:30`, `90m` and `1h30` alike and submits whole
        # minutes, so this script never parses a spelling.
        {"name": "spent", "label": "Time spent", "type": "duration"},
        {"name": "day", "label": "Day", "type": "date", "default": "today"},
        {
            "name": "activity",
            "label": "Activity",
            "type": "select",
            "options": ACTIVITIES,
            "default": "development",
        },
        {
            "name": "reviewed",
            "label": "Reviewed for",
            "type": "text",
            # Decidable from the form itself: no round, no process.
            "visible_when": {"field": "activity", "equals": "review"},
        },
        {
            "name": "comment",
            "label": "Comment",
            "type": "multiline",
            "lines": 4,
            "required": False,
        },
    ]


def check(values):
    """The rules the field declaration cannot express."""
    errors = []

    minutes = int(values.get("spent") or 0)
    if minutes > MAX_MINUTES:
        errors.append(
            {"name": "spent", "error": f"more than {MAX_MINUTES // 60} hours at once"}
        )

    day = values.get("day", "")
    try:
        # A `date` field submits the phrase as typed; resolving it is the
        # backend's job, and here that is us.
        parsed = dt.date.fromisoformat(day) if day != "today" else dt.date.today()
    except ValueError:
        errors.append({"name": "day", "error": "not a date I can read"})
    else:
        if parsed > dt.date.today():
            errors.append({"name": "day", "error": "must not be in the future"})

    if values.get("activity") == "review" and not values.get("reviewed", "").strip():
        errors.append({"name": "reviewed", "error": "say what was reviewed"})

    return errors


def main():
    with open(sys.argv[1]) as f:
        request = json.load(f)

    values = request.get("input", {})
    state = request.get("state") or {}
    node = request.get("node") or {}

    # Round 1: nothing collected yet.
    if not values:
        ask({"seen": True}, fields=first_form())

    # The confirmation round came back — `confirm` only exists there, and the
    # earlier fields are gone, so their answers come out of `state`.
    if "confirm" in values:
        logged = state.get("pending", {})
        if values["confirm"] != "true":
            # Back to the first form, with every answer prefilled — the fields
            # were replaced, so `input` no longer carries them.
            fields = first_form()
            for field in fields:
                if logged.get(field["name"]):
                    field["prefill"] = logged[field["name"]]
            ask(state, fields=fields, errors=[{"error": "nothing was logged"}])
        answer(
            {
                "result": {
                    # An ordinary script answer: whatever the front-end
                    # should do now that the writing is done.
                    "commands": ["reload"],
                    "logged": {
                        "node": node.get("id"),
                        "minutes": int(logged["spent"]),
                        "day": logged["day"],
                        "activity": logged["activity"],
                    },
                }
            }
        )

    errors = check(values)
    if errors:
        # No `fields`: keep the form on screen, change only what is wrong.
        ask(state, errors=errors)

    # Sound input — ask for the confirmation only this script can word.
    minutes = int(values["spent"])
    summary = f"{minutes // 60}h{minutes % 60:02d} of {values['activity']}"
    ask(
        # The answers have to survive the field list being replaced.
        {**state, "pending": values},
        header="Log this?",
        fields=[
            {
                "name": "confirm",
                "label": f"Log {summary} on {values['day']}",
                "type": "toggle",
                "default": "true",
            }
        ],
    )


if __name__ == "__main__":
    main()
