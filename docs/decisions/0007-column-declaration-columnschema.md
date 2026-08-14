# 0007 — One column declaration (`ColumnSchema`) instead of `SortableColumn`

- **Status:** accepted, implemented
- **Date:** 2026-08-11
- **Affects:** `not-yet-done-content` (`ColumnSchema` extended,
  `SortableColumn` dropped, `Child::columns`, `apply_sort`, the new shared
  cell lookup `cell()`, the conformance check in the generic list path),
  `not-yet-done-extended-query` (`ColumnTypes`, `SummaryRow`),
  `not-yet-done-jira-adapter`, `not-yet-done-local-adapter`,
  `not-yet-done-taiga-adapter`, `not-yet-done-kimai-adapter`,
  `not-yet-done-calendar-adapter`, `not-yet-done-custom-columns`,
  `not-yet-done-tui`, `not-yet-done-cli`
- **Builds on:** [0006 — Anonymization as a content-layer decorator](0006-anonymization-content-layer.md)
  (the same decorator chain has to pass the declaration through).

## Context

A row describes itself twice in the framework:

- as **data** — `NodeSummary { id, label, node_type, metadata.fields }`,
  where `metadata.fields` is a list of `MetadataField { key, value, … }`;
- as a **declaration** — `Child::sortable_columns: Vec<SortableColumn>`
  (which columns are sortable, with a `SortKind`) and
  `ContentAdapter::describe_columns` → `Vec<ColumnSchema>` (the type and
  allowed values for columns that are not native content; today only the
  locally stored custom columns).

Between the two there is a **tacit** contract: a column key names a metadata
field. Nothing enforces it, and it gets violated. The Jira adapter reports
`summary` as a sortable column but puts no `summary` field into the **list
row** — the title only lives in `NodeSummary::label` (`issue_summary()`).
The task adapter does the same with `description`.

To make sorting work anyway, `apply_sort` has always guessed:

```rust
s.metadata.fields.iter().find(|f| f.key == key)
    .map(|f| f.value.clone())
    .unwrap_or_else(|| s.label.clone())   // ← fall back to the label
```

The fallback is meant as a lookup path **for that one column**, but it
applies to **every** column that has no field in a row. As long as it was
only used for sorting the damage stayed invisible: sorting has no notion of
"absent", and when every row falls back the same way the order barely
changes noticeably.

With `local_filter` (extended queries) the same lookup got a second
consumer — `SummaryRow::cell` copied the line deliberately so that "filter
and sort see the same value". But filtering does treat "absent" as a
statement, and custom columns are missing from rows entirely (the decorator
only attaches **stored** cells). The result: the label is never empty, so
`is_not_null` is true for every row and `is_null` for none. A query that
should have returned 14 tickets with a local cell returned 306.

A second defect of the same kind was dormant next to it:
`issue_sortable_columns()` sets `kind: SortKind::Text` for every column,
reasoning "Jira sorts server-side via JQL ORDER BY, so the kind is unused
here". That is true for the server — but `ColumnTypes` feeds off the same
list, and `local_filter` takes the `kind` seriously as a type. So
`[updated, '>', …]` compares strings instead of timestamps. A declaration
that was marked "does not matter" for one consumer had become authoritative
for another.

The shared cause is not the fallback but the fact that there are **two
declarations and three sources of type** (`SortableColumn::kind`,
`ColumnSchema::value_type`, `kind:` in the view YAML) that nobody keeps
consistent.

## Options

1. **Only remove the fallback in `SummaryRow::cell`.** Fixes the filter bug
   and leaves the sort fallback, the duplicate declaration and the type lie
   in place. The next consumer inherits the same problem.
2. **Make the label reference declarable** — a `backed_by: Label` on
   `SortableColumn`, analogous to `source: label` in the view YAML. Removes
   the guessing, but cements the denormalization and keeps two declarations
   side by side.
3. **One declaration, checked presence** — `ColumnSchema` becomes the only
   column declaration and itself carries whether the column is present in
   the rows and whether it is sortable; `apply_sort` stops guessing; the
   generic list path checks the invariant.

## Decision

Option 3.

### One declaration

`SortableColumn` is dropped. `ColumnSchema` describes a column completely:

