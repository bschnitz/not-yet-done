# Grid component – requirements analysis

> **Status**: draft / specification
> **Version**: v1 (draft)

---

## Table of contents

- [1. Feature overview](#1-feature-overview)
- [2. Core concepts](#2-core-concepts)
  - [2.1 Layout: cells and constraints](#21-layout-cells-and-constraints)
  - [2.2 Gaps and borders](#22-gaps-and-borders)
  - [2.3 Cell groups](#23-cell-groups)
  - [2.4 Focus, navigation and events](#24-focus-navigation-and-events)
  - [2.5 GridChild trait](#25-gridchild-trait)
- [3. Layout & ASCII examples](#3-layout--ascii-examples)
  - [3.1 Gaps and groups](#31-gaps-and-groups)
  - [3.2 Borders](#32-borders-global-configuration)
- [4. Configuration (API reference)](#4-configuration)
  - [4.1 Grid size](#41-grid-size)
  - [4.2 Constraints](#42-constraints)
  - [4.3 Borders](#43-borders)
  - [4.4 Gaps](#44-gaps)
  - [4.5 Cell groups](#45-cell-groups)
  - [4.6 Border text](#46-border-text)
  - [4.7 Styling](#47-styling)
  - [4.8 Focus and navigation](#48-focus)
- [5. Technical details](#5-technical-details)
  - [5.1 Layout algorithm](#51-layout-algorithm)
  - [5.2 Rendering pipeline](#52-rendering-pipeline)
  - [5.3 Event flow](#53-event-flow)
  - [5.4 Corner computation](#54-corner-computation-at-gap-crossings)
  - [5.5 Gap width and space computation](#55-gap-width-and-space-computation)
  - [5.6 Groups and gaps](#56-groups-and-gaps)
- [6. Future ideas](#6-future-ideas)
- [Appendix A: AI instructions](#appendix-a-ai-instructions-for-future-ai-sessions)

---

## Quick start

A minimal 2×2 grid with two TextInput fields side by side and a title below them:

```rust
use grid::{Grid, CellGroup, GapPos, BorderPos, GridKeymap, BORDER_SIMPLE};

let mut grid = Grid::new(2, 2); // 2 rows, 2 columns

// Column widths and row heights
grid.with_column_constraints([Constraint::Percentage(50), Constraint::Percentage(50)]);
grid.with_row_constraints([Constraint::Length(3), Constraint::Length(3)]);

// Whitespace gap between the columns
grid.set_gap(GapPos::AfterCol(0));

// Group the bottom row into a title row
grid.group_cells(CellGroup::Row(1));

// Outer frame
grid.set_border(BorderPos::Grid, &BORDER_SIMPLE);

// Keyboard navigation
grid.set_keymap(GridKeymap {
    next_cell: Some(KeyEvent::from(KeyCode::Tab)),
    prev_cell: Some(KeyEvent::from(KeyCode::BackTab)),
    ..Default::default()
});

// Insert child components
grid.set_child(0, 0, Box::new(TextInput::new("First name")));
grid.set_child(0, 1, Box::new(TextInput::new("Last name")));
grid.set_child(1, 0, Box::new(Label::new("Enter personal data")));
```

Result:

```
┌───────────────────────────────┐
│ First name    │ Last name     │
│               │               │
├───────────────────────────────┤
│ Enter personal data           │
└───────────────────────────────┘
```

---

## 1. Feature overview

The grid is a `MockComponent`-based layout component that arranges any number of child components in an n×m raster.

| Feature                           | Description                                                                                                                   |
| --------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| **n×m raster**                    | Any number of rows and columns                                                                                                |
| **Constraints**                   | Column width / row height constraints as in ratatui (`Length`, `Min`, `Max`, `Percentage`, `Ratio`)                           |
| **Gaps / borders**                | Configurable separators between cells: no gap (cells directly adjacent), gap (whitespace), border (Unicode box-drawing chars) |
| **BorderChars**                   | Predefined Unicode box-drawing sets + user-defined sets as `pub static`                                                       |
| **2-level gap configuration**     | Global (for the whole grid) → per column/row                                                                                  |
| **Cell groups**                   | Static and dynamic grouping of cells (`Row`, `Col`, `ColSpan`, `RowSpan`, `Span`)                                             |
| **Focus management**              | The grid manages the active cell internally                                                                                   |
| **Keyboard navigation**           | Configurable shortcuts for row/column changes and sequential cell navigation                                                  |
| **Event forwarding**              | Keys are forwarded to the active child; unconsumed keys are handled by the grid                                               |
| **Styling**                       | Individually per gap, per cell, per grid; separately for focused/inactive                                                     |
| **`set_border_text`**             | Write text into gap/border areas                                                                                              |
| **Grid nesting**                  | Cells can hold arbitrary components, including further grids                                                                  |
| **Deterministic rendering order** | Zigzag (row by row, column by column); the active widget last                                                                 |

---

## 2. Core concepts

### 2.1 Layout: cells and constraints

A grid consists of `rows` rows and `cols` columns. Every cell is identified by its zero-based index `(row, col)`.

Column widths and row heights are determined by `Constraint` values, analogous to ratatui:

| Constraint      | Meaning                                |
| --------------- | -------------------------------------- |
| `Length(n)`     | Fixed width/height of n characters     |
| `Min(n)`        | At least n characters                  |
| `Max(n)`        | At most n characters                   |
| `Percentage(p)` | Percentage share of the available area |
| `Ratio(n, d)`   | Ratio n:d of the available area        |

Gaps consume 0 or 1 characters of width/height and are subtracted from the available area before the constraints are computed (→ [5.1 Layout algorithm](#51-layout-algorithm)).

There are two configuration levels (level 2 takes precedence):

1. **Global** – `GapPos::Grid`: all inner column and row gaps at once
2. **Per column/row** – `GapPos::AfterCol(i)` etc.: a single gap

→ API: [4.1 Grid size](#41-grid-size), [4.2 Constraints](#42-constraints)

### 2.2 Gaps and borders

Between two rows or columns a **gap** can optionally exist. Every gap takes exactly 1 character of width (vertical gap) resp. 1 character of height (horizontal gap). Without a gap the cells are directly adjacent.

Independently of the gap, a **border** can be set at the same position. A border consists of Unicode box-drawing characters and occupies the same 1-character space as the gap.

| State           | Content                                       |
| --------------- | --------------------------------------------- |
| No gap          | Cells are directly adjacent (0 characters)    |
| Gap, no border  | Whitespace (1 character)                      |
| Gap with border | Box-drawing character, e.g. `│` (1 character) |

Important rules:

- `set_border` implicitly sets a gap if none exists yet
- `remove_border` only removes the border characters; the gap remains as whitespace
- `remove_gap` removes the entire space including all borders
- Gaps always span the **entire** height of a column resp. width of a row

A **`BorderChars`** set defines all characters of a border style: horizontal/vertical lines, crossings, corners, T pieces and half endings. Predefined constants (e.g. `BORDER_SIMPLE`, `BORDER_ROUNDED`) are available; custom sets can be created as `pub static`.

When a horizontal and a vertical border cross, the matching corner character is set automatically (e.g. `─` + `│` → `┼`). With different border types the lines are not joined.

→ API: [4.3 Borders](#43-borders), [4.4 Gaps](#44-gaps), visual examples: [3.1](#31-gaps-and-groups), [3.2](#32-borders-global-configuration)

### 2.3 Cell groups

Cells can be grouped into a larger unit. The group behaves like a single cell — for layout, focus and rendering.

| `CellGroup` variant                                 | Meaning                    |
| --------------------------------------------------- | -------------------------- |
| `Row(r)`                                            | All columns in row `r`     |
| `Col(c)`                                            | All rows in column `c`     |
| `ColSpan { row, first_col, last_col }`              | Several columns in one row |
| `RowSpan { col, first_row, last_row }`              | Several rows in one column |
| `Span { first_row, first_col, last_row, last_col }` | Rectangular area           |

Gaps and borders **inside** a group are not rendered; at the edge they are preserved. When a new group fully encloses an existing one, the larger one wins. Partial overlaps lead to a panic.

→ API: [4.5 Cell groups](#45-cell-groups), visual examples: [3.1](#31-gaps-and-groups)

### 2.4 Focus, navigation and events

The grid manages the active cell internally. The default navigation order is row by row from left to right (zigzag). Navigation shortcuts are fully configurable — by default **none** are set.

**Event flow** for every incoming `KeyEvent`:

```
1. Grid forwards the event to the focused child: child.on_key(key)
   ├── true  → child consumed it → done
   └── false → child did not consume it
2. Grid checks its own keymap
   ├── navigation key → change focus
   └── no match       → ignore the event
```

The focused cell is rendered **last**, so that overlay widgets (e.g. dropdowns) can extend over neighbouring cells (→ [5.2 Rendering pipeline](#52-rendering-pipeline)).

→ API: [4.8 Focus and navigation](#48-focus)

### 2.5 GridChild trait

```rust
pub trait GridChild: MockComponent {
    /// Returns `true` if the key was consumed by the child.
    /// Returns `false` if the key was not handled — the grid then checks it as a navigation key.
    fn on_key(&mut self, key: KeyEvent) -> bool;
}
```

Every component inserted into a grid cell must implement `GridChild`. The `MockComponent` supertrait is used by the grid for `render()` and `attr()`/`state()`. The grid never calls `MockComponent::on()` on child components — keyboard routing runs exclusively through `on_key()`. `on()` is only needed if the component is also to be used outside a grid in the tui-realm event loop.

Existing components implement it trivially:

```rust
impl GridChild for TextInput {
    fn on_key(&mut self, key: KeyEvent) -> bool {
        self.on(Event::Keyboard(key)).is_some()
    }
}
```

---

## 3. Layout & ASCII examples

In all examples:

| Symbol                            | Meaning                                                                                                                     |
| --------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| `▓`                               | Background of cell A (and of every 3n-th cell)                                                                              |
| `░`                               | Background of cell B (and of every 3n+1-th cell)                                                                            |
| `█`                               | Background of cell C (non-focus examples)                                                                                   |
| `▒`                               | Background of cell C (in focus examples)                                                                                    |
| `╳`                               | Background of cell D (in focus examples)                                                                                    |
| `A`, `B`, `C` …                   | Cell content (placeholder)                                                                                                  |
| `▛▀▜▌▐▙▄▟`                        | Focus frame (Unicode block elements) — **for illustration in this documentation only; it is not rendered by the component** |
| `│`, `─`, `┼`, `╷`, `╵`, `╶`, `╴` | Border characters                                                                                                           |

Cells in normal examples: 7 characters wide × 3 characters high. In focus examples: 9 × 5 (enlarged so that the illustrative focus frame has room).

### 3.1 Gaps and groups

Cells are directly adjacent.

**1×2 grid (1 row, 2 columns):**

```
▓▓▓▓▓▓▓░░░░░░░
▓▓▓A▓▓▓░░░B░░░
▓▓▓▓▓▓▓░░░░░░░
```

**2×2 grid (2 rows, 2 columns), C+D grouped:**

4 cells (A–D), each 7×3 characters, no gaps. C and D are grouped into one cell via `CellGroup::Col(1)`.

```
▓▓▓▓▓▓▓░░░░░░░
▓▓▓A▓▓▓░░░B░░░
▓▓▓▓▓▓▓░░░░░░░
██████████████
████C + D█████
██████████████
```

**3×5 grid (3 rows, 5 columns):**

15 cells (A–O), all of equal width (7 characters) and equal height (3 characters), no gaps.
The cyclic change of the background character (▓ → ░ → █) only serves to make the cell boundaries visible; in the actual component the background of every cell is freely configurable. In focus examples `▒` is used instead of `█`.

```
▓▓▓▓▓▓▓░░░░░░░███████▓▓▓▓▓▓▓░░░░░░░
▓▓▓A▓▓▓░░░B░░░███C███▓▓▓D▓▓▓░░░E░░░
▓▓▓▓▓▓▓░░░░░░░███████▓▓▓▓▓▓▓░░░░░░░
███████▓▓▓▓▓▓▓░░░░░░░███████▓▓▓▓▓▓▓
███F███▓▓▓G▓▓▓░░░H░░░███I███▓▓▓J▓▓▓
███████▓▓▓▓▓▓▓░░░░░░░███████▓▓▓▓▓▓▓
░░░░░░░███████▓▓▓▓▓▓▓░░░░░░░███████
░░░K░░░███L███▓▓▓M▓▓▓░░░N░░░███O███
░░░░░░░███████▓▓▓▓▓▓▓░░░░░░░███████
```

**3×5 grid – gap between column 2 and 3:**

Same cells as above, with a gap (whitespace) between column 2 (C, H, M) and column 3 (D, I, N).

```
▓▓▓▓▓▓▓░░░░░░░███████ ▓▓▓▓▓▓▓░░░░░░░
▓▓▓A▓▓▓░░░B░░░███C███ ▓▓▓D▓▓▓░░░E░░░
▓▓▓▓▓▓▓░░░░░░░███████ ▓▓▓▓▓▓▓░░░░░░░
███████▓▓▓▓▓▓▓░░░░░░░ ███████▓▓▓▓▓▓▓
███F███▓▓▓G▓▓▓░░░H░░░ ███I███▓▓▓J▓▓▓
███████▓▓▓▓▓▓▓░░░░░░░ ███████▓▓▓▓▓▓▓
░░░░░░░███████▓▓▓▓▓▓▓ ░░░░░░░███████
░░░K░░░███L███▓▓▓M▓▓▓ ░░░N░░░███O███
░░░░░░░███████▓▓▓▓▓▓▓ ░░░░░░░███████
```

**3×5 grid – gap between row 1 and 2:**

Same cells as above, with a gap (whitespace) between row 1 (F–J) and row 2 (K–O).

```
▓▓▓▓▓▓▓░░░░░░░███████▓▓▓▓▓▓▓░░░░░░░
▓▓▓A▓▓▓░░░B░░░███C███▓▓▓D▓▓▓░░░E░░░
▓▓▓▓▓▓▓░░░░░░░███████▓▓▓▓▓▓▓░░░░░░░
███████▓▓▓▓▓▓▓░░░░░░░███████▓▓▓▓▓▓▓
███F███▓▓▓G▓▓▓░░░H░░░███I███▓▓▓J▓▓▓
███████▓▓▓▓▓▓▓░░░░░░░███████▓▓▓▓▓▓▓

░░░░░░░███████▓▓▓▓▓▓▓░░░░░░░███████
░░░K░░░███L███▓▓▓M▓▓▓░░░N░░░███O███
░░░░░░░███████▓▓▓▓▓▓▓░░░░░░░███████
```

**3×5 grid – gap between column 2/3 and between row 1/2:**

Same cells as above, with both gaps combined.

```
▓▓▓▓▓▓▓░░░░░░░███████ ▓▓▓▓▓▓▓░░░░░░░
▓▓▓A▓▓▓░░░B░░░███C███ ▓▓▓D▓▓▓░░░E░░░
▓▓▓▓▓▓▓░░░░░░░███████ ▓▓▓▓▓▓▓░░░░░░░
███████▓▓▓▓▓▓▓░░░░░░░ ███████▓▓▓▓▓▓▓
███F███▓▓▓G▓▓▓░░░H░░░ ███I███▓▓▓J▓▓▓
███████▓▓▓▓▓▓▓░░░░░░░ ███████▓▓▓▓▓▓▓

░░░░░░░███████▓▓▓▓▓▓▓ ░░░░░░░███████
░░░K░░░███L███▓▓▓M▓▓▓ ░░░N░░░███O███
░░░░░░░███████▓▓▓▓▓▓▓ ░░░░░░░███████
```

**2×2 grid (2 rows, 2 columns) with gaps:**

4 cells (A–D), each 7×3 characters, with gaps (whitespace) between columns and rows.
Illustrates that gaps between cells are configurable.

```
▓▓▓▓▓▓▓ ░░░░░░░
▓▓▓A▓▓▓ ░░░B░░░
▓▓▓▓▓▓▓ ░░░░░░░

███████ ▓▓▓▓▓▓▓
███C███ ▓▓▓D▓▓▓
███████ ▓▓▓▓▓▓▓
```

**2×2 grid – C and D grouped as a ColSpan:**

Same cells as above, but C and D are grouped into one cell (`CellGroup::Col(1)`, or equivalently `CellGroup::ColSpan { row: 1, first_col: 0, last_col: 1 }`).
The vertical gap between C and D disappears, since the two now form a single cell.
The horizontal gap between row 0 and row 1 is preserved.

```
▓▓▓▓▓▓▓ ░░░░░░░
▓▓▓A▓▓▓ ░░░B░░░
▓▓▓▓▓▓▓ ░░░░░░░

███████████████
█████C + D█████
███████████████
```

**3×4 grid – header, sidebar, ColSpan and Col:**

A 3×4 grid without gaps that shows all group types:

- `CellGroup::Row(0)` → A groups all columns in row 0
- `CellGroup::Col(3)` → G groups the whole column 3 (all rows)
- `CellGroup::RowSpan { col: 0, first_row: 1, last_row: 2 }` → B spans rows 1 and 2 in column 0
- `CellGroup::ColSpan { row: 1, first_col: 1, last_col: 2 }` → C and D are grouped into one cell
- E, F are individual cells

**Overlap rule**: `CellGroup::Row(0)` and `CellGroup::Col(3)` overlap in cell (0, 3). Since `Col(3)` covers the whole column 3 and `Row(0)` the whole row 0 — neither group covers the other completely, it is a pure intersection. **The rule here**: if one group fully encloses the other, the enclosing one wins; if they only intersect partially, the second `group_cells` assignment leads to a panic. The example is therefore only correct if `Col(3)` is defined first and `Row(0)` afterwards — in that case `Row(0)` (row 0, all 4 columns) covers the cell (0, 3), which is already part of `Col(3)`. Since neither contains the other completely, this combination is in practice an **invalid state** and should be avoided. Recommendation: use a `ColSpan { row: 0, first_col: 0, last_col: 2 }` instead of `Row(0)`.

```
▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓╬╬╬╬╬╬╬
▓▓▓▓▓▓▓▓▓▓A▓▓▓▓▓▓▓▓▓▓╬╬╬╬╬╬╬
▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓╬╬╬╬╬╬╬
░░░░░░░██████████████╬╬╬╬╬╬╬
░░░░░░░██████C+D█████╬╬╬G╬╬╬
░░░░░░░██████████████╬╬╬╬╬╬╬
░░░B░░░▒▒▒▒▒▒▒╳╳╳╳╳╳╳╬╬╬╬╬╬╬
░░░░░░░▒▒▒E▒▒▒╳╳╳F╳╳╳╬╬╬╬╬╬╬
░░░░░░░▒▒▒▒▒▒▒╳╳╳╳╳╳╳╬╬╬╬╬╬╬
```

**3×5 grid – `Span { first_row: 1, first_col: 1, last_row: 2, last_col: 3 }`:**

`CellGroup::Span { first_row: 1, first_col: 1, last_row: 2, last_col: 3 }` → cells B, C, D, G, H, I are grouped into one cell (3 columns × 2 rows). The background of the first cell (░, cell B) is used.

```
▓▓▓▓▓▓▓╬╬╬╬╬╬╬███████▓▓▓▓▓▓▓░░░░░░░
▓▓▓A▓▓▓╬╬╬B╬╬╬███C███▓▓▓D▓▓▓░░░E░░░
▓▓▓▓▓▓▓╬╬╬╬╬╬╬███████▓▓▓▓▓▓▓░░░░░░░
███████░░░░░░░░░░░░░░░░░░░░░▓▓▓▓▓▓▓
███F███░░░░░░░░B+C+D░░░░░░░░▓▓▓J▓▓▓
███████░░░░░░░░░░░░░░░░░░░░░▓▓▓▓▓▓▓
╬╬╬╬╬╬╬░░░░░░░░░░░░░░░░░░░░░███████
╬╬╬K╬╬╬░░░░░░░░G+H+I░░░░░░░░███O███
╬╬╬╬╬╬╬░░░░░░░░░░░░░░░░░░░░░███████
```

### 3.2 Borders (global configuration)

**3×5 grid – simple borders globally (`BorderPos::Grid`):**

Base: same cells as the 3×5 grid (A–O), 7×3 characters per cell.
`set_border(BorderPos::Grid, &BORDER_SIMPLE)` sets simple borders between all columns and rows as well as an outer frame.

```
┌───────┬───────┬───────┬───────┬───────┐
│▓▓▓▓▓▓▓│░░░░░░░│███████│▓▓▓▓▓▓▓│░░░░░░░│
│▓▓▓A▓▓▓│░░░B░░░│███C███│▓▓▓D▓▓▓│░░░E░░░│
│▓▓▓▓▓▓▓│░░░░░░░│███████│▓▓▓▓▓▓▓│░░░░░░░│
├───────┼───────┼───────┼───────┼───────┤
│███████│▓▓▓▓▓▓▓│░░░░░░░│███████│▓▓▓▓▓▓▓│
│███F███│▓▓▓G▓▓▓│░░░H░░░│███I███│▓▓▓J▓▓▓│
│███████│▓▓▓▓▓▓▓│░░░░░░░│███████│▓▓▓▓▓▓▓│
├───────┼───────┼───────┼───────┼───────┤
│░░░░░░░│███████│▓▓▓▓▓▓▓│░░░░░░░│███████│
│░░░K░░░│███L███│▓▓▓M▓▓▓│░░░N░░░│███O███│
│░░░░░░░│███████│▓▓▓▓▓▓▓│░░░░░░░│███████│
└───────┴───────┴───────┴───────┴───────┘
```

**3×5 grid – selective borders (`AfterCol(1)` + `BeforeRow(2)`):**

Base: same cells as the 3×5 grid (A–O), 7×3 characters per cell.

- `set_border(BorderPos::AfterCol(1), &BORDER_SIMPLE)` → vertical border between column 1 (B, G, L) and column 2 (C, H, M). Ends with a half ending at the top (`╷`) and at the bottom (`╵`).
- `set_border(BorderPos::BeforeRow(2), &BORDER_SIMPLE)` → horizontal border before row 2 (between F–J and K–O). Ends with a half ending on the left (`╶`) and on the right (`╴`).
- Both borders cross → corner character `┼`.

```
▓▓▓▓▓▓▓░░░░░░░╷███████▓▓▓▓▓▓▓░░░░░░░
▓▓▓A▓▓▓░░░B░░░│███C███▓▓▓D▓▓▓░░░E░░░
▓▓▓▓▓▓▓░░░░░░░│███████▓▓▓▓▓▓▓░░░░░░░
███████▓▓▓▓▓▓▓│░░░░░░░███████▓▓▓▓▓▓▓
███F███▓▓▓G▓▓▓│░░░H░░░███I███▓▓▓J▓▓▓
███████▓▓▓▓▓▓▓│░░░░░░░███████▓▓▓▓▓▓▓
╶─────────────┼────────────────────╴
░░░░░░░███████│▓▓▓▓▓▓▓░░░░░░░███████
░░░K░░░███L███│▓▓▓M▓▓▓░░░N░░░███O███
░░░░░░░███████╵▓▓▓▓▓▓▓░░░░░░░███████
```

**3×5 grid – `BORDER_SIMPLE_EXTENDED` (`AfterCol(1)` + `BeforeRow(2)`):**

Same positions as above, but with `&BORDER_SIMPLE_EXTENDED` instead of `&BORDER_SIMPLE`.
With `SimpleExtended` the lines run through at the ends (full endings instead of half endings).
Difference to `Simple`: no `╷`/`╵`/`╶`/`╴`, but `│`/`─` all the way to the edge.

```
▓▓▓▓▓▓▓░░░░░░░│███████▓▓▓▓▓▓▓░░░░░░░
▓▓▓A▓▓▓░░░B░░░│███C███▓▓▓D▓▓▓░░░E░░░
▓▓▓▓▓▓▓░░░░░░░│███████▓▓▓▓▓▓▓░░░░░░░
███████▓▓▓▓▓▓▓│░░░░░░░███████▓▓▓▓▓▓▓
███F███▓▓▓G▓▓▓│░░░H░░░███I███▓▓▓J▓▓▓
███████▓▓▓▓▓▓▓│░░░░░░░███████▓▓▓▓▓▓▓
──────────────┼─────────────────────
░░░░░░░███████│▓▓▓▓▓▓▓░░░░░░░███████
░░░K░░░███L███│▓▓▓M▓▓▓░░░N░░░███O███
░░░░░░░███████│▓▓▓▓▓▓▓░░░░░░░███████
```

**3×5 grid – partial borders (`AfterRowSpanned` + `BeforeColSpanned`):**

Base: same cells as the 3×5 grid (A–O), 7×3 characters per cell.

- `set_border(BorderPos::AfterRowSpanned { row: 1, col_start: 0, col_end: 1 }, &BORDER_SIMPLE)` → horizontal border after row 1, only below column 0 (F, K) and column 1 (G, L). Half endings (`╶`/`╴`).
- `set_border(BorderPos::BeforeColSpanned { col: 4, row_start: 1, row_end: 2 }, &BORDER_SIMPLE)` → vertical border before column 4, only in row 1 (I, J) and row 2 (N, O). Half endings (`╷`/`╵`).
- The borders do not cross (the horizontal border only reaches up to column 1, the vertical one starts at column 4).

```
▓▓▓▓▓▓▓░░░░░░░███████▓▓▓▓▓▓▓ ░░░░░░░
▓▓▓A▓▓▓░░░B░░░███C███▓▓▓D▓▓▓ ░░░E░░░
▓▓▓▓▓▓▓░░░░░░░███████▓▓▓▓▓▓▓ ░░░░░░░
███████▓▓▓▓▓▓▓░░░░░░░███████╷▓▓▓▓▓▓▓
███F███▓▓▓G▓▓▓░░░H░░░███I███│▓▓▓J▓▓▓
███████▓▓▓▓▓▓▓░░░░░░░███████│▓▓▓▓▓▓▓
╶────────────╴              │
░░░░░░░███████▓▓▓▓▓▓▓░░░░░░░│███████
░░░K░░░███L███▓▓▓M▓▓▓░░░N░░░│███O███
░░░░░░░███████▓▓▓▓▓▓▓░░░░░░░╵███████
```

**3×5 grid – `AfterRowSpanned` (simple) + `BeforeColSpanned` (double):**

- `set_border(BorderPos::AfterRowSpanned { row: 1, col_start: 2, col_end: 3 }, &BORDER_SIMPLE)` → horizontal border (─) after row 1, only below column 2 (C, H) and column 3 (D, I). Half endings (`╶`/`╴`).
- `set_border(BorderPos::BeforeColSpanned { col: 4, row_start: 1, row_end: 2 }, &BORDER_DOUBLE_EXTENDED)` → vertical border (║) before column 4, only in row 1 (I, J) and row 2 (N, O). Since there are no half endings for ║, `&BORDER_DOUBLE_EXTENDED` is used (full endings).
- The two borders are of **different** type (simple vs. double) → they do touch, but are **not** joined.

```
▓▓▓▓▓▓▓░░░░░░░███████▓▓▓▓▓▓▓ ░░░░░░░
▓▓▓A▓▓▓░░░B░░░███C███▓▓▓D▓▓▓ ░░░E░░░
▓▓▓▓▓▓▓░░░░░░░███████▓▓▓▓▓▓▓ ░░░░░░░
███████▓▓▓▓▓▓▓░░░░░░░███████║▓▓▓▓▓▓▓
███F███▓▓▓G▓▓▓░░░H░░░███I███║▓▓▓J▓▓▓
███████▓▓▓▓▓▓▓░░░░░░░███████║▓▓▓▓▓▓▓
              ╶────────────╴║
░░░░░░░███████▓▓▓▓▓▓▓░░░░░░░║███████
░░░K░░░███L███▓▓▓M▓▓▓░░░N░░░║███O███
░░░░░░░███████▓▓▓▓▓▓▓░░░░░░░║███████
```

**Same configuration + `set_border_text`:**

In addition: `set_border_text(BorderPos::AfterRowSpanned { row: 1, col_start: 2, col_end: 3 }, TextAnchor::End, 0, "─╢")`.
The text "─╢" is written from left to right, ending at the end of the horizontal border: ─ replaces the half ending ╴ (pos 27), ╢ replaces the ║ (pos 28). The horizontal border now runs through and joins the column with ╢.

```
▓▓▓▓▓▓▓░░░░░░░███████▓▓▓▓▓▓▓ ░░░░░░░
▓▓▓A▓▓▓░░░B░░░███C███▓▓▓D▓▓▓ ░░░E░░░
▓▓▓▓▓▓▓░░░░░░░███████▓▓▓▓▓▓▓ ░░░░░░░
███████▓▓▓▓▓▓▓░░░░░░░███████║▓▓▓▓▓▓▓
███F███▓▓▓G▓▓▓░░░H░░░███I███║▓▓▓J▓▓▓
███████▓▓▓▓▓▓▓░░░░░░░███████║▓▓▓▓▓▓▓
              ╶─────────────╢
░░░░░░░███████▓▓▓▓▓▓▓░░░░░░░║███████
░░░K░░░███L███▓▓▓M▓▓▓░░░N░░░║███O███
░░░░░░░███████▓▓▓▓▓▓▓░░░░░░░║███████
```

**3×3 grid with gaps everywhere:**

```
▓▓▓▓▓▓▓ ░░░░░░░ ███████
▓▓▓A▓▓▓ ░░░B░░░ ███C███
▓▓▓▓▓▓▓ ░░░░░░░ ███████

▓▓▓▓▓▓▓ ░░░░░░░ ███████
▓▓▓D▓▓▓ ░░░E░░░ ███F███
▓▓▓▓▓▓▓ ░░░░░░░ ███████

▓▓▓▓▓▓▓ ░░░░░░░ ███████
▓▓▓G▓▓▓ ░░░H░░░ ███I███
▓▓▓▓▓▓▓ ░░░░░░░ ███████
```

**3×5 grid – different border configurations combined:**

This example shows different border kinds and configuration levels in a single grid:

- `set_border(BorderPos::AfterRow(0), &BORDER_DOUBLE_EXTENDED)` → complete horizontal double border (═) across the whole width after row 0
- `set_border(BorderPos::AfterRowSpanned { row: 1, col_start: 2, col_end: 3 }, &BORDER_ROUNDED)`
- `set_border(BorderPos::AfterColSpanned { col: 1, row_start: 2, row_end: 2 }, &BORDER_ROUNDED)`
- `set_border(BorderPos::AfterColSpanned { col: 3, row_start: 2, row_end: 2 }, &BORDER_ROUNDED)`

```
▓▓▓▓▓▓▓░░░░░░░ ███████▓▓▓▓▓▓▓ ░░░░░░░
▓▓▓A▓▓▓░░░B░░░ ███C███▓▓▓D▓▓▓ ░░░E░░░
▓▓▓▓▓▓▓░░░░░░░ ███████▓▓▓▓▓▓▓ ░░░░░░░
═════════════════════════════════════
███████▓▓▓▓▓▓▓ ░░░░░░░███████ ▓▓▓▓▓▓▓
███F███▓▓▓G▓▓▓ ░░░H░░░███I███ ▓▓▓J▓▓▓
███████▓▓▓▓▓▓▓ ░░░░░░░███████ ▓▓▓▓▓▓▓
              ╭──────────────╮
░░░░░░░███████│▓▓▓▓▓▓▓░░░░░░░│███████
░░░K░░░███L███│▓▓▓M▓▓▓░░░N░░░│███O███
░░░░░░░███████╵▓▓▓▓▓▓▓░░░░░░░╵███████
```

**Same configuration + `set_border_text`:**

In addition: `set_border_text(BorderPos::AfterColSpanned { col: 1, row_start: 2, row_end: 2 }, TextAnchor::Start, 0, "Down")` and `set_border_text(BorderPos::AfterRow(0), TextAnchor::Start, 2, " My Header ")`.

- `set_border_text` writes text into an area (whitespace or border characters) and overwrites the characters there.
- " My Header " is written horizontally into the gap after row 0, starting at position 2. The ═ characters are overwritten by the text characters.
- "Do…" is written vertically into the column gap after column 1 (only in row 2). The gap is 3 rows high, the text "Down" has 4 characters → it is truncated with an ellipsis. The ╭ characters are overwritten by the text characters.

```
▓▓▓▓▓▓▓░░░░░░░ ███████▓▓▓▓▓▓▓ ░░░░░░░
▓▓▓A▓▓▓░░░B░░░ ███C███▓▓▓D▓▓▓ ░░░E░░░
▓▓▓▓▓▓▓░░░░░░░ ███████▓▓▓▓▓▓▓ ░░░░░░░
══ My Header ════════════════════════
███████▓▓▓▓▓▓▓ ░░░░░░░███████ ▓▓▓▓▓▓▓
███F███▓▓▓G▓▓▓ ░░░H░░░███I███ ▓▓▓J▓▓▓
███████▓▓▓▓▓▓▓ ░░░░░░░███████ ▓▓▓▓▓▓▓
              D──────────────╮
░░░░░░░███████o▓▓▓▓▓▓▓░░░░░░░│███████
░░░K░░░███L███w▓▓▓M▓▓▓░░░N░░░│███O███
░░░░░░░███████…▓▓▓▓▓▓▓░░░░░░░╵███████
```

---

## 4. Configuration

All public methods of `Grid` at a glance:

| Method                                        | Description                             |
| --------------------------------------------- | --------------------------------------- |
| `Grid::new(rows, cols)`                       | Create a grid                           |
| `with_column_constraints([..])`               | Set column width constraints            |
| `with_row_constraints([..])`                  | Set row height constraints              |
| `set_border(pos, chars)`                      | Set a border (implicitly creates a gap) |
| `remove_border(pos)`                          | Remove a border (the gap remains)       |
| `set_border_style(pos, style)`                | Set the style for a border/gap          |
| `set_border_text(pos, anchor, offset, text)`  | Write text into a border/gap area       |
| `remove_border_text(pos)`                     | Remove border text                      |
| `set_gap(pos)`                                | Set a gap                               |
| `remove_gap(pos)`                             | Remove gap and border                   |
| `set_style(style)`                            | Set the global default style            |
| `configure_cell_style(row, col, style)`       | Set the cell style                      |
| `group_cells(group)`                          | Group cells                             |
| `ungroup_cells(row, col)`                     | Dissolve a group                        |
| `set_keymap(keymap)`                          | Configure keyboard navigation           |
| `set_child(row, col, child)`                  | Insert a child component                |
| `focused_cell()`                              | Query the current focus position        |
| `focus_next()` / `focus_prev()`               | Move the focus sequentially             |
| `focus_next_in_row()` / `focus_prev_in_row()` | Move the focus within the row           |
| `focus_next_in_col()` / `focus_prev_in_col()` | Move the focus within the column        |

### 4.1 Grid size

```rust
let grid = Grid::new(rows: usize, cols: usize);
```

Example:

```rust
let grid = Grid::new(3, 4); // 3 rows, 4 columns
```

### 4.2 Constraints

Constraints for column widths and row heights, analogous to ratatui:

```rust
grid.with_column_constraints([
    Constraint::Length(10),
    Constraint::Min(20),
    Constraint::Percentage(30),
    Constraint::Ratio(1, 3),
]);

grid.with_row_constraints([
    Constraint::Length(3),
    Constraint::Min(5),
    Constraint::Max(10),
]);
```

### 4.3 Borders

#### `BorderPos` – where is the border set?

Quick reference of all variants:

| Variant                                        | Direction  | Area                                | Endings                |
| ---------------------------------------------- | ---------- | ----------------------------------- | ---------------------- |
| `Grid`                                         | both       | outer frame                         | corners                |
| `AfterCol(i)`                                  | vertical   | between column i and i+1, all rows  | full line              |
| `BeforeCol(i)`                                 | vertical   | between column i-1 and i, all rows  | full line              |
| `AfterRow(i)`                                  | horizontal | between row i and i+1, all columns  | full line              |
| `BeforeRow(i)`                                 | horizontal | between row i-1 and i, all columns  | full line              |
| `AfterColSpanned { col, row_start, row_end }`  | vertical   | only in rows row_start..=row_end    | half endings (`╷`/`╵`) |
| `BeforeColSpanned { col, row_start, row_end }` | vertical   | only in rows row_start..=row_end    | half endings           |
| `AfterRowSpanned { row, col_start, col_end }`  | horizontal | only in columns col_start..=col_end | half endings (`╶`/`╴`) |
| `BeforeRowSpanned { row, col_start, col_end }` | horizontal | only in columns col_start..=col_end | half endings           |

> `After` and `Before` address the same physical position — `AfterCol(i)` is identical to `BeforeCol(i+1)`. Both variants exist for more readable code.

```rust
pub enum BorderPos {
    /// Outer frame around the whole grid
    Grid,

    /// Vertical border after column i (between column i and i+1), across all rows
    AfterCol(usize),
    /// Vertical border before column i (between column i-1 and i), across all rows
    BeforeCol(usize),

    /// Horizontal border after row i (between row i and i+1), across all columns
    AfterRow(usize),
    /// Horizontal border before row i (between row i-1 and i), across all columns
    BeforeRow(usize),

    /// Vertical border after column col, only in rows row_start..=row_end
    AfterColSpanned { col: usize, row_start: usize, row_end: usize },
    /// Vertical border before column col, only in rows row_start..=row_end
    BeforeColSpanned { col: usize, row_start: usize, row_end: usize },

    /// Horizontal border after row row, only in columns col_start..=col_end
    AfterRowSpanned { row: usize, col_start: usize, col_end: usize },
    /// Horizontal border before row row, only in columns col_start..=col_end
    BeforeRowSpanned { row: usize, col_start: usize, col_end: usize },
}
```

`Grid` creates a closed outer frame around the whole grid. The `AfterCol`/`BeforeCol` variants create vertical lines across the full height, `AfterRow`/`BeforeRow` horizontal lines across the full width. The `Spanned` variants limit the border to a sub-area; half endings are set at the ends (see section [3.2](#32-borders-global-configuration) for visual examples).

#### `BorderChars` – which characters are used?

```rust
pub struct BorderChars {
    pub horizontal: char,
    pub vertical: char,
    pub cross: char,
    pub top_left: char,
    pub top_right: char,
    pub bottom_left: char,
    pub bottom_right: char,
    pub t_left: char,
    pub t_right: char,
    pub t_top: char,
    pub t_bottom: char,
    pub half_top: char,
    pub half_bottom: char,
    pub half_left: char,
    pub half_right: char,
}

impl BorderChars {
    pub const fn new(
        horizontal: char, vertical: char, cross: char,
        top_left: char, top_right: char, bottom_left: char, bottom_right: char,
        t_left: char, t_right: char, t_top: char, t_bottom: char,
        half_top: char, half_bottom: char, half_left: char, half_right: char,
    ) -> Self {
        Self { horizontal, vertical, cross, top_left, top_right, bottom_left, bottom_right, t_left, t_right, t_top, t_bottom, half_top, half_bottom, half_left, half_right }
    }
}
```

**Predefined constants:**

| Name                      | Lines   | Half endings    | Corners   |
| ------------------------- | ------- | --------------- | --------- |
| `BORDER_SIMPLE`           | `─` `│` | `╷` `╵` `╶` `╴` | `┌ ┐ └ ┘` |
| `BORDER_SIMPLE_EXTENDED`  | `─` `│` | `│` `│` `─` `─` | `┌ ┐ └ ┘` |
| `BORDER_DOUBLE_EXTENDED`  | `═` `║` | `║` `║` `═` `═` | `╔ ╗ ╚ ╝` |
| `BORDER_THICK_EXTENDED`   | `━` `┃` | `┃` `┃` `━` `━` | `┏ ┓ ┗ ┛` |
| `BORDER_ROUNDED`          | `─` `│` | `╷` `╵` `╶` `╴` | `╭ ╮ ╰ ╯` |
| `BORDER_ROUNDED_EXTENDED` | `─` `│` | `│` `│` `─` `─` | `╭ ╮ ╰ ╯` |
| `BORDER_DASHED`           | `┄` `┆` | `╷` `╵` `╶` `╴` | `┌ ┐ └ ┘` |
| `BORDER_DASHED_EXTENDED`  | `┄` `┆` | `│` `│` `─` `─` | `┌ ┐ └ ┘` |
| `BORDER_DOTTED`           | `┈` `┊` | `╷` `╵` `╶` `╴` | `┌ ┐ └ ┘` |
| `BORDER_DOTTED_EXTENDED`  | `┈` `┊` | `│` `│` `─` `─` | `┌ ┐ └ ┘` |

**Unterschied Extended vs. nicht Extended:** Extended-Varianten verwenden volle Enden — die Linien gehen bis zum Rand durch. Nicht-Extended verwenden halbe Enden.

Hinweis: `Double` und `Thick` gibt es nur als Extended, da es für `║`/`═` bzw. `┃`/`━` keine Halb-Enden in Unicode gibt. `Dashed`/`Dotted` verwenden Simple-Zeichen für Ecken und Halb-Enden, da es keine gestrichelten/gepunkteten Varianten davon gibt.

#### `set_border` – Borders setzen und entfernen

```rust
impl Grid {
    /// Border an einer Position setzen (überschreibt bestehenden Border).
    /// Erzeugt implizit einen Gap, falls an der Position keiner existiert.
    pub fn set_border(&mut self, pos: BorderPos, border: &'static BorderChars);

    /// Border an einer Position entfernen.
    /// Der Gap bleibt als Leerzeichen bestehen.
    pub fn remove_border(&mut self, pos: BorderPos);

    /// Style für eine Border-/Gap-Position setzen.
    pub fn set_border_style(&mut self, pos: BorderPos, style: Style);
}
```

**Beispiel:**

```rust
// Globaler Simple-Rahmen
grid.set_border(BorderPos::Grid, &BORDER_SIMPLE);

// Vertikaler Border nach Spalte 1, nur in Zeilen 1-2, mit Style
grid.set_border(
    BorderPos::AfterColSpanned { col: 1, row_start: 1, row_end: 2 },
    &BORDER_ROUNDED,
);
grid.set_border_style(
    BorderPos::AfterColSpanned { col: 1, row_start: 1, row_end: 2 },
    Style::default().fg(Color::Cyan),
);

// Horizontalen Border entfernen (Gap bleibt als Leerzeichen)
grid.remove_border(BorderPos::BeforeRow(2));
```

#### Auto-Join

Wenn sich zwei Borders des gleichen Typs kreuzen, wird automatisch das passende Corner-Zeichen verwendet (z.B. `─` + `│` → `┼` bei `BORDER_SIMPLE`). Bei unterschiedlichen Typen werden die Linien nicht gejoint und behalten jeweils ihre eigenen Enden.

#### Benutzerdefiniertes BorderChars

```rust
pub static BRAILLE_BORDER: BorderChars = BorderChars::new(
    '⠤', // horizontal: obere und untere Dots
    '⡇', // vertical:   linke Dots
    '⠿', // cross:      alle Dots
    '⡷', // top_left:   linke + untere Dots
    '⢾', // top_right:  rechte + obere Dots
    '⣇', // bottom_left: linke + untere Dots
    '⣸', // bottom_right: rechte + obere Dots
    '⡇', // t_left:     linke Dots + nach rechts
    '⢾', // t_right:    rechte Dots + nach links
    '⠤', // t_top:      obere + untere Dots
    '⠤', // t_bottom:   obere + untere Dots
    '⠂', // half_top:   einzelner Dot oben
    '⠂', // half_bottom:einzelner Dot unten
    '⠄', // half_left:  einzelner Dot links
    '⠄', // half_right: einzelner Dot rechts
);

grid.set_border(BorderPos::Grid, &BRAILLE_BORDER);
```

### 4.4 Gaps

Gaps definieren den Platz zwischen Zellen. Jeder Gap nimmt genau 1 Zeichen Breite (vertikale Gaps) bzw. 1 Zeichen Höhe (horizontale Gaps) ein. Standardmäßig gibt es keine Gaps — Zellen grenzen direkt aneinander.

#### `GapPos` – Wo wird ein Gap gesetzt?

```rust
pub enum GapPos {
    /// Gaps zwischen allen inneren Spalten und Zeilen (kein äußerer Rahmen)
    Grid,

    /// Vertikaler Gap nach Spalte i (zwischen Spalte i und i+1)
    AfterCol(usize),
    /// Vertikaler Gap vor Spalte i (zwischen Spalte i-1 und i)
    BeforeCol(usize),

    /// Horizontaler Gap nach Zeile i (zwischen Zeile i und i+1)
    AfterRow(usize),
    /// Horizontaler Gap vor Zeile i (zwischen Zeile i-1 und i)
    BeforeRow(usize),
}
```

> **Hinweis**: `GapPos::Grid` und `BorderPos::Grid` haben unterschiedliche Semantik. `GapPos::Grid` setzt Gaps zwischen allen inneren Spalten und Zeilen (ohne äußeren Rahmen). `BorderPos::Grid` setzt einen geschlossenen äußeren Rahmen um das gesamte Grid. `AfterCol(i)` in `GapPos` und `AfterCol(i)` in `BorderPos` adressieren dieselbe physische Position — `set_border(BorderPos::AfterCol(i), ...)` setzt automatisch auch einen Gap an dieser Position, falls noch keiner existiert.

#### `set_gap` / `remove_gap`

```rust
impl Grid {
    /// Gap an einer Position setzen (1 Zeichen Platz, gefüllt mit Leerzeichen).
    pub fn set_gap(&mut self, pos: GapPos);

    /// Gap an einer Position komplett entfernen.
    /// Zellen grenzen direkt aneinander. Eventuelle Borders in diesem Gap werden mit entfernt.
    pub fn remove_gap(&mut self, pos: GapPos);
}
```

**Beispiel:**

```rust
// Gaps zwischen allen Spalten und Zeilen
grid.set_gap(GapPos::Grid);

// Nur Gaps zwischen Zeilen
grid.set_gap(GapPos::AfterRow(0));
grid.set_gap(GapPos::AfterRow(1));

// Vertikalen Gap zwischen Spalte 2 und 3 entfernen
grid.remove_gap(GapPos::AfterCol(2));
```

#### Zusammenspiel mit Borders

- `set_border` erzeugt implizit einen Gap an der Position, falls keiner existiert.
- `remove_border` entfernt nur die Border-Zeichen; der Gap bleibt als Leerzeichen bestehen.
- `remove_gap` entfernt den kompletten Raum, einschließlich aller Borders darin.
- Ein Gap ohne Border ist mit Leerzeichen gefüllt.
- Ein Gap mit Border zeigt die Border-Zeichen (siehe Abschnitt [4.3](#43-borders) für visuelle Beispiele).

### 4.5 Cell Groups

Zellen können zu größeren Einheiten zusammengefasst werden. Eine Gruppe wird wie eine einzelne Zelle behandelt — für Layout, Fokus und Rendering.

#### `CellGroup`-Enum

```rust
pub enum CellGroup {
    /// Ganze Zeile zusammenfassen
    Row(usize),
    /// Ganze Spalte zusammenfassen
    Col(usize),
    /// Mehrere Spalten in einer Zeile zusammenfassen
    ColSpan { row: usize, first_col: usize, last_col: usize },
    /// Mehrere Zeilen in einer Spalte zusammenfassen
    RowSpan { col: usize, first_row: usize, last_row: usize },
    /// Rechteckiger Bereich zusammenfassen
    Span {
        first_row: usize,
        first_col: usize,
        last_row: usize,
        last_col: usize,
    },
}
```

#### `group_cells` / `ungroup_cells`

```rust
impl Grid {
    /// Zellen zu einer Gruppe zusammenfassen.
    /// Die zusammengefassten Zellen teilen sich den Platz ohne interne Gaps/Borders.
    pub fn group_cells(&mut self, group: CellGroup);

    /// Gruppe auflösen, in der sich die Zelle (row, col) befindet.
    /// Wenn die Zelle nicht Teil einer Gruppe ist, hat der Aufruf keine Wirkung.
    pub fn ungroup_cells(&mut self, row: usize, col: usize);
}
```

**Beispiel:**

```rust
// B, C, D, G, H, I zu einer Zelle zusammenfassen (vgl. Abschnitt 3.1)
grid.group_cells(CellGroup::Span { first_row: 1, first_col: 1, last_row: 2, last_col: 3 });

// Gruppe auflösen, die Zelle (1, 1) enthält
grid.ungroup_cells(1, 1);
```

#### Verhalten bei Grouping

- Die gruppierten Zellen teilen sich den kombinierten Platz aller Einzelzellen (ohne interne Gaps/Borders).
- Die erste Zelle (oben-links) bestimmt den Hintergrund der gruppierten Zelle.
- Ein Kind-Widget wird der gesamten Fläche der Gruppe zugewiesen.
- Fokus springt über die Gruppe als Ganzes.

#### Überlappungsverhalten

Wenn `group_cells` aufgerufen wird und die neue Gruppe mit einer bestehenden Gruppe überlappt, gelten folgende Regeln:

- **Vollständige Umschließung**: Wenn die neue Gruppe eine bestehende vollständig umschließt (oder umgekehrt), wird die kleinere ignoriert — die größere Gruppe gewinnt. `ungroup_cells` auf die kleinere hat dann keine Wirkung mehr.
- **Partielle Überschneidung**: Wenn sich zwei Gruppen nur teilweise überschneiden (ohne dass eine die andere vollständig enthält), **panic!** in Debug-Builds. In Release-Builds ist das Verhalten undefiniert. Partielle Überschneidungen müssen vom Aufrufer vermieden werden.

#### Zusammenspiel mit Gaps und Borders

Gaps und Borders, die **innerhalb** einer Gruppe verlaufen würden, werden unterbrochen und nicht gezeichnet. Visuell verhält es sich so, als wären die Borders auf beiden Seiten der Gruppe separat definiert worden:

- Eine durchgehende horizontale Border (`AfterRow(1)`) wird durch ein vertikales Grouping in zwei getrennte Segmente aufgeteilt, die jeweils eigene Enden erhalten (z.B. `╶────╴` auf jeder Seite).
- Ein vertikaler Gap zwischen zwei Spalten, die Teil einer `ColSpan`-Gruppe sind, entfällt innerhalb der Gruppe.
- Borders und Gaps, die am **Rand** der Gruppe verlaufen, werden normal gezeichnet.
- Es folgt, dass die Gruppendimensionen mit darin verlaufenden Borders/Gaps entsprechend breiter/höher sind als nur die Summe ihrer Bestandteile und das auch, wenn sie eine Border/Gap komplett überdecken.

Siehe Abschnitt [3.1](#31-gaps-und-groups) für visuelle Beispiele.

### 4.6 Border Text

Text kann in jeden Bereich geschrieben werden, der durch eine `BorderPos` definiert ist — unabhängig davon, ob dort ein Border, ein Gap mit Leerzeichen, oder beides vorhanden ist. Die vorhandenen Zeichen werden überschrieben.

#### `TextAnchor` – Relative Positionierung

```rust
pub enum TextAnchor {
    /// Text beginnt am Anfang der BorderPos, offset verschiebt nach rechts/unten
    Start,
    /// Text endet am Ende der BorderPos, offset verschiebt den Endpunkt nach links/oben
    End,
}
```

Bei `BorderPos::Grid` wird **ausschließlich die obere Kante** des Rahmens beschriftet. `Start` = Text beginnt links, `End` = Text endet rechts. Bei allen anderen `BorderPos`-Varianten bezieht sich `Start`/`End` auf den Anfang bzw. das Ende der Linie (horizontal: links/rechts; vertikal: oben/unten). Bei `Spanned`-Varianten bezieht sich `Start`/`End` auf den Bereich des Spans.

#### `set_border_text` / `remove_border_text`

```rust
impl Grid {
    /// Text an einer BorderPos schreiben. Überschreibt vorhandene Zeichen (Border, Leerzeichen).
    /// Wird der durch BorderPos festgelegte Bereich überschritten, wird der Text mit … abgeschnitten.
    pub fn set_border_text(&mut self, pos: BorderPos, anchor: TextAnchor, offset: usize, text: &str);

    /// Text an einer BorderPos entfernen. Border-Zeichen und Leerzeichen werden wiederhergestellt.
    pub fn remove_border_text(&mut self, pos: BorderPos);
}
```

**Beispiel:**

```rust
// " My Header " horizontal in den Gap nach Zeile 0, 2 Zeichen von links
grid.set_border_text(BorderPos::AfterRow(0), TextAnchor::Start, 2, " My Header ");

// "Down" vertikal, beginnend am Anfang des Spaltengaps nach Spalte 1
grid.set_border_text(BorderPos::AfterCol(1), TextAnchor::Start, 0, "Down");

// Text entfernen und Border-Zeichen wiederherstellen
grid.remove_border_text(BorderPos::AfterRow(0));
```

Siehe Abschnitt [3.2](#32-borders-globale-konfiguration) für visuelle Beispiele.

### 4.7 Styling

Styling ist auf mehreren Ebenen konfigurierbar:

#### `set_style` – Globaler Default

```rust
impl Grid {
    /// Globaler Default-Style für alle Gaps und Borders.
    pub fn set_style(&mut self, style: Style);
}
```

#### `set_border_style` – Pro-Position

```rust
grid.set_border_style(BorderPos::AfterCol(0), Style::default().fg(Color::Blue));
grid.set_border_style(BorderPos::AfterRow(1), Style::default().fg(Color::Red));
grid.set_border_style(BorderPos::Grid, Style::default().fg(Color::Yellow));
```

`set_border_style` setzt den Style für eine Position — unabhängig davon, ob dort ein Border, ein Gap mit Leerzeichen, oder beides ist. Überschreibt den globalen Default für diese Position.

#### Pro-Zellen-Styling

```rust
grid.configure_cell_style(0, 0, Style::default().bg(Color::DarkGray)); // Zelle (0,0)
```

#### Styling-Priorität

Die Prioritätsreihenfolge für das Styling eines Elements:

1. Spezifischste Konfiguration (z.B. partieller Gap, einzelne Zelle)
2. Gap-/Zell-Konfiguration
3. Globale Konfiguration
4. `Style::default()`

### 4.8 Fokus

#### Fokussierte Zelle

> **Hinweis:** Der in den ASCII-Beispielen dieser Dokumentation gezeigte Fokus-Rahmen (`▛▀▜▌▐▙▄▟`) dient **ausschließlich der Veranschaulichung**, um den Fokuswechsel zwischen Zellen sichtbar zu machen. Die Grid-Komponente rendert keinen solchen Rahmen. Fokus wird stattdessen ausschließlich an das aktive Kind-Widget weitergeleitet — wie dieses den Fokus darstellt, liegt vollständig in seiner eigenen Verantwortung.

**2×2 Grid – Zelle A fokussiert (illustrativ, 9×5 Zeichen pro Zelle):**

```
▛ ▀▀▀▀▀ ▜░░░░░░░░░
 ░░░░░░░ ░░░░░░░░░
▌░░░A░░░▐░░░░B░░░░
 ░░░░░░░ ░░░░░░░░░
▙ ▄▄▄▄▄ ▟░░░░░░░░░
▒▒▒▒▒▒▒▒▒╳╳╳╳╳╳╳╳╳
▒▒▒▒▒▒▒▒▒╳╳╳╳╳╳╳╳╳
▒▒▒▒C▒▒▒▒╳╳╳╳D╳╳╳╳
▒▒▒▒▒▒▒▒▒╳╳╳╳╳╳╳╳╳
▒▒▒▒▒▒▒▒▒╳╳╳╳╳╳╳╳╳
```

#### Keyboard-Navigation

Die Navigation wird über eine `GridKeymap` konfiguriert. Standardmäßig sind keine Shortcuts gesetzt — der Entwickler muss explizit konfigurieren. Es gibt zwei Navigationsarten:

**Bi-direktional** (links↔rechts, oben↔unten):

```rust
pub struct GridKeymap {
    /// In der aktuellen Zeile: eine Zelle nach rechts (wrappt zur ersten bei letzter)
    pub next_in_row: Option<KeyEvent>,
    /// In der aktuellen Zeile: eine Zelle nach links (wrappt zur letzten bei erster)
    pub prev_in_row: Option<KeyEvent>,
    /// In der aktuellen Spalte: eine Zelle nach unten (wrappt zur ersten bei letzter)
    pub next_in_col: Option<KeyEvent>,
    /// In der aktuellen Spalte: eine Zelle nach oben (wrappt zur letzten bei erster)
    pub prev_in_col: Option<KeyEvent>,
    /// Nächste Zelle in natürlicher Reihenfolge (Zick-Zack: zeilenweise links nach rechts).
    /// Nach der letzten Zelle kommt wieder die erste.
    pub next_cell: Option<KeyEvent>,
    /// Vorherige Zelle in natürlicher Reihenfolge.
    /// Nach der ersten Zelle kommt wieder die letzte.
    pub prev_cell: Option<KeyEvent>,
}
```

**Alle auf einmal setzen:**

```rust
grid.set_keymap(GridKeymap {
    next_in_row: Some(KeyEvent::from(KeyCode::Right)),
    prev_in_row: Some(KeyEvent::from(KeyCode::Left)),
    next_in_col: Some(KeyEvent::from(KeyCode::Down)),
    prev_in_col: Some(KeyEvent::from(KeyCode::Up)),
    next_cell: Some(KeyEvent::from(KeyCode::Tab)),
    prev_cell: Some(KeyEvent::from(KeyCode::BackTab)),
});
```

**Einzelne Shortcuts setzen:**

```rust
grid.set_key_next(KeyEvent::from(KeyCode::Right));
grid.set_key_prev(KeyEvent::from(KeyCode::Left));
grid.set_key_next_row(KeyEvent::from(KeyCode::Tab));
grid.set_key_prev_row(KeyEvent::from(KeyCode::BackTab));
grid.set_key_next_col(KeyEvent::from(KeyCode::Down));
grid.set_key_prev_col(KeyEvent::from(KeyCode::Up));
```

Gruppierte Zellen werden bei der Navigation als eine einzige Position behandelt und übersprungen.

#### Programmatische Navigation

```rust
impl Grid {
    /// Aktuelle Fokus-Position abfragen
    pub fn focused_cell(&self) -> (usize, usize);

    /// Nächste Zelle in natürlicher Reihenfolge (Zick-Zack, zyklisch)
    pub fn focus_next(&mut self);
    /// Vorherige Zelle in natürlicher Reihenfolge (Zick-Zack, zyklisch)
    pub fn focus_prev(&mut self);

    /// Eine Zelle nach rechts in der aktuellen Zeile (zyklisch)
    pub fn focus_next_in_row(&mut self);
    /// Eine Zelle nach links in der aktuellen Zeile (zyklisch)
    pub fn focus_prev_in_row(&mut self);

    /// Eine Zelle nach unten in der aktuellen Spalte (zyklisch)
    pub fn focus_next_in_col(&mut self);
    /// Eine Zelle nach oben in der aktuellen Spalte (zyklisch)
    pub fn focus_prev_in_col(&mut self);
}
```

#### Beispiel: `focus_next` in einem 2×2 Grid

Die Navigation folgt der natürlichen Reihenfolge (Zick-Zack): A → B → C → D → A → ...

```
   Start                → B                 → C                 → D

▛ ▀▀▀▀▀ ▜░░░░░░░░░  ▓▓▓▓▓▓▓▓▓▛ ▀▀▀▀▀ ▜  ▓▓▓▓▓▓▓▓▓░░░░░░░░░  ▓▓▓▓▓▓▓▓▓░░░░░░░░░
 ░░░░░░░ ░░░░░░░░░  ▓▓▓▓▓▓▓▓▓ ░░░░░░░   ▓▓▓▓▓▓▓▓▓░░░░░░░░░  ▓▓▓▓▓▓▓▓▓░░░░░░░░░
▌░░░A░░░▐░░░░B░░░░  ▓▓▓▓A▓▓▓▓▌░░░B░░░▐  ▓▓▓▓A▓▓▓▓░░░░B░░░░  ▓▓▓▓A▓▓▓▓░░░░B░░░░
 ░░░░░░░ ░░░░░░░░░  ▓▓▓▓▓▓▓▓▓ ░░░░░░░   ▓▓▓▓▓▓▓▓▓░░░░░░░░░  ▓▓▓▓▓▓▓▓▓░░░░░░░░░
▙ ▄▄▄▄▄ ▟░░░░░░░░░  ▓▓▓▓▓▓▓▓▓▙ ▄▄▄▄▄ ▟  ▓▓▓▓▓▓▓▓▓░░░░░░░░░  ▓▓▓▓▓▓▓▓▓░░░░░░░░░
▒▒▒▒▒▒▒▒▒╳╳╳╳╳╳╳╳╳  ▒▒▒▒▒▒▒▒▒╳╳╳╳╳╳╳╳╳  ▛ ▀▀▀▀▀ ▜╳╳╳╳╳╳╳╳╳  ▒▒▒▒▒▒▒▒▒▛ ▀▀▀▀▀ ▜
▒▒▒▒▒▒▒▒▒╳╳╳╳╳╳╳╳╳  ▒▒▒▒▒▒▒▒▒╳╳╳╳╳╳╳╳╳   ░░░░░░░ ╳╳╳╳╳╳╳╳╳  ▒▒▒▒▒▒▒▒▒ ░░░░░░░
▒▒▒▒C▒▒▒▒╳╳╳╳D╳╳╳╳  ▒▒▒▒C▒▒▒▒╳╳╳╳D╳╳╳╳  ▌░░░C░░░▐╳╳╳╳D╳╳╳╳  ▒▒▒▒C▒▒▒▒▌░░░D░░░▐
▒▒▒▒▒▒▒▒▒╳╳╳╳╳╳╳╳╳  ▒▒▒▒▒▒▒▒▒╳╳╳╳╳╳╳╳╳   ░░░░░░░ ╳╳╳╳╳╳╳╳╳  ▒▒▒▒▒▒▒▒▒ ░░░░░░░
▒▒▒▒▒▒▒▒▒╳╳╳╳╳╳╳╳╳  ▒▒▒▒▒▒▒▒▒╳╳╳╳╳╳╳╳╳  ▙ ▄▄▄▄▄ ▟╳╳╳╳╳╳╳╳╳  ▒▒▒▒▒▒▒▒▒▙ ▄▄▄▄▄ ▟

-> zurück auf A
▛ ▀▀▀▀▀ ▜░░░░░░░░░
 ░░░░░░░ ░░░░░░░░░
▌░░░A░░░▐░░░░B░░░░
 ░░░░░░░ ░░░░░░░░░
▙ ▄▄▄▄▄ ▟░░░░░░░░░
▒▒▒▒▒▒▒▒▒╳╳╳╳╳╳╳╳╳
▒▒▒▒▒▒▒▒▒╳╳╳╳╳╳╳╳╳
▒▒▒▒C▒▒▒▒╳╳╳╳D╳╳╳╳
▒▒▒▒▒▒▒▒▒╳╳╳╳╳╳╳╳╳
▒▒▒▒▒▒▒▒▒╳╳╳╳╳╳╳╳╳
```

Nach dem 4. Aufruf von `focus_next()` springt der Fokus zurück auf A.

#### Kind-Override-Verhalten

Das Kind-Widget bestimmt über seinen `GridChild::on_key()`-Rückgabewert, ob ein Key konsumiert wurde:

- `true` → Grid verarbeitet den Key nicht weiter
- `false` → Grid prüft, ob der Key ein Navigations-Shortcut ist

#### Fokus bei gruppierten Zellen

Wenn der Fokus von einer nicht-gruppierten Zelle auf eine gruppierte Zelle wechselt, berechnet das Grid zunächst die Zelle, die den Fokus annehmen würde (basierend auf der aktuellen Zeile/Spalte des Fokus). Der Fokus wird dann auf die gesamte gruppierte Zelle gesetzt, aber das Grid merkt sich intern die Position der berechneten Zelle.

Wenn der Fokus erneut gewechselt wird, wird anhand der gespeicherten Zellposition bestimmt, welche Zelle als Nächstes angesteuert wird. Dadurch ergibt sich ein natürliches Navigationsverhalten, das die geometrische Position des ursprünglichen Ziels respektiert.

Beispiel: In einem 2×3 Grid mit B und E gruppiert zu BE (Spalte 1, Zeilen 0–1):

1. Fokus liegt auf A (Zeile 0, Spalte 0)
2. `focus_next_in_row()` → Grid berechnet Ziel (Zeile 0, Spalte 1), erkennt dass (1, 0) Teil von BE ist → Fokus auf BE, gespeicherte Position: (Zeile 0, Spalte 1)
3. Erneut `focus_next_in_row()` → ausgehend von gespeicherter Position (Zeile 0, Spalte 1) → nächste Zelle in Zeile 0 ist C (Zeile 0, Spalte 2)

```
  Start                → BE                → C

▛ ▀▀▀▀▀ ▜░░░░░░░░░▒▒▒▒▒▒▒▒▒  ▓▓▓▓▓▓▓▓▓▛ ▀▀▀▀▀ ▜▒▒▒▒▒▒▒▒▒  ▓▓▓▓▓▓▓▓▓░░░░░░░░░▛ ▀▀▀▀▀ ▜
 ░░░░░░░ ░░░░░░░░░▒▒▒▒▒▒▒▒▒  ▓▓▓▓▓▓▓▓▓ ░░░░░░░ ▒▒▒▒▒▒▒▒▒  ▓▓▓▓▓▓▓▓▓░░░░░░░░░ ░░░░░░░
▌░░░A░░░▐░░░░░░░░░▒▒▒▒C▒▒▒▒  ▓▓▓▓A▓▓▓▓▌░░░░░░░▐▒▒▒▒C▒▒▒▒  ▓▓▓▓A▓▓▓▓░░░░░░░░░▌░░░C░░░▐
 ░░░░░░░ ░░░░░░░░░▒▒▒▒▒▒▒▒▒  ▓▓▓▓▓▓▓▓▓▌░░░░░░░▐▒▒▒▒▒▒▒▒▒  ▓▓▓▓▓▓▓▓▓░░░░░░░░░ ░░░░░░░
▙ ▄▄▄▄▄ ▟░░░░░░░░░▒▒▒▒▒▒▒▒▒  ▓▓▓▓▓▓▓▓▓▌░░░░░░░▐▒▒▒▒▒▒▒▒▒  ▓▓▓▓▓▓▓▓▓░░░░░░░░░▙ ▄▄▄▄▄ ▟
╳╳╳╳╳╳╳╳╳░░░BE░░░░▓▓▓▓▓▓▓▓▓  ╳╳╳╳╳╳╳╳╳▌░░BE░░░▐▓▓▓▓▓▓▓▓▓  ╳╳╳╳╳╳╳╳╳░░░BE░░░░▓▓▓▓▓▓▓▓▓
╳╳╳╳╳╳╳╳╳░░░░░░░░░▓▓▓▓▓▓▓▓▓  ╳╳╳╳╳╳╳╳╳▌░░░░░░░▐▓▓▓▓▓▓▓▓▓  ╳╳╳╳╳╳╳╳╳░░░░░░░░░▓▓▓▓▓▓▓▓▓
╳╳╳╳D╳╳╳╳░░░░░░░░░▓▓▓▓F▓▓▓▓  ╳╳╳╳D╳╳╳╳▌░░░░░░░▐▓▓▓▓F▓▓▓▓  ╳╳╳╳D╳╳╳╳░░░░░░░░░▓▓▓▓F▓▓▓▓
╳╳╳╳╳╳╳╳╳░░░░░░░░░▓▓▓▓▓▓▓▓▓  ╳╳╳╳╳╳╳╳╳ ░░░░░░░ ▓▓▓▓▓▓▓▓▓  ╳╳╳╳╳╳╳╳╳░░░░░░░░░▓▓▓▓▓▓▓▓▓
╳╳╳╳╳╳╳╳╳░░░░░░░░░▓▓▓▓▓▓▓▓▓  ╳╳╳╳╳╳╳╳╳▙ ▄▄▄▄▄ ▟▓▓▓▓▓▓▓▓▓  ╳╳╳╳╳╳╳╳╳░░░░░░░░░▓▓▓▓▓▓▓▓▓
```

Hinweis: Der Fokus-Rahmen einer gruppierten Zelle erstreckt sich über die gesamte Höhe der gruppierten Zelle. Die Lücken zwischen Rahmen und Seitenrahmen verwenden das gleiche Muster wie bei nicht-gruppierten Zellen (`░░░░░░░` — Leerzeichen an den Rändern, Interior-BG im Innenraum).

Gleiches Beispiel, aber Fokus startet auf D (Zeile 1, Spalte 0): `focus_next_in_row()` berechnet Ziel (Zeile 1, Spalte 1) → Fokus auf BE, gespeicherte Position: (Zeile 1, Spalte 1) → erneut `focus_next_in_row()` → nächste Zelle in Zeile 1 ist F:

```
  Start                → BE                → F

▓▓▓▓▓▓▓▓▓░░░░░░░░░▒▒▒▒▒▒▒▒▒  ▓▓▓▓▓▓▓▓▓▛ ▀▀▀▀▀ ▜▒▒▒▒▒▒▒▒▒  ▓▓▓▓▓▓▓▓▓░░░░░░░░░▒▒▒▒▒▒▒▒▒
▓▓▓▓▓▓▓▓▓░░░░░░░░░▒▒▒▒▒▒▒▒▒  ▓▓▓▓▓▓▓▓▓ ░░░░░░░ ▒▒▒▒▒▒▒▒▒  ▓▓▓▓▓▓▓▓▓░░░░░░░░░▒▒▒▒▒▒▒▒▒
▓▓▓▓A▓▓▓▓░░░░░░░░░▒▒▒▒C▒▒▒▒  ▓▓▓▓A▓▓▓▓▌░░░░░░░▐▒▒▒▒C▒▒▒▒  ▓▓▓▓A▓▓▓▓░░░░░░░░░▒▒▒▒C▒▒▒▒
▓▓▓▓▓▓▓▓▓░░░░░░░░░▒▒▒▒▒▒▒▒▒  ▓▓▓▓▓▓▓▓▓▌░░░░░░░▐▒▒▒▒▒▒▒▒▒  ▓▓▓▓▓▓▓▓▓░░░░░░░░░▒▒▒▒▒▒▒▒▒
▓▓▓▓▓▓▓▓▓░░░░░░░░░▒▒▒▒▒▒▒▒▒  ▓▓▓▓▓▓▓▓▓▌░░░░░░░▐▒▒▒▒▒▒▒▒▒  ▓▓▓▓▓▓▓▓▓░░░░░░░░░▒▒▒▒▒▒▒▒▒
▛ ▀▀▀▀▀ ▜░░░BE░░░░▓▓▓▓▓▓▓▓▓  ╳╳╳╳╳╳╳╳╳▌░░BE░░░▐▓▓▓▓▓▓▓▓▓  ╳╳╳╳╳╳╳╳╳░░░BE░░░░▛ ▀▀▀▀▀ ▜
 ░░░░░░░ ░░░░░░░░░▓▓▓▓▓▓▓▓▓  ╳╳╳╳╳╳╳╳╳▌░░░░░░░▐▓▓▓▓▓▓▓▓▓  ╳╳╳╳╳╳╳╳╳░░░░░░░░░ ░░░░░░░
▌░░░D░░░▐░░░░░░░░░▓▓▓▓F▓▓▓▓  ╳╳╳╳D╳╳╳╳▌░░░░░░░▐▓▓▓▓F▓▓▓▓  ╳╳╳╳D╳╳╳╳░░░░░░░░░▌░░░F░░░▐
 ░░░░░░░ ░░░░░░░░░▓▓▓▓▓▓▓▓▓  ╳╳╳╳╳╳╳╳╳ ░░░░░░░ ▓▓▓▓▓▓▓▓▓  ╳╳╳╳╳╳╳╳╳░░░░░░░░░ ░░░░░░░
▙ ▄▄▄▄▄ ▟░░░░░░░░░▓▓▓▓▓▓▓▓▓  ╳╳╳╳╳╳╳╳╳▙ ▄▄▄▄▄ ▟▓▓▓▓▓▓▓▓▓  ╳╳╳╳╳╳╳╳╳░░░░░░░░░▙ ▄▄▄▄▄ ▟
```

> **Rendering und Event-Flow** sind in [5.2 Rendering-Pipeline](#52-rendering-pipeline) und [5.3 Event-Flow](#53-event-flow) beschrieben.

---

## 5. Technische Details

### 5.1 Layout-Algorithmus

Die verfügbare Fläche wird vor der Constraint-Berechnung um alle Gaps reduziert:

```
Verfügbare Breite für Zellen = Gesamtbreite − Σ(Gap-Breiten)
Verfügbare Höhe für Zellen  = Gesamthöhe  − Σ(Gap-Höhen)
```

Anschließend werden die Constraints (ratatui-Logik: `Length`, `Min`, `Max`, `Percentage`, `Ratio`) auf die verbleibende Fläche angewendet. Jede Zelle erhält ein `Rect`, das ihre absolute Position und Größe innerhalb des Grid-Bereichs beschreibt.

Gruppierte Zellen erhalten ein `Rect`, das alle ihre Einzelflächen sowie die Gaps zwischen ihnen umfasst.

### 5.2 Rendering-Pipeline

1. Alle Gaps und Borders werden gerendert (Hintergrundfarbe, Border-Zeichen)
2. Alle Zellen werden in natürlicher Reihenfolge gerendert (Zick-Zack: zeilenweise, spaltenweise)
3. **Ausnahme**: Die aktive (fokussierte) Zelle wird **ganz zum Schluss** gerendert

Der späte Render der fokussierten Zelle ermöglicht Overlay-Widgets (z.B. MultiChoice-Dropdowns), die über benachbarte Zellen ragen.

### 5.3 Event-Flow

```
1. KeyEvent kommt im Grid an
2. Grid leitet KeyEvent an aktives Kind: child.on_key(key)
   ├── true  → Event konsumiert. Grid macht nichts.
   └── false → Event nicht konsumiert.
3. Grid prüft eigene Keymap:
   ├── Match → Navigation ausführen
   └── Kein Match → Event ignorieren
```

Das Grid ruft `MockComponent::on()` auf Kinder **nicht** auf — nur `GridChild::on_key()`.

### 5.4 Corner-Berechnung bei Gap-Kreuzungen

Wenn sich ein horizontaler und ein vertikaler Gap kreuzen:

| Horizontaler Gap  | Vertikaler Gap    | Ergebnis                                   |
| ----------------- | ----------------- | ------------------------------------------ |
| Border            | Border            | Corner-Zeichen (aus `BorderChars`)         |
| Border            | Gap (Leerzeichen) | Horizontal: Linie geht durch (kein Corner) |
| Border            | None              | Horizontal: Linie geht durch               |
| Gap (Leerzeichen) | Border            | Vertikal: Linie geht durch (kein Corner)   |
| Gap (Leerzeichen) | Gap (Leerzeichen) | Leerzeichen                                |
| Gap (Leerzeichen) | None              | Nichts                                     |
| None              | Border            | Vertikal: Linie geht durch                 |
| None              | Gap (Leerzeichen) | Nichts                                     |
| None              | None              | Nichts                                     |

**Corner-Zeichen-Auswahl**: Wenn beide Gaps Borders haben, wird das Corner-Zeichen basierend auf den `BorderChars` bestimmt. Bei unterschiedlichen `BorderChars` wird der Corner des horizontalen Gaps verwendet (bzw. konfigurierbar).

### 5.5 Gap-Breite und Platzberechnung

Jeder Gap nimmt genau 1 Zeichen Breite (vertikal) bzw. 1 Zeichen Höhe (horizontal) ein. Ein fehlender Gap (`remove_gap`) nimmt 0 Zeichen ein.

Die Platzberechnung berücksichtigt alle Gaps, bevor die verbleibende Fläche auf die Zellen aufgeteilt wird:

```
Gesamtbreite = Gap_0 + Zelle_0 + Gap_1 + Zelle_1 + ... + Gap_n-1 + Zelle_n-1
```

### 5.6 Groups und Gaps

Wenn Zellen zusammengefasst sind, werden Gaps, die **innerhalb** des zusammengefassten Bereichs liegen, nicht gezeichnet. Gaps, die am **Rand** des zusammengefassten Bereichs liegen, werden normal gezeichnet.

```
Normal:
┌───┬───┬───┐
│ A │ B │ C │
└───┴───┴───┘

A+B gruppiert (ColSpan):
┌───────┬───┐
│ A + B │ C │
└───────┴───┘
       ↑
  Gap zwischen Spalte 1 und 2 bleibt erhalten
  Gap zwischen Spalte 0 und 1 wird nicht gezeichnet
```

---

## 6. Zukunfts-Ideen

Die folgenden Ideen werden **nicht** in die erste Version aufgenommen, sind aber für zukünftige Versionen denkbar:

- **Mouse-Support**: Klick für Fokuswechsel, Drag für Resize
- **Runtime-Resize mit Keyboard**: Vordefinierte Shortcuts zum Ändern von Constraints zur Laufzeit
- **Zeilen/Spalten ausblenden**: Dynamisches Verbergen von Zeilen oder Spalten
- **Cell-Header/Labels**: Konfigurierbare Titel pro Zelle (oben oder links)
- **Overflow-Verhalten**: Konfigurierbares Verhalten wenn Zellinhalt größer als der zugewiesene Platz (Truncate, Wrap, Scroll)
- **Sticky Rows/Columns**: Fixierte Kopfzeilen/-spalten bei großen Grids
- **Animation**: Animierte Übergänge bei Fokuswechsel oder Group-Änderungen
- **Accessibility**: Screen-Reader-Unterstützung, konfigurierbare Labels
- **Gap-Styles pro Zeile/Spalte**: Verschiedene Styles für unterschiedliche Zeilen oder Spalten

---

## Anhang A: KI-Instruktionen (für zukünftige KI-Sessions)

Dieser Abschnitt enthält Konventionen und Referenzen, die für die KI-gestützte Weiterarbeit an diesem Dokument wichtig sind.

### ASCII/Unicode Grid-Konventionen

- **Zellgrößen**: Normal = 7×3 Zeichen pro Zelle. Fokus-Beispiele = 9×5 Zeichen pro Zelle.
- **Spaltenanzahl**: Immer ungerade Anzahl Spalten.
- **Hintergrund-Zeichen**: Zyklen pro Zelle von links nach rechts, oben nach unten: ▓ → ░ → █. In Fokus-Beispielen: ▓, ░, ▒, ╳ (keine zwei benachbarten Zellen teilen denselben Hintergrund).
- **Fokus-Rahmen**: ▛(U+259B) ▀(U+2580) ▜(U+259C) ▙(U+2599) ▄(U+2584) ▟(U+259F) ▌(U+258C) ▐(U+2590) — immer diese exakten Codepoints verwenden, nicht ╛(U+255B), ╙(U+2559), ╒(U+2552) etc.
- **Gap-Konzept**: Es gibt kein `GapType`-Enum. Eine Gap-Position hat zwei unabhängige Zustände: _Gap vorhanden_ (ja/nein, je 0 oder 1 Zeichen) und _Border gesetzt_ (ja/nein, belegt denselben 1-Zeichen-Raum). `set_gap` setzt den Raum, `set_border` füllt ihn mit Zeichen (und setzt ihn ggf. implizit). Default ohne `set_gap`: kein Gap.
- **Border-Half-Endings**: Borders haben standardmäßig Half-Endings (╷/╵/╶/╴). `BORDER_SIMPLE_EXTENDED` / `BORDER_DOUBLE_EXTENDED` haben Full-Endings. Für ║ gibt es kein Half-Ending → `BORDER_DOUBLE_EXTENDED` ist die einzige Option für Double.
- **Auto-Join**: Gleiche Border-Typen, die aufeinandertreffen, werden automatisch verbunden (z.B. ─ + │ → ┼). Verschiedene Border-Typen werden NICHT verbunden.
- **Pixel-Perfect**: Jede Zeile in einem Code-Block muss exakt dieselbe Länge haben. Niemals Hand-Schreiben — immer Python-Scripts verwenden.

### Python-Scripts

Scripts liegen unter `ai/scripts/`. Vor jedem Grid-Beispiel das entsprechende Script ausführen und mit Assertions verifizieren (alle Zeilen gleiche Länge, korrekte Unicode-Codepoints).

| Script                              | Zweck                                                                         |
| ----------------------------------- | ----------------------------------------------------------------------------- |
| `focus_grids.py 2x2`                | 2×2 Grid, 9×5 Zellen, 4 Fokus-Zustände (A/B/C/D), 78 Zeichen breit            |
| `focus_grids.py 2x3_grouped`        | 2×3 Grid, 9×5 Zellen, B+E gruppiert, Fokus A/BE/C (Zeile 0), 85 Zeichen breit |
| `focus_grids.py 2x3_grouped_from_d` | 2×3 Grid, 9×5 Zellen, B+E gruppiert, Fokus D/BE/F (Zeile 1), 85 Zeichen breit |

### API-Konventionen

- `BorderChars` sind `pub static` Konstanten, kein Trait, kein Enum.
- `set_border` nimmt `&'static BorderChars` (kein Style-Parameter). Style wird separat via `set_border_style` gesetzt.
- `set_gap` nimmt keinen Style-Parameter. Style via `set_border_style`.
- Border-Syntax in Code-Beispielen: `&BORDER_SIMPLE`, `&BORDER_DOUBLE_EXTENDED`, `&BORDER_THICK_EXTENDED`, etc. (es gibt kein `BORDER_DOUBLE` ohne `_EXTENDED`).
- `set_border_text` mit `BorderPos`/`TextAnchor`, nicht `write_to_gap`.
- `CellGroup` in der API — kein "Merge"-Begriff verwenden.

### Workflow

1. Ein Beispiel nach dem anderen bearbeiten: aktuellen Zustand zeigen, korrigierten Zustand zeigen, User-Approval einholen, in Dokument schreiben.
2. User prüft Änderungen in der Datei, nicht im Chat.
3. Nichts ändern, was der User nicht explizit angefordert hat.

---

## Anhang B: Rendering-Algorithmus (Referenz)

Dieser Abschnitt beschreibt den internen Rendering-Algorithmus der Grid-Komponente. Er dient als Referenz für zukünftige Implementierungsarbeiten, insbesondere für Schritt 5 (Border-Zeichen).

### Motivation

Das zentrale Problem des alten Algorithmus war die Rendering-Reihenfolge: Gap-Zeichen wurden vor den Kindkomponenten gezeichnet. Gruppierte Zellen (mehrere Spalten zusammengefasst) hatten ein `fill_rect`, das über die gesamte Gruppenbreite inklusive der Gap-Spalte schrieb und damit bereits gezeichnete Gap-Zeichen überschrieb.

Die neue Lösung trennt _Style_ (Hintergrundfarbe) von _Zeichen_ sauber in Schritte auf und lässt Kindkomponenten — wie bisher — als letztes rendern, damit sie bei Bedarf (z.B. Dropdown-Overlays) über den Rahmen hinauswachsen können.

### Die sieben Schritte

```
1. Layout berechnen
2. (Teil von 1) Gitterdimensionen ableiten
3. Globalen Stil auf den gesamten Bereich anwenden
4. Gap-Stile anwenden
5. Border-Zeichen zeichnen
6. Gap-Texte schreiben
7. Zell-Hintergründe füllen + Kindkomponenten rendern
```

#### Schritt 1+2 — Layout

`compute_layout(grid, area)` liefert ein `GridLayout` mit:

- `row_rects[r]` / `col_rects[c]` — Rect für jede Zeile/Spalte (ohne Gap-Platz)
- `v_gap_x[i]` / `h_gap_y[i]` — x/y-Position jeder Gap-Spalte/Zeile (None wenn kein Gap)
- `has_outer` — ob der äußere Rahmen aktiv ist
- `cell_rect(r, c)` — Rect der einzelnen Zelle (aus row_rects + col_rects)
- `group_rect(r, c)` — Rect für gruppierte Zellen (umfasst alle Spalten + Gap-Spalten der Gruppe)

#### Schritt 3 — Globaler Stil

Füllt den gesamten Grid-Bereich mit Leerzeichen im `global_style`. Damit hat jeder nachfolgende Schritt eine saubere, einheitlich gestylte Leinwand.

#### Schritt 4 — Gap-Stile

Wendet den konfigurierten Stil auf alle Gap-Bereiche an:

- **Äußerer Rahmen**: obere/untere Zeile + linke/rechte Spalte (volle Breite/Höhe)
- **Vertikale Gap-Spalten**: jede v-gap-Spalte, zeilenweise (Gruppen-Unterdrückung beachten)
  - Full-Stil (für alle Zeilen)
  - Span-Stile (überschreiben full-Stil für bestimmte Zeilen)
- **Horizontale Gap-Zeilen**: jede h-gap-Zeile, spaltenweise (Gruppen-Unterdrückung beachten)
  - Full-Stil (für alle Spalten)
  - Span-Stile (überschreiben full-Stil für bestimmte Spalten)

**Gruppen-Unterdrückung**: Eine Gap-Position innerhalb einer Gruppe (d.h. zwischen den zusammengefassten Spalten/Zeilen) wird übersprungen — dort gehört der Gap-Bereich logisch zur Zelle.

#### Schritt 5 — Border-Zeichen

> **Status: Placeholder** (`render_borders` ist derzeit ein No-op.)

Soll die eigentlichen Box-Drawing-Zeichen in die Gap-Bereiche schreiben. Zu implementieren:

| Bereich                               | Zeichen                 |
| ------------------------------------- | ----------------------- |
| Äußerer Rahmen (einfach)              | `─` `│` `┌` `┐` `└` `┘` |
| V-Gap (volle Länge, mit Half-Endings) | `│` `╷` `╵`             |
| H-Gap (volle Länge, mit Half-Endings) | `─` `╶` `╴`             |
| V-Gap (extended, ohne Half-Endings)   | `│` durchgehend         |
| Kreuzungen V+H-Gap                    | `┼`                     |
| T-Stücke V-Gap + Außenrahmen          | `┬` `┴`                 |
| T-Stücke H-Gap + Außenrahmen          | `├` `┤`                 |
| Ecken Außenrahmen + Gap               | Teil des Außenrahmens   |
| Gruppen-unterdrückte Gaps             | keine Zeichen           |

Jeder Gap-Typ hat seinen eigenen `BorderChars`-Satz. Bei Span-Overrides werden Halb-Endstücke an den Span-Enden gezeichnet (z.B. `╷` oben, `╵` unten für einen vertikalen Span). Gleiche Border-Typen, die sich treffen, werden zu Kreuzungszeichen verbunden; verschiedene Typen werden nicht verbunden.

#### Schritt 6 — Gap-Texte

Schreibt Text-Overlays in die Gap-Bereiche, immer nach Schritt 5, damit Text immer über den Border-Zeichen liegt:

- Outer-Border-Titel (obere Kante, `TextAnchor`-gesteuert)
- V-Gap-Texte: full + Span-Texte (vertikale Schreibrichtung)
- H-Gap-Texte: full + Span-Texte (horizontale Schreibrichtung)

#### Schritt 7 — Zellen rendern

7a. **Hintergrund füllen** (`fill_rect`): Füllt den Zell-Bereich mit Leerzeichen im Zell-Stil. Bei gruppierten Zellen wird `group_rect` verwendet — das überschreibt bewusst den Gap-Inhalt innerhalb der Gruppe (Gap-Zeichen zwischen gruppierten Spalten/Zeilen sollen nicht sichtbar sein).

7b. **Kindkomponenten rendern**: Erst alle nicht-fokussierten Zellen, dann die fokussierte Zelle zuletzt. Diese Reihenfolge erlaubt es Kindkomponenten (z.B. Dropdown-Listen), bei Bedarf über benachbarte Zellen zu zeichnen.

Nur Gruppen-Ursprungszellen (`is_group_origin(r, c)`) werden gerendert; abhängige Zellen einer Gruppe werden übersprungen.

### Warum Stile vor Zeichen?

Buffer-Zellen in ratatui haben Stil und Zeichen getrennt. `set_style` ändert nur den Stil (Farbe, Modifikatoren), `set_char` ändert nur das Zeichen. Schritt 3–4 setzen ausschließlich Stile, Schritt 5–6 setzen ausschließlich Zeichen. Das ermöglicht z.B. einen farbigen Gap-Hintergrund, ohne das Border-Zeichen zu überschreiben.

### Gruppen-Unterdrückung im Detail

`is_inside_h_group(grid, row, v_gap_index)` — gibt `true` zurück, wenn der vertikale Gap `v_gap_index` zwischen zwei Spalten liegt, die in `row` zur selben `CellGroup` gehören. Dann wird der Gap in Schritt 4 (Stil) und Schritt 5 (Zeichen) für diese Zeile übersprungen.

`is_inside_v_group(grid, h_gap_index, col)` — analog für horizontale Gaps und Gruppen über mehrere Zeilen.
