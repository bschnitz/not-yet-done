# Generic View Specification

## Vision

Jeder Haupttab in der TUI wird durch eine deklarative YAML-Datei definiert.
Die TUI kennt keine Jira-spezifische Logik mehr — sie rendert generische
"Content Views", die durch einen ContentAdapter mit Daten versorgt werden.
Tasks und Trackings bleiben native Tabs (eigene DB, eigene Logik).

---

## View-Konfiguration

Verzeichnis: `~/.config/not_yet_done/views/`

Jede `.yaml`-Datei = ein Haupttab. Die TUI lädt beim Start alle Dateien
und erzeugt je einen Tab. Reihenfolge über `order`-Feld oder alphabetisch.

Ein Tab = eine Connection. Mehrere Instanzen desselben Adapter-Typs
(z.B. zwei Jira-Server) = mehrere YAML-Dateien = mehrere Tabs.

### Vollständiges Beispiel: `jira.yaml`

```yaml
tab:
  name: Jira
  order: 3
  icon: 🎫 # optional, für Tab-Bar

adapter:
  type: jira # registrierter AdapterFactory-Name
  # Opaker Config-String — Format wird vom Adapter bestimmt.
  # Kann ein Dateipfad sein (relativ zu views/ oder absolut)...
  config: jira-globex.yaml
  # ...oder inline als String:
  # config_inline: |
  #   url: https://jira.example.com
  #   session_id: abc123

views:
  # Jeder Eintrag ist ein Subtab oder eine navigierbare Ebene.
  - name: Tickets
    node_type: jira:issue
    default: true # dieser Subtab wird beim Tab-Wechsel gezeigt

    # Welche Query beim Laden verwendet wird.
    query:
      default: "assignee = currentUser() ORDER BY updated DESC"
      editable: true # User kann via : eigene Query eingeben
      menu_key: q # öffnet das q-Menü mit allen Saved Queries

      # Anmerkung: Gespeicherte Queries werden hier *nicht* mehr
      # aufgezählt. Bodies liegen Adapter-seitig als einzelne Dateien
      # unter `<XDG_DATA_HOME>/not_yet_done/<adapter>/<instance>/queries/`
      # (siehe SavedQueryStore / FsSavedQueryStore), Keyboard-Shortcuts
      # in der DB-Tabelle `query_shortcut(scope, name, shortcut)`.
      # Verwaltung erfolgt zur Laufzeit: `:query new`, `:query edit`,
      # `:query delete`, sowie Ctrl+f im q-Menü zum Binden eines
      # Shortcuts.
      #
      # Shortcut-Validierung: Saved-Query-Shortcuts greifen auf der
      # View-Claim-Ebene und würden jede danach dispatchte Taste
      # überschatten (z. B. j/k-Navigation). Beim Binden wird der Key
      # deshalb gegen *alle* im Tab aktiven Bindings geprüft (Globals,
      # Common-Navigation, Window-Chords inkl. Leader-Präfix, Subtab-
      # Keys, menu_key, YAML-`actions:`/`shortcuts:`, Chord-Präfixe wie
      # `z` vor `zg`, andere Saved-Query-Shortcuts) und bei Kollision
      # mit Nennung des Konflikts abgelehnt. Beim Laden aus der DB
      # (extern geschriebene oder durch Config-Änderungen veraltete
      # Rows) erzeugt eine Kollision eine Warnung in der
      # Notification-Leiste; der Shortcut bleibt aktiv, bis der User
      # ihn neu bindet.
      #
      # Default-Query: Im q-Menü markiert `ctrl+t` (Keybinding
      # `query_menu.set_default`, konfigurierbar in tui.yaml) die
      # selektierte Saved Query als Default (★ vor dem Namen;
      # Shortcuts erscheinen als dimmer `[key]`-Suffix). Die Default-
      # Query wird beim App-Start automatisch angewendet — sie schlägt
      # das YAML-`query.default` dieses Views (Content-Tabs) bzw. den
      # Restore des zuletzt aktiven Filters (native Tasks/Trackings).
      # Warum: das YAML-`default` ist die geteilte, eingecheckte
      # Vorgabe; die Default-Query ist die persönliche, zur Laufzeit
      # umsteckbare Wahl ohne Config-Edit. `ctrl+t` auf der aktuellen
      # Default-Query löscht die Markierung wieder. Persistenz: eine
      # Settings-Row `default_query:{scope}` pro Scope (Content:
      # `query_scope` des Tabs; nativ: `task`/`tracking`); verschwindet
      # die Query aus dem Store, wird der Default beim Start still
      # übersprungen. Grenze: Queries mit Pflicht-Variablen (`{var}`)
      # werden beim Start ohne Variablen-Popup roh angewendet.
      #
      # `inherit_default: true` (Default false) stempelt die User-
      # Default-Query (★) beim Start zusätzlich auf *diesen* Subtab —
      # der einfache Start-Apply trifft nur die Default-View des Tabs.
      # Warum opt-in: Geschwister-Views zeigen meist *andere* Daten,
      # wo dieselbe Query nichts bedeutet (Postgres tables vs.
      # scripts); Views, die nur eine andere Projektion derselben
      # Zeilen sind (Trackings normal/condensed/tree), wollen den
      # Default-Filter dagegen überall (analog zum nativen Tab, der
      # EINEN Filterzustand über alle Subviews hatte).

    # Spalten der Tabelle — welche Metadata-Keys angezeigt werden.
    columns:
      - key: key
        label: Key
        style: accent # Referenz auf Theme-Farbe
        sizing: max # max = so breit wie Inhalt, flex(N) = anteilig
      - key: type
        label: Type
        style: text_med
        sizing: max
      - key: status
        label: Status
        style: success
        sizing: max
      - key: priority
        label: Priority
        style: warning
        sizing: max
      - key: summary
        label: Summary
        source: label # "label" = node.label(), sonst metadata key
        style: text_high
        sizing: flex(1)

    # Preview-Pane Konfiguration
    preview:
      enabled: true
      source: content # "content" = node.content().read_text()
      split: horizontal # horizontal (links/rechts) oder vertical (oben/unten)
      ratio: 50 # Prozent für die Preview-Seite
      keybinding: P

    # Aktionen auf selektierten Nodes
    actions:
      - name: Edit
        key: e
        type: edit # öffnet externen Editor
        # Was wird editiert? Der Adapter generiert das Template
        # und parst die Ausgabe (editor_template / parse_editor_output).
        # Hier nur angeben, WELCHE Felder editierbar sein sollen:
        edit:
          content: true # Content-Body (description)
          metadata: # Zusätzlich editierbare Metadata-Felder im Template
            - summary

      - name: Refresh
        key: r
        type: reload # Liste neu laden

      - name: Open in Browser
        key: o
        type: open_url # öffnet node.metadata("url") im Browser

    # Navigation zu Kind-Nodes (z.B. Kommentare eines Tickets)
    children:
      - name: Comments
        key: Enter
        node_type: jira:comment
        columns:
          - key: author
            label: Author
            sizing: max
          - key: created
            label: Date
            sizing: max
          - key: body
            source: label
            label: Comment
            sizing: flex(1)
        actions:
          - name: Add Comment
            key: a
            type: create
          - name: Edit Comment
            key: e
            type: edit
            edit:
              content: true

      - name: Attachments
        key: A
        node_type: jira:attachment
        columns:
          - key: filename
            label: File
            sizing: flex(1)
          - key: size
            label: Size
            sizing: max
          - key: author
            label: Author
            sizing: max
        actions:
          - name: Download
            key: d
            type: download # node.content().read() → Datei speichern

  # Zweiter Subtab: Projekte
  - name: Projects
    node_type: jira:project
    query:
      default: null # alle Projekte
      editable: false
    columns:
      - key: key
        label: Key
        sizing: max
        style: accent
      - key: name
        source: label
        label: Name
        sizing: flex(1)
    actions:
      - name: Open
        key: Enter
        type: navigate # wechselt in die Ticket-Liste dieses Projekts
        navigate_to: Tickets # Name des Views
        query_template: "project = {key}" # setzt die Query des Ziel-Views
```

### Multi-Line-Rows: `row_layout`

Standardmäßig rendert eine View ihre Items als **einzeilige** Tabelle: eine
logische Zeile = eine Terminalzeile, alle `columns` nebeneinander. Mit dem
optionalen Feld `row_layout` wird eine logische Zeile stattdessen als
**Stapel mehrerer physischer Zeilen** gerendert (Chat-Layout):

```yaml
columns:
  - { key: author, source: author, style: accent, sizing: max }
  - { key: time, source: time, style: text_dim, sizing: max }
  - { key: content, source: content, markdown: true, sizing: "flex(1)" }
row_layout:
  - [author, time] # Zeile 1: Meta (hervorgehoben über die Spalten-`style:`)
  - [content] # Zeile 2: Nachrichtentext (markdown, mehrzeilig)
  - [] # Zeile 3: Leerzeile (Spacer)
```

**Warum:** Eine flache Tabelle aus `Author | Time | Message` ist für
Chat-/Feed-artige Daten unleserlich, sobald die Nachricht lang ist. Das
Chat-Layout trennt Metadaten und Inhalt visuell und gibt dem Body die volle
Breite.

Regeln und Verhalten:

- **Jeder Eintrag** in `row_layout` ist eine physische Zeile und listet die
  `columns`-Keys auf, die dort (von links nach rechts) gerendert werden. Die
  Keys müssen in `columns` deklariert sein (sonst harter Validierungsfehler).
- **Leere Liste `[]`** = Leerzeile/Spacer.
- **Hervorhebung** läuft rein über die per-Spalten-`style:` (Theme-Referenz,
  also über `tui.yaml` überschreibbar). Es gibt keine separate Farboption —
  wer die Meta-Zeile hervorheben will, gibt deren Spalten ein `style:`.
- **Kopfzeile:** Im Multi-Line-Modus wird der Spaltenkopf unterdrückt.
- **Selektion:** Bei Auswahl einer Zeile bekommen alle physischen Zeilen den
  Auswahl-Hintergrund — **außer** der Spacer und jede Zeile, die sich explizit
  ausklinkt: `- { columns: [foo], highlight_on_select: false }`. (Eine leere
  Zeile defaultet auf `highlight_on_select: false`, eine nicht-leere auf
  `true`.)
- **Einschränkung:** Multi-Line gilt nur für flache Drill-Listen, nicht für
  Tree-Views. Spalten-Cursor / Horizontal-Scroll und der Jump-Mode (`f`)
  arbeiten weiterhin nur auf der ersten (primären) Zeile.

#### `markdown:` — mehrzeiliger, soft-gewrappter Body

Eine Spalte mit `markdown: true` rendert ihren Wert als **Markdown**: harte
Zeilenumbrüche _und_ Soft-Wrap an der Pane-Breite, plus Inline-Styling
(`**fett**`, `*kursiv*`, `` `code` ``), Listen, Überschriften und Blockquotes.
Gedacht für Chat-/Langtext-Spalten (z. B. der Stoat-Nachrichtenbody).

