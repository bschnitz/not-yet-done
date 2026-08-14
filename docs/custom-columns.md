# Custom columns

Custom columns are **your own columns on somebody else's table**. Add a
`Dev ticket`, `Estimate` or `Review state` column to a Jira, Taiga or Postgres
view, fill it per row, sort and filter by it — without the remote system
knowing anything about it, and without touching the adapter that talks to it.

The values live in one local SQLite database on your machine. Nothing is ever
sent upstream.

---

## How-to

### Declare a column

A custom column is declared in the view YAML like any other column, with
`source: custom`:

```yaml
columns:
  - key: estimate
    label: Est.
    source: custom
    kind: number
    style: secondary
    sizing: max
```

The `key` is what ties the column to its stored values. `label`, `style`,
`sizing`, `kind` and everything else behave exactly as for an adapter column —
see the [generic view spec](generic-view-spec.md).

Two things are worth knowing:

- **A real adapter field of the same key always wins.** A custom column can
  only _add_ a column, never shadow live data.
- **`source: custom` is what makes the column editable.** Values render by key
  regardless of `source` (that is just how column resolution works), so a
  column without it will still _display_ stored cells — but it will not appear
  in the edit form, and nothing will offer to fill it.

### Bind the editor

The write path is a synthetic action every row carries. Bind it to a key in the
same view:

```yaml
actions:
  - name: edit custom cells
    key: M
    type: custom
    id: edit-cells
```

Pressing `M` on a row opens one form with a field per custom column of that
level, prefilled with the current values. Submitting writes only what changed;
clearing a field deletes the cell.

You do not have to define a column before filling it — the first write creates
it, taking its type from the view's `kind:` (**type-on-first-write**).

### Fill a cell from the CLI

The same actions are reachable through `nyd`, which is the practical route for
scripting or a bulk import:

```sh
nyd adapter jira:issue PROJ-1 set-cell --field column_key=estimate \
                                       --field value=5 \
                                       --field value_type=number
nyd adapter jira:issue PROJ-1 clear-cell --field column_key=estimate
```

The address is `<instance>[:child…] [ID] <action>` — the child path picks the
level, which matters because a column is defined per node type. Run
`nyd adapter <instance> actions` to see the actions available at a level.

### Change a column's type

Retyping is its own action, not a side effect of a cell write:

```sh
nyd adapter jira:issue PROJ-1 retype-column --field column_key=estimate \
                                            --field value_type=duration
```

It is **scope-wide** despite being invoked from a row: either every stored cell
survives the new type, or nothing is written and the error names the rows in
the way.

### Restrict a column to a set of values

Turn a column into an enumeration — a closed set of values it accepts:

```sh
nyd adapter jira:issue PROJ-1 set-column-options \
    --field column_key=state --field options=todo,in_review,merged
```

From then on a write outside the set is rejected, and the edit form renders the
column as a select instead of a text field. Passing an empty `options` removes
the restriction and makes the column free again.

Like a retype this is scope-wide and refuses to strand data: the set is
accepted only if _every_ stored cell is already in it, otherwise nothing is
written and the error names the offending rows. Each option must also be a
valid value for the column's type, so a `number` column cannot be given a set
of words.

Values are trimmed, blanks dropped and duplicates collapsed, so the stored list
is exactly what the select offers.

---

## Reference

### Actions

Every node carries these, whatever adapter produced it. They appear in the TUI
action menu and under `nyd adapter <instance> actions` without any per-adapter
wiring.

| Action id            | Fields                                    | Scope  |
| -------------------- | ----------------------------------------- | ------ |
| `edit-cells`         | one per `source: custom` column           | Row    |
| `set-cell`           | `column_key`, `value`, `value_type`       | Row    |
| `clear-cell`         | `column_key`                              | Row    |
| `retype-column`      | `column_key`, `value_type`                | Column |
| `set-column-options` | `column_key`, `options` (comma-separated) | Column |

"Column" scope means: invoked from a row, but it changes the column for every
row in the scope.

### Value types

`text` · `number` · `duration` · `datetime` · `json`

`number` parses as a decimal, `duration` as integer seconds, `datetime` as
RFC 3339. An empty value is "unset" and always valid. `text` accepts anything.

The stored type is authoritative: it overrides the view's `kind:` for
rendering, while the view keeps deciding width, order and visibility.

### Where the data lives

One SQLite database for all adapters:

```
$XDG_DATA_HOME/not_yet_done/custom_columns.sqlite
```

Two tables: `custom_column` holds the schema per
`(scope, node_type, column_key)` — type, label, options — and `custom_cell`
holds the values per `(scope, row_id, column_key)`.

`scope` is `"<adapter_type>/<instance_id>"`, so two Jira instances keep
separate values for the same column key, and a column defined on
`jira:issue` is independent of one on `jira:comment`.

---

## Explanation

### Why a decorator, and why no adapter knows about this

Custom columns are a **local annotation layered on top of any adapter's rows**,
not adapter content. So no adapter implements anything. The host wraps every
factory, and from then on every row an adapter hands out — list rows, eager
subtrees, the post-edit row projection, detail metadata — passes through a
layer that looks the row up by its `id` and appends stored cells as ordinary
metadata fields. A `source: custom` column then renders through the normal
metadata path.

The payoff is that the feature works on adapters that will be written years
from now, and that a new adapter cannot forget to support it.

The write side is symmetric: the actions live on the node, so both the CLI and
the TUI menu reach them with no front-end code.

### Why it costs nothing when unused

The wrapper is applied unconditionally, but does nothing observable until cells
exist: each list costs one batched `row_id IN (…)` lookup, and a scope with no
cells injects nothing. A store read error degrades to "no cells" rather than
failing the row — a broken local annotation must never take out the view of the
real data.

### Why type-on-first-write

The alternative — a separate "define a column" step before you can fill it —
buys nothing. The view YAML already states the type via `kind:`, so the first
write can carry it. The store then owns the type, and later writes are checked
against it rather than silently redefining it.

That is also why retyping is its own action. `set-cell`'s type field defaults
to `text` and every value validates as text, so folding a retype into it would
silently downgrade a `number` column whenever someone set a cell without
touching the default.

### Why options are separate from the type

A closed value set could have been modelled as an `enum` value type. It is kept
orthogonal instead, because the two answer different questions — "how does this
value parse, sort and render?" versus "which values are allowed?" — and a
closed set is just as meaningful over `number` or `datetime` as over `text`.

A column whose stored options cannot be parsed is treated as free rather than
as restricted: a column you cannot read the rules of must not start rejecting
writes.

### Ordering with anonymization

The decorator sits _inside_ the anonymizer. In a screenshot run the injected
custom values are therefore scrubbed like any other free text (numbers and
dates survive) — your local notes cannot leak past the mask.

---

## See also

- [Generic view spec](generic-view-spec.md) — column and action syntax
- [`docs/examples/views/jira.yaml`](examples/views/jira.yaml) — a worked example
- [Content adapter spec](content-adapter-spec.md) — the adapter contract this
  layer decorates
- [ADR 0007](decisions/0007-spalten-deklaration-columnschema.md) — the single
  `ColumnSchema` declaration that carries a column's type and options from the
  backend to the front-end
