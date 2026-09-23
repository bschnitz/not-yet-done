#!/usr/bin/env python3
# mode: form
# scope: node
"""Every field type once, with the checks a declaration cannot make.

This script writes nothing and talks to nothing. It collects one of each
field kind, applies the rules that only a script can apply, shows what it
got and waits for a yes — which makes it the thing to run when you want to
see what a form can do, or to try a change to the form driver against
something that exercises all of it.

Four rounds, driven by ``state["step"]``:

1. **about** — the everyday kinds: ``text``, a masked ``text``, ``select``,
   ``multiselect``, ``toggle``, and a ``text`` that appears only for one
   answer (``visible_when``, decided in the widget, no round needed).
2. **details** — the measured ones: ``date``, ``datetime``, ``number``,
   ``decimal``, ``duration``, ``multiline``, ``file``.
3. **confirm** — what you entered, and the yes.
4. **result** — the values back out, unchanged.

The split into two asking rounds is not ceremony: thirteen fields in one
column do not fit a popup on a normal terminal, and a round is what a form
costs when it has more to ask than fits. It also puts ``state`` to work —
replacing ``fields`` drops the values of the fields that went away, so the
first round's answers ride along in ``state`` rather than in ``input``.

Why the checks live here and not in the field declaration: an estimate must
be a Fibonacci number, ``ready`` and ``blocked`` contradict each other, a
bug needs notes but a chore does not. No ``required: true``, ``min`` or
``step`` expresses any of that, and each one is reported *on its field*,
with everything typed still on screen.

Run it from a terminal with::

    not-yet-done-cli script run ./field_tour.py

``multiline`` becomes ``$EDITOR`` on the command line — that is the only
usable equivalent dialoguer has — so the confirmation summary opens an
editor there. ``--set summary=`` skips it, and a fully supplied form runs
without a single question::

    not-yet-done-cli script run ./field_tour.py \\
        --set title='Rework the exporter' --set kind=feature \\
        --set size=5 --set due=today --set summary= --set confirm=yes
"""

import datetime as dt
import json
import os
import sys

KINDS = ["bug", "feature", "chore"]
LABELS = ["ready", "blocked", "needs-design", "quick-win"]

# An estimate is one of these or it is not an estimate. `min`/`max`/`step`
# can describe a range with even spacing; this is neither.
FIBONACCI = [1, 2, 3, 5, 8, 13]

MAX_MINUTES = 8 * 60
MAX_FILES = 3
CHORE_BUDGET = 500.0


# ---------------------------------------------------------------------------
# The protocol: one process per round, answer in $NYD_OUTPUT_FILE
# ---------------------------------------------------------------------------


def answer(payload):
    """Write one round's answer and leave."""
    with open(os.environ["NYD_OUTPUT_FILE"], "w") as f:
        json.dump(payload, f)
    sys.exit(0)


def ask(state, header, fields=None, errors=None):
    """Show a form.

    Omitting `fields` keeps the form that is on screen and changes only the
    errors — the usual case after a failed check, and the reason nothing the
    user typed is lost.
    """
    form = {"header": header}
    if fields is not None:
        form["fields"] = fields
    if errors:
        form["errors"] = errors
    answer({"form": form, "state": state})


def prefilled(fields, values):
    """Put remembered answers back into a form.

    `prefill` overrides what the user has typed, which is exactly right for
    a form being re-shown after its fields were replaced: `input` no longer
    carries those values, so without this they would come back empty.
    """
    for field in fields:
        remembered = values.get(field["name"])
        if remembered:
            field["prefill"] = remembered
    return fields


# ---------------------------------------------------------------------------
# Round 1 — the everyday kinds
# ---------------------------------------------------------------------------


