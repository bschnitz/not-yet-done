# grid-render-sim

A simulation and testbed for the render function of the `Grid` component of the
`not-yet-done-ratatui` project. The simulation produces the scaffolding output
(gaps, borders, cell backgrounds) as a `Vec<String>`, without needing a real
ratatui buffer or a terminal.

---

## Goal

The grid render logic is complex: gaps interrupt borders, groups suppress gap
characters, intersections require matching corner characters, and spanned borders
produce half endings. All of that is far easier to develop and debug in a string
buffer than directly in a ratatui `Buffer`.

The goal of this simulation is to implement the entire render algorithm for gaps
and borders — including all the special cases — completely correctly, and to test
it pixel-exact against the specification, before the code is transferred into the
actual `MockComponent` implementation.

---

## Architecture

```
src/
  lib.rs       Public API, test module
  types.rs     All data types: BorderChars, BorderPos, GapPos,
               CellGroup, TextAnchor, GridConfig, GapSlot,
               BorderText, SpannedBorder
  layout.rs    Layout computation (GridLayout, compute_layout)
  render.rs    Render pipeline: render_gaps_and_borders,
               render_with_cells
```

### Render pipeline (6 steps)

```
1. Compute the layout       → coordinates for all cells and gaps
2. Buffer full of spaces    → a clean starting point
2a. Fill cell backgrounds   → ▓ ░ █ cycle per cell (render_with_cells only)
3. Horizontal lines         → ─, half endings ╶/╴, extended ─ continuous
4. Vertical lines           → │, half endings ╷/╵, extended │ continuous
5. Intersections and corners→ ┼ ┬ ┴ ├ ┤ ┌ ┐ └ ┘, computed from the context
6. Border texts             → overwrite everything beneath them
```

Important: the outer frame (`draw_outer_frame`) is drawn **first** in steps 3 and
4, so that inner borders can write their half endings over it.

---

## What has been achieved so far

### Implemented and tested

- **Outer frame** (`BorderPos::Grid`) with all the predefined styles:
  `BORDER_SIMPLE`, `BORDER_ROUNDED`, `BORDER_DOUBLE_EXTENDED`,
  `BORDER_THICK_EXTENDED`
- **Complete inner borders** (`AfterCol`, `AfterRow`, `BeforeCol`, `BeforeRow`)
  with half endings and extended variants
- **Intersections** for identical styles (`┼`), no join for differing styles
- **T-pieces** (`┬`, `┴`, `├`, `┤`) where inner borders meet an outer frame of
  the same style
- **Spanned borders** (`AfterColSpanned`, `AfterRowSpanned` etc.) with half
  endings at the span boundaries
- **Spanned crossings**: intersections between spanned horizontal and vertical
  borders, and between full and spanned borders
- **Gap-only positions** (spaces, no border char)
- **Border texts**: horizontal and vertical, `TextAnchor::Start`/`End`, offset,
  truncation with `…`
- **Cell backgrounds**: the `▓ ░ █` cycle by the formula `(2*row + col) % 3`, so
  that no two adjacent cells (horizontally or vertically) share the same
  background
- **Half-ending logic**: half endings are set when there is no outer frame, or
  when the outer frame has a different style than the inner border (no join
  possible → no T-pieces → the half endings remain)

### Predefined BorderChars constants

| Name                      | Horizontal | Vertical | Corners | Half endings |
| ------------------------- | ---------- | -------- | ------- | ------------ |
| `BORDER_SIMPLE`           | `─`        | `│`      | `┌┐└┘`  | `╷╵╶╴`       |
| `BORDER_SIMPLE_EXTENDED`  | `─`        | `│`      | `┌┐└┘`  | `│││─`       |
| `BORDER_DOUBLE_EXTENDED`  | `═`        | `║`      | `╔╗╚╝`  | (full)       |
| `BORDER_THICK_EXTENDED`   | `━`        | `┃`      | `┏┓┗┛`  | (full)       |
| `BORDER_ROUNDED`          | `─`        | `│`      | `╭╮╰╯`  | `╷╵╶╴`       |
| `BORDER_ROUNDED_EXTENDED` | `─`        | `│`      | `╭╮╰╯`  | (full)       |
| `BORDER_DASHED`           | `┄`        | `┆`      | `┌┐└┘`  | `╷╵╶╴`       |
| `BORDER_DASHED_EXTENDED`  | `┄`        | `┆`      | `┌┐└┘`  | (full)       |
| `BORDER_DOTTED`           | `┈`        | `┊`      | `┌┐└┘`  | `╷╵╶╴`       |
| `BORDER_DOTTED_EXTENDED`  | `┈`        | `┊`      | `┌┐└┘`  | (full)       |

