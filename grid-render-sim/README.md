# grid-render-sim

Simulation und Testbed für die Render-Funktion der `Grid`-Komponente des
`not-yet-done-ratatui`-Projekts. Die Simulation erzeugt die Gerüst-Ausgabe
(Gaps, Borders, Zellhintergründe) als `Vec<String>`, ohne einen echten
ratatui-Buffer oder ein Terminal zu benötigen.

---

## Ziel

Die Grid-Render-Logik ist komplex: Gaps unterbrechen Borders, Gruppen
unterdrücken Gap-Zeichen, Kreuzungspunkte erfordern passende Corner-Zeichen,
und Spanned-Borders erzeugen Half-Endings. All das lässt sich in einem
String-Buffer weit einfacher entwickeln und debuggen als direkt in einem
ratatui-`Buffer`.

Das Ziel dieser Simulation ist es, den gesamten Render-Algorithmus für
Gaps und Borders — inklusive aller Sonderfälle — vollständig korrekt zu
implementieren und pixel-genau gegen die Spezifikation zu testen, bevor
der Code in die eigentliche `MockComponent`-Implementierung übertragen wird.

---

## Architektur

```
src/
  lib.rs       Öffentliche API, Test-Modul
  types.rs     Alle Datentypen: BorderChars, BorderPos, GapPos,
               CellGroup, TextAnchor, GridConfig, GapSlot,
               BorderText, SpannedBorder
  layout.rs    Layout-Berechnung (GridLayout, compute_layout)
  render.rs    Render-Pipeline: render_gaps_and_borders,
               render_with_cells
```

### Render-Pipeline (6 Schritte)

```
1. Layout berechnen          → Koordinaten für alle Zellen und Gaps
2. Puffer mit Leerzeichen    → saubere Ausgangslage
2a. Zellhintergründe füllen  → ▓ ░ █ Zyklus pro Zelle (nur render_with_cells)
3. Horizontale Linien        → ─, Half-Endings ╶/╴, Extended ─ durchgehend
4. Vertikale Linien          → │, Half-Endings ╷/╵, Extended │ durchgehend
5. Kreuzungen und Ecken      → ┼ ┬ ┴ ├ ┤ ┌ ┐ └ ┘, aus Kontext berechnet
6. Border-Texte              → überschreibt alles darunter
```

Wichtig: Der äußere Rahmen (`draw_outer_frame`) wird als **erstes** in
Schritt 3/4 gezeichnet, damit innere Borders ihre Half-Endings darüber
schreiben können.

---

## Was wurde bereits erreicht

### Implementiert und getestet

- **Äußerer Rahmen** (`BorderPos::Grid`) mit allen vordefinierten Styles:
  `BORDER_SIMPLE`, `BORDER_ROUNDED`, `BORDER_DOUBLE_EXTENDED`,
  `BORDER_THICK_EXTENDED`
- **Vollständige innere Borders** (`AfterCol`, `AfterRow`, `BeforeCol`,
  `BeforeRow`) mit Half-Endings und Extended-Varianten
- **Kreuzungen** bei gleichen Styles (`┼`), kein Join bei verschiedenen Styles
- **T-Pieces** (`┬`, `┴`, `├`, `┤`) wo innere Borders auf gleich-style
  äußeren Rahmen treffen
- **Spanned Borders** (`AfterColSpanned`, `AfterRowSpanned` etc.) mit
  Half-Endings an den Span-Grenzen
- **Spanned Crossings**: Schnittpunkte zwischen Spanned H- und V-Borders,
  zwischen Full- und Spanned-Borders
- **Nur-Gap-Positionen** (Leerzeichen, kein Border-Char)
- **Border-Texte**: horizontal und vertikal, `TextAnchor::Start`/`End`,
  Offset, Truncation mit `…`
- **Zellhintergründe**: `▓ ░ █` Zyklus nach Formel `(2*row + col) % 3`,
  sodass keine zwei benachbarten Zellen (horizontal oder vertikal) denselben
  Hintergrund haben
- **Half-Ending-Logik**: Half-Endings werden gesetzt wenn kein äußerer Rahmen
  vorhanden ist, oder wenn der äußere Rahmen einen anderen Style hat als die
  innere Border (kein Join möglich → keine T-Pieces → Half-Endings bleiben)

