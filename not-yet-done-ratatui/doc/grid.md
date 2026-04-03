# Grid Component – Anforderungsanalyse

> **Status**: Entwurf / Spezifikation
> **Version**: v1 (Entwurf)

---

## 1. Feature-Übersicht

Das Grid ist eine `MockComponent`-basierte Layout-Komponente, die beliebig viele Kind-Komponenten in einem n×m-Raster anordnet.

| Feature                                    | Beschreibung                                                                                                                      |
|--------------------------------------------|-----------------------------------------------------------------------------------------------------------------------------------|
| **n×m Raster**                             | Beliebig viele Zeilen und Spalten                                                                                                 |
| **Constraints**                            | Spaltenbreiten-/Zeilenhöhen-Constraints wie in ratatui (`Length`, `Min`, `Max`, `Percentage`, `Ratio`)                            |
| **Gaps / Borders**                         | Konfigurierbare Separatoren zwischen Zellen: `None` (kein Abstand), Leerzeichen, oder Border-Varianten (Simple, Double, Thick, Custom) |
| **BorderChars**                          | Vordefinierte Unicode Box-Drawing-Sets + benutzerdefinierte Sets als `pub static` |
| **3-Ebenen-Gap-Konfiguration**             | Global → pro Spalte/Zeile → partiell (Abschnitt eines Gaps)                                                                       |
| **Cell Groups (Merges)**                   | Statische und dynamische Zusammenführung von Zellen (`Row`, `Col`, `ColSpan`, `RowSpan`, `Span`)                                  |
| **Fokus-Management**                       | Grid verwaltet die aktive Zelle intern                                                                                            |
| **Keyboard-Navigation**                    | Konfigurierbare Shortcuts für Zeilen-/Spaltenwechsel und sequenzielle Zell-Navigation                                             |
| **Event-Forwarding**                       | Keys werden an aktives Kind weitergeleitet; unkonsumierte Keys vom Grid verarbeiten                                               |
| **Styling**                                | Individuell pro Gap, pro Zelle, pro Grid; getrennt für Fokus/Inaktiv                                                              |
| **`set_border_text`**                     | Text in Gap-/Border-Bereiche schreiben                                                                                           |
| **Grid-Nesting**                           | Zellen können beliebige Komponenten aufnehmen, inkl. weiterer Grids                                                               |
| **Deterministische Rendering-Reihenfolge** | Zick-Zack (zeilenweise, spaltenweise); aktives Widget zuletzt                                                                     |

---

## 2. Grundkonzepte

### 2.1 Zellen (Rows × Cols)

Ein Grid besteht aus `rows` Zeilen und `cols` Spalten. Jede Zelle wird durch ihren nullbasierten Index `(row, col)` identifiziert.

### 2.2 Constraints

Spaltenbreiten und Zeilenhöhen werden über `Constraint`-Werte bestimmt, analog zu ratatui:
- `Length(n)` – Feste Breite/Höhe
- `Min(n)` – Mindestens n Zeichen
- `Max(n)` – Maximal n Zeichen
- `Percentage(p)` – Prozentualer Anteil
- `Ratio(n, d)` – Verhältnis n:d

Gaps verbrauchen 0 oder 1 Zeichen Breite/Höhe.

### 2.3 Gaps / Borders

Gaps sind Separatoren zwischen Zellen (1 Leerzeichen). Borders sind Unicode Box-Drawing-Trennlinien (1 Zeichen).

| Gap-Typ                         | Breite/Höhe | Beschreibung                               |
|---------------------------------|-------------|--------------------------------------------|
| `None`                          | 0           | Kein Gap, Zellen grenzen direkt aneinander |
| `Gap`                           | 1           | Ein Leerzeichen als Abstand                |
| `Border` (beliebige `BorderChars`) | 1           | Unicode Box-Drawing-Zeichen als Trennlinie |

Jeder Gap ist maximal 1 Zeichen breit/hoch.

### 2.4 BorderChars

Ein `BorderChars` definiert die Zeichen für horizontale Linien, vertikale Linien, Verbindungen und Ecken. Vordefinierte Konstanten stehen zur Verfügung, benutzerdefinierte Sets können als `pub static` erstellt werden.

