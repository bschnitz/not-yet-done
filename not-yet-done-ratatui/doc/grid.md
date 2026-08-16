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

The grid is a `Component`-based layout component (tuirealm 4; the trait was called `MockComponent` up to tuirealm 3) that arranges any number of child components in an n×m raster.

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
pub trait GridChild: tuirealm::component::Component {
    /// Returns `true` if the key was consumed by the child.
    /// Returns `false` if the key was not handled — the grid then checks it as a navigation key.
    fn on_key(&mut self, key: KeyEvent) -> bool;
}
```

Every component inserted into a grid cell must implement `GridChild`. The `Component` supertrait is what the grid uses for `view()`, `attr()`/`state()` and `perform()`. The grid never calls `AppComponent::on()` on child components — keyboard routing runs exclusively through `on_key()`. `on()` is only needed if the component is also to be used outside a grid in the tui-realm event loop.

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

**Extended vs. non-extended:** extended variants use full ends — the lines run all the way to the edge. Non-extended variants use half ends.

Note: `Double` and `Thick` exist only as extended variants, because Unicode has no half ends for `║`/`═` and `┃`/`━`. `Dashed`/`Dotted` use simple characters for corners and half ends, because no dashed/dotted variants of those exist.

#### `set_border` – setting and removing borders

```rust
impl Grid {
    /// Set the border at a position (overwrites an existing border).
    /// Implicitly creates a gap if none exists at that position.
    pub fn set_border(&mut self, pos: BorderPos, border: &'static BorderChars);

    /// Remove the border at a position.
    /// The gap remains, filled with spaces.
    pub fn remove_border(&mut self, pos: BorderPos);

    /// Set the style for a border/gap position.
    pub fn set_border_style(&mut self, pos: BorderPos, style: Style);
}
```

**Example:**

```rust
// Global simple frame
grid.set_border(BorderPos::Grid, &BORDER_SIMPLE);

// Vertical border after column 1, only in rows 1-2, with a style
grid.set_border(
    BorderPos::AfterColSpanned { col: 1, row_start: 1, row_end: 2 },
    &BORDER_ROUNDED,
);
grid.set_border_style(
    BorderPos::AfterColSpanned { col: 1, row_start: 1, row_end: 2 },
    Style::default().fg(Color::Cyan),
);

// Remove a horizontal border (the gap remains as spaces)
grid.remove_border(BorderPos::BeforeRow(2));
```

#### Auto-join

When two borders of the same type cross, the matching corner character is used automatically (e.g. `─` + `│` → `┼` for `BORDER_SIMPLE`). Borders of different types are not joined and keep their own ends.

#### Custom BorderChars

```rust
pub static BRAILLE_BORDER: BorderChars = BorderChars::new(
    '⠤', // horizontal: top and bottom dots
    '⡇', // vertical:   left dots
    '⠿', // cross:      all dots
    '⡷', // top_left:   left + bottom dots
    '⢾', // top_right:  right + top dots
    '⣇', // bottom_left: left + bottom dots
    '⣸', // bottom_right: right + top dots
    '⡇', // t_left:     left dots + towards the right
    '⢾', // t_right:    right dots + towards the left
    '⠤', // t_top:      top + bottom dots
    '⠤', // t_bottom:   top + bottom dots
    '⠂', // half_top:   single dot at the top
    '⠂', // half_bottom:single dot at the bottom
    '⠄', // half_left:  single dot on the left
    '⠄', // half_right: single dot on the right
);

grid.set_border(BorderPos::Grid, &BRAILLE_BORDER);
```

### 4.4 Gaps

Gaps define the space between cells. Every gap takes exactly 1 character of width (vertical gaps) or 1 character of height (horizontal gaps). By default there are no gaps — cells touch each other directly.

#### `GapPos` – where is a gap placed?

```rust
pub enum GapPos {
    /// Gaps between all inner columns and rows (no outer frame)
    Grid,