### Vordefinierte BorderChars-Konstanten

| Name                   | Horizontal | Vertikal | Ecken         | Half-Enden     |
|------------------------|-----------|----------|---------------|----------------|
| `BORDER_SIMPLE`        | `─`       | `│`      | `┌┐└┘`        | `╷╵╶╴`         |
| `BORDER_SIMPLE_EXTENDED` | `─`     | `│`      | `┌┐└┘`        | `│││─`         |
| `BORDER_DOUBLE_EXTENDED` | `═`     | `║`      | `╔╗╚╝`        | (voll)         |
| `BORDER_THICK_EXTENDED`  | `━`     | `┃`      | `┏┓┗┛`        | (voll)         |
| `BORDER_ROUNDED`         | `─`     | `│`      | `╭╮╰╯`        | `╷╵╶╴`         |
| `BORDER_ROUNDED_EXTENDED`| `─`     | `│`      | `╭╮╰╯`        | (voll)         |
| `BORDER_DASHED`          | `┄`     | `┆`      | `┌┐└┘`        | `╷╵╶╴`         |
| `BORDER_DASHED_EXTENDED` | `┄`     | `┆`      | `┌┐└┘`        | (voll)         |
| `BORDER_DOTTED`          | `┈`     | `┊`      | `┌┐└┘`        | `╷╵╶╴`         |
| `BORDER_DOTTED_EXTENDED` | `┈`     | `┊`      | `┌┐└┘`        | (voll)         |

---

## Was noch fehlt / getestet werden muss

### Gruppierte Zellen (CellGroup) — noch nicht implementiert

Dies ist der nächste große Schritt. Laut Spezifikation:

- Gaps und Borders **innerhalb** einer Gruppe werden nicht gerendert
- Das Gruppen-Rect umfasst alle Mitgliedszellen **inklusive** der
  internen Gap-Spalten/-Zeilen
- Zellhintergründe werden für die gesamte Gruppenflache gezeichnet
  (Hintergrundfarbe der ersten Zelle oben-links)
- Bei der Navigation wird die Gruppe als eine einzige Position behandelt

Zu testende Szenarien:

- `CellGroup::Row(r)` — ganze Zeile als eine Zelle
- `CellGroup::Col(c)` — ganze Spalte als eine Zelle
- `CellGroup::ColSpan { row, first_col, last_col }` — mehrere Spalten in
  einer Zeile
- `CellGroup::RowSpan { col, first_row, last_row }` — mehrere Zeilen in
  einer Spalte
- `CellGroup::Span { ... }` — rechteckiger Bereich
- Gruppenränder mit und ohne Border/Gap
- Eine durchgehende Border die durch eine Gruppe unterbrochen wird
  (soll in zwei Segmente mit eigenen Half-Endings aufgeteilt werden)
- Schachtelungsverbot: partielle Überschneidung zweier Gruppen → Panic
- Vollständige Umschließung: größere Gruppe gewinnt

### Weitere offene Punkte

- `BORDER_DASHED`/`BORDER_DOTTED` visuell testen (noch kein Test dafür)
- `GapPos::Grid` kombiniert mit Borders an einzelnen Positionen
- Benutzerdefinierte `BorderChars` (eigene `pub static`)
- `set_border_text` auf Spanned-Positionen (existiert im Code, kein Test)
- `remove_border` / `remove_gap` / `ungroup_cells` (noch nicht implementiert)
- Sehr große Grids (Performance, kein funktionaler Bug erwartet)
- Grids mit `Constraint::Percentage`, `Min`, `Max`, `Ratio` statt nur
  `Length` (Layout-Engine wird korrekt verwendet, aber nie getestet)

---

## Tests schreiben und durchführen

### Ausführen

```sh
# Alle Tests, nur Failures mit Output:
cargo test 2>&1

# Einzelnen Test mit vollständiger Ausgabe:
cargo test test_name -- --nocapture 2>&1

# Alle Tests mit vollständiger Ausgabe (inkl. erfolgreiche):
cargo test -- --nocapture 2>&1
```

### Struktur eines Tests