### 2.5 Gap-Konfiguration (3 Ebenen)

Konfigurationsebenen mit Priorität: **Ebene 3 > Ebene 2 > Ebene 1**

1. **Ebene 1 – Global**: Für das gesamte Grid (horizontal und/oder vertikal gemeinsam)
2. **Ebene 2 – Voll**: Für einen kompletten Spalten-/Zeilengap
3. **Ebene 3 – Partiell**: Für einen Abschnitt eines Gaps (z.B. nur bestimmte Zeilen eines Spaltengaps)

### 2.6 Cell Groups (Merges)

Zellen können zu größeren Zellen zusammengefasst werden. Merges können statisch (bei Erstellung) und dynamisch (zur Laufzeit) definiert werden.

### 2.7 Fokus-Management

Das Grid verwaltet die aktive Zelle intern. Die Fokus-Reihenfolge ist zeilenweise von links nach rechts (Zick-Zack).

### 2.8 Keyboard-Navigation

- Navigations-Shortcuts sind vollständig konfigurierbar (keine Default-Konfiguration für Navigation)
- Kind-Komponenten können Keys konsumieren und so das Grid blockieren
- Unkonsumierte Keys werden vom Grid auf Navigations-Shortcuts geprüft

### 2.9 Event-Forwarding

```
Grid::on(KeyEvent)
  ├─ 1. Leite KeyEvent an aktives Kind: child.on_key(key)
  │     ├─ true  → Kind hat konsumiert. Grid macht nichts weiter.
  │     └─ false → Kind hat nicht konsumiert. Weiter bei 2.
  └─ 2. Prüfe Grid-Keymap: Ist es ein Navigations-Key?
        ├─ Ja  → Grid navigiert (nächste/vorherige Zelle)
        └─ Nein → Event wird ignoriert
```

### 2.10 Rendering-Reihenfolge

1. Zellen werden zeilenweise von links nach rechts gerendert (Zick-Zack)
2. Ausnahme: Das aktuell fokussierte Widget wird **ganz zum Schluss** gerendert
   (wichtig für Overlay-Widgets wie MultiChoice-Dropdowns)

### 2.11 Corner-Berechnung

Wenn ein horizontaler Gap (Row-Gap) und ein vertikaler Gap (Column-Gap) sich kreuzen:
- **Beide Gaps haben Border** → Corner-Zeichen wird gesetzt (z.B. `─` + `│` → `┼`)
- **Mindestens ein Gap hat `None` oder ist ein Leerzeichen** → Kein Corner, die Linie geht durch

### 2.12 GridChild-Trait

```rust
pub trait GridChild: MockComponent {
    fn on_key(&mut self, key: KeyEvent) -> bool;
}
```

Jede Komponente, die in eine Grid-Zelle eingefügt wird, muss `GridChild` implementieren. Bestehende Komponenten implementieren das trivial:

```rust
impl GridChild for TextInput {
    fn on_key(&mut self, key: KeyEvent) -> bool {
        self.on(Event::Keyboard(key)).is_some()
    }
}
```

---

## 3. Layout & ASCII-Beispiele

In allen Beispielen:
- `▓` = Hintergrund der Zelle A
- `░` = Hintergrund der Zelle B
- `▒` = Hintergrund der Zelle C (in Fokus-Beispielen)
- `╳` = Hintergrund der Zelle D (in Fokus-Beispielen)
- `A`, `B`, `C` = Zellinhalt
- `X` = Fokus-Rahmen
- `│`, `─`, `┼`, `╷`, `╵`, etc. = Border-Zeichen

### 3.1 Gaps und Merges

Zellen grenzen direkt aneinander.

**2×1 Grid (2 Spalten, 1 Zeile):**

```
▓▓▓▓▓▓▓░░░░░░░
▓▓▓A▓▓▓░░░B░░░
▓▓▓▓▓▓▓░░░░░░░
```

**2×2 Grid (2 Spalten, 2 Zeile), C+D gemerget:**

4 Zellen (A–D), jeweils 7×3 Zeichen, keine Gaps. C und D sind über `CellGroup::Col(1)` zu einer Zelle zusammengefasst.