```yaml
- { key: content, source: content, markdown: true, sizing: "flex(1)" }
```

**Warum es das gibt:** Ohne `markdown:` kürzt eine Spalte ihren Wert auf eine
einzelne (ggf. newline-kollabierte) Zeile — für längere Nachrichten unleserlich.
`markdown: true` expandiert den Wert stattdessen in so viele physische Zeilen,
wie der Umbruch braucht; die Zeilenhöhe der Row wächst entsprechend mit.

Regeln:

- **Allein auf ihrer Zeile:** Eine `markdown`-Spalte muss in ihrer
  `row_layout`-Zeile die **einzige** Spalte sein (`- [content]`). Andernfalls
  harter Validierungsfehler statt stillem Verwerfen der Nachbarspalten.
- **Quelle:** In aller Regel `source: content` (oder ein anderes Metadata-Feld),
  damit der **rohe** Body gelesen wird — nicht das auf eine Zeile kollabierte
  `label`.
- **Farben** kommen aus dem Theme (über die Markdown-Theme-Bridge); es gibt
  keine Hardcodes und nichts pro Spalte zu setzen.
- **Aktuelle Schnitte:** Die `/`-Suche markiert Treffer **nicht** im
  gerenderten Body (Filtern/Matching laufen weiter über `label`/Body), und
  Code-Blöcke bekommen keinen Hintergrund. Syntax-Highlighting ist optional und
  separat.
- **Preview-Pane:** Dasselbe Markdown-Rendering gibt es im Preview-Pane über
  `preview.markdown: true` (siehe `preview:`-Block) — z. B. um den vollen
  Nachrichtenbody mit `p` schön gerendert zu zeigen.

#### `sizing:` — Spaltenbreite

Pro Spalte, default `max`. Bestimmt, wie die Tabellen-Engine die Spaltenbreite
gegen das Breiten-Budget (= **tatsächliche Pane-Breite zum Render-Zeitpunkt**)
verteilt:

| `sizing:`       | Verhalten                                                           |
| --------------- | ------------------------------------------------------------------- |
| `max`           | so breit wie der breiteste Inhalt (gedeckelt auf den freien Rest)   |
| `fixed(N)`      | exakt `N` Spalten breit                                             |
| `flex(N)`       | teilt sich den **Rest** nach Gewicht `N` mit anderen `flex`-Spalten |
| `fit`           | `min(Inhaltsbreite, freier Rest)` — content-breit, stretcht nicht   |
| `auto(min,max)` | inhaltsbreit zwischen `min`/`max`; **ignoriert das Budget** (s. u.) |

`flex`-Spalten füllen den nach `max`/`fixed` verbleibenden Platz **bis zur
Pane-Breite** — sie blähen die Tabelle nicht über die Fläche hinaus. Eine
`flex`-Spalte darf damit auch **mitten** in der Spaltenliste stehen (z. B. die
Task-/Description-Spalte): die Spalten dahinter bleiben sichtbar.

`fit` ist die **Kombination aus `max` und `flex`** (entspricht CSS
`fit-content`): die Spalte wird so breit wie ihr Inhalt, aber nie breiter als
der nach allen `fixed`/`max`/`auto`-Spalten verbleibende Platz —
`min(Inhaltsbreite, freier Rest)`. Anders als `flex` stretcht sie also **nicht**
auf die volle Pane-Breite (ist der Inhalt kurz, bleibt die Tabelle schmaler als
die Fläche), und anders als `max` wird sie **deferred** ausgelegt (erst nach
allen Fixbreiten-Spalten) — eine `fit`-Spalte in der Mitte verdrängt die
nachfolgenden Spalten daher nie off-screen. Gedacht z. B. für die Task-Spalte
der Tasks-Ansicht, die nur so breit sein soll wie der längste sichtbare
Task-Titel. Stehen mehrere `fit`-Spalten nebeneinander, bedient sich die
linkeste zuerst; `flex` füllt einen danach noch übrigen Rest. (Historisch
legte die Engine gegen ein fixes Budget von 300 aus statt gegen die Pane-Breite;
eine nicht-letzte `flex`-Spalte schob dann die nachfolgenden Spalten off-screen.
Behoben — die Engine fittet jetzt auf die reale Pane-Breite und re-fittet bei
Resize / Preview-Toggle.)

`auto(min,max)` ist der Sonderfall für Tabellen mit **unbekannter Spaltenzahl**
(z. B. dynamische Postgres-Rows): solche Spalten ignorieren das Pane-Budget
bewusst und dürfen die Tabelle breiter als die Fläche machen — dann greift der
**Horizontal-Scroll** (Spalten-Cursor, nur aktiv mit `column_cursor: true`).
Für Views mit fester, in die Pane passender Spaltenliste gibt es keinen
Horizontal-Scroll, weil alle Spalten on-screen liegen.

#### `kind:` — typisierte Spaltenwerte

Eine Spalte deklariert mit `kind:` den **semantischen Typ** ihres Werts. Der
Adapter liefert weiterhin nur Strings, aber in einer **kanonischen** Form; die
Tabellen-Engine parst sie und kümmert sich um Formatierung, Ausrichtung und
Styling. Default ist `text` — jede bestehende (Remote-)Spalte bleibt damit
unverändert.

```yaml
- { key: elapsed, source: elapsed, kind: duration } # rechtsbündig, H:MM:SS
- { key: started, source: started, kind: datetime } # lokalisiert
- { key: count, source: count, kind: number } # rechtsbündig
- { key: taskpath, source: taskpath, kind: path, separator: " › " }
- { key: running, kind: elapsed, elapsed_from: started } # live now − started
```

| `kind:`    | Kanonische Eingabe des Adapters      | Anzeige                             | Ausrichtung |
| ---------- | ------------------------------------ | ----------------------------------- | ----------- |
| `text`     | beliebig                             | unverändert                         | links       |
| `number`   | Dezimalzahl (`"42"`)                 | unverändert                         | rechts      |
| `duration` | Ganzzahl **Sekunden** (`"5400"`)     | `H:MM:SS` (über `format_duration`)  | rechts      |
| `datetime` | RFC 3339 (`"2026-06-09T08:15:00Z"`)  | lokale Zeitzone, `%Y-%m-%d %H:%M`   | links       |
| `path`     | `/`-getrennte Segmente (`"/a/b/c"`)  | mit `separator:` verbunden, gestylt | links       |
| `elapsed`  | _kein eigener Wert_ — liest ein Feld | `now − Feld` als `H:MM:SS`, live    | rechts      |

Drei optionale Begleitfelder:

- **`format:`** — nur für `datetime`: ein strftime-Muster, das das Default
  `%Y-%m-%d %H:%M` ersetzt (z. B. `format: "%H:%M"`).
- **`separator:`** — nur für `path`: das Anzeige-Trennzeichen (Default `/`).
  Es wird im Theme-Stil `taskpath_separator` (fett) gezeichnet, der Pfad führt
  immer mit einem Separator (eine Wurzel rendert als reines Trennzeichen).
- **`elapsed_from:`** — nur für `kind: elapsed`: der Schlüssel des
  `datetime`-Feldes (RFC 3339), gegen das gerechnet wird; Default ist der
  eigene `key` der Spalte. Die Spalte hat keinen eigenen Wert, sie rendert
  `now − <elapsed_from>` als Dauer und wird **bei jedem Repaint-Tick neu
  berechnet** (kein Refetch) — so tickt z. B. die laufende Zeit eines aktiven
  Trackings live. Eine Zukunfts-Instant (Uhren-Drift) wird auf `00` geklemmt,
  ein leeres Feld bleibt leer, ein unparsbarer Wert wird unverändert gezeigt.

**Warum es das gibt:** Ohne `kind:` müsste jeder Adapter Dauer/Datum/Pfad selbst
fürs Display vorformatieren — entweder als unausgerichteter Roh-String oder mit
adapter-spezifischer Layout-Logik, die nicht aggregierbar/sortierbar ist. Mit
typisierten Spalten bleibt der **maschinenlesbare** Wert die Quelle der Wahrheit
(Sekunden, RFC 3339, Pfadsegmente), und Ausrichtung, lokalisierte Formatierung
und das Separator-Styling der Taskpath-Spalte sind ein generisches
Engine-Feature statt Copy-&-Paste pro Adapter. Der Typ steht bewusst in der
View-YAML und nicht am `MetadataField` der Adapter, damit Remote-Adapter (Jira,
Taiga, Postgres, Confluence, Stoat) keine Zeile ändern müssen. `elapsed` ist
zusätzlich der einzige **zeitabhängige** Typ: sein Wert hängt nur vom
Anzeige-Zeitpunkt ab, nicht von den geladenen Daten — getrieben durch das
`Repaint`-Signal des Domain-Event-Bus rendert die Engine die betroffenen Panes
pro Tick neu, ohne nachzuladen.

#### `smooth_scroll:` — kontinuierliches zeilenweises Scrollen

Steht auf `ViewDef` **und** `ChildDef` (gleiche Ebene wie `row_layout`),
default `false`. Mit `smooth_scroll: true` scrollt die Tabelle nicht mehr
diskret von Eintrag zu Eintrag, sondern **eine physische Zeile pro Schritt**
über den gesamten Inhalt — der Inhalt „wandert“ kontinuierlich über den
Bildschirm. Gedacht für lange, mehrzeilige Listen (z. B. den Chat).

```yaml
- name: messages
  node_type: "stoat:message"
  smooth_scroll: true
  row_layout: [...]
```

**Warum es das gibt:** Bei mehrzeiligen Rows (Chat: Meta + Body + Spacer)
springt der diskrete Modus ganze Nachrichtenblöcke rein/raus, was bei langen
Verläufen ruckelig wirkt. Zeilenweises Scrollen liest sich flüssig.

Verhalten:

- **Navigation:** ↑/↓ scrollen je eine Zeile; `Ctrl+u`/`Ctrl+d` und
  PageUp/Down um eine halbe bzw. ganze Pane-Höhe (in Zeilen); `g`/`G` an
  Anfang/Ende. Bottom-Clamp: man scrollt nicht über das Ende hinaus.
- **Auswahl (frühe Übergabe in Scrollrichtung):** Der Highlight (und das Ziel
  von `e`/`d`/`+`/`p`) ist an _eine_ Row gebunden, nicht an eine
  Bildschirm­position. Scrollen verschiebt nur den Viewport; die hervorgehobene
  Row bleibt, **solange sie vollständig sichtbar ist**. Der einzige Auslöser
  für eine Übergabe ist „die aktuell fokussierte Row ist nicht mehr ganz zu
  sehen": sobald das Scrollen auch nur eine ihrer physischen Zeilen am Rand
  abschneidet, springt die Auswahl zur **benachbarten** selektierbaren Row in
  Scrollrichtung (runter → nächste, hoch → vorherige). Die neue Row muss dabei
  **nicht selbst schon vollständig sichtbar sein** — ist der Nachbar hoch und
  ragt noch über den gegenüberliegenden Rand hinaus, wird er trotzdem
  fokussiert (er wird beim Weiterscrollen ganz sichtbar). Es zählt nur, ob die
  _aktuelle_ Auswahl anfängt zu verschwinden, nie ob die nächste schon passt.
  (Ist eine einzelne Row höher als der ganze Viewport, sodass nichts ganz
  hineinpasst, bleibt die Auswahl auf ihr — sie geht nie verloren.) `g`/`G`
  wählen explizit erste/letzte Row; programmatische Auswahl (Reload, Jump,
  Suche) scrollt das Ziel _minimal_ in den sichtbaren Bereich.
