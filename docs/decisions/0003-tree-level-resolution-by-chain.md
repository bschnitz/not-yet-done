# 0003 — Tree-Ebenen-Auflösung per `node_type_chain` statt Tiefe

- **Status:** akzeptiert, umgesetzt (`0f998c9`)
- **Datum:** 2026-06-05
- **Betrifft:** `not-yet-done-tui` (`content_view.rs` —
  `build_tree_data_rows`, `current_columns`, `tree_current_actions`,
  `tree_active_child_def`, neue `cursor_tree_level` /
  `cursor_node_type_chain`; `content_tree.rs` — Auflösungs-Helfer)

## Kontext

Der generische `ContentView` rendert hierarchische Adapter-Daten als
Baum: verschachtelte Knoten werden zu einer flachen Zeilenliste
(`TreeEntry`) abgeflacht und über `not-yet-done-table` gelayoutet. Jede
Zeile braucht zur Render-Zeit ihre **Ebene** im Baum — daraus folgen
Spalten-Set, welche Spalte das `indent+glyph+label` trägt (die
„Label-Spalte"), Aktionen und Preview-Config.

Diese Ebene wurde **zweifach** aufgelöst, und die beiden Wege konnten
sich widersprechen:

1. **Per Tiefe** (`tree_level_at_depth(depth)` und Verwandte) — ein
   Walk, der ab der Wurzel an jedem Schritt das **erste**
   tree-fortsetzende Kind nimmt (`first_tree_child`).
2. **Per `node_type_chain`** (`tree_level_for_chain(&chain)`) — der
   exakte Typ-Pfad, den jede `TreeEntry` ohnehin trägt.

Ein **Multi-Branch-Baum** mit unterschiedlich tiefen Zweigen bricht die
depth-Variante: dieselbe Tiefe bildet je Zweig auf einen **anderen** Typ
ab. Beispiel Stoat-Chat:

- Zweig A (uncategorized): `server(0) → channel(1) → message(2, Leaf)`
- Zweig B (Kategorie): `server(0) → category(1) → channel(2) → message(3)`

`tree_label_at_depth(2)` läuft Zweig A ab → `message` (kein `tree_label`)
→ `None`. Die Channels **unter einer Kategorie** sitzen aber auch auf
Tiefe 2 (Zweig B) → ihre Label-Spalte wurde nicht gefunden → die Zeilen
rendern als **Leerzeilen**.

Das Symptom trat über die Zeit auf mehreren Ebenen verschiedener Adapter
auf (Confluence `name`/`title`, dann Stoat). Der jeweilige „Fix" war die
Konvention **„`tree_label`-Keys über alle Ebenen gleich benennen"** — ein
Workaround, der nur unter der Single-Chain-Annahme hält und bei jedem
neuen, anders tiefen Zweig wieder bricht.

## Entscheidung

Die `node_type_chain` einer Zeile ist ihre **eindeutige Koordinate** im
Baum; `depth` ist eine verlustbehaftete Projektion. Daher: **Jede
Auflösung, die eine Zeile (oder die Cursor-Zeile) zur Hand hat, läuft
über die Chain, nie über die Tiefe.** Zwei zusammengehörige Teile:

1. **Single Source of Truth.** Neue `cursor_tree_level()` /
   `cursor_node_type_chain()` lösen Spalten-Set, Label-Spalte, Aktionen
   und Preview der Cursor-Zeile chain-basiert auf. `current_columns`,
   `tree_current_actions` (aktive Ebene) und `tree_active_child_def`
   wurden darauf migriert; die toten depth-Helfer `tree_label_at_depth` /
   `tree_columns_at_depth` sind entfernt.

2. **Label-Spalte als designierter Slot.** Die Label-Spalte wird
   **einmal** aus der Cursor-Ebene bestimmt (deren `tree_label` — ein
   fixer Schlüssel des aktiven Spalten-Sets). **Jede** Zeile malt ihr
   `indent+glyph+label` in genau diese Spalte, unabhängig von der eigenen
   Ebene. Weil Label-Spalte und Spalten-Set aus **derselben** Ebene
   stammen, sind sie per Konstruktion konsistent — die frühere
   Cross-Level-Key-Alignment-Konvention entfällt vollständig.

Die einzige verbleibende Invariante — `tree_label` muss ein Schlüssel der
**eigenen** Spalten der Ebene sein — erzwingt der Config-Validator bereits
(`view_config.rs`, `check_tree`/`walk_tree_child`). Der frühere stille
Fehlermodus (Leerzeile) ist damit ein lauter Config-Fehler.

## Optionen (und warum verworfen)

1. **Pro Zeile per _eigener_ Chain auflösen** (Label-Spalte =
   Spalte mit dem Key der jeweiligen Zeilen-Ebene). Behebt den
   Stoat-Fall (alle Ebenen nutzen `name`), lässt aber die Konvention für
   **heterogene** Keys bestehen: hat eine Ebene `title`, der aktive
   Spalten-Satz aber nur `name`, blankt die Zeile weiter. Verschiebt das
   Problem, statt es zu beseitigen.
2. **Konvention beibehalten + im Validator erzwingen** (alle `tree_label`
   eines Chains müssen gleich heißen). Verworfen: legt eine künstliche
   Kopplung über Ebenen fest, die fachlich nichts miteinander zu tun
   haben (warum sollte ein Channel-Label-Key einem Kategorie-Label-Key
   gleichen?), und schränkt heterogene Bäume unnötig ein.
3. **Eigene Tabelle/Spalten-Geometrie pro Tiefe** rendern. Verworfen:
   großer Umbau der Tabellen-Schicht; der eigentliche Fehler war die
   Auflösung, nicht das Single-Table-Layout.

## Konsequenzen

- Die frühere Konvention „`tree_label`-Keys müssen über Ebenen alignen"
  ist **obsolet**. Multi-Branch-Bäume mit unterschiedlich tiefen Zweigen und
  divergenten Label-Keys rendern korrekt (Stoat `Server → Kategorie →
Channel`, Postgres `Schemas`/`Scripts`, Confluence-Seitenbäume).
- Sichtbare Konsequenz von Teil 2: In Bäumen mit **unterschiedlichen**
  Label-Keys pro Ebene wandert die Label-**Spaltenposition** mit dem
  Cursor, wenn er die Ebene wechselt. Das ist konsistent mit dem schon
  bestehenden Verhalten (der Header/Spalten-Satz wechselt ohnehin pro
  Cursor-Ebene) und wurde bewusst akzeptiert.
- Regressionstest `tree_renders_deep_branch_label_despite_divergent_keys`
  baut einen Multi-Branch-Baum (ungleich tief, Keys `name` vs `title`)
  und prüft, dass die tiefen Zweig-Zeilen nicht-leere Labels rendern —
  verifiziert gegen die alte depth-Auflösung (schlug dort fehl).
- **Bewusst belassen:** `tree_self_at_depth` im Tree-Find-Walker
  (`tree_find_dispatch_step`) und der `current_children`-Fallback bleiben
  depth-basiert — der Walker hat dort keinen `node_type_chain` zur Hand,
  und beides ist nicht Teil des Render-Pfads. Falls Tree-Find auf
  Multi-Branch-Bäumen einmal falsch springt, ist das die nächste Stelle.
