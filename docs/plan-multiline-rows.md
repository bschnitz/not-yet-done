# Plan: multi-line rows in the table engine (chat layout)

> **Status: implemented.** Layers 1–5 are done, 487 TUI + 80 ratatui + 32
> table tests pass (6 new), release built and installed. Unpushed locally.
> The smoke test (the chat layout checklist in `smoke-tests.md`) is still
> open.

## Goal

One logical table row can be rendered as a **stack of several physical
lines**. The first use case is the Stoat message list as a chat layout —

- line 1: `author` + `time` (colour-highlighted, fg)
- line 2: `content` (the message)
- line 3: empty (spacer)

Today's table is the special case "exactly one physical line with all
columns". The guiding principle: **`height == 1` reduces the new engine
bit for bit to the old behaviour** → existing tabs are unchanged, and the
existing widget tests are the regression guard.

## Data model (composite)

- `RowTemplate { lines: Vec<LineTemplate> }` — the blueprint of a row.
- `LineTemplate { columns: Vec<ColumnId>, highlight_on_select: bool }` — one
  physical line. Empty `columns` = a spacer. `highlight_on_select` defaults
  to `true`, and to `false` for spacers.
- `ComputedRow.cells: Vec<String>` → `lines: Vec<ComputedLine>`.
- `ComputedLine { cells: Vec<String>, highlights: Vec<Vec<Range>>, highlight_on_select }`.

`highlight_on_select` is the **generalized** form of "do not include this
line in the selection" — it is not tied to "is empty"; any line can opt out
(an escape hatch in the config).

## Layer 1 — `not-yet-done-table` (strategy + reuse)

A new `compute_multiline_table(rows, config, template, header)`. Per **line
index**, the existing `ColSizer::col_widths()` + `fit_aligned…` is run over
the columns of _that_ line against the full pane width (so vertical
alignment per line index is preserved). The single-line path `compute_table`
stays **untouched** — multi-line is composed from it.

## Layer 2 — `not-yet-done-ratatui`/table (variable height, encapsulated)

- `TableWidgetRow.cells` → `lines: Vec<TableWidgetLine>`.
  `TableWidgetLine { cells: Vec<TableWidgetCell>, highlight_on_select: bool }`.
- `TableWidgetRow::new(cells)` still means one line
  (`highlight_on_select=true`); `::multiline(lines)` is new.
- The helpers `primary_line() -> &[TableWidgetCell]` (= `lines[0]`) and
  `height()`. Single-line-oriented code (jump mode, column count,
  `compute_col_widths`) uses `primary_line()` → unchanged behaviour at
  `height==1`.
- `render.rs`: per row `let h = row.height(); render lines; y += h`. The
  selection colours all `h` lines with the `RowSelected` base — except lines
  with `highlight_on_select == false`, which get the `Row` base.
- `component.rs`: `scroll_offset`/`selected_row` remain **row** indices;
  visible rows and `adjust_scroll` go through accumulated line heights. At
  `height==1` that is identical to today.

**Scope cut:** jump mode (`f`) and the column cursor / horizontal scrolling
stay single-line only (they operate on `primary_line()`). The chat view uses
neither (`selected_column = None`). Multi-line plus those features is "not
supported", rather than widening the position arithmetic.

## Layer 3 — `view_config.rs`

`ViewDef.row_layout: Option<Vec<LineLayout>>`. Absent → the classic table.
`LineLayout` deserializes from

- the short list form `[author, time]` → `columns`,
  `highlight_on_select=true`
- an empty list `[]` → a spacer, `highlight_on_select=false`
- a map `{ columns: [...], highlight_on_select: false }` → the escape hatch

Validator: every column referenced in `row_layout` has to exist in
`columns`.

## Layer 4 — `content_view.rs`

When `row_layout` is set:

- Build a `RowTemplate` from `row_layout` (plus the `ColumnId`s).
- `compute_multiline_table(...)` instead of `compute_table`.
- Per-column fg: a StyleMap with one entry per column (the fg resolved from
  `ColumnDef.style` or `text_med`), and every cell gets the `style_id` of
  its column → `resolve_cell_fg` always uses the style entry (independent of
  position).
- Suppress the header: pass `headers = vec![]` to `set_data`.

## Layer 5 — config + docs

- `~/.config/not_yet_done/views/stoat.yaml` (deployed) and
  `docs/examples/views/stoat.yaml`: move the `messages` branches onto
  `row_layout`, with `style:` on `author`/`time`.
- `docs/reference/generic-view-spec.md`: document `row_layout` (what and
  why).
- `docs/smoke-tests.md`: the chat layout checklist.

## Order / verification

1. Layer 1 plus tests → `cargo test -p not-yet-done-table`.
2. Layer 2, existing tests passing plus new tests →
   `cargo test -p not-yet-done-ratatui`.
3. Layers 3 and 4 → `cargo build --release`, `cargo test`.
4. Config and docs, `prettier`, `cargo install`.