- **Cursor-Step, wenn nichts scrollt:** Weil die Auswahl _vom Scrollen_
  getrieben wird, würde `j`/`k` nichts tun, sobald es nichts zu scrollen gibt —
  die ganze Liste passt auf den Bildschirm, oder der Viewport sitzt schon am
  Rand. Damit der virtuelle Cursor trotzdem wandert, springt die Auswahl in
  diesem Fall zur nächsten/vorherigen selektierbaren Row. Das hält auch die
  allererste/letzte Nachricht erreichbar, wenn das Scrollen am Ende angekommen
  ist.
- **Orthogonal zu `markdown:`/`row_layout:`** — funktioniert auch für
  einzeilige Tabellen (dort = zeilenweises Scrollen), entfaltet seinen Nutzen
  aber bei mehrzeiligen Rows.

#### `group_by:` / `then_by:` / `aggregates:` / `summary_only:` — Gruppierung & Summen (M3)

Stehen auf `ViewDef` **und** `ChildDef` (gleiche Ebene wie `row_layout` /
`smooth_scroll`), alle optional. Sie schalten den **gruppierten Render-Pfad** der
einzeiligen Tabelle ein: die gefilterten Einträge werden nach einem (oder
mehreren, s. `then_by:`) Schlüsseln partitioniert, jede Partition bekommt eine
**Gruppen-Kopfzeile** mit Zwischensumme, und unter der ganzen Tabelle steht eine
angepinnte **Gesamtsumme** (Footer).

```yaml
- name: condensed
  node_type: "tracking"
  group_by: { column: started, bucket: day } # äußere Ebene: nach Tag
  then_by:
    - { column: task_id } # innere Ebene: pro Tag nach Task
  aggregates:
    - { column: duration, op: sum } # Zwischensumme je Ebene + Gesamt
  summary_only: true # innerste Gruppe = eine repräsentative Zeile
```

**`group_by:`** — wonach gruppiert wird. Pflichtfeld `column:` ist ein
Spalten-`key` (oder ein roher Metadaten-Feldname, falls keine Spalte ihn
anzeigt). Optionales `bucket:` fasst einen **`kind: datetime`**-Wert zu einem
Datums-Eimer zusammen, statt nach dem exakten Zeitstempel zu gruppieren:

| `bucket:` | Label-Format   | Beispiel     |
| --------- | -------------- | ------------ |
| `day`     | `%Y-%m-%d`     | `2026-06-09` |
| `week`    | `%G-W%V` (ISO) | `2026-W23`   |
| `month`   | `%Y-%m`        | `2026-06`    |
| `year`    | `%Y`           | `2026`       |

Ohne `bucket:` wird der Spaltenwert **verbatim** als Gruppenschlüssel benutzt
(z. B. eine Status- oder Kategorie-Spalte). Die Labels sind bewusst
**ISO-sortierbar** gewählt, sodass die lexikografische Sortierung der Gruppen
zugleich die chronologische ist.

In der **Kopfzeile** wird der ISO-Schlüssel zusätzlich menschenlesbar
aufbereitet (reines Display — Identität und Sortierung bleiben der
ISO-Schlüssel): `day` rendert als `W24 2026-06-08 Mon` (ISO-Woche + Wochentag),
`week` als `W23 2026`; `month`/`year` und Verbatim-Schlüssel bleiben
unverändert.

Optionales `order:` (`asc` Default, `desc`) bestimmt die **Reihenfolge der
Gruppen** — `desc` zeigt bei Datums-Buckets den neuesten Eimer zuerst (das
übliche Layout eines Zeit-Logs). Die Zeilen _innerhalb_ einer Gruppe behalten
unabhängig davon die Adapter-Reihenfolge. `zg` (s. u.) übernimmt das
konfigurierte `order:` beim Durchschalten.

**`then_by:`** — **verschachtelte** Gruppierung. Eine Liste weiterer Ebenen
(gleiche Felder wie `group_by:`), nach denen _innerhalb_ jeder äußeren Gruppe
weiter partitioniert wird. Die volle Ebenenliste ist `[group_by] ++ then_by`.
Beispiel Trackings-„Condensed": `group_by` nach Tag, `then_by` nach Task → ein
Tages-Header, darunter je Task eine Zeile mit der Tages-Task-Summe. Ohne
`group_by:` wird `then_by:` ignoriert. Tipp: die innere Ebene auf einen
**stabilen, evtl. unsichtbaren** Schlüssel (`task_id`) gruppieren, nicht auf das
angezeigte Label, damit gleichnamige Einträge nicht fälschlich verschmelzen.

**`aggregates:`** — Liste der Spalten, die je Gruppe und gesamt summiert werden.
Jeder Eintrag hat `column:` (ein Spalten-`key`) und `op:` (aktuell nur `sum`,
der Default). Summiert wird auf dem **kanonischen** Wert (für `kind: duration`
also die Sekunden-Zahl); die Summe wird durch denselben typisierten Formatter
gerendert wie die Datenzellen, eine Dauer-Summe erscheint also wieder als
`H:MM:SS`. Ohne `aggregates:` entfallen Zwischensummen und Footer — es bleibt
die reine Gruppierung mit Kopfzeilen.

Optionales `total_column:` (ein Spalten-`key`) verlegt die Gruppen-Summe von
der `──`-Kopfzeile in diese **eigene Spalte**, geschrieben auf die **letzte
Datenzeile** jeder äußersten Gruppe (und auf den `Σ`-Footer). Das ist das
klassische Stundenzettel-Layout, bei dem eine „Total"-Spalte jeden Tag
abschließt — die Kopfzeile bleibt dann ein reines Label. Die Zielspalte wird
ganz normal als Spalte deklariert (typisch `kind: duration`); solange die
Gruppierung ausgeschaltet ist (`zg` auf `None`), wird sie **ausgeblendet**,
weil eine Gruppensumme ohne Gruppen keinen Inhalt hätte.

**`summary_only:`** — `true` blendet die einzelnen Daten-Rows aus. Die
**innerste** Gruppen-Ebene kollabiert dann zu **je einer repräsentativen
Daten-Zeile** (aufgebaut aus einem Member der Gruppe, mit dem Gruppen-Total in
den Aggregat-Spalten) — diese Zeile ist **selektierbar**, sodass Row-Aktionen
(delete/toggle …) auf sie wirken. Äußere Ebenen bleiben `── label ──`-Header mit
Zwischensumme. Das ist die „Condensed"-Ansicht der Trackings: pro (Tag, Task)
genau eine Zeile mit Pfad, Task und Summe. (Ohne `then_by:`, also einstufig,
kollabiert jede Gruppe direkt zu ihrer repräsentativen Zeile.)

**Laufzeit-Umschaltung (`cycle_grouping`, Default `zg`):** Die Aktion
`cycle_grouping` (in `keybindings.yaml` bindbar, Default `zg`) schaltet die
Gruppierung der aktiven Ebene durch: ungruppiert → `day` → `week` → `month` →
`year` → ungruppiert. Bei verschachtelter Gruppierung rotiert sie nur die
**äußere** (`group_by`-)Ebene; die inneren `then_by:`-Ebenen bleiben erhalten
(„Condensed" mit `zg` auf `None` = eine Zeile pro Task über den ganzen
Zeitraum). Sie ist nur aktiv, wenn die Ebene überhaupt ein `group_by:`
konfiguriert hat. Der Umschalt-Status ist View-State (nicht persistiert) und
überschreibt das konfigurierte `group_by:` nur für die laufende Sitzung.

**Direktsprung-Menü (`group_menu`, Default `u`):** Statt durchzuschalten
öffnet `group_menu` ein kleines Hotkey-Popup über dieselben fünf Zustände —
`n` No grouping, `d` Day, `w` Week, `m` Month, `y` Year (Pfeile +
Enter/Space gehen auch, Esc bricht ab). Optik wie das native Grouping-Menü:
Standard-Popup-Chrome mit Keybinding-Legende unten, `●` markiert den
aktuellen Zustand, der Hotkey-Buchstabe ist im Label unterstrichen.
Gleiche Bedingung
und gleiche Semantik wie `zg` (rotiert nur die äußere Ebene, View-State,
nicht persistiert) — es ist die Parität zum `u`-Menü des nativen
Trackings-Tabs. Auf Ebenen ohne `group_by:` bleibt `u` frei für
YAML-`shortcuts:`.

> **Einschränkungen.** Der gruppierte Pfad gilt nur für **einzeilige**
> Tabellen — `group_by:` zusammen mit `row_layout:` (mehrzeilig/Chat) wird
> ignoriert. Engine-seitige Gruppierung ist außerdem ein Flat-List-Feature;
> im Tree-Mode gruppiert stattdessen der **Adapter** (siehe nächster
> Abschnitt) — ohne entsprechende Adapter-Fähigkeit greift `group_by:` im
> Tree nicht.

Die Farbe der Gruppen-Kopf- und Footer-Zeilen ist über das Theme konfigurierbar
(`group_header`, siehe `tui-theme.yaml` / Theme-Referenz).

#### Adapter-seitige Tree-Gruppierung (`group_by_via_adapter`)

Ein **Tree** kann nicht engine-seitig gruppiert werden: Die Engine lädt lazy
und kann die Teilbaum-Summen eines einzelnen Buckets nicht selbst falten —
der Adapter besitzt den Fold (siehe `tree_aggregate:` unten). Deshalb dreht
sich die Zuständigkeit im Tree um: Die Engine reicht das aktive `group_by:`
der Pane im Root-`list()`-Aufruf an den Adapter durch
(`ListParams.group_by`), und der Adapter antwortet mit **einem
Bucket-Knoten pro Gruppe** als Root-Ebene; jeder Bucket expandiert in einen
Teilbaum, dessen Werte nur aus den Einträgen _dieses_ Buckets gefaltet sind.

```yaml
- name: tree
  node_type: "tracking:tree-group" # Root-Ebene = Bucket-Knoten des Adapters
  tree_label: task
  group_by: { column: started, bucket: day, order: desc }
  children:
    - name: subtasks
      node_type: "tracking:tree-item" # rekursive Item-Ebene
      recursive: true
```

- **Capability-Gate.** Der Adapter deklariert `group_by_via_adapter` (siehe
  `AdapterCapabilities`). Nur dann sind `zg`/`u` im Tree aktiv; ohne die
  Fähigkeit bleibt ein `group_by:` auf einer Tree-Root-Ebene wirkungslos
  (gleiche Doppel-Gate-Logik wie `tree_aggregate`).