```rust
#[test]
fn test_my_scenario() {
    let mut cfg = make_3x3(7, 3);  // 3 Zeilen × 3 Spalten, je 7×3 Zeichen
    cfg.apply_border_pos(&BorderPos::Grid, &BORDER_SIMPLE);
    cfg.apply_border_pos(&BorderPos::AfterCol(0), &BORDER_SIMPLE);
    // ...weitere Konfiguration...

    assert_grid("my_scenario", &render(&cfg), &[
        "┌───────┬──────────────┐",
        "│▓▓▓▓▓▓▓│░░░░░░░███████│",
        // ...eine Zeile pro Terminal-Zeile...
        "└───────┴──────────────┘",
    ]);
}
```

### Hilfsfunktionen

```rust
make_3x3(col_len, row_len)   // 3×3 GridConfig mit Length-Constraints
make_3x5(col_len, row_len)   // 3×5 GridConfig (3 Zeilen, 5 Spalten)
render(&cfg)                  // → Vec<String> mit Zellhintergründen
assert_grid(label, lines, expected)  // pixel-genaue Zeilenvergleiche
print_grid(label, lines)     // gibt das Grid auf stdout aus
```

### Neue expected Strings ermitteln

Wenn eine Änderung die Ausgabe verändert (z.B. neue Zellhintergrund-Formel),
alle Tests einmal mit `--nocapture` laufen lassen und die tatsächliche Ausgabe
als neue expected Strings übernehmen:

```sh
cargo test -- --nocapture 2>&1 | grep -A 30 "── mein_test ──"
```

---

## Besonderheiten und Fallstricke

### Zellhintergrund-Formel

```rust
CELL_BG[(2 * row + col) % 3]  // ▓=0  ░=1  █=2
```

Der Faktor `2` stellt sicher dass benachbarte Zellen (horizontal **und**
vertikal) immer verschiedene Hintergründe haben, unabhängig von der
Spaltenanzahl. Bei `(row * cols + col) % 3` würde bei `cols % 3 == 0`
jede Spalte eine einheitliche Farbe bekommen.

### Half-Ending-Logik

Eine innere Border bekommt Half-Endings (`╷╵╶╴`) wenn:

- kein äußerer Rahmen vorhanden ist, **oder**
- der äußere Rahmen einen anderen Style hat (kein Join → kein T-Piece)

Bei gleichem Style überschreibt `draw_crossings` die Half-Endings mit
T-Pieces (`┬`, `┴`, `├`, `┤`). Daher ist die Reihenfolge entscheidend:
äußerer Rahmen zuerst zeichnen, dann innere Borders, dann Crossings.

### Same-Style-Vergleich

```rust
fn same_style(a: &BorderChars, b: &BorderChars) -> bool {
    std::ptr::eq(a, b) || (a.horizontal == b.horizontal && a.vertical == b.vertical)
}
```

`BORDER_SIMPLE` und `BORDER_ROUNDED` haben identische `─`/`│`-Chars aber
verschiedene Ecken. Sie gelten als "same style" für Join-Zwecke — das ist
korrekt, da T-Pieces und Crossings nur `─`/`│` und `cross` verwenden,
nicht die Ecken.

### Gap-Zeilen haben keine Zellhintergründe

Eine Gap-Zeile (horizontal) oder Gap-Spalte (vertikal) gehört zu keiner
Zelle. Der Buffer bleibt dort nach `fill_cell_backgrounds` leer (Spaces).
Nur die Border-Zeichen füllen diese Bereiche. Das bedeutet: bei einem
Spanned Border der nur einen Teil einer Gap-Zeile abdeckt, bleiben die
restlichen Positionen in dieser Zeile Spaces — auch wenn eine Zelle
"daneben" liegt.

### `Before*` ist Alias für `After*(i-1)`

`BorderPos::BeforeCol(i)` ist identisch mit `AfterCol(i-1)`.
`apply_border_pos` normalisiert intern auf `After*`-Indizes.

### `GapPos::Grid` vs. `BorderPos::Grid`

- `GapPos::Grid` → setzt Gaps zwischen **allen inneren** Spalten und Zeilen
  (kein äußerer Rahmen)
- `BorderPos::Grid` → setzt einen geschlossenen **äußeren Rahmen**