    /// Vertical gap after column i (between column i and i+1)
    AfterCol(usize),
    /// Vertical gap before column i (between column i-1 and i)
    BeforeCol(usize),

    /// Horizontal gap after row i (between row i and i+1)
    AfterRow(usize),
    /// Horizontal gap before row i (between row i-1 and i)
    BeforeRow(usize),
}
```

> **Note**: `GapPos::Grid` and `BorderPos::Grid` have different semantics. `GapPos::Grid` places gaps between all inner columns and rows (without an outer frame). `BorderPos::Grid` draws a closed outer frame around the whole grid. `AfterCol(i)` in `GapPos` and `AfterCol(i)` in `BorderPos` address the same physical position — `set_border(BorderPos::AfterCol(i), ...)` automatically creates a gap at that position if none exists yet.

#### `set_gap` / `remove_gap`

```rust
impl Grid {
    /// Set a gap at a position (1 character of space, filled with blanks).
    pub fn set_gap(&mut self, pos: GapPos);

    /// Remove a gap at a position entirely.
    /// Cells then touch directly. Any borders inside that gap are removed with it.
    pub fn remove_gap(&mut self, pos: GapPos);
}
```

**Example:**

```rust
// Gaps between all columns and rows
grid.set_gap(GapPos::Grid);

// Gaps between rows only
grid.set_gap(GapPos::AfterRow(0));
grid.set_gap(GapPos::AfterRow(1));

// Remove the vertical gap between column 2 and 3
grid.remove_gap(GapPos::AfterCol(2));
```

#### Interaction with borders

- `set_border` implicitly creates a gap at the position if none exists.
- `remove_border` removes only the border characters; the gap remains as spaces.
- `remove_gap` removes the whole space, including any borders inside it.
- A gap without a border is filled with spaces.
- A gap with a border shows the border characters (see section [4.3](#43-borders) for visual examples).

### 4.5 Cell Groups

Cells can be merged into larger units. A group is treated like a single cell — for layout, focus and rendering.

#### The `CellGroup` enum

```rust
pub enum CellGroup {
    /// Merge a whole row
    Row(usize),
    /// Merge a whole column
    Col(usize),
    /// Merge several columns within one row
    ColSpan { row: usize, first_col: usize, last_col: usize },
    /// Merge several rows within one column
    RowSpan { col: usize, first_row: usize, last_row: usize },
    /// Merge a rectangular area
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
    /// Merge cells into a group.
    /// The merged cells share the space without internal gaps/borders.
    pub fn group_cells(&mut self, group: CellGroup);