- **Umschalten = Reload.** `zg` und das `u`-Menü funktionieren im Tree wie
  in der Flat-List, aber jeder Wechsel ist ein **Adapter-Reload** (der
  Adapter muss neu bucketen), kein lokaler Rebuild. Der Status bleibt
  View-State (nicht persistiert).
- **„No grouping" = ein Config, zwei Formen.** Schaltet man die Gruppierung
  aus, liefert der Adapter auf denselben Root-Request den ungebucketeten
  Baum (Items statt Buckets). Die Chain-Auflösung der Engine matcht
  **typbasiert**: Mit Buckets greift die Root-`ViewDef`-Ebene
  (`tracking:tree-group`), ohne Buckets matcht die rekursive
  Item-`ChildDef` ab Tiefe 0. Es braucht also keine zweite View — aber
  Spalten/`shortcuts:` der Root-Ebene gelten nur für Bucket-Zeilen
  (Buckets sind read-only Aggregate; Row-Aktionen gehören auf die
  Item-Ebene).
- **Bucket-Identität steckt in der Node-ID.** Derselbe Task kann in
  mehreren Buckets erscheinen; Knoten unter einem Bucket tragen den
  Bucket-Scope in ihrer ID, damit `get_by_id` ohne Query-Kontext die
  richtigen (bucket-gefalteten) Werte rechnen kann. Der Saved-Query-Filter
  der Pane kommt zusätzlich pro `list()` an
  (`propagates_query_to_subtree`) und schneidet den Bucket weiter zu.
- **Konsistente Labels.** Bucket-Keys und -Anzeigelabels kommen aus
  demselben Modul (`not_yet_done_content::grouping`), das auch die
  engine-seitige Flat-Gruppierung benutzt — ein Tag heißt im gruppierten
  Tree exakt so wie in der gruppierten Flat-List.

##### `group_headers:` — Buckets als `── label`-Header-Zeilen

Ohne weitere Config sind die Bucket-Knoten **normale Tree-Zeilen**:
selektierbar, mit den Items eine Einrückungsebene tiefer. Das liest sich
anders als dieselbe Gruppierung auf einer Flat-List (dort sind Gruppenköpfe
nicht-selektierbare `── label`-Zeilen ohne Extra-Einrückung). `group_headers:`
auf der Tree-Root-Ebene stellt die Bucket-Zeilen auf genau dieses
Header-Rendering um:

```yaml
- name: tree
  node_type: "tracking:tree-group"
  tree_label: task
  group_by: { column: started, bucket: day, order: desc }
  expand_depth: all # Pflicht in der Praxis, s. u.
  group_headers:
    total: # optional: Gruppen-Total in eigener Spalte
      key: total
      label: Total
      kind: duration
      style: accent
      sizing: max
      source: duration # Metadata-Feld des BUCKET-Knotens mit dem Total
```

- **Rendering.** Bucket-Zeilen werden `── label`-Zeilen im
  Gruppen-Header-Style (gleiche Chrome wie die Flat-Gruppierung), **nicht
  selektierbar**; die Zeilen darunter verlieren die Einrückungsebene des
  Buckets — der Wald beginnt unter jedem Header bei Einrückung 0.
- **`total:` (optional).** Eine vollwertige `ColumnDef`, die nur bei aktiver
  Gruppierung als letzte Spalte erscheint und das Gruppen-Total auf der
  **letzten** Zeile jeder Gruppe zeigt (das klassische
  Stundenzettel-Layout — dieselbe Semantik wie `total_column` der
  Flat-Gruppierung). `source:` benennt das Metadata-Feld des
  Bucket-Knotens, das das Total trägt (Fallback: `key`). Mit ausgeschalteter
  Gruppierung verschwindet die Spalte.
- **`expand_depth` ist praktisch Pflicht:** Header sind nicht selektierbar,
  ein eingeklappter Bucket ließe sich also per Cursor nie öffnen. Der
  Validator verlangt `tree_label` + `group_by` auf derselben View.
- Gilt nur, solange tatsächlich gruppiert wird (Capability + aktives
  `group_by`); mit „No grouping" rendert der Tree normal.

#### `tree_aggregate:` — Eigen- vs. Summenwert im Tree (M4)

Das Gegenstück zu `group_by:` für den **Tree-Mode**: Eine Spalte kann pro Knoten
entweder ihren **Eigenwert** (das Feld `key:` der Spalte) oder den vom Adapter
berechneten **Teilbaum-Summenwert** (`cumulated_field:`) anzeigen — zur Laufzeit
umschaltbar.

```yaml
columns:
  - { key: name, source: label }
  - key: duration # Eigenwert des Knotens (kanonisch: Sekunden)
    kind: duration
    tree_aggregate:
      cumulated_field: duration_cumulated # Adapter liefert die Teilbaum-Summe
      default: own # own (Standard) | cumulated
```

- **`cumulated_field:`** (Pflicht) — der Metadaten-Feldschlüssel, unter dem der
  Adapter den **bereits aufsummierten** Teilbaumwert liefert (kanonisch für die
  `kind:` der Spalte, z. B. Sekunden bei `kind: duration`). Der Eigenwert kommt
  weiterhin aus dem `key:` der Spalte.
- **`default:`** — welcher Wert vor dem ersten Umschalten gezeigt wird: `own`
  (Standard) oder `cumulated`.

**Warum adapter-getrieben?** Der Tree wird **lazy** geladen — eingeklappte Äste
liegen gar nicht im Speicher. Die TUI kann also nicht selbst falten; **nur der
Adapter** weiß, ob er den vollen Baum hat und einen Teilbaum aufsummieren kann.
Er liefert beide Werte als Metadatenfelder und deklariert dazu die Fähigkeit
`supports_tree_aggregation` (siehe `AdapterCapabilities`). Kann er nicht
kumulieren, lässt er das Feld weg.

**Laufzeit-Umschaltung (`toggle_tree_aggregate`, Default `zt`):** Die Aktion
schaltet **alle** `tree_aggregate`-Spalten der aktiven Ebene zwischen Eigen- und
Summenwert um. Sie ist nur aktiv, wenn **zwei** Bedingungen erfüllt sind: die
Ebene (im Tree-Mode) hat überhaupt eine `tree_aggregate`-Spalte **und** der
Adapter meldet `supports_tree_aggregation`. Meldet er die Fähigkeit nicht (oder
ist gar kein Adapter gebunden), bleibt die Taste unbelegt und der Toggle ein
No-op — eine `tree_aggregate:`-Deklaration allein genügt also nicht. Der Status
ist View-State (nicht persistiert).

> **Eigen- _und_ Summenwert nebeneinander** braucht keinen neuen Mechanismus —
> dafür zwei normale Spalten auf die beiden Felder legen (z. B. `key: duration`
> und `key: duration_cumulated`, beide `kind: duration`).

> **Einschränkungen.** `tree_aggregate:` greift nur im **Tree-Mode**; in
> Flat-Listen wird es ignoriert. Spiegelbildlich zu `group_by:`, das
> engine-seitig nur in Flat-Listen greift (im Tree gruppiert der Adapter,
> siehe `group_by_via_adapter` oben).

#### `tree_connector_style:` — Farbe der Connector-Glyphen pro Tree

Im Tree-Mode malt die `tree_label`-Spalte vor das Label einen **Connector-Lauf**:
die Box-Zeichen `├──`/`└──`/`│` und die Aufklapp-Pfeile `▶`/`▼`. Dieser Lauf wird
getrennt vom Label eingefärbt — er soll als leise Struktur _hinter_ den Labels
lesbar sein, nicht mit ihnen konkurrieren.

```yaml
views:
  - name: tasks
    tree_label: description
    tree_connector_style: text_dim # optional; sonst Theme-Farbe `tree_connector`
```

- Ein Theme-Farbname (`text_dim`, `tree_connector`, `accent`, … — dieselbe
  Vokabular wie bei einer Spalten-`style:`). Ohne Angabe gilt die globale
  Theme-Farbe `tree_connector` (`tui.yaml`).
- Steht **auf dem Wurzel-`ViewDef`** und gilt für den **ganzen Tree** (alle
  Tiefen) — bewusst _pro Tree_, nicht pro Ebene: ein dichter, tiefer Task-Tree
  will mattere Connectoren als ein flacher; ein Tree auf farbiger Fläche eine
  andere Tönung als einer auf dem Basis-Hintergrund. So tunt jede Ansicht den
  Kontrast unabhängig, statt eine globale Connector-Farbe zu erzwingen.
- Greift nur im **Tree-Mode**; ohne `tree_label` ohne Wirkung.

#### `tree_lines:` / `tree_markers:` — Linien und Aufklappmarker pro Tree

Die beiden Bestandteile des Connector-Laufs sind getrennt konfigurierbar: die
Box-Linien (`├──`/`└──`/`│`) über `tree_lines`, die Aufklappmarker (`▶`/`▼`)
über `tree_markers`. Beide stehen — wie `tree_connector_style` — auf dem
**Wurzel-`ViewDef`** und gelten für den ganzen Tree.

```yaml
views:
  - name: databases
    tree_label: name
    tree_lines: false # Default true; false = nur Einrückung statt Linien
    tree_markers: # optional; weglassen = ▶/▼ wie gehabt
      enabled: true # false versteckt die Marker komplett
      collapsed: "+" # Default ▶
      expanded: "-" # Default ▼
```

- **`tree_lines: false`** ersetzt die Linien durch schlichte Einrückung
  (zwei Leerzeichen pro Tiefe). Warum: die Linien transportieren
  Geschwister-/Fortsetzungsstruktur — das lohnt sich auf tiefen, unregelmäßigen
  Bäumen (Tasks), ist aber visuelles Rauschen auf flachen, regelmäßigen Drills
  (Datenbank → Schema → Tabelle). Marker und `leaf_glyph` bleiben unberührt.
- **`tree_markers.enabled: false`** versteckt die Aufklappmarker; die Zeilen
  bleiben über die üblichen Tasten aufklappbar, nur der visuelle Hinweis
  entfällt. `collapsed`/`expanded` überschreiben einzeln die Glyphen — z. B.
  `+`/`-` für einen kompakteren Look oder Nerd-Font-Icons.
- Beide greifen nur im **Tree-Mode**; ohne `tree_label` ohne Wirkung. Das
  Blatt-Symbol konfiguriert weiterhin `leaf_glyph` (pro Ebene), die Farbe des
  gesamten Laufs `tree_connector_style`.

#### `expand_depth:` — initiale Aufklapptiefe pro Tree

```yaml
views:
  - name: tasks
    tree_label: description
    expand_depth: 2 # Tiefe 0 und 1 klappen nach dem Laden automatisch auf
  - name: tree
    tree_label: task
    expand_depth: all # immer komplett ausgeklappt (z. B. Trackings-Tree)
```