def about_fields():
    return [
        {"name": "title", "label": "Title", "type": "text"},
        {
            "name": "kind",
            "label": "Kind",
            "type": "select",
            "options": KINDS,
            "default": "feature",
        },
        {
            "name": "labels",
            "label": "Labels",
            "type": "multiselect",
            "options": LABELS,
            "required": False,
        },
        {"name": "urgent", "label": "Urgent", "type": "toggle", "default": "false"},
        {
            "name": "approver",
            "label": "Approved by",
            "type": "text",
            # Derivable from the form itself, so no round: the widget
            # re-evaluates this on every keystroke. A hidden field is left
            # out of `input` entirely — that absence is how this script
            # learns the question did not apply.
            "visible_when": {"field": "urgent", "equals": "true"},
            # Declared optional and enforced below, because "needed when
            # urgent" is a rule, not a property of the field.
            "required": False,
        },
        {
            "name": "token",
            "label": "Access token",
            "type": "text",
            "masked": True,
            "required": False,
        },
    ]


def check_about(values):
    errors = []

    title = values.get("title", "").strip()
    if len(title) < 5:
        errors.append({"name": "title", "error": "too short to tell anyone anything"})
    elif title.endswith("."):
        errors.append({"name": "title", "error": "a title is a name, not a sentence"})

    labels = split(values.get("labels"))
    if "ready" in labels and "blocked" in labels:
        errors.append({"name": "labels", "error": "ready and blocked contradict each other"})

    urgent = values.get("urgent") == "true"
    if urgent and not values.get("approver", "").strip():
        errors.append({"name": "approver", "error": "an urgent item needs a name behind it"})

    # A message without a `name` is the general one: it belongs to the form,
    # not to any single field.
    if urgent and values.get("kind") == "chore":
        errors.append({"error": "a chore is never urgent — pick another kind or drop the flag"})

    return errors


# ---------------------------------------------------------------------------
# Round 2 — the measured kinds
# ---------------------------------------------------------------------------


def detail_fields():
    return [
        {"name": "due", "label": "Due", "type": "date", "default": "today"},
        {"name": "starts", "label": "Starts at", "type": "datetime", "required": False},
        {
            "name": "size",
            "label": "Size",
            "type": "number",
            "min": 1,
            "max": 13,
            "step": 1,
            "default": "3",
        },
        {
            "name": "budget",
            "label": "Budget (EUR)",
            "type": "decimal",
            "min": 0,
            "max": 10000,
            "precision": 2,
            "required": False,
        },
        {
            # Accepts `1:30`, `90m` and `1h30` alike and submits whole
            # minutes, so no script ever parses a spelling of a span.
            "name": "spent",
            "label": "Spent so far",
            "type": "duration",
            "required": False,
        },
        {
            "name": "notes",
            "label": "Notes",
            "type": "multiline",
            "lines": 4,
            "required": False,
        },
        {
            "name": "files",
            "label": "Attachments",
            "type": "file",
            "multi": True,
            "required": False,
        },
    ]


def check_details(values, about):
    errors = []

    size = whole(values.get("size"))
    if size is not None and size not in FIBONACCI:
        errors.append(
            {
                "name": "size",
                "error": "estimates are " + ", ".join(str(n) for n in FIBONACCI),
            }
        )

    due = as_date(values.get("due", ""))
    if due is None:
        errors.append({"name": "due", "error": "not a date I can read (try today or 2026-10-01)"})
    elif due < dt.date.today():
        errors.append({"name": "due", "error": "the past is not a deadline"})

    starts = as_date(first_word(values.get("starts", "")))
    if starts is not None and due is not None and starts > due:
        errors.append({"name": "starts", "error": "starts after it is due"})

    budget = fractional(values.get("budget"))
    if budget is not None and about.get("kind") == "chore" and budget > CHORE_BUDGET:
        errors.append(
            {
                "name": "budget",
                "error": f"a chore over {CHORE_BUDGET:.0f} EUR wants a ticket, not a form",
            }
        )

    minutes = whole(values.get("spent"))
    if minutes is not None and minutes > MAX_MINUTES:
        errors.append({"name": "spent", "error": "more than a working day in one entry"})

    # Needs an answer from the round before — which is why it is in `state`.
    if about.get("kind") == "bug" and not values.get("notes", "").strip():
        errors.append({"name": "notes", "error": "a bug report without notes helps nobody"})

    files = split(values.get("files"))
    if len(files) > MAX_FILES:
        errors.append({"name": "files", "error": f"{MAX_FILES} attachments are plenty"})

    return errors


# ---------------------------------------------------------------------------
# Round 3 — show it back
# ---------------------------------------------------------------------------