---

## What is still missing / needs testing

### Grouped cells (CellGroup) — not implemented yet

This is the next big step. According to the specification:

- Gaps and borders **inside** a group are not rendered
- The group rect covers all member cells **including** the internal gap columns
  and rows
- Cell backgrounds are drawn across the whole group area (the background colour
  of the first cell, top left)
- During navigation the group is treated as a single position

Scenarios to test:

- `CellGroup::Row(r)` — a whole row as one cell
- `CellGroup::Col(c)` — a whole column as one cell
- `CellGroup::ColSpan { row, first_col, last_col }` — several columns in one row
- `CellGroup::RowSpan { col, first_row, last_row }` — several rows in one column
- `CellGroup::Span { ... }` — a rectangular area
- Group edges with and without a border/gap
- A continuous border interrupted by a group (it should be split into two
  segments with their own half endings)
- The no-nesting rule: a partial overlap of two groups → panic
- Complete enclosure: the larger group wins

### Further open points

- Test `BORDER_DASHED`/`BORDER_DOTTED` visually (no test for it yet)
- `GapPos::Grid` combined with borders at individual positions
- User-defined `BorderChars` (your own `pub static`)
- `set_border_text` on spanned positions (exists in the code, no test)
- `remove_border` / `remove_gap` / `ungroup_cells` (not implemented yet)
- Very large grids (performance; no functional bug expected)
- Grids with `Constraint::Percentage`, `Min`, `Max`, `Ratio` instead of only
  `Length` (the layout engine is used correctly, but never tested)

---

## Writing and running tests

### Running

```sh
# All tests, only failures with output:
cargo test 2>&1

# A single test with full output:
cargo test test_name -- --nocapture 2>&1

# All tests with full output (including the successful ones):
cargo test -- --nocapture 2>&1
```

### The structure of a test

```rust
#[test]
fn test_my_scenario() {
    let mut cfg = make_3x3(7, 3);  // 3 rows × 3 columns, 7×3 characters each
    cfg.apply_border_pos(&BorderPos::Grid, &BORDER_SIMPLE);
    cfg.apply_border_pos(&BorderPos::AfterCol(0), &BORDER_SIMPLE);
    // ...further configuration...

    assert_grid("my_scenario", &render(&cfg), &[
        "┌───────┬──────────────┐",
        "│▓▓▓▓▓▓▓│░░░░░░░███████│",
        // ...one line per terminal row...
        "└───────┴──────────────┘",
    ]);
}
```

### Helper functions

```rust
make_3x3(col_len, row_len)   // 3×3 GridConfig with Length constraints
make_3x5(col_len, row_len)   // 3×5 GridConfig (3 rows, 5 columns)
render(&cfg)                  // → Vec<String> with cell backgrounds
assert_grid(label, lines, expected)  // pixel-exact line comparisons
print_grid(label, lines)     // prints the grid to stdout
```

### Determining new expected strings

When a change alters the output (e.g. a new cell-background formula), run all the
tests once with `--nocapture` and adopt the actual output as the new expected
strings:

```sh
cargo test -- --nocapture 2>&1 | grep -A 30 "── my_test ──"
```

---

## Peculiarities and pitfalls

### The cell-background formula

```rust
CELL_BG[(2 * row + col) % 3]  // ▓=0  ░=1  █=2
```

The factor `2` makes sure that adjacent cells (horizontally **and** vertically)
always have different backgrounds, regardless of the number of columns. With
`(row * cols + col) % 3`, every column would end up with a uniform colour
whenever `cols % 3 == 0`.

### Half-ending logic

An inner border gets half endings (`╷╵╶╴`) when:

- there is no outer frame, **or**
- the outer frame has a different style (no join → no T-piece)

With the same style, `draw_crossings` overwrites the half endings with T-pieces
(`┬`, `┴`, `├`, `┤`). That is why the order matters: draw the outer frame first,
then the inner borders, then the crossings.