Lazy geladene Trees starten standardmäßig komplett zugeklappt — richtig für
teure Remote-Adapter (Postgres, Confluence), falsch für billige In-Memory-
Forests (Tasks), wo der User sein Arbeitsset sofort sehen will. `expand_depth`
auf dem **Wurzel-`ViewDef`** klappt nach dem (Neu-)Laden der Root-Liste alle
Zeilen mit Tiefe `< expand_depth` automatisch auf — `2` zeigt also drei Ebenen
(Wurzeln, Kinder, Enkel) und spiegelt das native
`tasks.tree.default_expand_depth`.

- **One-Shot-Kaskade:** Jede Ebene lädt über den normalen Expand-Pfad
  (dieselben Requests wie ein manuelles Enter). Sobald nichts mehr zu laden
  ist, deaktiviert sich die Kaskade — manuelles Auf-/Zuklappen wird danach
  nie überschrieben. Eine neue Saved-Query startet die Kaskade auf dem
  gefilterten Tree erneut.
- **`expand_depth: all`:** keine Tiefen-Obergrenze — die Kaskade läuft, bis
  eine Runde nichts Aufklappbares mehr findet. Für kleine In-Memory-Trees
  gedacht, die immer komplett offen sein sollen (z. B. der Trackings-Tree,
  native Parität); auf Remote-Adaptern stattdessen eine Zahl verwenden.
- **Kosten:** eine Runde Adapter-Calls pro Ebene (Fan-out pro Knoten). Auf
  Remote-Adaptern klein halten; `0`/weggelassen = aus (Default, bisheriges
  Verhalten).
- **Reload erneuert aufgeklappte Ebenen:** Landet ein Root-Reload (die
  `r`-Reload-Action, ein `Invalidation::All` des Adapters oder der
  Nach-Mutation-Reload einer Action) in einem Tree-Pane, werden zusätzlich
  die Children **jedes aufgeklappten** Knotens re-fetcht — dieselben
  Requests wie ein manuelles Zu-/Aufklappen, die alten Zeilen bleiben bis
  zum Eintreffen sichtbar (kein Flackern). Ohne das blieben tiefere Ebenen
  auf dem Stand vor dem Reload (z. B. ein gerade gestartetes Tracking ohne
  `⏱`-Marker auf einem verschachtelten Task). Aufgeklappte Pfade, die
  unter einem **zugeklappten** Vorfahren verborgen sind, werden nicht
  erneuert — sie holen sich frische Daten beim nächsten Aufklappen.

#### Eager-Subtree (`supports_eager_subtree`) — der ganze Baum in einem Call

Die oben beschriebene Kaskade ist korrekt, aber teuer: pro Ebene ein
Adapter-Fan-out und pro gelandeter Antwort ein Tree-Rebuild. Bei
`expand_depth: all` über einen tiefen In-Memory-Forest wird das quadratisch
(O(N²) Rebuilds), weil jeder einzelne Knoten seine Kinder separat anfordert
und der Baum nach jeder Antwort neu flach gerechnet wird. Für Adapter, deren
Daten ohnehin komplett im Speicher liegen (Tasks, Trackings), ist das reine
Verschwendung — sie könnten den ganzen Teilbaum in einem Rutsch liefern.

Dafür gibt es den Vertrags-Zusatz `list_subtree(params, depth)` auf dem `Node`-
Trait und das Capability-Gate `supports_eager_subtree` auf `AdapterCapabilities`:

- **`list_subtree(params, depth)`** liefert einen rekursiven `Subtree`
  (`{ items: Vec<SubtreeNode>, page }`, jeder `SubtreeNode` trägt sein
  `summary` plus seinen eigenen `children: Subtree`). `depth` ist die
  Ziel-Ebene: `list_subtree(depth)` liefert `depth + 1` sichtbare Ebenen, also
  exakt die Tiefen `0..=depth`, die die Kaskade erreichen würde. Die
  **Default-Implementierung** rekursiert über `list()` + `get_child()` (ein
  Call pro Knoten, identisch zur Kaskade, nur server-seitig gebündelt) — jeder
  Adapter erbt sie kostenlos. In-Memory-Adapter **überschreiben** sie mit einem
  reinen Projektions-Walk über ihren Snapshot (kein I/O, ein Durchlauf). Knoten
  mit `has_children == Some(false)` werden nicht weiter abgestiegen.
- **`supports_eager_subtree`** schaltet die TUI von der Kaskade auf einen
  einzigen `list_subtree`-Call um: Sobald die Root-Liste gelandet ist, fragt die
  Engine — falls `expand_depth` non-zero ist — den ganzen erwarteten Teilbaum
  (`all` → unbegrenzt, `Levels(n)` → Tiefe `n`) in **einem** Call an und legt ihn
  in **einem** Pass in den Tree-Cache (`ingest_subtree_level`), gefolgt von
  **einem** Rebuild. Das Pfad-Schema ist byte-genau das der Kaskade
  (`parent_path + [node.id]`), damit Selektion, Collapse und Re-Expand
  ununterscheidbar bleiben.
- **Warum Gate statt immer:** Remote-Adapter (Jira, Taiga, Postgres,
  Confluence) melden `supports_eager_subtree: false` und behalten die
  progressive Kaskade — ein einzelner blockierender Call über viele Ebenen würde
  die UI einfrieren, während die Kaskade Ebene für Ebene nachlädt und sichtbar
  Fortschritt zeigt. Eager lohnt nur, wenn der Adapter den Baum ohne Netz-I/O
  liefern kann.
- **Fallback:** Schlägt der eager Call fehl, fällt die Engine automatisch auf
  die Kaskade zurück (`drive_tree_auto_expand`) — der Baum klappt dann eben
  progressiv auf. Pagination (`… N weitere`) und Live-Row-Patches bleiben
  unberührt, weil pro Ebene dieselbe `PageInfo` durchgereicht wird und die IDs
  identisch sind.

#### Column-Config-Popup (`c`) — Sichtbarkeit & Reihenfolge zur Laufzeit

Das Column-Config-Popup (`common.column_config`, Default `c`) funktioniert auf
Content-Tabs genauso wie auf den nativen Tasks/Trackings-Tabs: Spalten
ein-/ausblenden (`Space`) und umsortieren (`Ctrl+D`/`Ctrl+F`), angewendet mit
`Enter`. Es existiert, damit User das Spalten-Layout an ihre Arbeit anpassen
können, **ohne die View-YAML zu editieren** — die YAML bleibt die geteilte
Default-Definition, das Popup ist der persönliche Override darüber.

- **Pro Level konfigurierbar:** Jede Ebene hat ihr eigenes Layout — die
  Wurzel-View, jede gedrillte Child-Ebene und im Tree-Mode jede
  `node_type_chain` (Cursor-Zeile entscheidet, welche Ebene konfiguriert
  wird). Splits derselben Ebene teilen sich das Layout.
- **Persistenz:** Eine Settings-Row pro Tab (`content_columns:<Tab-Name>`,
  JSON-Map Level-Key → sichtbare Spalten-Keys in Reihenfolge), geladen beim
  Start.
- **Reset-Semantik:** Entspricht die Auswahl wieder exakt der
  YAML-Reihenfolge, wird der Override entfernt (und bei leerer Map die
  Settings-Row gelöscht) — ein zurückgesetztes Layout hinterlässt keinen
  Zustand, der spätere YAML-Änderungen verdecken könnte.
- **`tree_label`-Spalte ist fix:** Sie trägt den Tree selbst (Connectoren,
  Einrückung) und kann nicht ausgeblendet werden.
- **Auto-Fallback-Ebenen** (keine `columns:` in der YAML, Schema aus den
  Item-Metadaten abgeleitet — z. B. Postgres-Rows) sind nicht konfigurierbar;
  `c` meldet das per Notification. Es gibt dort keine stabile
  Spalten-Identität, an der ein Override über Reloads hinweg festmachen
  könnte.

### Zweites Beispiel: `confluence.yaml`

```yaml
tab:
  name: Wiki
  order: 4

adapter:
  type: confluence
  config: confluence-prod.yaml

views:
  - name: Spaces
    node_type: confluence:space
    default: true
    columns:
      - key: key
        label: Key
        sizing: max
        style: accent
      - key: name
        source: label
        label: Name
        sizing: flex(1)
    children:
      - name: Pages
        key: Enter
        node_type: confluence:page
        columns:
          - key: title
            source: label
            label: Title
            sizing: flex(1)
          - key: last_modified
            label: Modified
            sizing: max
          - key: author
            label: Author
            sizing: max
        preview:
          enabled: true
          source: content
        actions:
          - name: Edit
            key: e
            type: edit
            edit:
              content: true
          - name: Create Page
            key: a
            type: create
```

### Beispiel: Adapter-Config `jira-globex.yaml`

Die Adapter-Config ist adapter-spezifisch. Für Jira z.B.:

```yaml
url: https://jira.example.com
session_id: "JSESSIONID=abc123; atlassian.xsrf.token=..."

# Adapter-internes Caching (optional, Defaults im Adapter)
cache:
  labels:
    enabled: true
    ttl: 3600
  users:
    enabled: true
    ttl: 3600
```

Das Caching ist Adapter-intern — der Adapter braucht es, um z.B.
Labels/Users für Autocomplete im Editor-Template bereitstellen zu können.
Die generische View-YAML weiß nichts davon.

---

## Adapter-Konstruktion

### Aktuell (Jira-spezifisch)

```rust
JiraAdapter::from_connection(&JiraConnection) -> Result<JiraAdapter>
```

### Generisch (über AdapterFactory)

```rust
pub trait AdapterFactory: Send + Sync {
    fn adapter_type(&self) -> &str;
    fn create(&self, config: &str) -> Result<Box<dyn ContentAdapter>>;
}
```

Die TUI:

1. Liest `adapter.type` aus der View-YAML → z.B. `"jira"`
2. Findet die registrierte `AdapterFactory` für diesen Typ
3. Liest `adapter.config` (Dateiinhalt) oder `adapter.config_inline`
4. Ruft `factory.create(config_string)` auf
5. Erhält einen `Box<dyn ContentAdapter>`

Der Adapter parst den Config-String intern — kann YAML, JSON, TOML,
Connection-String oder was auch immer sein.

---

## Caching

### Verantwortung: Adapter-intern

Caching ist **komplett Adapter-intern**. Die TUI weiß nichts davon.

**Warum?** Der Adapter braucht gecachte Daten für seine eigene Logik:

- Editor-Templates mit Autocomplete-Hints (Labels, Users, Status-Werte)
- Schema-Discovery (welche Felder gibt es?)
- Vermeidung redundanter API-Calls bei schnellen list/get_by_id-Folgen

### Schnittstelle zur TUI

Die TUI braucht keinen expliziten Cache-Zugriff. Stattdessen liefert der
Adapter die gecachten Daten implizit über die bestehenden Trait-Methoden:

- `editor_template()` enthält bereits die Autocomplete-Hints aus dem Cache
- `schema()` liefert Felder inkl. `allowed_values` aus dem Cache
- `list()` kann intern cachen, die TUI merkt es nicht

Einzige TUI-Interaktion: ein generisches "Refresh" könnte
`adapter.invalidate_all()` aufrufen, falls der Adapter es anbietet.

### Adapter-interne Implementierung

Jeder Adapter entscheidet selbst über seine Cache-Strategie. Z.B.:

```rust
// Im JiraAdapter intern
struct JiraCache {
    labels: Option<(Vec<String>, Instant)>,   // (data, loaded_at)
    users: Option<(Vec<JiraUser>, Instant)>,
    ttl: Duration,
}

impl JiraCache {
    fn labels(&self) -> Option<&[String]> { /* prüft TTL */ }
    async fn ensure_labels(&mut self, client: &JiraClient) -> &[String] { /* lazy load */ }
}
```

Konfigurierbar über die adapter-eigene Config (z.B. `cache.labels.ttl`
in `jira-globex.yaml`), nicht über die View-YAML.

---

## Editor-Templates

### Problem

Aktuell baut die TUI das Editor-Template manuell zusammen
(`"# KEY: summary\n\ndescription"`). Das ist Jira-spezifisch.

### Lösung: Adapter gibt Templates vor

Erweiterung des `Node`-Traits (nicht `Content`, da auch Metadata-Felder
einbezogen werden):

```rust
#[async_trait]
pub trait Node: Send + Sync {
    // ... bestehende Methoden ...

    /// Template für den externen Editor generieren.
    /// Enthält editierbare Felder + Content-Body in einem
    /// für den Benutzer lesbaren Format.
    /// `editable_fields`: welche Metadata-Keys editierbar sein sollen
    /// (aus der View-YAML actions.edit.metadata).
    async fn editor_template(&self, editable_fields: &[String]) -> Result<String>;

    /// Editor-Ausgabe parsen → (metadata_changes, new_content_body).
    /// Gibt einen Fehler zurück wenn das Format ungültig ist.
    fn parse_editor_output(
        &self,
        text: &str,
    ) -> Result<(Vec<MetadataChange>, Option<Vec<u8>>)>;
}
```

### Beispiel: Jira Issue Template

```
# PROJ-202
# Type: Bug | Status: In Progress | Priority: High
# ─────────────────────────────────────────────────
summary: Fix login timeout on mobile devices
labels: mobile, auth, bug       # available: mobile, auth, bug, feature, backend, ...
assignee: jane.doe           # available: jane.doe, john.smith, ...
# ─────────────────────────────────────────────────

Login timeout occurs after 30 seconds on iOS devices...
```

Der Adapter generiert dieses Template und weiß auch wie er es zurück-parst:

- Zeilen mit `#` am Anfang = Kommentare (read-only Info)
- `key: value` Zeilen vor der Trennlinie = editierbare Metadata
- Alles nach der Trennlinie = Content-Body

Die `# available:` Hints kommen aus dem adapter-internen Cache. Die TUI
muss sich darum nicht kümmern.

### Beispiel: Confluence Page Template

```
# Space: DEV | Last modified: 2024-03-15
# ─────────────────────────────────────────────────
title: Architecture Decision Records
labels: architecture, adr
# ─────────────────────────────────────────────────

## ADR-001: Use PostgreSQL for persistence
...
```

### Generischer Editor-Flow in der TUI

```
1. TUI ruft node.editor_template(editable_fields) auf
   → Adapter generiert Template (mit Autocomplete-Hints aus Cache)
2. TUI öffnet externen Editor mit dem Template
3. User editiert und speichert
4. TUI ruft node.parse_editor_output(text) auf
   → Adapter parst zurück: (metadata_changes, new_content)
5. TUI ruft node.update_metadata(changes, version) und/oder
   node.content_mut().write(content, version) auf
```

Die TUI kennt das Template-Format nicht — der Adapter ist dafür zuständig.

---

## Navigation: Breadcrumbs & Zurück

Wenn der User in Kind-Nodes navigiert (Ticket → Comments), braucht die TUI
einen Navigations-Stack:

```
Jira > Tickets > PROJ-202 > Comments
```

### Implementierung

```rust
struct ContentView {
    adapter: Arc<dyn ContentAdapter>,
    view_config: ViewConfig,          // aus der YAML
    nav_stack: Vec<NavFrame>,         // Breadcrumb-Stack
}

struct NavFrame {
    node_id: String,                  // ID des Parent-Nodes
    node_label: String,               // für Breadcrumb-Anzeige
    view_def: ChildViewDef,           // column/action Config
    scroll_position: usize,           // für Restore beim Zurückgehen
    selected_index: usize,
}
```

- `Enter` / konfigurierter Key → push Frame, Liste der Kind-Nodes laden
- `Esc` / `Backspace` → pop Frame, zurück zur Elternliste
- Breadcrumb wird in der Tab-Bar oder als eigene Zeile angezeigt

---

## Per-Node-Aktionen (`shortcuts:`)

Neben den `actions:`-Einträgen einer View (Refresh, Filter, Search,
…) können einzelne Nodes _eigene_ Aktionen anbieten — z. B. eine
TableNode bietet `edit_sql`, eine DbScriptNode bietet `execute`,
`edit`, `delete`. Diese werden vom Adapter via
[`Node::actions()`](../not-yet-done-content/src/lib.rs)
ausgewiesen und vom YAML über die `shortcuts:`-Map an Tasten gebunden.

```yaml
children:
  - name: DB Script
    node_type: "postgres:db_script"
    shortcuts:
      x: execute # → Node::invoke_action("execute", …)
      e: edit # → Node::invoke_action("edit", …)
      d: delete # → Node::invoke_action("delete", …)
```

Eine `shortcuts:`-Map gibt es sowohl auf der View-Ebene
(`ViewDef.shortcuts`) als auch auf jeder `ChildDef`. Der TUI-Resolver
wählt den tiefsten passenden Match entlang der `node_type`-Kette;
greift keiner, fällt er auf die View-Ebene zurück.

Aktions-Werte können mit dem Prefix `parent:` versehen werden — der
Resolver feuert dann gegen den unmittelbaren Eltern-Node, nicht
gegen den selektierten. Beispiel:

```yaml
- name: Rows
  node_type: "postgres:row"
  shortcuts:
    Q: "parent:edit_sql" # → wirkt auf den zugrundeliegenden Table-Node
```

Was `Node::invoke_action(name, ctx)` zurückgibt, beschreibt das
[`ActionDispatch`](../not-yet-done-content/src/lib.rs)-Enum
(`OpenEditor`, `ExecuteQuery`, `CreateChild`, `DeleteSelf`, `Reload`,
`Noop`, `Error`). Die TUI übersetzt das in den passenden View-Flow —
ein Editor öffnet sich, eine Query landet in einem paginierten
Result-Pane, ein Delete spawnt einen Confirm-Popup.

Validator (start-time): leere Action-IDs werden abgelehnt; ein
`shortcuts:`-Key, der bereits von einem `actions:`-Eintrag der
gleichen View belegt ist, ebenfalls.

### Strukturierte Eingabe-Formulare (`InputSpec::Form`, M6/E5)

Eine Action kann statt eines externen Editors (`InputSpec::Editor`) ein
**generisches Formular** im Terminal anfordern. Der Adapter deklariert
die Felder im Action-Deskriptor; die TUI rendert sie generisch (Text-,
Select- und Toggle-Widget aus `ratatui_form_widgets`), sammelt die
Werte und liefert sie über `ActionInput::Form(HashMap<String,String>)`
an `Node::execute` zurück. Es gibt **kein** YAML hierfür — die
Feldstruktur ist Adapter-Wissen, nicht View-Konfiguration.

```rust
// im Adapter, in Node::actions():
NodeAction::new(
    "edit",
    "Edit",
    InputSpec::Form {
        fields: vec![
            FormFieldSpec::text("title", "Title"),
            FormFieldSpec::select("status", "Status",
                vec!["todo".into(), "in_progress".into(), "done".into()]),
            FormFieldSpec::toggle("urgent", "Urgent"),
        ],
    },
)
```

Feldtypen (`FormFieldKind`):

| Kind     | Widget               | Wert in `ActionInput::Form`        |
| -------- | -------------------- | ---------------------------------- |
| `Text`   | einzeiliges Textfeld | freier String                      |
| `Select` | horizontale Auswahl  | gewählter `allowed_values`-Eintrag |
| `Toggle` | An/Aus               | `"true"` / `"false"`               |

Pro Feld:

- **`required`** (Default: `text`/`select` = true, `toggle` = false) —
  die TUI blockiert das Absenden, solange ein Pflichtfeld leer ist, und
  zeigt einen Hinweis in der Popup-Fußzeile.
- **`default`** — statischer Initialwert. Für ein Edit-Formular
  überschreibt der Adapter ihn pro Feld über **`Node::form_prep(action_id)`**
  (liefert eine `HashMap<key → Initialwert>`; fehlende Keys fallen auf
  `default` zurück). Der Default-Trait gibt eine leere Map zurück — das
  passt für ein reines Anlege-Formular.

Bedienung im Popup: `tab`/`↑`/`↓` wechselt das Feld, `←`/`→` bewegt den
Cursor (Text) bzw. die Auswahl (Select), `space` wählt die Option unter
dem Cursor bzw. kippt den Toggle, `enter` sendet ab, `esc` bricht ab.

Warum ein Formular statt eines Editor-Templates: für kleine, klar
typisierte Eingaben (Status-Auswahl, Boolean-Flag, kurzer Titel) ist ein
strukturiertes Popup schneller und fehlerärmer als ein YAML-Buffer im
`$EDITOR`. Für lange Freitexte (Issue-Body, Wiki-Seite) bleibt
`InputSpec::Editor` die richtige Wahl. Beide Wege sind uniform über alle
Adapter nutzbar.

### Markieren & Verschieben (`mark-move` / `paste-move`, M7/E6)

Strukturelle Verschiebungen (einen Task umhängen, eine Seite in einen
anderen Knoten ziehen) laufen über ein generisches **Move-Clipboard**.
Zwei Standard-Action-Namen bilden das Vokabular:

- **`mark-move`** — merkt sich den aktuellen Knoten als Verschiebe-Quelle.
  Reine Frontend-Session-State; der Adapter gibt `ActionDispatch::Noop`
  zurück (er muss die Action nur in `Node::actions()` listen, damit das
  Keybinding/Hint-Greift). Die TUI zeigt die markierte Quelle als
  Indikator in der Status-Bar (`move: <label>`), bis Paste oder `esc`.
- **`paste-move`** — die TUI ruft `Node::invoke_action("paste-move", ctx)`
  auf dem **Ziel**-Knoten auf, wobei `ctx.marked` die markierte Quelle
  trägt (`ActionContext.marked: Option<MarkedNode>`). **Der Adapter führt
  den Move aus** (Reparent/Relocate) und gibt `ActionDispatch::Reload`
  zurück; die TUI lädt das Ziel-Pane neu und leert das Clipboard.