```
▓▓▓▓▓▓▓░░░░░░░
▓▓▓A▓▓▓░░░B░░░
▓▓▓▓▓▓▓░░░░░░░
██████████████
████C + D█████
██████████████
```

**5×3 Grid (5 Spalten, 3 Zeilen):**

15 Zellen (A–O), alle gleich breit (7 Zeichen) und gleich hoch (3 Zeichen), keine Gaps.
Der zyklische Wechsel des Hintergrundzeichens (▓ → ░ → █) dient nur der Verdeutlichung der Zellgrenzen; in der tatsächlichen Komponente ist der Hintergrund jeder Zelle frei konfigurierbar. In Fokus-Beispielen wird `▒` statt `█` verwendet.

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

**5×3 Grid – Gap zwischen Spalte 2 und 3:**

Gleiche Zellen wie oben, mit einem Gap (Leerzeichen) zwischen Spalte 2 (C, H, M) und Spalte 3 (D, I, N).

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

**5×3 Grid – Gap zwischen Zeile 1 und 2:**

Gleiche Zellen wie oben, mit einem Gap (Leerzeichen) zwischen Zeile 1 (F–J) und Zeile 2 (K–O).

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

**5×3 Grid – Gap zwischen Spalte 2/3 und zwischen Zeile 1/2:**

Gleiche Zellen wie oben, mit beiden Gaps kombiniert.

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

**2×2 Grid mit Gaps:**

4 Zellen (A–D), jeweils 7×3 Zeichen, mit Gaps (Leerzeichen) zwischen Spalten und Zeilen.
Veranschaulicht, dass Gaps zwischen Zellen konfigurierbar sind.

```
▓▓▓▓▓▓▓ ░░░░░░░
▓▓▓A▓▓▓ ░░░B░░░
▓▓▓▓▓▓▓ ░░░░░░░
               
███████ ▓▓▓▓▓▓▓
███C███ ▓▓▓D▓▓▓
███████ ▓▓▓▓▓▓▓
```

**2×2 Grid – C und D als ColSpan zusammengefasst:**

Gleiche Zellen wie oben, aber C und D sind zu einer Zelle zusammengefasst (`CellGroup::Col(1)` bzw. äquivalent `ColSpan(row=1, first_col=0, last_col=1)`).
Der vertikale Gap zwischen C und D entfällt, da beide nun eine einzige Zelle bilden.
Der horizontale Gap zwischen Zeile 0 und Zeile 1 bleibt erhalten.

```
▓▓▓▓▓▓▓ ░░░░░░░
▓▓▓A▓▓▓ ░░░B░░░
▓▓▓▓▓▓▓ ░░░░░░░
               
███████████████
█████C + D█████
███████████████
```

**4×3 Grid – Header, Sidebar, ColSpan und Col:**

Ein 4×3 Grid ohne Gaps, das alle Merge-Typen zeigt:
- `CellGroup::Row(0)` → A überspannt Spalte 0–2 in Zeile 0
- `CellGroup::Col(3)` → G überspannt die gesamte Spalte 3 (alle Zeilen)
- `CellGroup::RowSpan(col=0, 1, 2)` → B überspannt Zeile 1 und 2 in Spalte 0
- `ColSpan(row=1, 1, 2)` → C und D sind zu einer Zelle zusammengefasst
- E, F sind einzelne Zellen

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

**5×3 Grid – `Span(1, 1, 2, 3)`:**

`Span(first_row=1, first_col=1, last_row=2, last_col=3)` → Zellen B, C, D, G, H, I werden zu einer Zelle zusammengefasst (3 Spalten × 2 Zeilen). Der Hintergrund der ersten Zelle (░, Zelle B) wird verwendet.

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

### 3.2 Borders (globale Konfiguration)

**5×3 Grid – Simple Borders global (`BorderPos::Grid`):**

Basis: Gleiche Zellen wie das 5×3 Grid (A–O), 7×3 Zeichen pro Zelle.
`set_border(BorderPos::Grid, &BORDER_SIMPLE)` setzt Simple Borders zwischen allen Spalten und Zeilen sowie einen äußeren Rahmen.

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

**5×3 Grid – Selektive Borders (`AfterCol(1)` + `BeforeRow(2)`):**