Diese haben völlig verschiedene Semantik trotz identischen Namens.

### `set_border` impliziert `set_gap`

Ein Border braucht immer einen Gap-Slot (1 Zeichen Platz). `set_border`
erzeugt diesen implizit. `remove_border` entfernt nur die Zeichen, der
Gap-Slot (Leerzeichen) bleibt. `remove_gap` entfernt beides.

### Reihenfolge in `draw_all`

```rust
draw_outer_frame(...)      // äußerer Rahmen zuerst
draw_horizontal_lines(...) // innere H-Linien + Spanned H
draw_vertical_lines(...)   // innere V-Linien + Spanned V
draw_crossings(...)        // Kreuzungen überschreiben Enden
draw_border_texts(...)     // Texte zuletzt, überschreiben alles
```

Diese Reihenfolge ist nicht beliebig. Insbesondere:
- Outer frame vor inneren Linien → Half-Endings können outer frame
  überschreiben (bei different-style)
- Crossings nach den Linien → T-Pieces überschreiben Half-Endings
  (bei same-style)
- Texte ganz am Ende → immer sichtbar, unabhängig von Border-Chars

---

## Instruktionen für KI-Assistenten

### Allgemein

- **Immer vollständige Funktionen liefern**, keine Diffs mit `// ...rest
  bleibt gleich`. Der Nutzer trägt den Code manuell ein.
- **`cargo check` oder `cargo test` nach jeder Änderung** — der Nutzer
  führt diese aus und liefert den Output zurück.
- **Nie raten was die Ausgabe sein wird** — wenn expected Strings unklar
  sind, den Test erst ohne expected laufen lassen und die tatsächliche
  Ausgabe übernehmen.
- **Keine Änderungen an funktionierenden Tests** ohne explizite Anfrage.

### Neue Tests hinzufügen

1. Test schreiben mit vorläufigen expected Strings (aus Kopfrechnen oder
   aus der Spec)
2. `cargo test test_name -- --nocapture` ausführen lassen
3. Tatsächliche Ausgabe als expected Strings übernehmen
4. Erst dann weitere Tests hinzufügen

### Debugging-Workflow

Wenn ein Test unerwartet fehlschlägt:

1. `cargo test test_name -- --nocapture` für isolierten Output
2. `got`/`want` in der Fehlermeldung vergleichen
3. Bei Layoutproblemen: `render_debug` Hilfsfunktion einbauen die
   `col_x`, `col_w`, `row_y`, `row_h`, `v_gap_x`, `h_gap_y` ausgibt
4. Nie blind raten — lieber eine gezielte Debug-Ausgabe anfordern

### CellGroup (noch zu implementieren)

Bei der Implementierung von Gruppen gelten folgende Regeln laut Spec:

- `is_inside_h_group(grid, row, v_gap_index)` → prüft ob der vertikale
  Gap zwischen zwei Spalten liegt, die in `row` zur selben Gruppe gehören
- `is_inside_v_group(grid, h_gap_index, col)` → analog für horizontale Gaps
- Gruppen-Unterdrückung gilt für Schritt 4 (Stil) **und** Schritt 5 (Zeichen)
- Das Gruppen-Rect (`group_rect`) umfasst alle Zellen **plus** die Gap-Spalten
  dazwischen — diese werden in Schritt 7a mit `fill_rect` überschrieben,
  wodurch Gap-Zeichen innerhalb der Gruppe verschwinden

### Dateipfade immer angeben

Jeder Code-Block muss mit dem vollständigen Dateipfad beginnen, z.B.:

```rust
// grid-render-sim/src/render.rs
```

### Shell-Befehle in Nushell-Syntax

Der Nutzer verwendet Nushell. Keine `&&`-Verkettung in direkten
Shell-Befehlen. Shell-Skript-**Dateien** dürfen sh/bash-kompatibel sein.

---

## Abhängigkeiten

```toml
[dependencies]
ratatui-core = "0.1.0"
```

Nur `ratatui-core` — kein volles ratatui, kein crossterm, kein tui-realm.
Die Simulation ist vollständig terminal-unabhängig und läuft als reine
Unit-Tests ohne UI-Initialisierung.