```rust
// im Adapter, in Node::invoke_action():
async fn invoke_action(&self, name: &str, ctx: &ActionContext)
    -> Result<ActionDispatch> {
    match name {
        "mark-move" => Ok(ActionDispatch::Noop), // Clipboard ist Frontend-State
        "paste-move" => match &ctx.marked {
            Some(src) => {
                // src.node_id / src.node_type prüfen, dann verschieben …
                self.reparent(&src.node_id, self.id()).await?;
                Ok(ActionDispatch::Reload)
            }
            None => Ok(ActionDispatch::Error("nichts markiert".into())),
        },
        _ => Ok(ActionDispatch::Noop),
    }
}
```

`MarkedNode` trägt `node_id` (Adapter-lokale id, wie von `get_by_id`
akzeptiert), `node_type` (damit das Ziel inkompatible Typen ablehnen kann)
und `label` (für den Indikator). Die Move-Semantik liegt vollständig im
Adapter — er allein kennt seine Hierarchie und Restriktionen; die TUI
hält nur das Clipboard und reicht es beim Paste durch.

Warum ein generischer Mechanismus statt bespoke Cut/Paste pro View: der
native Tasks-Tree, die Link-Funktion und die DB-Skript-Ordner trugen
bisher je eigene Mark/Paste-Pfade. Mit `ActionContext.marked` +
`mark-move`/`paste-move` profitiert jeder Adapter (ab A1 der TaskAdapter)
vom selben Clipboard, ohne TUI-Code anzufassen.

## Pagination-Modi (`pagination:`)

Jede ChildDef kann ihren Pagination-Mode konfigurieren:

```yaml
- name: Rows
  node_type: "postgres:row"
  pagination:
    mode: server # oder: cursor
    page_size: 100
```

- **`server`** — der Adapter zieht eine `PageRequest { offset, limit }`
  und wickelt den Pull über `LIMIT`/`OFFSET` ab (oder vergleichbare
  serverseitige Pagination). Geeignet, wenn `ORDER BY` / Stabilität
  über Page-Grenzen hinweg gefordert ist.

- **`cursor`** — der Adapter hält einen serverseitigen Cursor (Postgres:
  `DECLARE … CURSOR FOR …` in einer offenen TX) über die Lebensdauer
  des Result-Panes. `>`/`<` rufen `FETCH FORWARD N` / re-open. Hinweise:
  - Multi-Statement-Bodies (z. B. `CREATE TEMP TABLE … ; SELECT …`)
    sind unterstützt — alle nicht-SELECTs laufen als Prelude in der
    selben TX, der finale SELECT wird zum Cursor.
  - `ORDER BY` ist **nicht** automatisch — Reihenfolge entspricht dem
    Cursor-Plan. Wer stabile Reihenfolge braucht, schreibt sie ins
    Statement.
  - Backward-Navigation (`<`) re-öffnet den Cursor (NO SCROLL).
  - Pane-Close emittiert `CloseAdapterCursor`, die TX wird beendet.
    Bei `query_timeout_secs`-Timeout wird der gesamte Connection-Pool
    aufgeräumt; aktive Cursor sterben mit, Pane zeigt "cursor lost".

## Edit-in-Place (`editor_in_place:`)

Eine ChildDef kann opten, Editor-Tempfiles **im Zielverzeichnis** statt
in `$TMPDIR` anzulegen:

```yaml
- name: DB Script
  node_type: "postgres:db_script"
  editor_in_place: true
  shortcuts:
    e: edit
```

**Wann sinnvoll**: wenn ein externer Editor / Language Server
Konfigurations- oder Projektkontext aus dem Pfad der geöffneten Datei
herleitet — z. B. `postgres-language-server.jsonc` neben dem Skript,
`.editorconfig`, `.clang-format`, `pyrightconfig.json`. Solche Tools
walken üblicherweise von der Datei nach oben, finden in `$TMPDIR`
aber nichts.

**Wie es funktioniert**: Die TUI legt das Tempfile mit dem
festen Prefix `.nyd_tmp_` und einer zufälligen Komponente direkt
unter `<instance_data_dir>/db_scripts/<db>/…/` an (also im selben
Verzeichnis wie das persistente Skript). Beim `:w` liest die TUI
den Buffer, strippt Editor-Only-Marker (Banner, Completion-Line)
und schreibt aufs reale Ziel. Anschließend wird das Tempfile
entfernt; das passiert auch wenn der Editor mit Fehler beendet wird,
weil die `NamedTempFile`-Drop-Logik dafür sorgt.

**Aufräum-Garantie**: Bei einem Crash der TUI mitten in einer
Editor-Session können `.nyd_tmp_*`-Dateien zurückbleiben. Sie sind
durch den Prefix klar als TUI-Artefakte erkennbar; löschen ist
gefahrlos.

**Default**: `false`. Andere Sessions (Tasks/Trackings/Filter) liegen
weiterhin in `$TMPDIR` — der Flag betrifft nur den Editor-Pfad der
ChildDef, auf der er gesetzt ist (aktuell genutzt vom Postgres-Adapter
für DB Scripts).

## Adapter-Child-Process-Environment

Adapter können beim Start eines Kindprozesses (Editor _oder_ Skript)
zusätzliche Environment-Variablen mitschicken. Konfiguration gibt es
keine — das Feature ist eine **Trait-Erweiterung des Adapters**:

```rust
fn child_process_env(&self, node: &NodeRef) -> HashMap<String, String>
```

**Wozu**: Der `postgres-language-server` (Editor-LSP für SQL) braucht
für jede Form von Completion eine echte Datenbankverbindung. Ein TUI-
Nutzer hat aber Tunnel-Port + Passwort _nur in der TUI_ — Hand-Pflege
einer `postgres-language-server.jsonc` mit Klartext-Passwort wäre eine
Verletzung der „keine echten Customer-Daten in/neben dem Repo"-Regel
und der dynamische Tunnel-Port wandert mit jedem Reconnect.

Der Postgres-Adapter beantwortet `child_process_env` mit:

| Variable     | Quelle                                                    |
| ------------ | --------------------------------------------------------- |
| `PGHOST`     | `TransportConnection::host` (`127.0.0.1` bei SSH-Tunnel)  |
| `PGPORT`     | dynamischer Tunnel-Port aus der live Verbindung           |
| `PGUSER`     | `postgres.user` aus `postgres-adapter.yaml`               |
| `PGPASSWORD` | resolved Passwort (z. B. aus `pass`), zur Laufzeit im RAM |
| `PGDATABASE` | zweites Segment der NodeRef (Fallback `admin_database`)   |
| `PGSSLMODE`  | spiegelt `postgres.sslmode`                               |

Solange die Adapter-Verbindung noch nicht offen ist, liefert die
Funktion eine leere Map (kein erzwungener Connect aus dem Sync-Path).

