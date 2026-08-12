# 0007 — Eine Spalten-Deklaration (`ColumnSchema`) statt `SortableColumn`

- **Status:** akzeptiert, umgesetzt
- **Datum:** 2026-08-11
- **Betrifft:** `not-yet-done-content` (`ColumnSchema` erweitert, `SortableColumn`
  entfällt, `Child::columns`, `apply_sort`, neuer geteilter Zellen-Lookup
  `cell()`, Konformitätsprüfung im generischen Listenpfad),
  `not-yet-done-extended-query` (`ColumnTypes`, `SummaryRow`),
  `not-yet-done-jira-adapter`, `not-yet-done-local-adapter`,
  `not-yet-done-taiga-adapter`, `not-yet-done-kimai-adapter`,
  `not-yet-done-calendar-adapter`, `not-yet-done-custom-columns`,
  `not-yet-done-tui`, `not-yet-done-cli`
- **Baut auf:** [0006 — Anonymisierung als Content-Layer-Dekorator](0006-anonymisierung-content-layer.md)
  (dieselbe Dekorator-Kette muss die Deklaration durchreichen).

## Kontext

Eine Zeile beschreibt sich im Framework zweimal:

- als **Daten** — `NodeSummary { id, label, node_type, metadata.fields }`, wobei
  `metadata.fields` eine Liste von `MetadataField { key, value, … }` ist;
- als **Deklaration** — `Child::sortable_columns: Vec<SortableColumn>` (welche
  Spalten sortierbar sind, mit `SortKind`) und `ContentAdapter::describe_columns`
  → `Vec<ColumnSchema>` (Typ und erlaubte Werte für Spalten, die kein nativer
  Inhalt sind; heute nur die lokal gespeicherten Custom Columns).

Zwischen beiden gilt ein **stillschweigender** Vertrag: ein Spalten-Key benennt
ein Metadatenfeld. Nichts erzwingt ihn, und er wird verletzt. Der Jira-Adapter
meldet `summary` als sortierbare Spalte, legt in der **Listenzeile** aber kein
`summary`-Feld an — der Titel steht nur in `NodeSummary::label`
(`issue_summary()`). Der Task-Adapter macht dasselbe mit `description`.

Damit die Sortierung trotzdem funktioniert, rät `apply_sort` seit jeher:

```rust
s.metadata.fields.iter().find(|f| f.key == key)
    .map(|f| f.value.clone())
    .unwrap_or_else(|| s.label.clone())   // ← Fallback aufs Label
```

Der Fallback ist als Nachschlagepfad **für genau diese eine Spalte** gedacht,
gilt aber für **jede** Spalte, die in einer Zeile kein Feld hat. Solange nur
sortiert wurde, blieb der Schaden unsichtbar: Sortieren kennt kein „nicht
vorhanden", und wenn alle Zeilen gleich zurückfallen, ändert sich die
Reihenfolge kaum wahrnehmbar.

Mit `local_filter` (Extended Queries) bekam derselbe Lookup einen zweiten
Konsumenten — `SummaryRow::cell` kopierte die Zeile bewusst, damit „Filter und
Sortierung denselben Wert sehen". Filtern kennt „nicht vorhanden" aber als
Aussage, und Custom Columns fehlen zeilenweise völlig (der Dekorator hängt nur
**gespeicherte** Zellen an). Ergebnis: das Label ist nie leer, also ist
`is_not_null` für jede Zeile wahr und `is_null` für keine. Eine Abfrage, die
14 Tickets mit lokaler Zelle liefern sollte, lieferte 306.

Ein zweiter, gleichartiger Defekt schläft daneben: `issue_sortable_columns()`
setzt für alle Spalten `kind: SortKind::Text` mit der Begründung „Jira sorts
server-side via JQL ORDER BY, so the kind is unused here". Für den Server
stimmt das — aber `ColumnTypes` speist sich aus derselben Liste, und
`local_filter` nimmt den `kind` als Typ ernst. `[updated, '>', …]` vergleicht
darum Zeichenketten statt Zeitpunkte. Eine Deklaration, die für einen
Konsumenten als „egal" markiert war, ist für einen anderen autoritativ
geworden.

Die gemeinsame Ursache ist nicht der Fallback, sondern dass es **zwei
Deklarationen und drei Typquellen** gibt (`SortableColumn::kind`,
`ColumnSchema::value_type`, `kind:` im View-YAML), zwischen denen niemand
Konsistenz erzwingt.

## Optionen