    /// Dissolve the group that the cell (row, col) belongs to.
    /// If the cell is not part of a group, the call has no effect.
    pub fn ungroup_cells(&mut self, row: usize, col: usize);
}
```

**Example:**

```rust
// Merge B, C, D, G, H, I into one cell (cf. section 3.1)
grid.group_cells(CellGroup::Span { first_row: 1, first_col: 1, last_row: 2, last_col: 3 });

// Dissolve the group containing cell (1, 1)
grid.ungroup_cells(1, 1);
```

#### Grouping behaviour

- The grouped cells share the combined space of all individual cells (without internal gaps/borders).
- The first cell (top left) determines the background of the grouped cell.
- A child widget is assigned the entire area of the group.
- Focus moves over the group as a whole.

#### Overlap behaviour

When `group_cells` is called and the new group overlaps an existing one, the following rules apply:

- **Full enclosure**: if the new group fully encloses an existing one (or vice versa), the smaller one is ignored — the larger group wins. `ungroup_cells` on the smaller one then has no effect any more.
- **Partial intersection**: if two groups only partially intersect (without one fully containing the other), **panic!** in debug builds. In release builds the behaviour is undefined. Partial intersections must be avoided by the caller.

#### Interaction with gaps and borders

Gaps and borders that would run **inside** a group are interrupted and not drawn. Visually it behaves as if the borders had been defined separately on either side of the group:

- A continuous horizontal border (`AfterRow(1)`) is split by a vertical grouping into two separate segments, each getting its own ends (e.g. `╶────╴` on each side).
- A vertical gap between two columns that are part of a `ColSpan` group disappears inside the group.
- Borders and gaps running along the **edge** of the group are drawn normally.
- It follows that the group's dimensions, with borders/gaps running inside it, are correspondingly wider/taller than the mere sum of its parts — and that also holds when it covers a border/gap completely.

See section [3.1](#31-gaps-and-groups) for visual examples.

### 4.6 Border text

Text can be written into any area defined by a `BorderPos` — regardless of whether it holds a border, a gap of spaces, or both. The existing characters are overwritten.

#### `TextAnchor` – relative positioning

```rust
pub enum TextAnchor {
    /// The text starts at the beginning of the BorderPos, offset shifts it right/down
    Start,
    /// The text ends at the end of the BorderPos, offset shifts the end point left/up
    End,
}
```

For `BorderPos::Grid`, **only the top edge** of the frame is labelled. `Start` = the text starts on the left, `End` = the text ends on the right. For all other `BorderPos` variants, `Start`/`End` refer to the beginning and the end of the line (horizontal: left/right; vertical: top/bottom). For the `Spanned` variants, `Start`/`End` refer to the area of the span.

#### `set_border_text` / `remove_border_text`

```rust
impl Grid {
    /// Write text at a BorderPos. Overwrites existing characters (border, spaces).
    /// If the area defined by BorderPos is exceeded, the text is truncated with ….
    pub fn set_border_text(&mut self, pos: BorderPos, anchor: TextAnchor, offset: usize, text: &str);

    /// Remove text at a BorderPos. Border characters and spaces are restored.
    pub fn remove_border_text(&mut self, pos: BorderPos);
}
```

**Example:**

```rust
// " My Header " horizontally into the gap after row 0, 2 characters from the left
grid.set_border_text(BorderPos::AfterRow(0), TextAnchor::Start, 2, " My Header ");

// "Down" vertically, starting at the beginning of the column gap after column 1
grid.set_border_text(BorderPos::AfterCol(1), TextAnchor::Start, 0, "Down");

// Remove the text and restore the border characters
grid.remove_border_text(BorderPos::AfterRow(0));
```

See section [3.2](#32-borders-global-configuration) for visual examples.

### 4.7 Styling

Styling is configurable at several levels:

#### `set_style` – global default

```rust
impl Grid {
    /// Global default style for all gaps and borders.
    pub fn set_style(&mut self, style: Style);
}
```

#### `set_border_style` – per position

```rust
grid.set_border_style(BorderPos::AfterCol(0), Style::default().fg(Color::Blue));
grid.set_border_style(BorderPos::AfterRow(1), Style::default().fg(Color::Red));
grid.set_border_style(BorderPos::Grid, Style::default().fg(Color::Yellow));
```

`set_border_style` sets the style for a position — regardless of whether it holds a border, a gap of spaces, or both. It overrides the global default for that position.

#### Per-cell styling

```rust
grid.configure_cell_style(0, 0, Style::default().bg(Color::DarkGray)); // cell (0,0)
```

#### Styling priority

The priority order for styling an element:

1. Most specific configuration (e.g. partial gap, individual cell)
2. Gap/cell configuration
3. Global configuration
4. `Style::default()`

### 4.8 Focus

#### Focused cell

> **Note:** The focus frame shown in the ASCII examples of this documentation (`▛▀▜▌▐▙▄▟`) is **purely illustrative**, to make the focus change between cells visible. The grid component renders no such frame. Focus is instead forwarded exclusively to the active child widget — how that widget displays the focus is entirely its own responsibility.

**2×2 grid – cell A focused (illustrative, 9×5 characters per cell):**

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

#### Keyboard navigation

Navigation is configured through a `GridKeymap`. By default no shortcuts are set — the developer has to configure them explicitly. There are two kinds of navigation:

**Bi-directional** (left↔right, up↔down):

```rust
pub struct GridKeymap {
    /// Within the current row: one cell to the right (wraps to the first after the last)
    pub next_in_row: Option<KeyEvent>,
    /// Within the current row: one cell to the left (wraps to the last before the first)
    pub prev_in_row: Option<KeyEvent>,
    /// Within the current column: one cell down (wraps to the first after the last)
    pub next_in_col: Option<KeyEvent>,
    /// Within the current column: one cell up (wraps to the last before the first)
    pub prev_in_col: Option<KeyEvent>,
    /// Next cell in natural order (zig-zag: row by row, left to right).
    /// After the last cell comes the first one again.
    pub next_cell: Option<KeyEvent>,
    /// Previous cell in natural order.
    /// Before the first cell comes the last one again.
    pub prev_cell: Option<KeyEvent>,
}
```

**Setting them all at once:**

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

**Setting individual shortcuts:**

```rust
grid.set_key_next(KeyEvent::from(KeyCode::Right));
grid.set_key_prev(KeyEvent::from(KeyCode::Left));
grid.set_key_next_row(KeyEvent::from(KeyCode::Tab));
grid.set_key_prev_row(KeyEvent::from(KeyCode::BackTab));
grid.set_key_next_col(KeyEvent::from(KeyCode::Down));
grid.set_key_prev_col(KeyEvent::from(KeyCode::Up));
```

During navigation, grouped cells are treated as a single position and skipped over.

#### Programmatic navigation

```rust
impl Grid {
    /// Query the current focus position
    pub fn focused_cell(&self) -> (usize, usize);