Basis: Gleiche Zellen wie das 5×3 Grid (A–O), 7×3 Zeichen pro Zelle.
- `set_border(BorderPos::AfterCol(1), &BORDER_SIMPLE)` → Vertikaler Border zwischen Spalte 1 (B, G, L) und Spalte 2 (C, H, M). Endet oben halb (`╷`) und unten halb (`╵`).
- `set_border(BorderPos::BeforeRow(2), &BORDER_SIMPLE)` → Horizontaler Border vor Zeile 2 (zwischen F–J und K–O). Endet links halb (`╶`) und rechts halb (`╴`).
- Beide Borders kreuzen sich → Corner-Zeichen `┼`.

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

**5×3 Grid – `BORDER_SIMPLE_EXTENDED` (`AfterCol(1)` + `BeforeRow(2)`):**

Gleiche Positionen wie oben, aber mit `&BORDER_SIMPLE_EXTENDED` statt `&BORDER_SIMPLE`.
Bei `SimpleExtended` gehen die Linien an den Endstellen durch (volle Enden statt halber Enden).
Unterschied zu `Simple`: Keine `╷`/`╵`/`╶`/`╴`, sondern `│`/`─` bis zum Rand.

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

**5×3 Grid – Partielle Borders (`AfterRowSpanned(1, 0, 1)` + `BeforeColSpanned(4, 1, 2)`):**

Basis: Gleiche Zellen wie das 5×3 Grid (A–O), 7×3 Zeichen pro Zelle.
- `set_border(BorderPos::AfterRowSpanned(1, 0, 1), &BORDER_SIMPLE)` → Horizontaler Border nach Zeile 1, nur unter Spalte 0 (F, K) und Spalte 1 (G, L). Endet halb (`╶`/`╴`).
- `set_border(BorderPos::BeforeColSpanned(4, 1, 2), &BORDER_SIMPLE)` → Vertikaler Border vor Spalte 4, nur in Zeile 1 (I, J) und Zeile 2 (N, O). Endet halb (`╷`/`╵`).
- Die Borders kreuzen sich nicht (horizontaler Border reicht nur bis Spalte 1, vertikaler Border beginnt bei Spalte 4).

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

**5×3 Grid – `AfterRowSpanned(1, 2, 3)` (Simple) + `BeforeColSpanned(4, 1, 2)` (Double):**

- `set_border(BorderPos::AfterRowSpanned(1, 2, 3), &BORDER_SIMPLE)` → Horizontaler Border (─) nach Zeile 1, nur unter Spalte 2 (C, H) und Spalte 3 (D, I). Endet halb (`╶`/`╴`).
- `set_border(BorderPos::BeforeColSpanned(4, 1, 2), &BORDER_DOUBLE_EXTENDED)` → Vertikaler Border (║) vor Spalte 4, nur in Zeile 1 (I, J) und Zeile 2 (N, O). Da es keine halben Enden für ║ gibt, wird `&BORDER_DOUBLE_EXTENDED` verwendet (volle Enden).
- Beide Borders sind **unterschiedlicher** Typ (Simple vs. Double) → sie grenzen zwar aneinander, werden aber **nicht** gejoint.

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

**Gleiche Konfiguration + `set_border_text`:**

Zusätzlich: `set_border_text(BorderPos::AfterRowSpanned { row: 1, col_start: 2, col_end: 3 }, TextAnchor::End, offset=0, "─╢")`.
Der Text "─╢" wird von links nach rechts geschrieben, endend am Ende des horizontalen Borders: ─ ersetzt das halbe Ende ╴ (pos 27), ╢ ersetzt das ║ (pos 28). Der horizontale Border läuft nun durch und mündet mit ╢ in die Spaltenspalte.

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

**3×3 Grid mit Gaps überall:**

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

**5×3 Grid – Verschiedene Border-Konfigurationen kombiniert:**

Dieses Beispiel zeigt verschiedene Border-Arten und Konfigurationsebenen in einem einzigen Grid:
- `set_border(BorderPos::AfterRow(0), &BORDER_DOUBLE_EXTENDED)` → Vollständiger horizontaler Double-Border (═) über die gesamte Breite nach Zeile 0
- `set_border(BorderPos::AfterRowSpanned(1, 2, 3), &BORDER_ROUNDED)`
- `set_border(BorderPos::AfterColSpanned(1, 2, 2), &BORDER_ROUNDED)`
- `set_border(BorderPos::AfterColSpanned(3, 2, 2), &BORDER_ROUNDED)`
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