**TUI-Seite**: Die Variablen werden über
[`EditorSpawnContext`](#editor-spawn-context) bzw. die Skript-Spawn-
Pfade in `app/script.rs` per `Command::envs(map)` weitergereicht. Die
TUI kennt die Inhalte nicht — sie kopiert nur. Das ist die saubere
Architektur-Grenze: **Connection-Details bleiben beim Adapter**, die
TUI ist daten-/credential-agnostisch.

**Für andere Adapter**: Jira/Taiga implementieren das Default
(leere Map). Wenn ein zukünftiger Adapter eigene CLI-Tools per
`:script`/Editor anfahren möchte, kann er denselben Hook nutzen — z. B.
ein `git-jira`-Plugin mit `JIRA_HOST`/`JIRA_TOKEN`.

**Lifecycle**: Snapshot zum Spawn-Zeitpunkt. Reconnects ändern den
Port; bereits laufende Editor-Kinder behalten ihr altes Env (geht
schief, wenn der Reconnect _während_ einer LSP-Session passiert —
in der Praxis selten genug, dass kein Refresh-Mechanismus existiert).

<a id="editor-spawn-context"></a>**EditorSpawnContext**: Die TUI bündelt
Editor-Spawn-Knöpfe (Tempfile-Pfad/-Prefix für Edit-in-Place + Child-
Env) in einer Struct, die `EditSession::spawn_context()` liefert.
Neue Spawn-Time-Knöpfe (z. B. cwd, ulimit) lassen sich damit ergänzen,
ohne jede Session und jeden Dispatch-Aufruf anzufassen.

## Action-Typen

Die View-YAML kennt folgende generische Action-Typen:

| Typ            | Beschreibung                                     | Action Bar |
| -------------- | ------------------------------------------------ | ---------- |
| `fuzzy_filter` | Fuzzy-Suche über konfigurierbare Felder          | ✅ (modal) |
| `edit`         | Editor-Template vom Adapter, externer Editor     | ✅ (modal) |
| `create`       | Neuen Kind-Node erstellen (Schema vom Adapter)   | ✅ (modal) |
| `query_edit`   | Query im Editor bearbeiten                       | ✅ (modal) |
| `reload`       | Liste neu laden                                  | ❌         |
| `navigate`     | In Kind-Node-Ebene wechseln                      | ❌         |
| `open_url`     | URL aus Metadata im Browser öffnen               | ❌         |
| `download`     | `node.content().read()` → in Datei speichern     | ❌         |
| `custom`       | Adapter-spezifische Aktion (via `custom_action`) | ❌         |
| `delete`       | Node löschen (mit Bestätigung)                   | ❌         |

### Action Bar vs. Status Bar

Actions werden in zwei Bars angezeigt:

- **Action Bar** (oben): Actions mit persistentem/modalem Zustand. Diese
  übernehmen die Eingabe (Fuzzy-Filter-Eingabefeld) oder zeigen an, dass ein
  Editor gerade offen ist. Typen: `fuzzy_filter`, `edit`, `create`, `query_edit`.
  Zukünftig auch: custom scripts mit laufendem Prozess.

- **Status Bar** (unten): Fire-and-forget Actions die sofort ausgeführt werden
  und keinen anhaltenden Zustand haben. Typen: `reload`, `navigate`, `custom`,
  `open_url`, `download`.

Jede Action hat ein optionales `hide_from_bar: true` Flag, um den Default zu
überschreiben (z.B. eine edit-Action aus der Action Bar ausblenden).

### Editor-Profil pro Action (`editor:`)

`edit`- und `create`-Actions können optional ein **benanntes Editor-Profil**
wählen:

```yaml
actions:
  - { name: new, key: n, type: create, id: send_message, editor: compose-below }
  - { name: edit, key: e, type: edit, id: edit_message, editor: compose-below }
```

`editor:` referenziert einen Schlüssel unter dem Top-Level-Block `editors:`
in `tui.yaml` (ein Profil aus `command`/`inline`/`pause_tui`/…). Fehlt das
Feld, wird das Profil `default` verwendet. Ein unbekannter Profilname ist ein
**harter Validierungsfehler** beim Config-Laden.

Wozu: verschiedene Actions wollen verschiedene Editor-Geometrien — z.B. ein
Chat-Compose in einem schmalen Terminal-Split unten statt im vollen vsplit.
Da der Editor immer ein Fremdprozess ist (kein PTY-Embedding in ein TUI-Pane),
wird die Geometrie über das `command` des Profils vom Terminal realisiert.
Siehe `editors:` in der `tui.yaml`-Doku.

### Anwenden bei jedem Speichern (`commit_on_save`)

```yaml
actions:
  - {
      name: new,
      key: n,
      type: create,
      id: send_message,
      editor: compose-below,
      commit_on_save: true,
    }
```

Standardmäßig wird eine `edit`/`create`-Action erst beim **Schließen** des
Editors angewendet. Mit `commit_on_save: true` greift sie bei **jedem
Speichern (`:w`)**, während der Editor offen bleibt — gebaut für Chat-Compose:

- Das erste `:w` führt die Action aus (z.B. Nachricht senden).
- Erzeugt sie einen neuen Node (`ActionOutcome::Navigate`), schaltet die
  Session auf dessen Editor-Action um; jedes weitere `:w` **editiert** diesen
  Node in-place (z.B. die gerade gesendete Nachricht).
- Speichern ohne Änderung seit dem letzten Anwenden — auch mehrfaches `:w`
  hintereinander und das finale Schließen — ist ein No-op. Es wird also nie
  doppelt gesendet.

Voraussetzung: ein **detached** Editor-Profil (`inline: false`), damit
Zwischen-Speicherungen überhaupt beobachtbar sind (der Launch-/Detached-Pfad
überwacht die mtime der Temp-Datei). Default `false` — das Flag nur dort
setzen, wo dieses Verhalten gewollt ist; bei einem Jira-Ticket-Edit würde es
auf jedes `:w` einen halbfertigen Body pushen.

### Fuzzy Filter

```yaml
actions:
  - name: fuzzy filter
    key: f
    type: fuzzy_filter
    fuzzy_filter:
      fields:
        [key, summary] # optional — nur diese Felder durchsuchen
        # leer/absent = alle Felder + label
```

Der Fuzzy Filter filtert die aktuelle Liste live. In der Action Bar erscheint
ein Eingabefeld. `fields` erlaubt die Einschränkung auf bestimmte Metadata-Keys.
Der Spezialwert `label` sucht im Node-Label (meist der Titel/Summary).

**Im Tree-Mode filtert er per Pfad-Pruning über _alle_ Ebenen:** ein Knoten
bleibt sichtbar, wenn er selbst matcht **oder** einen matchenden Nachfahren
hat. So tauchen Treffer samt ihrer Vorfahren-Kette auf, während nicht-
matchende Geschwister-Teilbäume verschwinden. Ein tief verschachtelter
Treffer wird also gefunden und über seine Eltern sichtbar gemacht — auch
wenn die `fuzzy_filter`-Action nur auf der Wurzel-View deklariert ist
(sie dient dort nur als Schalter, der den Filter scharf macht). Die Suche
ist auf die aktuell geladenen/aufgeklappten Knoten beschränkt: ein Treffer
in einem eingeklappten oder noch nicht nachgeladenen Ast wird erst sichtbar,
wenn dieser Ast geladen ist.

### Custom Actions

Adapter registrieren eigene Aktionen:

```rust
pub trait ContentAdapter: Send + Sync {
    // ...
    fn custom_actions(&self, node_type: &NodeType) -> Vec<CustomAction>;
    async fn execute_action(
        &self,
        node_id: &str,
        action_id: &str,
        input: Option<&str>,
    ) -> Result<()>;
}

pub struct CustomAction {
    pub id: String,
    pub label: String,
    pub needs_input: bool,            // z.B. Transition braucht Ziel-Status
    pub allowed_values: Option<Vec<String>>,  // für Popup/Dropdown
}
```

Beispiel in der YAML:

```yaml
actions:
  - name: Transition
    key: t
    type: custom
    custom_action: transition # → Popup mit allowed_values
  - name: Assign
    key: a
    type: custom
    custom_action: assign # → Popup mit User-Liste
```

---

## Offene Fragen & Schwierigkeiten

### 1. Async & Ladezeiten

Problem: `list()` für Labels/Users kann Sekunden dauern (Jira fan-out).
Die TUI darf nicht blockieren.

**Vorschlag**:

- Alle Adapter-Aufrufe grundsätzlich async über den bestehenden LoadMsg-Channel
- TUI zeigt "Loading..." Indikator
- Adapter-interner Cache macht die meisten Folge-Aufrufe instant

### 2. Schema-Discovery

Wenn ein User `type: create` konfiguriert, muss die TUI wissen welche Felder
beim Erstellen angegeben werden können. Aktuell nicht im ContentAdapter.

**Vorschlag**: Node-Typ liefert Schema-Info:

```rust
pub trait ContentAdapter: Send + Sync {
    // ...
    fn schema(&self, node_type: &NodeType) -> Option<Vec<FieldSchema>>;
}

pub struct FieldSchema {
    pub key: String,
    pub label: String,
    pub field_type: FieldType,       // Text, Select, MultiSelect, Date, User
    pub required: bool,
    pub default: Option<String>,
}

pub enum FieldType {
    Text,
    Select,         // Dropdown, allowed_values kommen vom Adapter/Cache
    MultiSelect,    // z.B. Labels
    Date,
    User,           // Autocomplete aus cached Users
}
```

### 3. Dynamische vs. Statische Spalten

Die YAML definiert feste Spalten. Aber manche Adapter haben dynamische Felder
(Jira Custom Fields, DB-Spalten). Zwei Optionen:

a) YAML definiert alles explizit → User muss Custom Fields kennen
b) `columns: auto` → Adapter liefert die Spalten basierend auf dem ersten
Result-Set

**Vorschlag**: Beides unterstützen. `auto` als Default, explizite
Konfiguration überschreibt.

### 4. Bulk-Aktionen

Mehrere Nodes selektieren und gleichzeitig bearbeiten (z.B. 5 Tickets
transitionieren). Braucht Multi-Select in der Tabelle.

**Vorschlag**: Erstmal out-of-scope, aber das Datenmodell sollte es nicht
verhindern. `actions` könnten ein `bulk: true` Flag bekommen.

### 5. Abhängigkeit Tasks ↔ Content-Views

Tasks/Trackings sind native Tabs mit eigener DB-Logik. Aber es gibt
Querverbindungen:

- Task mit Jira-Ticket verlinken
- Tracking automatisch stoppen wenn Jira-Ticket transitioniert wird

**Vorschlag**: Erstmal unabhängig lassen. Spätere Integration über ein
Event-System oder Hooks möglich.

### 6. Hot-Reload der View-Konfiguration

Wenn der User eine YAML ändert, soll sich der Tab sofort aktualisieren
(ohne Neustart). Braucht File-Watch auf das views/-Verzeichnis.

**Vorschlag**: Nice-to-have. Erstmal nur beim Start laden. Refresh via
`:reload-views` Command.

---

## Implementierungsplan

### Phase 1: Traits erweitern (`not-yet-done-content`) ✅

Erweitere das Content-Trait-Crate um die fehlenden Abstraktionen.
App bleibt lauffähig — nur neue Trait-Methoden mit Default-Impls.

- [x] `editor_template()` und `parse_editor_output()` auf `Node` Trait
- [x] `custom_actions()` und `execute_action()` auf `ContentAdapter`
- [x] `FieldSchema`, `FieldType`, `CustomAction`, `EditorOutput` Typen
- [x] `schema()` auf `ContentAdapter`

### Phase 2: JiraAdapter auf Config-String umstellen ✅

Generische Konstruktion über `AdapterFactory`. JiraAdapter erhält
internen Cache für Labels/Users (für Templates/Autocomplete).

- [x] `JiraAdapterFactory` implementieren (`create(config_string)`)
- [x] Config-Parsing aus YAML-String (url, auth, cache settings)
- [x] Interner Cache mit TTL (Labels, Users) — in `JiraRoot`, shared via `Arc<Mutex<JiraCache>>`
- [x] `from_connection()` bleibt als Convenience

### Phase 3: Editor-Templates im JiraAdapter ✅

Adapter generiert und parst Editor-Templates. TUI-Code wird generisch.

- [x] `editor_template()` auf `JiraIssueNode` — mit Autocomplete-Hints aus Cache
- [x] `parse_editor_output()` auf `JiraIssueNode`
- [x] TUI `editor.rs`: generischer `ContentEdit` EditorAction + `process_content_edit()`
- [x] TUI `mod.rs`: `OpenJiraTicketEditor` nutzt `editor_template()` statt hartcodiertem Template
- Alter `JiraTicketEdit` Pfad noch vorhanden als Legacy-Fallback

### Phase 4: ViewConfig YAML-Parser ✅

Deklarative View-Konfiguration laden und parsen.

- [x] Rust-Structs für YAML-Struktur (TabConfig, ViewDef, ColumnDef, ActionDef, ChildDef, PreviewConfig, QueryConfig, EditConfig, SavedQuery)
- [x] `load_views()`: `~/.config/not_yet_done/views/*.yaml` laden
- [x] AdapterFactory-Registry (HashMap<String, Box<dyn AdapterFactory>>)
- [x] Adapter-Instanzen aus Config-String erstellen (inline oder Dateireferenz)

### Phase 5: ContentView Komponente 🔧 (in Arbeit)

Generische TUI-Komponente die JiraView ersetzt.

- [x] `ContentView` Struct mit `ViewFileConfig` + `Arc<dyn ContentAdapter>` — Skeleton erstellt
- [x] Tabelle aus `ColumnDef` + `NodeSummary` Metadata aufbauen
- [x] Preview-Pane aus `PreviewConfig`
- [x] Keybindings aus `ActionDef` (config-driven actions)
- [ ] **App-Integration: `Tab::Jira` → `Tab::Content(usize)`, `jira_view` → `content_views: Vec<ContentView>`**
      Großer koordinierter Umbau über ~6 Dateien: mod.rs, editor.rs, render.rs, tab_bar.rs, tabs/mod.rs, jira_view.rs.
      Empfohlener Ansatz: erst mechanische 1:1 Migration, dann YAML-Loading.
- [ ] Generischer Action-Handler (edit, create, delete, reload, download, open_url, custom)
- [ ] Saved Queries / Favorites aus `QueryConfig`
- [ ] App: dynamische Tab-Erzeugung aus geladenen ViewConfigs
- [ ] JiraView + Legacy JiraTicketEdit Pfad entfernen

### Phase 6: NavStack & Children ⬜

Breadcrumb-Navigation für verschachtelte Node-Typen.

- [ ] `NavFrame` Struct (node_id, label, view_def, scroll_pos, selected_idx)
- [ ] Push/Pop Logik mit Scroll-Position-Restore
- [ ] Breadcrumb-Rendering (Zeile über der Tabelle oder in Tab-Bar)
- [ ] `ChildDef` aus YAML → automatisch Enter-Keybinding für Navigation