1. **Nur den Fallback in `SummaryRow::cell` entfernen.** Behebt den
   Filter-Bug, lässt den Sortier-Fallback, die doppelte Deklaration und die
   Typ-Lüge stehen. Der nächste Konsument erbt dasselbe Problem.
2. **Den Label-Bezug deklarierbar machen** (`SortableColumn { backed_by: Label }`,
   analog zu `source: label` im View-YAML). Beseitigt das Raten, zementiert
   aber die Denormalisierung und lässt zwei Deklarationen nebeneinander
   bestehen.
3. **Eine Deklaration, geprüfte Anwesenheit** — `ColumnSchema` wird die
   einzige Spalten-Deklaration und trägt selbst, ob die Spalte in den Zeilen
   liegt und ob sie sortierbar ist; `apply_sort` rät nicht mehr; der
   generische Listenpfad prüft die Invariante.

## Entscheidung

Option 3.

### Eine Deklaration

`SortableColumn` entfällt. `ColumnSchema` beschreibt eine Spalte vollständig:

| Feld         | Bedeutung                                                                                 |
| ------------ | ----------------------------------------------------------------------------------------- |
| `key`        | Spalten-Key; zugleich der Metadatenfeld-Key, unter dem der Wert in Zeilen liegt           |
| `label`      | optionaler Anzeigename (`None` = das Frontend nimmt seinen eigenen)                       |
| `value_type` | `text` / `number` / `duration` / `datetime` — **die** Typquelle                           |
| `options`    | geschlossene Wertemenge, falls vorhanden                                                  |
| `in_rows`    | der Wert liegt als Metadatenfeld in jeder gelisteten Zeile ⇒ lokal sortier- und filterbar |
| `sortable`   | der Adapter sagt zu, eine Sortierung nach dieser Spalte zu erfüllen                       |

`in_rows` und `sortable` sind bewusst **unabhängig**:

- Server-seitige Sortierung braucht keine lokale Zelle. Jira ordnet die
  Ticketliste per JQL `ORDER BY`; eine Spalte darf sortierbar sein, ohne je in
  einer Zeile aufzutauchen.
- Umgekehrt ist jede Spalte, die in den Zeilen liegt, lokal filterbar — auch
  wenn sie nicht sortierbar ist.

Mit welchem Mechanismus ein Adapter eine zugesagte Sortierung erfüllt (JQL
`ORDER BY` oder lokales `apply_sort`), bleibt seine Sache und ist keine
Framework-Information mehr.

### Zwei Kanäle, ein Typ

Die Deklaration wird weiterhin auf zwei Wegen geliefert, weil die
Custom-Column-Spalten aus einem Store gelesen werden müssen:

- `Child::columns` — synchron, statisch je Kindtyp, vom Adapter selbst;
- `ContentAdapter::describe_columns` — asynchron, dynamisch, für Spalten, die
  ein Dekorator hinzufügt.

Beide liefern `ColumnSchema`. Die Vereinigung passiert **einmal** in
`children::columns_for`, nicht mehr je Frontend.

### Kein Raten mehr

`apply_sort` löst nur Spalten mit `in_rows` auf und liest ausschließlich das
Metadatenfeld; fehlt es, ist die Zelle leer. Der Label-Fallback entfällt
ersatzlos, in `apply_sort` **und** in `SummaryRow::cell`. Beide benutzen
denselben geteilten Lookup `content::cell()`, sodass „Filter und Sortierung
sehen denselben Wert" durch geteilten Code garantiert ist statt durch einen
Kommentar.

Damit die Label-gestützten Spalten weiter sortierbar bleiben, liefern
`jira::issue_summary()` und `local::task::summary()` ihr Feld künftig mit —
`taiga`, `calendar` und `local::projects` tun das längst. `label` bleibt als
Anzeigekopie erhalten (Breadcrumbs, Detail-Titel, Link-Beschriftungen), ist
aber nicht mehr der einzige Aufbewahrungsort.

### Geprüfte Invariante

> Jede Spalte mit `in_rows` muss in jeder gelisteten Zeile als Metadatenfeld
> vorkommen.

`children::list` prüft das per `debug_assert` — in Tests und Debug-Builds
scheitert ein Adapter, der eine Spalte deklariert und nicht liefert, in
Release-Builds kostet es nichts. Zusätzlich gibt es einen öffentlichen
Prüf-Helfer, den Adapter-Unit-Tests auf ihre Fixture-Zeilen anwenden.

## Konsequenzen