    /// Next cell in natural order (zig-zag, cyclic)
    pub fn focus_next(&mut self);
    /// Previous cell in natural order (zig-zag, cyclic)
    pub fn focus_prev(&mut self);

    /// One cell to the right in the current row (cyclic)
    pub fn focus_next_in_row(&mut self);
    /// One cell to the left in the current row (cyclic)
    pub fn focus_prev_in_row(&mut self);

    /// One cell down in the current column (cyclic)
    pub fn focus_next_in_col(&mut self);
    /// One cell up in the current column (cyclic)
    pub fn focus_prev_in_col(&mut self);
}
```

#### Example: `focus_next` in a 2×2 grid

Navigation follows the natural order (zig-zag): A → B → C → D → A → ...

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

-> back to A
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

After the 4th call to `focus_next()` the focus jumps back to A.

#### Child override behaviour

The child widget decides through the return value of its `GridChild::on_key()` whether a key was consumed:

- `true` → the grid does not process the key any further
- `false` → the grid checks whether the key is a navigation shortcut

#### Focus with grouped cells

When the focus moves from a non-grouped cell to a grouped cell, the grid first computes the cell that would take the focus (based on the current row/column of the focus). The focus is then set on the whole grouped cell, but the grid internally remembers the position of the computed cell.

When the focus moves again, the stored cell position determines which cell is targeted next. This yields a natural navigation behaviour that respects the geometric position of the original target.

Example: in a 2×3 grid with B and E grouped into BE (column 1, rows 0–1):

1. The focus is on A (row 0, column 0)
2. `focus_next_in_row()` → the grid computes the target (row 0, column 1), sees that (1, 0) is part of BE → focus on BE, stored position: (row 0, column 1)
3. `focus_next_in_row()` again → starting from the stored position (row 0, column 1) → the next cell in row 0 is C (row 0, column 2)

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

Note: the focus frame of a grouped cell spans the entire height of the grouped cell. The gaps between the frame and the side frames use the same pattern as for non-grouped cells (`░░░░░░░` — spaces at the edges, interior background inside).

Same example, but the focus starts on D (row 1, column 0): `focus_next_in_row()` computes the target (row 1, column 1) → focus on BE, stored position: (row 1, column 1) → `focus_next_in_row()` again → the next cell in row 1 is F:

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

> **Rendering and event flow** are described in [5.2 Rendering pipeline](#52-rendering-pipeline) and [5.3 Event flow](#53-event-flow).

---

## 5. Technical details

### 5.1 Layout algorithm

The available area is reduced by all gaps before the constraints are computed:

```
Available width for cells  = total width  − Σ(gap widths)
Available height for cells = total height − Σ(gap heights)
```

The constraints (ratatui logic: `Length`, `Min`, `Max`, `Percentage`, `Ratio`) are then applied to the remaining area. Every cell receives a `Rect` describing its absolute position and size within the grid area.

Grouped cells receive a `Rect` covering all of their individual areas plus the gaps between them.

### 5.2 Rendering pipeline

1. All gaps and borders are rendered (background colour, border characters)
2. All cells are rendered in natural order (zig-zag: row by row, column by column)
3. **Exception**: the active (focused) cell is rendered **last of all**

Rendering the focused cell late enables overlay widgets (e.g. MultiChoice dropdowns) that extend over neighbouring cells.

### 5.3 Event flow

```
1. A KeyEvent arrives at the grid
2. The grid forwards the KeyEvent to the active child: child.on_key(key)
   ├── true  → event consumed. The grid does nothing.
   └── false → event not consumed.
3. The grid checks its own keymap:
   ├── Match → perform navigation
   └── No match → ignore the event
```

The grid does **not** call `AppComponent::on()` on children — only `GridChild::on_key()`.

### 5.4 Corner computation at gap crossings

When a horizontal and a vertical gap cross:

| Horizontal gap | Vertical gap | Result                                        |
| -------------- | ------------ | --------------------------------------------- |
| Border         | Border       | Corner character (from `BorderChars`)         |
| Border         | Gap (spaces) | Horizontal: the line runs through (no corner) |
| Border         | None         | Horizontal: the line runs through             |
| Gap (spaces)   | Border       | Vertical: the line runs through (no corner)   |
| Gap (spaces)   | Gap (spaces) | Spaces                                        |
| Gap (spaces)   | None         | Nothing                                       |
| None           | Border       | Vertical: the line runs through               |
| None           | Gap (spaces) | Nothing                                       |
| None           | None         | Nothing                                       |

**Corner character selection**: if both gaps have borders, the corner character is determined from the `BorderChars`. With differing `BorderChars`, the corner of the horizontal gap is used (or configurable).

### 5.5 Gap width and space computation

Every gap takes exactly 1 character of width (vertical) or 1 character of height (horizontal). A missing gap (`remove_gap`) takes 0 characters.

The space computation accounts for all gaps before the remaining area is distributed across the cells:

```
Total width = gap_0 + cell_0 + gap_1 + cell_1 + ... + gap_n-1 + cell_n-1
```

### 5.6 Groups and gaps

When cells are merged, gaps lying **inside** the merged area are not drawn. Gaps lying at the **edge** of the merged area are drawn normally.

```
Normal:
┌───┬───┬───┐
│ A │ B │ C │
└───┴───┴───┘

A+B grouped (ColSpan):
┌───────┬───┐
│ A + B │ C │
└───────┴───┘
       ↑
  the gap between column 1 and 2 is preserved
  the gap between column 0 and 1 is not drawn
```

---

## 6. Future ideas

The following ideas will **not** be part of the first version, but are conceivable for future versions:

- **Mouse support**: click to change focus, drag to resize
- **Runtime resize with the keyboard**: predefined shortcuts for changing constraints at runtime
- **Hiding rows/columns**: dynamically hiding rows or columns
- **Cell headers/labels**: configurable titles per cell (top or left)
- **Overflow behaviour**: configurable behaviour when cell content is larger than the assigned space (truncate, wrap, scroll)
- **Sticky rows/columns**: fixed header rows/columns for large grids
- **Animation**: animated transitions on focus change or group changes
- **Accessibility**: screen-reader support, configurable labels
- **Gap styles per row/column**: different styles for different rows or columns

---

## Appendix A: AI instructions (for future AI sessions)

This section holds the conventions and references that matter for AI-assisted work on this document.

### ASCII/Unicode grid conventions

- **Cell sizes**: normal = 7×3 characters per cell. Focus examples = 9×5 characters per cell.
- **Column count**: always an odd number of columns.
- **Background characters**: cycle per cell from left to right, top to bottom: ▓ → ░ → █. In focus examples: ▓, ░, ▒, ╳ (no two adjacent cells share the same background).
- **Focus frame**: ▛(U+259B) ▀(U+2580) ▜(U+259C) ▙(U+2599) ▄(U+2584) ▟(U+259F) ▌(U+258C) ▐(U+2590) — always use these exact code points, not ╛(U+255B), ╙(U+2559), ╒(U+2552) etc.
- **Gap concept**: there is no `GapType` enum. A gap position has two independent states: _gap present_ (yes/no, 0 or 1 character) and _border set_ (yes/no, occupying that same 1-character space). `set_gap` creates the space, `set_border` fills it with characters (and creates it implicitly if needed). Default without `set_gap`: no gap.
- **Border half endings**: borders have half endings by default (╷/╵/╶/╴). `BORDER_SIMPLE_EXTENDED` / `BORDER_DOUBLE_EXTENDED` have full endings. There is no half ending for ║ → `BORDER_DOUBLE_EXTENDED` is the only option for double.
- **Auto-join**: identical border types meeting each other are joined automatically (e.g. ─ + │ → ┼). Different border types are NOT joined.
- **Pixel-perfect**: every line in a code block must have exactly the same length. Never write them by hand — always use Python scripts.

### Python scripts

The scripts live under `ai/scripts/`. Before every grid example, run the corresponding script and verify it with assertions (all lines the same length, correct Unicode code points).

| Script                              | Purpose                                                                    |
| ----------------------------------- | -------------------------------------------------------------------------- |
| `focus_grids.py 2x2`                | 2×2 grid, 9×5 cells, 4 focus states (A/B/C/D), 78 characters wide          |
| `focus_grids.py 2x3_grouped`        | 2×3 grid, 9×5 cells, B+E grouped, focus A/BE/C (row 0), 85 characters wide |
| `focus_grids.py 2x3_grouped_from_d` | 2×3 grid, 9×5 cells, B+E grouped, focus D/BE/F (row 1), 85 characters wide |

### API conventions

- `BorderChars` are `pub static` constants, not a trait and not an enum.
- `set_border` takes `&'static BorderChars` (no style parameter). The style is set separately via `set_border_style`.
- `set_gap` takes no style parameter. Style via `set_border_style`.
- Border syntax in code examples: `&BORDER_SIMPLE`, `&BORDER_DOUBLE_EXTENDED`, `&BORDER_THICK_EXTENDED`, etc. (there is no `BORDER_DOUBLE` without `_EXTENDED`).
- `set_border_text` with `BorderPos`/`TextAnchor`, not `write_to_gap`.
- `CellGroup` in the API — do not use the term "merge".

### Workflow

1. Work through one example at a time: show the current state, show the corrected state, get the user's approval, write it into the document.
2. The user reviews changes in the file, not in the chat.
3. Change nothing the user has not explicitly asked for.

---

## Appendix B: Rendering algorithm (reference)

This section describes the internal rendering algorithm of the grid component. It serves as a reference for future implementation work, in particular for step 5 (border characters).

### Motivation

The central problem of the old algorithm was the rendering order: gap characters were drawn before the child components. Grouped cells (several columns merged) had a `fill_rect` that wrote across the entire group width including the gap column, thereby overwriting gap characters that had already been drawn.

The new solution cleanly separates _style_ (background colour) from _characters_ into distinct steps and lets child components render last — as before — so that they can grow beyond the frame when needed (e.g. dropdown overlays).

### The seven steps

```
1. Compute the layout
2. (part of 1) Derive the grid dimensions
3. Apply the global style to the entire area
4. Apply the gap styles
5. Draw the border characters
6. Write the gap texts
7. Fill the cell backgrounds + render the child components
```

#### Steps 1+2 — layout

`compute_layout(grid, area)` returns a `GridLayout` with:

- `row_rects[r]` / `col_rects[c]` — rect for each row/column (without gap space)
- `v_gap_x[i]` / `h_gap_y[i]` — x/y position of each gap column/row (None if there is no gap)
- `has_outer` — whether the outer frame is active
- `cell_rect(r, c)` — rect of the individual cell (from row_rects + col_rects)
- `group_rect(r, c)` — rect for grouped cells (covers all columns + gap columns of the group)

#### Step 3 — global style

Fills the entire grid area with spaces in the `global_style`. Every following step therefore starts from a clean, uniformly styled canvas.

#### Step 4 — gap styles

Applies the configured style to all gap areas:

- **Outer frame**: top/bottom row + left/right column (full width/height)
- **Vertical gap columns**: each v-gap column, row by row (respecting group suppression)
  - Full style (for all rows)
  - Span styles (override the full style for specific rows)
- **Horizontal gap rows**: each h-gap row, column by column (respecting group suppression)
  - Full style (for all columns)
  - Span styles (override the full style for specific columns)

**Group suppression**: a gap position inside a group (i.e. between the merged columns/rows) is skipped — there the gap area logically belongs to the cell.

#### Step 5 — border characters

Writes the actual box-drawing characters into the gap areas. `render_borders` translates the grid into the crate-independent `GridConfig`/`GridLayout` pair and hands it, together with a buffer target, to `not_yet_done_grid_core::render::draw_borders` — which draws the outer frame, the horizontal and vertical lines, the crossings and the border texts. What ends up on screen:

| Area                                | Characters              |
| ----------------------------------- | ----------------------- |
| Outer frame (simple)                | `─` `│` `┌` `┐` `└` `┘` |
| V-gap (full length, with half ends) | `│` `╷` `╵`             |
| H-gap (full length, with half ends) | `─` `╶` `╴`             |
| V-gap (extended, without half ends) | `│` continuous          |
| V+H gap crossings                   | `┼`                     |
| T pieces, v-gap + outer frame       | `┬` `┴`                 |
| T pieces, h-gap + outer frame       | `├` `┤`                 |
| Corners, outer frame + gap          | part of the outer frame |
| Group-suppressed gaps               | no characters           |

Each gap type has its own `BorderChars` set. With span overrides, half end pieces are drawn at the span ends (e.g. `╷` at the top, `╵` at the bottom for a vertical span). Identical border types meeting each other are joined into crossing characters; different types are not joined.

#### Step 6 — gap texts

Writes text overlays into the gap areas, always after step 5, so that text always sits on top of the border characters:

- Outer border titles (top edge, driven by `TextAnchor`)
- V-gap texts: full + span texts (vertical writing direction)
- H-gap texts: full + span texts (horizontal writing direction)

#### Step 7 — render the cells

7a. **Fill the background** (`fill_rect`): fills the cell area with spaces in the cell style. For grouped cells, `group_rect` is used — this deliberately overwrites the gap content inside the group (gap characters between grouped columns/rows are not meant to be visible).

7b. **Render the child components**: first all non-focused cells, then the focused cell last. This order lets child components (e.g. dropdown lists) draw over neighbouring cells when needed.

Only group origin cells (`is_group_origin(r, c)`) are rendered; dependent cells of a group are skipped.

### Why styles before characters?

Buffer cells in ratatui keep style and character separate. `set_style` changes only the style (colour, modifiers), `set_char` changes only the character. Steps 3–4 set styles exclusively, steps 5–6 set characters exclusively. That makes e.g. a coloured gap background possible without overwriting the border character.

### Group suppression in detail

`is_inside_h_group(grid, row, v_gap_index)` — returns `true` if the vertical gap `v_gap_index` lies between two columns that belong to the same `CellGroup` in `row`. The gap is then skipped for that row in step 4 (style) and step 5 (characters).

`is_inside_v_group(grid, h_gap_index, col)` — analogous, for horizontal gaps and groups spanning several rows.
