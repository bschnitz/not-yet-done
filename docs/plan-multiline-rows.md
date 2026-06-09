# Plan: Multi-Line-Rows in der Tabellen-Engine (Chat-Layout)

> **Status: umgesetzt.** Schichten 1–5 fertig, 487 TUI- + 80 ratatui- + 32
> table-Tests grün (6 neu), release gebaut + installiert. Lokal ungepusht.
> Smoke-Test (Chat-Layout-Checkliste in `smoke-tests.md`) offen.

## Ziel

Eine logische Tabellen-Row kann als **Stapel mehrerer physischer Zeilen**
gerendert werden. Erster Anwendungsfall: die Stoat-Message-Liste als
Chat-Layout —

- Zeile 1: `author` + `time` (farblich hervorgehoben, fg)
- Zeile 2: `content` (Nachricht)
- Zeile 3: leer (Spacer)

Die heutige Tabelle ist der Spezialfall „genau eine physische Zeile mit allen
Spalten". Leitprinzip: **`height == 1` reduziert die neue Engine bit-genau auf
das alte Verhalten** → bestehende Tabs unverändert, vorhandene Widget-Tests
sind der Regressions-Wächter.

## Datenmodell (Composite)

- `RowTemplate { lines: Vec<LineTemplate> }` — Bauplan einer Row.
- `LineTemplate { columns: Vec<ColumnId>, highlight_on_select: bool }` — eine
  physische Zeile. Leere `columns` = Spacer. `highlight_on_select` default
  `true`, für Spacer `false`.
- `ComputedRow.cells: Vec<String>` → `lines: Vec<ComputedLine>`.
- `ComputedLine { cells: Vec<String>, highlights: Vec<Vec<Range>>, highlight_on_select }`.

`highlight_on_select` ist die **verallgemeinerte** Form von „diese Zeile nicht
mitselektieren" — nicht an „ist leer" gekoppelt; jede Zeile kann sich
ausklinken (Escape-Hatch im Config).

## Schicht 1 — `not-yet-done-table` (Strategy + Reuse)

Neuer `compute_multiline_table(rows, config, template, header)`. Pro
**Zeilen-Index** wird das bestehende `ColSizer::col_widths()` + `fit_aligned…`
über die Spalten _dieser_ Zeile gegen die volle Pane-Breite laufen gelassen
(vertikale Ausrichtung je Zeilen-Index bleibt erhalten). Der Single-Line-Pfad
`compute_table` bleibt **unangetastet** — Multi-Line ist daraus komponiert.

## Schicht 2 — `not-yet-done-ratatui`/table (variable Höhe, gekapselt)

- `TableWidgetRow.cells` → `lines: Vec<TableWidgetLine>`.
  `TableWidgetLine { cells: Vec<TableWidgetCell>, highlight_on_select: bool }`.
- `TableWidgetRow::new(cells)` bleibt = eine Zeile (`highlight_on_select=true`);
  neu `::multiline(lines)`.
- Helper `primary_line() -> &[TableWidgetCell]` (= `lines[0]`) und `height()`.
  Single-Line-orientierter Code (Jump-Mode, Column-Count, `compute_col_widths`)
  nutzt `primary_line()` → unverändertes Verhalten bei `height==1`.
- `render.rs`: pro Row `let h = row.height(); render lines; y += h`. Selektion
  färbt alle `h` Zeilen mit `RowSelected`-Basis — außer Zeilen mit
  `highlight_on_select == false`, die `Row`-Basis bekommen.
- `component.rs`: `scroll_offset`/`selected_row` bleiben **Row-Indizes**;
  sichtbare Rows + `adjust_scroll` über akkumulierte Zeilenhöhen. Bei
  `height==1` identisch zu heute.

**Scope-Schnitt:** Jump-Mode (`f`) und Spalten-Cursor / H-Scroll bleiben
Single-Line-only (operieren auf `primary_line()`). Der Chat-View nutzt beide
nicht (`selected_column = None`). Multi-Line + diese Features = „nicht
unterstützt", statt die Positions-Mathematik aufzubohren.

## Schicht 3 — `view_config.rs`

`ViewDef.row_layout: Option<Vec<LineLayout>>`. Fehlt → klassische Tabelle.
`LineLayout` deserialisiert aus

- Kurzform Liste `[author, time]` → `columns`, `highlight_on_select=true`
- Leere Liste `[]` → Spacer, `highlight_on_select=false`
- Map `{ columns: [...], highlight_on_select: false }` → Escape-Hatch

Validator: jede in `row_layout` referenzierte Spalte muss in `columns`
existieren.

## Schicht 4 — `content_view.rs`

Wenn `row_layout` gesetzt:

- Baue `RowTemplate` aus `row_layout` (+ `ColumnId`s).
- `compute_multiline_table(...)` statt `compute_table`.
- Per-Spalten-fg: StyleMap mit einem Eintrag je Spalte (resolved fg aus
  `ColumnDef.style` bzw. `text_med`), jede Cell bekommt `style_id` ihrer Spalte
  → `resolve_cell_fg` nutzt immer den Style-Eintrag (positionsunabhängig).
- Header unterdrücken: `headers = vec![]` an `set_data`.

## Schicht 5 — Config + Docs

- `~/.config/not_yet_done/views/stoat.yaml` (deployed) + `docs/examples/views/stoat.yaml`:
  `messages`-Branches auf `row_layout` umstellen, `author`/`time` mit `style:`.
- `docs/reference/generic-view-spec.md`: `row_layout` dokumentieren (was + warum).
- `docs/smoke-tests.md`: Chat-Layout-Checkliste.

## Reihenfolge / Verifikation

1. Schicht 1 + Tests → `cargo test -p not-yet-done-table`.
2. Schicht 2 + Bestandstests grün + neue Tests → `cargo test -p not-yet-done-ratatui`.
3. Schicht 3 + 4 → `cargo build --release`, `cargo test`.
4. Config + Docs, `prettier`, `cargo install`.