- Der Filter-Bug (`is_null`/`is_not_null`/`has`/`like` auf Custom Columns) und
  der Sortier-Unfug (Titel zwischen Statuswerten) verschwinden gemeinsam.
- Custom Columns werden **sortierbar**: da der Fallback weg ist, ist eine
  fehlende Zelle einfach leer, und der Dekorator kann seine Spalten ehrlich als
  `sortable` melden. Im TUI erscheinen sie damit im `S`-Menü.
- Die Typ-Lüge bei `updated` fällt weg: `value_type` ist die einzige Typquelle,
  und sie wird pro Spalte richtig gesetzt statt pauschal auf `Text`.
- Das `kind:` im View-YAML wird redundant, sobald Adapter ihre nativen Spalten
  beschreiben; es bleibt vorerst als Überschreibung bestehen.
- `source: label` in den View-Configs wird für Jira kosmetisch (die Spalte
  könnte den Wert nun aus dem Feld lesen). Es bleibt als Feature erhalten —
  Adapter dürfen weiter Spalten haben, die nur aus dem Label rendern, solange
  sie diese nicht mit `in_rows` deklarieren.
- **Nicht** Teil dieser Entscheidung: `value_type` von `String` auf ein Enum zu
  heben. Der Wert wird in der Custom-Columns-SQLite und an der CLI-Oberfläche
  persistiert; die Umstellung ist mechanisch, aber eine eigene Änderung mit
  eigenem Migrationsrisiko.