def summary(collected):
    """Every answer as one block of text, for the confirmation round.

    A form has no label-only field, so the way to put text in front of
    someone is a `multiline` field carrying it as its `prefill`.
    """
    lines = []
    for name, label, shown in shown_values(collected):
        lines.append(f"{label + ':':<16}{shown}")
    return "\n".join(lines) or "nothing at all"


def shown_values(collected):
    """(name, label, printable value) for everything that was answered."""
    labels = {f["name"]: f.get("label", f["name"]) for f in about_fields() + detail_fields()}
    for name, raw in collected.items():
        if not raw:
            continue
        if name == "token":
            shown = "•" * len(raw)  # collected, never shown back
        elif name == "spent":
            minutes = whole(raw) or 0
            shown = f"{minutes // 60}h{minutes % 60:02d} ({minutes} min)"
        elif "\n" in raw:
            shown = ", ".join(split(raw))
        else:
            shown = raw
        yield name, labels.get(name, name), shown


def confirm_fields(collected):
    return [
        {
            "name": "summary",
            "label": "What you entered",
            "type": "multiline",
            "lines": 12,
            "required": False,
            "prefill": summary(collected),
        },
        {
            "name": "confirm",
            "label": "These are the values",
            "type": "toggle",
            "default": "true",
        },
    ]


# ---------------------------------------------------------------------------
# Small helpers — the shapes the protocol submits
# ---------------------------------------------------------------------------


def split(raw):
    """A multi-valued field arrives newline-joined."""
    return [line for line in (raw or "").split("\n") if line.strip()]


def first_word(raw):
    return raw.split()[0] if raw.split() else ""


def whole(raw):
    try:
        return int(raw)
    except (TypeError, ValueError):
        return None


def fractional(raw):
    try:
        # A decimal always arrives with a dot, whatever separator was typed.
        return float(raw)
    except (TypeError, ValueError):
        return None


def as_date(raw):
    """Resolve what a date field submits — the phrase, exactly as written."""
    phrase = (raw or "").strip().lower()
    if phrase in ("", "today"):
        return dt.date.today() if phrase == "today" else None
    if phrase == "tomorrow":
        return dt.date.today() + dt.timedelta(days=1)
    try:
        return dt.date.fromisoformat(phrase)
    except ValueError:
        return None


# ---------------------------------------------------------------------------
# The conversation
# ---------------------------------------------------------------------------


def main():
    with open(sys.argv[1]) as f:
        request = json.load(f)

    values = request.get("input", {})
    state = request.get("state") or {}
    node = request.get("node") or {}
    step = state.get("step")

    if step is None:
        ask({"step": "about"}, "About the work", fields=about_fields())

    if step == "about":
        errors = check_about(values)
        if errors:
            # No `fields`: the form stays, only the complaints change.
            ask(state, "About the work", errors=errors)
        remembered = {**state.get("about", {}), **values}
        ask(
            {"step": "details", "about": remembered},
            "Numbers and attachments",
            fields=prefilled(detail_fields(), state.get("details", {})),
        )

    if step == "details":
        about = state.get("about", {})
        errors = check_details(values, about)
        if errors:
            ask(state, "Numbers and attachments", errors=errors)
        collected = {**about, **values}
        ask(
            {"step": "confirm", "about": about, "details": values},
            "Does this look right?",
            fields=confirm_fields(collected),
        )

    if step == "confirm":
        about = state.get("about", {})
        details = state.get("details", {})
        if values.get("confirm") != "true":
            # Back to the top with everything prefilled — the fields were
            # replaced twice over, so `input` carries none of it any more.
            ask(
                {"step": "about", "about": about, "details": details},
                "About the work",
                fields=prefilled(about_fields(), about),
                errors=[{"error": "nothing was submitted — change what you like"}],
            )
        answer(
            {
                "result": {
                    # This script changes nothing, so it asks the front-end
                    # for nothing. The key still has to be here: an answer
                    # without `commands` is reported as malformed.
                    "commands": [],
                    "node": node.get("id"),
                    "values": {**about, **details},
                }
            }
        )

    answer({"error": f"lost track of the conversation at step {step!r}"})


if __name__ == "__main__":
    main()