### The same-style comparison

```rust
fn same_style(a: &BorderChars, b: &BorderChars) -> bool {
    std::ptr::eq(a, b) || (a.horizontal == b.horizontal && a.vertical == b.vertical)
}
```

`BORDER_SIMPLE` and `BORDER_ROUNDED` have identical `─`/`│` chars but different
corners. They count as "the same style" for join purposes — which is correct,
since T-pieces and crossings only use `─`/`│` and `cross`, not the corners.

### Gap rows have no cell backgrounds

A gap row (horizontal) or gap column (vertical) belongs to no cell. The buffer
stays empty there after `fill_cell_backgrounds` (spaces). Only the border
characters fill those areas. That means: with a spanned border covering only part
of a gap row, the remaining positions in that row stay spaces — even if a cell
lies "next to" them.

### `Before*` is an alias for `After*(i-1)`

`BorderPos::BeforeCol(i)` is identical to `AfterCol(i-1)`. `apply_border_pos`
normalizes internally to `After*` indices.

### `GapPos::Grid` vs. `BorderPos::Grid`

- `GapPos::Grid` → sets gaps between **all inner** columns and rows (no outer
  frame)
- `BorderPos::Grid` → sets a closed **outer frame**

These have completely different semantics despite the identical name.

### `set_border` implies `set_gap`

A border always needs a gap slot (1 character of space). `set_border` creates it
implicitly. `remove_border` removes only the characters, the gap slot (a space)
remains. `remove_gap` removes both.

### The order in `draw_all`

```rust
draw_outer_frame(...)      // the outer frame first
draw_horizontal_lines(...) // inner horizontal lines + spanned H
draw_vertical_lines(...)   // inner vertical lines + spanned V
draw_crossings(...)        // crossings overwrite the endings
draw_border_texts(...)     // texts last, they overwrite everything
```

This order is not arbitrary. In particular:

- Outer frame before the inner lines → half endings can overwrite the outer frame
  (with a different style)
- Crossings after the lines → T-pieces overwrite half endings (with the same
  style)
- Texts right at the end → always visible, regardless of the border chars

---

## Instructions for AI assistants

### General

- **Always deliver complete functions**, no diffs with `// ...rest stays the
same`. The user enters the code manually.
- **`cargo check` or `cargo test` after every change** — the user runs these and
  returns the output.
- **Never guess what the output will be** — when the expected strings are
  unclear, first run the test without them and adopt the actual output.
- **No changes to working tests** without an explicit request.

### Adding new tests

1. Write the test with provisional expected strings (from mental arithmetic or
   from the spec)
2. Have `cargo test test_name -- --nocapture` run
3. Adopt the actual output as the expected strings
4. Only then add further tests

### Debugging workflow

When a test fails unexpectedly:

1. `cargo test test_name -- --nocapture` for isolated output
2. Compare `got`/`want` in the error message
3. For layout problems: add a `render_debug` helper that prints `col_x`, `col_w`,
   `row_y`, `row_h`, `v_gap_x`, `h_gap_y`
4. Never guess blindly — better to request a targeted debug output

### CellGroup (still to be implemented)

When implementing groups, the following rules apply according to the spec:

- `is_inside_h_group(grid, row, v_gap_index)` → checks whether the vertical gap
  lies between two columns that belong to the same group in `row`
- `is_inside_v_group(grid, h_gap_index, col)` → the same for horizontal gaps
- The group suppression applies to step 4 (style) **and** step 5 (characters)
- The group rect (`group_rect`) covers all cells **plus** the gap columns between
  them — these are overwritten with `fill_rect` in step 7a, which makes the gap
  characters inside the group disappear

### Always give the file paths

Every code block must start with the full file path, e.g.:

```rust
// grid-render-sim/src/render.rs
```

### Shell commands in Nushell syntax

The user uses Nushell. No `&&` chaining in direct shell commands. Shell script
**files** may be sh/bash compatible.

---

## Dependencies

```toml
[dependencies]
ratatui-core = "0.1.0"
```

Only `ratatui-core` — no full ratatui, no crossterm, no tui-realm. The simulation
is completely terminal-independent and runs as pure unit tests without any UI
initialization.