| Field        | Meaning                                                                                        |
| ------------ | ---------------------------------------------------------------------------------------------- |
| `key`        | the column key; also the metadata field key the value is stored under in rows                  |
| `label`      | optional display name (`None` = the frontend picks its own)                                    |
| `value_type` | `text` / `number` / `duration` / `datetime` — **the** source of type                           |
| `options`    | a closed set of values, if there is one                                                        |
| `in_rows`    | the value is present as a metadata field in every listed row ⇒ sortable and filterable locally |
| `sortable`   | the adapter promises to honour a sort by this column                                           |

`in_rows` and `sortable` are deliberately **independent**:

- Server-side sorting needs no local cell. Jira orders the ticket list via
  JQL `ORDER BY`; a column may be sortable without ever appearing in a row.
- Conversely, every column present in the rows is filterable locally — even
  if it is not sortable.

Which mechanism an adapter uses to honour a promised sort (JQL `ORDER BY` or
a local `apply_sort`) remains its own business and is no longer information
the framework carries.

### Two channels, one type

The declaration still arrives over two paths, because the custom column
columns have to be read from a store:

- `Child::columns` — synchronous, static per child type, from the adapter
  itself;
- `ContentAdapter::describe_columns` — asynchronous, dynamic, for columns a
  decorator adds.

Both deliver `ColumnSchema`. The union happens **once** in
`children::columns_for`, no longer per frontend.

### No more guessing

`apply_sort` only resolves columns with `in_rows` and reads exclusively the
metadata field; if it is missing, the cell is empty. The label fallback is
gone with nothing replacing it, in `apply_sort` **and** in
`SummaryRow::cell`. Both use the same shared lookup `content::cell()`, so
"filter and sort see the same value" is guaranteed by shared code instead of
by a comment.

So that the label-backed columns stay sortable, `jira::issue_summary()` and
`local::task::summary()` now ship their field along — `taiga`, `calendar`
and `local::projects` have long done so. `label` is kept as a display copy
(breadcrumbs, detail titles, link captions), but it is no longer the only
place the value is held.

### The checked invariant

> Every column with `in_rows` has to appear as a metadata field in every
> listed row.

`children::list` checks that with a `debug_assert` — in tests and debug
builds an adapter that declares a column and does not deliver it fails,
while in release builds it costs nothing. On top of that there is a public
check helper that adapter unit tests apply to their fixture rows.

## Consequences

- The filter bug (`is_null`/`is_not_null`/`has`/`like` on custom columns)
  and the sorting nonsense (titles among status values) disappear together.
- Custom columns become **sortable**: with the fallback gone a missing cell
  is simply empty, and the decorator can honestly report its columns as
  `sortable`. In the TUI they therefore show up in the `S` menu.
- The type lie on `updated` falls away: `value_type` is the single source of
  type, and it is set correctly per column instead of blanket `Text`.
- The `kind:` in the view YAML becomes redundant as soon as adapters
  describe their native columns; it stays for now as an override.
- `source: label` in the view configs becomes cosmetic for Jira (the column
  could now read the value from the field). It is kept as a feature —
  adapters may still have columns that render only from the label, as long
  as they do not declare them with `in_rows`.
- **Not** part of this decision: lifting `value_type` from `String` to an
  enum. The value is persisted in the custom columns SQLite database and on
  the CLI surface; the change is mechanical, but it is a change of its own
  with its own migration risk.
- **Not** part of this decision: the second local sort implementation in
  `taiga::client::query::apply_sort` (typed field comparisons over the
  adapter's own `ItemSummary`, including a sort priority per item type and
  "unset goes last"). Folding it into the generic function presupposes a
  sort kind with a prescribed value order and an empty-sorts-last rule for
  text — see the open points.

## Implementation

Implemented on 2026-08-11 across every adapter and frontend.

Three adapters violated the invariant and now ship the field: `jira`
(`summary`), `local::task` (`description`) and `taiga` (`project`). The
Taiga case is at the same time the textbook example for the separation:
`project` is sortable via the adapter's own `ItemSummary` comparator but
appears in no row — so it is `sortable` but not `in_rows`. Conversely,
Jira's `attachments` and `bookmarked` are in every row and still not
sortable. That Jira's ticket list (sorted server-side) and bookmark list
(sorted locally) report different sortability over the same rows is pinned
down by a test each.