**Gleiche Konfiguration + `set_border_text`:**

Zusätzlich: `set_border_text(BorderPos::AfterColSpanned { col: 1, row_start: 2, row_end: 2 }, TextAnchor::Start, offset=0, "Down")` und `set_border_text(BorderPos::AfterRow(0), TextAnchor::Start, offset=2, " My Header ")`.
- `set_border_text` schreibt Text in einen Bereich (Leerzeichen oder Border-Zeichen) und überschreibt die dortigen Zeichen.
- " My Header " wird horizontal in den Gap nach Zeile 0 geschrieben, beginnend an Position 2. Die ═-Zeichen werden durch die Textzeichen überschrieben.
- "Do…" wird vertikal in den Spaltengap nach Spalte 1 geschrieben (nur in Zeile 2). Der Gap hat 3 Zeilen Höhe, der Text "Down" hat 4 Zeichen → wird mit Ellipsis abgeschnitten. Die ╭-Zeichen werden durch die Textzeichen überschrieben.

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

## 4. Konfiguration

### 4.1 Grid-Größe

```rust
let grid = Grid::new(rows: usize, cols: usize);
```

Beispiel:
```rust
let grid = Grid::new(3, 4); // 3 Zeilen, 4 Spalten
```

### 4.2 Constraints

Constraints für Spaltenbreiten und Zeilenhöhen, analog zu ratatui:

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

#### `BorderPos` – Wo wird der Border gesetzt?

```rust
pub enum BorderPos {
    /// Äußerer Rahmen um das gesamte Grid
    Grid,

    /// Vertikaler Border nach Spalte i (zwischen Spalte i und i+1), über alle Zeilen
    AfterCol(usize),
    /// Vertikaler Border vor Spalte i (zwischen Spalte i-1 und i), über alle Zeilen
    BeforeCol(usize),

    /// Horizontaler Border nach Zeile i (zwischen Zeile i und i+1), über alle Spalten
    AfterRow(usize),
    /// Horizontaler Border vor Zeile i (zwischen Zeile i-1 und i), über alle Spalten
    BeforeRow(usize),

    /// Vertikaler Border nach Spalte col, nur in Zeilen row_start..=row_end
    AfterColSpanned { col: usize, row_start: usize, row_end: usize },
    /// Vertikaler Border vor Spalte col, nur in Zeilen row_start..=row_end
    BeforeColSpanned { col: usize, row_start: usize, row_end: usize },

    /// Horizontaler Border nach Zeile row, nur in Spalten col_start..=col_end
    AfterRowSpanned { row: usize, col_start: usize, col_end: usize },
    /// Horizontaler Border vor Zeile row, nur in Spalten col_start..=col_end
    BeforeRowSpanned { row: usize, col_start: usize, col_end: usize },
}
```