- **Nicht** Teil dieser Entscheidung: die zweite lokale Sortierimplementierung
  in `taiga::client::query::apply_sort` (typisierte Feldvergleiche über den
  adaptereigenen `ItemSummary`, inkl. Sortierpriorität je Item-Typ und
  „Unbesetztes ans Ende"). Sie in die generische Funktion zu falten setzt eine
  Sortierart mit vorgegebener Wertereihenfolge und eine Leer-sortiert-ans-Ende-
  Regel für Text voraus — siehe offene Punkte.

## Umsetzung

Umgesetzt am 2026-08-11 über alle Adapter und Frontends hinweg.

Drei Adapter verletzten die Invariante und liefern das Feld jetzt mit:
`jira` (`summary`), `local::task` (`description`) und `taiga` (`project`).
Der Taiga-Fall ist zugleich das Musterbeispiel für die Trennung: `project`
ist über den adaptereigenen `ItemSummary`-Vergleicher sortierbar, taucht aber
in keiner Zeile auf — es ist damit `sortable`, aber nicht `in_rows`.
Umgekehrt sind Jiras `attachments` und `bookmarked` in jeder Zeile und
trotzdem nicht sortierbar. Dass Jiras Ticketliste (server-sortiert) und
Bookmark-Liste (lokal sortiert) über dieselben Zeilen unterschiedliche
Sortierbarkeit melden, hält je ein Test fest.

`SortKind` wird nicht mehr deklariert, sondern von `ColumnSchema::sort_kind()`
aus `value_type` abgeleitet — damit ist die Typ-Lüge strukturell unmöglich
geworden statt nur behoben.

Die Invariantenprüfung hat beim ersten Lauf sofort einen Lügner gefangen: ein
Test-Fake in `not-yet-done-extended-query` deklarierte `status` und lieferte
es nicht.

Ende-zu-Ende bestätigt an der Abfrage, die den Bug ausgelöst hat: 306 → 12
Zeilen.

Der als offener Punkt notierte Umbenenner `jql::apply_sort` → `apply_order_by`
ist Teil der Umsetzung.

### Nachtrag: zwei Stellen, die der erste Durchgang übersehen hat

`describe.rs` (das eingebaute `help`) las die Spalten direkt aus
`Child::columns` statt aus `columns_for` — derselbe Fehler in klein, ein
zweiter Leser derselben Deklaration, der an der Vereinigung vorbeigreift.
`help --full` verschwieg damit genau die Spalten, die es hätte melden müssen:
die des Dekorators. Der Renderpfad ist jetzt asynchron und geht durch
`columns_for`.

Und die Vorrangregel war zu grob: „beschrieben gewinnt" hat auch das Label
überschrieben. Ein Store kennt nur Key und Typ, kein Label — eine Custom
Column namens `status` löschte damit den Anzeigenamen der gleichnamigen
Adapter-Spalte und ließ den nackten Key stehen. Ein fehlendes Label behält
jetzt das deklarierte: eine Aussage über die Felder, die eine Quelle trifft,
ist keine über die, die sie nicht trifft.

Beide Male fiel es erst beim Benutzen auf, nicht beim Bauen — der Grund,
warum `columns_for` und `content::cell()` genau eine Stelle sein müssen.

### Nachtrag: eine deklarierte Spalte ist noch keine sortierte

Die Spalten des Dekorators standen im Menü, aber ein Sortieren nach ihnen tat
nichts. Der Grund liegt genau an der Naht, die diese Entscheidung zieht: Jira
sortiert serverseitig, `jql::build_order_by` verwirft stillschweigend jeden Key
ohne JQL-Feld — und der Adapter _kann_ die Spalten eines über ihm liegenden
Dekorators gar nicht kennen. Für Custom Columns ist das nicht der Sonderfall,
sondern der Normalfall.

Repariert wird das dort, wo beide Seiten sichtbar sind: `children::list`
sortiert nach, was der Adapter nicht bedient hat. Drei Bedingungen halten das
ehrlich — nachsortiert wird nur, wenn das Ergebnis _vollständig_ ist (eine
einzelne Seite zu sortieren ordnete eine Stichprobe und gäbe sie als das Ganze
aus), nur wenn dabei keine bereits bediente Ordnung verloren geht, und
`applied_sort` meldet hinterher die Wahrheit. Welche Keys eine lokale Sortierung
überhaupt vergleichen kann, entscheidet weiterhin genau eine Stelle
(`resolve_sort`, öffentlich als `honoured_sort_keys`).

### Nachtrag: Type-on-first-write brauchte einen Rückweg

Der Typ einer Custom Column wird beim ersten Schreiben festgenagelt — und das
ist praktisch immer `text`, weil das der Default der Formulare ist. Eine Spalte
mit Zahlen verglich damit lexikalisch (`100` vor `20`), ohne Ausweg außer der
SQLite von Hand.

`retype_column` ist der Rückweg: Der neue Typ wird angenommen, wenn _jeder_
gespeicherte Wert unter ihm validiert, sonst abgelehnt — mit Zeilen-ID und Wert
jedes Treffers, sodass die Fehlermeldung selbst die Korrekturliste ist. Auf dem
Fehlerpfad wird nichts geschrieben.

Bewusst eine eigene Aktion (`retype-column`) statt einer Lockerung von
`set-cell`: Dessen Typ-Auswahl hat den Default `text`, und `text` akzeptiert
jeden Wert. Ein `set-cell`, das eine Typänderung durchwinkt sobald alle Werte
passen, würde eine `number`-Spalte still zu Text degradieren, sobald jemand den
Default stehen lässt. Umtypisieren ist eine Entscheidung und bekommt darum eine
Aktion, die man wählen muss.

### Nachtrag: `json` als fünfter Werttyp

Der Retype legte eine Lücke offen: `validate_value` ließ unbekannte Typen
durch (`_ => true`), also konnte ein Aufrufer einen beliebigen String als Typ
schreiben und der Store nahm ihn an. Genau so entstanden `json`-Spalten, die
nirgends deklariert waren — sichtbar erst daran, dass `retype-column` sie
ablehnte, weil `json` nicht in `VALUE_TYPES` steht.

`json` ist damit ein echter Typ: Werte müssen als JSON-Wert parsen. Er sagt
aber, anders als die anderen vier, nichts über Vergleich oder Darstellung —
eine `json`-Spalte sortiert weiter als Text und rendert weiter verbatim. Das
ist Absicht: er beschreibt die _Nutzlast_ einer Zelle, für die kein skalarer
Typ passt (eine Liste von Tags, eine Liste von Records), nicht ihre Ordnung.
Alle übrigen Typkonsumenten (`sort_kind`, `value_type_to_col_kind`,
`column_kind_from_value_type`) haben einen Catch-all-Zweig und behandeln ihn
genau so, ohne Änderung.

Offen bleibt die andere Hälfte: ein `ColumnKind::Json`, das eine Liste als
`a, b` statt als `["a", "b"]` zeigt. Das ist eine Darstellungsentscheidung
(Komma-Join? Anzahl? erste n?) und gehört nicht in den Store.

## Offene Punkte

- Generische Sortierung um eine Enum-Ordnung (`SortKind::Ranked`) und
  „leer sortiert aufsteigend ans Ende" für Text erweitern, dann Taigas
  eigenen Sortierer ablösen.
- `source: label` für die `summary`-Spalte aus `views/jira.yaml` entfernen —
  seit der Adapter das Feld liefert, ist es wirkungslos.