`SortKind` is no longer declared but derived from `value_type` by
`ColumnSchema::sort_kind()` — which makes the type lie structurally
impossible rather than merely fixed.

The invariant check caught a liar on its very first run: a test fake in
`not-yet-done-extended-query` declared `status` and did not deliver it.

Confirmed end to end on the query that triggered the bug: 306 → 12 rows.

The rename `jql::apply_sort` → `apply_order_by`, noted as an open point, is
part of the implementation.

### Addendum: two places the first pass missed

`describe.rs` (the built-in `help`) read the columns straight from
`Child::columns` instead of from `columns_for` — the same mistake in
miniature, a second reader of the same declaration reaching past the union.
`help --full` thereby concealed exactly the columns it should have reported:
the decorator's. The render path is asynchronous now and goes through
`columns_for`.

And the precedence rule was too coarse: "described wins" overrode the label
too. A store only knows a key and a type, no label — so a custom column
named `status` erased the display name of the adapter column of the same
name and left the bare key. A missing label now keeps the declared one: a
statement a source makes about the fields it covers is not a statement about
the ones it does not.

Both times it only showed up in use, not at build time — which is the reason
`columns_for` and `content::cell()` have to be exactly one place each.

### Addendum: a declared column is not yet a sorted one

The decorator's columns showed up in the menu, but sorting by them did
nothing. The reason sits exactly at the seam this decision draws: Jira sorts
server-side, `jql::build_order_by` silently discards every key without a JQL
field — and the adapter simply _cannot_ know the columns of a decorator
sitting above it. For custom columns that is not the special case but the
normal one.

The fix goes where both sides are visible: `children::list` sorts afterwards
by whatever the adapter did not serve. Three conditions keep that honest —
it only re-sorts when the result is _complete_ (sorting a single page would
order a sample and present it as the whole), only when no already-served
order is lost in the process, and `applied_sort` reports the truth
afterwards. Which keys a local sort can compare at all is still decided in
exactly one place (`resolve_sort`, public as `honoured_sort_keys`).

### Addendum: type-on-first-write needed a way back

The type of a custom column is pinned down on the first write — and that is
practically always `text`, because that is the forms' default. A column
holding numbers therefore compared lexically (`100` before `20`), with no
way out short of editing the SQLite database by hand.

`retype_column` is the way back: the new type is accepted if _every_ stored
value validates under it, and rejected otherwise — with the row ID and value
of each offender, so that the error message is itself the list of things to
fix. Nothing is written on the error path.

Deliberately a separate action (`retype-column`) rather than a loosening of
`set-cell`: the latter's type selection defaults to `text`, and `text`
accepts any value. A `set-cell` that waved a type change through as soon as
all values fit would silently degrade a `number` column to text the moment
someone left the default in place. Retyping is a decision, and therefore
gets an action you have to choose.

### Addendum: `json` as a fifth value type

The retype exposed a gap: `validate_value` let unknown types through
(`_ => true`), so a caller could write an arbitrary string as a type and the
store would accept it. That is exactly how `json` columns came about that
were declared nowhere — visible only once `retype-column` rejected them,
because `json` is not in `VALUE_TYPES`.

`json` is a real type now: values have to parse as a JSON value. Unlike the
other four, though, it says nothing about comparison or presentation — a
`json` column still sorts as text and still renders verbatim. That is
intentional: it describes the _payload_ of a cell for which no scalar type
fits (a list of tags, a list of records), not its ordering. All the other
consumers of the type (`sort_kind`, `value_type_to_col_kind`,
`column_kind_from_value_type`) have a catch-all branch and treat it exactly
that way, unchanged.

The other half is still open: a `ColumnKind::Json` that shows a list as
`a, b` instead of `["a", "b"]`. That is a presentation decision (comma join?
a count? the first n?) and does not belong in the store.

## Open points

- Extend the generic sort with an enum ordering (`SortKind::Ranked`) and
  "empty sorts last when ascending" for text, then retire Taiga's own
  sorter.
- Remove `source: label` for the `summary` column from `views/jira.yaml` —
  now that the adapter delivers the field it has no effect.