`Grid` erzeugt einen geschlossenen äußeren Rahmen um das gesamte Grid. Die `AfterCol`/`BeforeCol`-Varianten erzeugen vertikale Linien über die volle Höhe, `AfterRow`/`BeforeRow` horizontale Linien über die volle Breite. Die `Spanned`-Varianten begrenzen den Border auf einen Teilbereich; an den Enden werden halbe Enden gesetzt (siehe Abschnitt [3.2](#32-borders-globale-konfiguration) für visuelle Beispiele).

#### `BorderChars` – Welche Zeichen werden verwendet?

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

**Vordefinierte Konstanten:**

| Name | Linien | Halb-Enden | Ecken |
|---|---|---|---|
| `BORDER_SIMPLE` | `─` `│` | `╷` `╵` `╶` `╴` | `┌ ┐ └ ┘` |
| `BORDER_SIMPLE_EXTENDED` | `─` `│` | `│` `│` `─` `─` | `┌ ┐ └ ┘` |
| `BORDER_DOUBLE_EXTENDED` | `═` `║` | `║` `║` `═` `═` | `╔ ╗ ╚ ╝` |
| `BORDER_THICK_EXTENDED` | `━` `┃` | `┃` `┃` `━` `━` | `┏ ┓ ┗ ┛` |
| `BORDER_ROUNDED` | `─` `│` | `╷` `╵` `╶` `╴` | `╭ ╮ ╰ ╯` |
| `BORDER_ROUNDED_EXTENDED` | `─` `│` | `│` `│` `─` `─` | `╭ ╮ ╰ ╯` |
| `BORDER_DASHED` | `┄` `┆` | `╷` `╵` `╶` `╴` | `┌ ┐ └ ┘` |
| `BORDER_DASHED_EXTENDED` | `┄` `┆` | `│` `│` `─` `─` | `┌ ┐ └ ┘` |
| `BORDER_DOTTED` | `┈` `┊` | `╷` `╵` `╶` `╴` | `┌ ┐ └ ┘` |
| `BORDER_DOTTED_EXTENDED` | `┈` `┊` | `│` `│` `─` `─` | `┌ ┐ └ ┘` |

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
    /// Gaps zwischen allen Spalten und Zeilen
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

- Die zusammengefassten Zellen teilen sich den kombinierten Platz aller Einzelzellen (ohne interne Gaps/Borders).
- Die erste Zelle (oben-links) bestimmt den Hintergrund der zusammengefassten Zelle.
- Ein Kind-Widget wird der gesamten Fläche der Gruppe zugewiesen.
- Fokus springt über die Gruppe als Ganzes.

#### Zusammenspiel mit Gaps und Borders

Gaps und Borders, die **innerhalb** einer Gruppe verlaufen würden, werden unterbrochen und nicht gezeichnet. Visuell verhält es sich so, als wären die Borders auf beiden Seiten der Gruppe separat definiert worden:

- Eine durchgehende horizontale Border (`AfterRow(1)`) wird durch ein vertikales Grouping in zwei getrennte Segmente aufgeteilt, die jeweils eigene Enden erhalten (z.B. `╶────╴` auf jeder Seite).
- Ein vertikaler Gap zwischen zwei Spalten, die Teil einer `ColSpan`-Gruppe sind, entfällt innerhalb der Gruppe.
- Borders und Gaps, die am **Rand** der Gruppe verlaufen, werden normal gezeichnet.
- Es folgt, dass die Gruppendimensionen mit darin verlaufenden Borders/Gaps entsprechend breiter/höher sind als nur die Summe ihrer Bestandteile und das auch, wenn sie eine Border/Gap komplett überdecken.

Siehe Abschnitt [3.1](#31-gaps-und-merges) für visuelle Beispiele.

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

Bei `BorderPos::Grid` bezieht sich `Start`/`End` auf die obere linke bzw. untere rechte Ecke des Rahmens. Bei `Spanned`-Varianten bezieht sich `Start`/`End` auf den Bereich des Spans.

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

#### Fokus-Styling

```rust
grid.with_focus_style(Style::default()
    .fg(Color::White)
    .bg(Color::DarkBlue));
grid.with_focus_frame_style(Style::default()
    .fg(Color::Cyan)
    .bg(Color::DarkBlue));
```

#### Styling-Priorität

Die Prioritätsreihenfolge für das Styling eines Elements:
1. Spezifischste Konfiguration (z.B. partieller Gap, einzelne Zelle)
2. Gap-/Zell-Konfiguration
3. Globale Konfiguration
4. `Style::default()`

### 4.8 Fokus

#### Fokussierte Zelle

Die aktive (fokussierte) Zelle wird durch einen dekorativen Rahmen hervorgehoben, der **innerhalb** der Zelle gerendert wird. Der Rahmen verbraucht Platz innerhalb der Zelle (1 Zeichen breit an jeder Seite) und vergrößert die Zelle daher nicht.

**2×2 Grid – Zelle A fokussiert (9×5 Zeichen pro Zelle):**

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

Der Fokus-Rahmen besteht aus Unicode Block-Elementen:
- Ecken: `▛` (oben-links), `▜` (oben-rechts), `▙` (unten-links), `▟` (unten-rechts)
- Rahmen: `▀` (oben), `▄` (unten), `▌` (links), `▐` (rechts)
- Innenraum: Leerzeichen (oder konfigurierbar)

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
    next_cell: Some(KeyEvent::from(Code::Tab)),
    prev_cell: Some(KeyEvent::from(Code::BackTab)),
});
```

**Einzelne Shortcuts setzen:**

```rust
grid.set_key_next(KeyEvent::from(KeyCode::Right));
grid.set_key_prev(KeyEvent::from(KeyCode::Left));
grid.set_key_next_row(KeyEvent::from(Code::Tab));
grid.set_key_prev_row(KeyEvent::from(Code::BackTab));
grid.set_key_next_col(KeyEvent::from(KeyCode::Down));
grid.set_key_prev_col(KeyEvent::from(KeyCode::Up));
```

Merged Zellen werden bei der Navigation als eine einzige Position behandelt und übersprungen.

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

#### Fokus bei gemergten Zellen

Wenn der Fokus von einer nicht-gemergten Zelle auf eine gemergte Zelle wechselt, berechnet das Grid zunächst die Zelle, die den Fokus annehmen würde (basierend auf der aktuellen Zeile/Spalte des Fokus). Der Fokus wird dann auf die gesamte gemergte Zelle gesetzt, aber das Grid merkt sich intern die Position der berechneten Zelle.

Wenn der Fokus erneut gewechselt wird, wird anhand der gespeicherten Zellposition bestimmt, welche Zelle als Nächstes angesteuert wird. Dadurch ergibt sich ein natürliches Navigationsverhalten, das die geometrische Position des ursprünglichen Ziels respektiert.

Beispiel: In einem 2×3 Grid mit B und E gemerget zu BE (Spalte 1, Zeilen 0–1):

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

Hinweis: Der Fokus-Rahmen einer gemergten Zelle erstreckt sich über die gesamte Höhe der gemergten Zelle. Die Lücken zwischen Rahmen und Seitenrahmen verwenden das gleiche Muster wie bei nicht-gemergten Zellen (` ░░░░░░░ ` — Leerzeichen an den Rändern, Interior-BG im Innenraum).

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

#### Rendering-Reihenfolge

1. Alle Zellen werden in natürlicher Reihenfolge gerendert (Zick-Zack: Zeilenweise, spaltenweise)
2. **Ausnahme**: Die aktive (fokussierte) Zelle wird **ganz zum Schluss** gerendert

**Warum?** Overlay-Widgets (wie MultiChoice-Dropdowns) können über benachbarte Zellen ragen. Durch das späte Rendern wird das Dropdown über den anderen Zellen angezeigt.

```
Rendere: Zelle A → Zelle B → Zelle C → [Zelle D übersprungen] → Zelle E → Zelle F
Danach: Zelle D (fokussiert, mit Dropdown)
```

#### Event-Forwarding

```
1. KeyEvent kommt im Grid an
2. Grid leitet KeyEvent an aktives Kind weiter: child.on_key(key)
   ├── Kind return true  → Event konsumiert. Grid macht nichts.
   └── Kind return false → Event nicht konsumiert.
3. Grid prüft eigene Keymap:
   ├── Match mit Navigations-Key → Navigation ausführen
   └── Kein Match → Event ignorieren (None zurückgeben)
```

**Wichtig**: Der Entwickler ist verantwortlich für die Keymap-Konfiguration. Standardmäßig sind keine Navigations-Keys definiert.

### 5.4 Corner-Berechnung bei Gap-Kreuzungen

Wenn sich ein horizontaler und ein vertikaler Gap kreuzen:

| Horizontaler Gap | Vertikaler Gap | Ergebnis |
|---|---|---|
| Border | Border | Corner-Zeichen (aus `BorderChars`) |
| Border | Gap (Leerzeichen) | Horizontal: Linie geht durch (kein Corner) |
| Border | None | Horizontal: Linie geht durch |
| Gap (Leerzeichen) | Border | Vertikal: Linie geht durch (kein Corner) |
| Gap (Leerzeichen) | Gap (Leerzeichen) | Leerzeichen |
| Gap (Leerzeichen) | None | Nichts |
| None | Border | Vertikal: Linie geht durch |
| None | Gap (Leerzeichen) | Nichts |
| None | None | Nichts |

**Corner-Zeichen-Auswahl**: Wenn beide Gaps Borders haben, wird das Corner-Zeichen basierend auf den `BorderChars` bestimmt. Bei unterschiedlichen `BorderChars` wird der Corner des horizontalen Gaps verwendet (bzw. konfigurierbar).

### 5.5 Gap-Breite und Platzberechnung

Jeder Gap nimmt genau 1 Zeichen Breite (vertikal) bzw. 1 Zeichen Höhe (horizontal) ein. Ein fehlender Gap (`remove_gap`) nimmt 0 Zeichen ein.

Die Platzberechnung berücksichtigt alle Gaps, bevor die verbleibende Fläche auf die Zellen aufgeteilt wird:

```
Gesamtbreite = Gap_0 + Zelle_0 + Gap_1 + Zelle_1 + ... + Gap_n-1 + Zelle_n-1
```

### 5.6 Merges und Gaps

Wenn Zellen zusammengefasst sind, werden Gaps, die **innerhalb** des zusammengefassten Bereichs liegen, nicht gezeichnet. Gaps, die am **Rand** des zusammengefassten Bereichs liegen, werden normal gezeichnet.

```
Normal:
┌───┬───┬───┐
│ A │ B │ C │
└───┴───┴───┘

A+B gemerget (ColSpan):
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
- **Animation**: Animierte Übergänge bei Fokuswechsel oder Merge-Änderungen
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
- **Gap-Typen**: Nur `GapType::None` (0 Zeichen) und `GapType::Border(...)` (1 Zeichen). Es gibt kein `GapType::Empty`. Default-Gap = Leerzeichen.
- **Border-Half-Endings**: Borders haben standardmäßig Half-Endings (╷/╵/╶/╴). `Border::SimpleExtended`/`Border::DoubleExtended` haben Full-Endings. Für ║ gibt es kein Half-Ending → `Border::DoubleExtended` ist die einzige Option für Double.
- **Auto-Join**: Gleiche Border-Typen, die aufeinandertreffen, werden automatisch verbunden (z.B. ─ + │ → ┼). Verschiedene Border-Typen werden NICHT verbunden.
- **Pixel-Perfect**: Jede Zeile in einem Code-Block muss exakt dieselbe Länge haben. Niemals Hand-Schreiben — immer Python-Scripts verwenden.

### Python-Scripts

Scripts liegen unter `ai/scripts/`. Vor jedem Grid-Beispiel das entsprechende Script ausführen und mit Assertions verifizieren (alle Zeilen gleiche Länge, korrekte Unicode-Codepoints).

| Script | Zweck |
|--------|-------|
| `focus_grids.py 2x2` | 2×2 Grid, 9×5 Zellen, 4 Fokus-Zustände (A/B/C/D), 78 Zeichen breit |
| `focus_grids.py 2x3_merged` | 2×3 Grid, 9×5 Zellen, B+E gemerget, Fokus A/BE/C (Zeile 0), 85 Zeichen breit |
| `focus_grids.py 2x3_merged_from_d` | 2×3 Grid, 9×5 Zellen, B+E gemerget, Fokus D/BE/F (Zeile 1), 85 Zeichen breit |

### API-Konventionen

- `BorderChars` sind `pub static` Konstanten, kein Trait, kein Enum.
- `set_border` nimmt `&'static BorderChars` (kein Style-Parameter). Style wird separat via `set_border_style` gesetzt.
- `set_gap` nimmt keinen Style-Parameter. Style via `set_border_style`.
- Border-Syntax in Code-Beispielen: `&BORDER_SIMPLE`, `&BORDER_DOUBLE`, etc.
- `set_border_text` mit `BorderPos`/`TextAnchor`, nicht `write_to_gap`.
- `CellGroup` statt Merge-Begriff in der API.

### Workflow

1. Ein Beispiel nach dem anderen bearbeiten: aktuellen Zustand zeigen, korrigierten Zustand zeigen, User-Approval einholen, in Dokument schreiben.
2. User prüft Änderungen in der Datei, nicht im Chat.
3. Nichts ändern, was der User nicht explizit angefordert hat.
