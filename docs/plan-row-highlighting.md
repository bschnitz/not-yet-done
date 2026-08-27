# Plan: configurable row / column / cell highlighting

> **Status: phases 1-6 implemented, phase 7 (docs + smoke tests) planned.** The style grammar of
> [Part 1](#part-1--the-style-grammar) and the rule surface of
> [Part 2](#part-2--the-rules) live in
> `not-yet-done-tui/src/config/highlight.rs`: `highlights:` parses on every
> level, and its rules are resolved and validated at config load. In table
> mode (`not-yet-done-tui/src/views/content_highlights.rs`) a rule paints its
> cells when it names `columns:` and the whole row when it does not, and a
> `load`-hook script can answer with `highlights` that layer over those
> rules. Card, record-detail and tree paint from the same decision, each
> mapping it onto its own geometry. This
> document fixes the config surface and the layering before the rest is
> written, because one part of it (row-level styles) reaches into
> `not-yet-done-ratatui`.

## Goal

A view can declare that certain **rows**, **columns** and **cell values**
are painted differently — foreground and background, for the normal and
for the selected state, each of the four independently optional. On top of
that, a **script** bound to the `load` hook can compute highlights per row
and hand them back with the rows, so a colour can be a function of the data
rather than a fixed rule.

The motivating case for the script half: a script compares an "actual
(days)" column against an "estimated (days)" column for every row and
paints the background in an increasingly alarming colour as the ratio
climbs. A ramp like that cannot be written as a handful of declarative
rules — the colour is the output of a calculation, so it belongs where the
calculation is.

Two producers, **one style grammar**. Everything below is shared between
the YAML rules and the script channel; only the way a style is _selected_
differs.

## Part 1 — the style grammar

### Named styles: `styles:`

A map of name → style. Allowed in `tui-theme.yaml` (shared by all views) and
in a view file (wins on a name collision, so a view can specialise a shared
name without renaming it everywhere).

```yaml
styles:
  over-budget:
    fg: "#ffffff"
    bg: "#7a1c1c"
    modifiers: [bold]
    selected: { bg: "#a52222" }
  muted: { fg: "#6c6c6c" }
```

### A style value

Wherever a style is expected (`style:`, `selected:`, a script's answer), three
forms are accepted:

| Form                               | Meaning                     |
| ---------------------------------- | --------------------------- |
| `over-budget`                      | a name from `styles:`       |
| `{ fg: "#ff5555", bg: "#2a1111" }` | inline definition           |
| `[bold, italic]`                   | modifiers only, no recolour |

This mirrors the three forms `TabUnreadStyle` already accepts
(`config/view_config.rs:819`), so the surface is not a new invention — only
its reach is wider.

Fields of the inline form, **all optional**:

| Field       | Value                                                               |
| ----------- | ------------------------------------------------------------------- |
| `fg`        | `#rrggbb`, `auto`, or a theme role name                             |
| `bg`        | `#rrggbb` or a theme role name                                      |
| `modifiers` | list of `bold`/`dim`/`italic`/`underlined`/`reversed`/`crossed_out` |
| `selected`  | a nested style used while the row is the cursor row                 |
| `modes`     | where the style applies (see [Modes](#modes)); default: everywhere  |

**Hex is the primary form here, deliberately.** Elsewhere in the config
colours go through theme roles, and for structural chrome that is right — a
theme swap should recolour the app. Highlights are the opposite case: they
encode a _domain_ meaning ("over budget", "stale") that the theme knows
nothing about, and a ramp needs values the theme has no names for. Theme
role names stay accepted so `fg: accent` keeps working, but they are the
convenience, not the rule.

**Name resolution order:** `styles:` of the view → `styles:` of
`tui-theme.yaml` → theme role name (`accent`, `error`, … via
`resolve_theme_color`, `views/content_view.rs:13421`) → unknown, reported by
the validator and ignored.

### `fg: auto`

Picks a foreground with usable contrast against the **effective background at
that point**, from two configurable candidates:

```yaml
# tui-theme.yaml
auto_fg_light: "#f5f5f5"
auto_fg_dark: "#101010"
```

Both candidates are scored by WCAG relative-luminance contrast ratio against
the resolved background and the better one wins — a fixed lightness threshold
would be wrong as soon as someone configures two candidates that are not
black and white.

Two rules that are easy to get wrong:

- The **selected state is computed separately.** If `selected:` carries a
  different `bg`, `fg: auto` is re-evaluated against it. Otherwise contrast
  collapses on exactly the row the user is looking at.
- If **no `bg` is set anywhere** in the stack at that point, `fg: auto`
  resolves to nothing and the layer below keeps its foreground. Resolving it
  against the theme background instead would silently throw the column colour
  away.

### Modes

`modes:` restricts a highlight to certain render surfaces:

| Mode      | Surface                       |
| --------- | ----------------------------- |
| `table`   | the normal table              |
| `card`    | card mode (`card:`)           |
| `details` | the record-detail split (`o`) |
| `tree`    | the label line in tree mode   |

Omitted = every surface where the style can technically apply.

It may be written in **two places**, and they mean different things:

| Written on | Means                                     |
| ---------- | ----------------------------------------- |
| a style    | this _colour_ is meant for those surfaces |
| a rule     | this _rule_ fires only on those surfaces  |

**The rule wins**, replacing the style's list outright rather than
intersecting with it. The style's list is the default a shared style brings
along; a rule that states its own reach has the last word.

Both places are needed. Without the rule level, a named style shared by two
rules of different reach has to be duplicated under two names that differ in
nothing but their `modes:` -- see the pair of rules in
[Part 2](#part-2--declarative-rules-highlights), where the same `over-budget`
paints a whole row in the table but only one cell on a card. Without the
style level, a script has no way to say it at all: a `load` hook answers with
style objects, never with rules.

Intersecting instead of replacing was considered and dropped: a style saying
`[table]` and a rule saying `[card]` would then paint nothing at all, with
nothing to report -- a silent empty result is a worse failure than a rule
overriding its style.

`modes:` inside a nested `selected:` style is meaningless (the selected state
is not a surface). The validator warns and ignores it.

In `card` and `details` there are no columns in the layout sense, but there
are fields — a rule naming `columns: [estimate]` paints the **value** of the
`estimate` field there. The field _label_ is not touched (that is what
`card.label_style` is for); a `label_style:` on the rule can be added later
if it turns out to be wanted.

## Part 2 — declarative rules: `highlights:`

A list per level, next to `columns:`. Every entry has an optional selector
for _what_ is painted and an optional condition for _when_.

```yaml
- name: tickets
  node_type: "jira:issue"
  columns: [...]

  highlights:
    # column — no condition, so the whole column
    - columns: [estimate]
      style: { bg: "#1c2430" }
      selected: { bg: "#2c3a50" }

    # row — no column selector, so the whole row
    - when:
        and:
          - [status, "!=", Done]
          - [blocked_by, is_not_null]
      style: over-budget

    # cell — both
    - columns: [priority]
      when: { field: priority, matches: "(?i)^(highest|blocker)$" }
      style: hot
      modes: [table, card]

    # one shared style, two reaches — see Modes
    - when: { field: over_budget, matches: "^yes$" }
      style: over-budget
      modes: [table] # a red band across the row reads well in a table
    - columns: [actual_days]
      when: { field: over_budget, matches: "^yes$" }
      style: over-budget
      modes: [card, details] # a fully red card would not
```

| `columns:` | `when:` | Result                                               |
| ---------- | ------- | ---------------------------------------------------- |
| set        | unset   | those columns, in every row                          |
| unset      | set     | the whole row, where the condition holds             |
| set        | set     | those columns, in the rows where the condition holds |

`columns:` selects _what is painted_, `when:` decides _when_ — the two are
orthogonal, which is why there is no `scope:` field. In particular
`columns: [key]` with `when: { field: status, … }` is well-defined: paint
`key`, decide on `status`.

Columns are named by their **`key`**, not their `label` — stable across
renames, and the key is what the script channel and the filter expressions
use too. A rule naming a column that the level does not have is a validator
warning (the mechanism from the unknown-view-field warning); a rule naming a
column that is currently _hidden_ simply paints nothing.

Every problem a highlight rule can have is a **warning, never a broken file**:
an unresolvable style paints nothing and a regex that does not compile matches
nothing, so a typo costs that one rule while the rest of the view keeps
working. The messages carry the level they came from
(`tickets > comments.highlights[1]`), because a rule buried three drill levels
deep is otherwise a needle.

### `when:` — two forms

**Short form** — one field, one regex:

```yaml
when: { field: status, matches: "^(Blocked|On Hold)$" }
```

**Full form** — a `FilterExpr` from `not-yet-done-filter`, evaluated with
`eval::matches()` against the row:

```yaml
when:
  or:
    - [due, "<", "today"]
    - [labels, has, hotfix]
```

The short form is sugar for `[<field>, matches, <regex>]`, so there is one
evaluator, not two.

**This is pure in-memory evaluation** over the rows the adapter delivered —
custom columns and `load`-hook-patched cells included, since both are
ordinary metadata fields by the time highlighting runs. No database is
involved.

The one place the shared DSL is touched: `Operator` in
`not-yet-done-filter/src/expr.rs:122` gains a `Matches` variant, which forces
one arm in the SeaORM translator
(`not-yet-done-task-core/src/filter/builder.rs:98`). That arm returns an
error — "operator not supported in SQL filters". Consequence: `matches`
written into a _saved query_ against the task DB fails loudly instead of
matching nothing. Acceptable, and in line with the tree operators, which
already evaluate to `false` on the in-memory side.

### Regex semantics

- Matched against the **raw field value**, not the rendered cell text. Against
  the rendering, `kind: duration` would be matched as `1h 30m` instead of
  `5400`, and a narrow column would be matched including its `…`.
- Unanchored, case-**sensitive**; `(?i)` is available. This deviates from the
  filter DSL's text comparisons, which are case-insensitive by design (they
  are a search feature) — a highlight is a classification, and silent
  case-folding there is a surprise. To be called out in the docs.
- Compiled once when the view config is loaded, not per row. An invalid regex
  is a validator error that disables that rule, never a panic in a render
  path.
- Write regexes as **single-quoted** YAML scalars. A double-quoted scalar
  processes backslash escapes, so `"\d+"` is a YAML error before the regex
  engine ever sees it, while `'\d+'` arrives intact. Worth one sentence in
  the user docs — it is the first thing anyone will trip over.

### Layering between rules

All matching rules apply, **in file order, later wins per field**. A row rule
can lay down a background and a cell rule can put a foreground on top of it.

If a rule sets `style.bg` but no `selected.bg`, the selection keeps its
`RowSelected` background. Otherwise the cursor disappears on exactly the rows
that were made conspicuous.

The complement, settled while implementing: the cursor row **does** keep the
normal layer's foreground and modifiers. Only the background is dropped. A
highlight that vanishes under the cursor would be worse than none, and a rule
that wants its background there too can say so explicitly with the same value
in `selected.bg`.

## Part 3 — the script channel

The `load` hook's answer gains a `highlights` key next to `cells`
(`app/script_hook.rs:465` is where `cells` is applied today):

```json
{
  "cells": { "ABC-1": { "actual_days": "6.5" } },
  "highlights": {
    "ABC-1": {
      "actual_days": { "bg": "#7a1c1c", "fg": "auto" },
      "*": { "bg": "#2a1414" }
    },
    "*": { "actual_days": { "modifiers": ["bold"] } }
  }
}
```

Same addressing as `cells` (`row id → column key → …`), same error handling
(unknown row ids counted and reported, never fatal), and the value is a style
in the grammar of [Part 1](#part-1--the-style-grammar) — a name from `styles:`
or an inline object, `modes:` included.

`"*"` is a reserved key in **both** axes:

| Painted         | JSON                                  |
| --------------- | ------------------------------------- |
| one cell        | `{"ABC-1": {"actual_days": <style>}}` |
| one row         | `{"ABC-1": {"*": <style>}}`           |
| one column      | `{"*": {"actual_days": <style>}}`     |
| the whole table | `{"*": {"*": <style>}}`               |

Specificity, least to most specific: `*`/`*` → `*`/column → row/`*` →
row/column. A row whose adapter id is literally `*` is not addressable; to be
documented.

**Only the `load` hook.** A `reload` hook answering with `highlights` is
refused with a message, exactly as it refuses `commands` in a `load` hook
today — no silent drop.

**Script highlights win over the YAML rules**, still layering field by field:
a rule that only sets `fg` and a script that only sets `bg` combine. The
script looked at the actual row, so it holds the more specific information.

### Full precedence stack

Bottom to top, each layer setting only the fields it names:

`Theme` → `ColumnDef.style` → `highlights:` rules (file order) → script
highlights → `selected:` of whichever layers set one

### The motivating script

```python
#!/usr/bin/env python3
# scope: table
# mode: background
import json, sys

payload = json.load(open(sys.argv[1]))
out = {}

def ramp(ratio):
    """0.0 -> unobtrusive, 1.0 -> warning, >1.3 -> alarm."""
    t = max(0.0, min(1.5, ratio)) / 1.5
    r = int(0x1E + t * (0xC0 - 0x1E))
    g = int(0x1E + (1 - t) * 0x40)
    return f"#{r:02x}{g:02x}1e"

for row in payload["rows"]:
    f = row["fields"]
    try:
        actual, est = float(f.get("actual_days", 0)), float(f.get("estimated_days", 0))
    except ValueError:
        continue
    if est <= 0:
        continue
    out[row["id"]] = {"actual_days": {"bg": ramp(actual / est), "fg": "auto"}}

print(json.dumps({"highlights": out}))
```

Bound with `ctrl+h` → `load` in the script menu. `# mode: background` is
mandatory — `hook_mode_rejection` refuses interactive and capture scripts as
hooks.

## Where this lives in the code

### Not on `NodeSummary`

The obvious home for a per-row style would be the row itself — but
`NodeSummary` (`not-yet-done-content/src/lib.rs:739`) is the adapter contract
and is built with struct literals in **132 places** across the workspace. A
new field breaks all of them. Letting adapters emit highlights is worth
having eventually, but it needs a `Default`/builder in front of it and is a
separate cut.

Instead: a **side map on the pane**, `HashMap<row_id, RowHighlight>`, filled
in `run_load_hooks` alongside `apply_cell_patch`, read where the row is built
and `style_id`s are handed out. The highlights live exactly as long as the
rows; every load refills them.

Two traps:

- **Tree mode** loads children per parent, and each load fires its own hook.
  The map must be **merged per parent level**, not replaced wholesale —
  otherwise expanding one node wipes the colours of every other.
- **Staleness**: the hook runs on load. Editing a value in the TUI leaves the
  colour stale until the next reload. For the ratio case the value and its
  colour are computed in the same run, so they cannot disagree — but it
  belongs in the docs.

### The changes outside the TUI

Column and cell highlights mostly fit the existing machinery: extra `StyleMap`
slots (slots 0–6 are statically assigned today, `views/content_view.rs`) plus
`TableWidgetCell.style_id`. **Correction from the implementation:** that slot
only ever contributed a _foreground_, which is fine for the six chrome slots
and useless for a highlight, whose point is usually the background. A cell
override now carries fg, bg and modifiers, and a cell can name a _second_
slot (`selected_style_id`, set together via `with_style_pair`) that takes over
while its row is the cursor row. Without that pair the table would have to be
rebuilt on every cursor move to answer "what does this cell look like now".

**Row** highlights need one more thing. `TableWidgetRow` carries no style of
its own; the only row-level styles are the global `Row`/`RowSelected`
entries. It needs the same optional style pair and a place in the existing
precedence `CellSelected > RowSelected > ColumnSelected > Row`
(`widgets/table/render.rs`). Everything else stays inside `not-yet-done-tui`.

## Phases

Each phase ends in something demonstrable.

| Ph  | Content                                                                                                                                                                                                                 |
| --- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1 ✓ | Style grammar: `styles:` in theme + view file, the three style forms, `ColorSpec` (hex / role / `auto`), resolution incl. contrast pick. Unit tests only.                                                               |
| 2 ✓ | Matcher: `Matches` in `not-yet-done-filter` + eval, the SQL translator's refusal arm, the short form as sugar, regex compile + cache at config load. Plus `highlights:` on `ViewDef`/`ChildDef` and the load-time walk. |
| 3 ✓ | Column and cell highlights in table mode, via `StyleMap` slots. First visible result.                                                                                                                                   |
| 4 ✓ | Row highlights: the row style pair in `not-yet-done-ratatui` + precedence, then wired up.                                                                                                                               |
| 5 ✓ | Script channel: `highlights` in the `load`-hook answer, the pane-side map, the `*` axes, `reload` refusal. (Tree-merge is not reachable yet: the `load` hook only fires on the flat load.)                              |
| 6 ✓ | Modes: `card`, `details`, `tree` — one `Surface`, three geometries. In a tree a row wears its own level's rules.                                                                                                        |
| 7   | Docs (`generic-view-spec.md` §`highlights:` and the hook table's "may answer with", README) + smoke tests in `smoke-tests.md`.                                                                                          |

## Non-goals (for now)

- Adapters emitting highlights themselves (needs the `NodeSummary` cut).
- Highlighting from the `reload` hook.
- Styling field _labels_ in card / details mode (`label_style:` on a rule).
- Animation, blinking, or anything time-dependent.

## Open points

- Whether a rule should be able to opt _out_ of the layering ("this style
  replaces everything below") — cheap to add (`replace: true`), but only
  worth it if the layering turns out to fight the user in practice.
