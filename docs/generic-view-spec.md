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

    # Aktionen auf selektierten Nodes.
    #
    # `key:` darf eine Einzeltaste (`e`) ODER ein Mehrzeichen-Chord
    # (`al`, `ay`) sein. Chords funktionieren auf Content-Tabs ohne
    # weitere Verdrahtung: Der App-Chord-Interceptor kennt zwar nur die
    # getypten `keybindings.*`-Sektionen, fragt aber zusätzlich die
    # View-Keymap über `ContentView::yaml_action_chord_prefix` ab — das
    # erste Zeichen eines Chords wird als Präfix gestasht, das zweite
    # löst aus. (Node-`shortcuts:` sind dagegen per Definition
    # einzeichig und nie Chords.)
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

#### `record_detail:` — Datensatz-Detailansicht im Split (`o`)

Steht auf `ViewDef` **und** `ChildDef` (gleiche Ebene wie `column_cursor`),
default `false`. Auf einer so markierten **flachen Tabellen-Ebene** öffnet die
Taste `o` rechts einen gekoppelten Split, der den **aktuell selektierten
Datensatz transponiert** zeigt: eine Zeile pro Feld, Spalte 1 = Feldname,
Spalte 2 = Feldwert. Bewegt sich der Cursor in der Quelltabelle, aktualisiert
sich die Detailansicht automatisch (sie folgt der Auswahl Frame für Frame).
`o` erneut schließt den Follower wieder; `X` schaltet im Follower den
Wert-Umbruch um (default aus → Werte einzeilig geclippt; an → lange
Werte/harte Zeilenumbrüche werden auf Fortsetzungszeilen umbrochen).

```yaml
- name: Rows
  node_type: "postgres:row"
  column_cursor: true
  record_detail: true
```

**Warum es das gibt:** Postgres-Zeilen (und Skript-Ergebnisse) haben oft sehr
viele, breite Spalten — einen einzelnen Datensatz über all diese Spalten hinweg
zu lesen ist in der Zeilenansicht mühsam (man scrollt horizontal). Die
transponierte Detailansicht stellt _einen_ Record vollständig untereinander dar,
ohne dass die Tabellenansicht ihr Layout verliert.

Verhalten / Grenzen:

- **Nur flache Ebenen.** Tree-Ebenen sind ausgeschlossen — ein Tree klappt
  Records ohnehin inline auf, und der Detail-Split zielt auf breite _flache_
  Zeilen (Postgres-Rows, Skript-Ergebnisse). Ein Follower bietet `o` selbst
  nicht erneut an.
- **Read-only (v1).** Die Detailansicht zeigt Werte, editiert sie nicht.
- **Kein eigener Fetch.** Der Follower ist rein synthetisch aus dem bereits
  geladenen Quell-Record gebaut; er löst keine zusätzliche Abfrage aus.
- **Kaskadiert beim Schließen.** Wird die Quell-Pane geschlossen, verschwindet
  ihr Detail-Follower mit (eigener Backlink, getrennt von gekoppelten Drills).

#### `group_by:` / `aggregates:` — Gruppierung & Summen (M3)

Stehen auf `ViewDef` **und** `ChildDef` (gleiche Ebene wie `row_layout` /
`smooth_scroll`), beide optional. Sie schalten den **gruppierten Render-Pfad** der
einzeiligen Tabelle ein: die gefilterten Einträge werden nach **einem** Schlüssel
partitioniert, jede Partition bekommt eine **Gruppen-Kopfzeile** mit Zwischensumme,
und unter der ganzen Tabelle steht eine angepinnte **Gesamtsumme** (Footer).

Die Engine-Gruppierung ist bewusst **einstufig**. Feinere „Condensing"-Layouts
(z. B. Trackings-„Condensed": pro Tag _und_ Task eine summierte Zeile) sind
**Sache des Adapters**, nicht der Engine — sie gehören zur Datenhaltung und
-Interpretation und lassen sich, wo eine DB darunter liegt, nativ als `GROUP BY`
erledigen. Der Adapter kondensiert seine Zeilen vorab selbst und liefert eine
flache Liste, die hier einstufig nach Tag gruppiert wird (s.
`grouping::condense_cells` als generischen, opt-in nutzbaren Baustein).

```yaml
- name: grouped
  node_type: "tracking"
  group_by: { column: started, bucket: day, order: desc } # nach Tag, neueste zuerst
  aggregates:
    - { column: duration, op: sum, total_column: total } # Summe je Tag + Gesamt
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

**Laufzeit-Umschaltung (`cycle_grouping`, Default `zg`):** Die Aktion
`cycle_grouping` (in `keybindings.yaml` bindbar, Default `zg`) schaltet die
Gruppierung der aktiven Ebene durch: ungruppiert → `day` → `week` → `month` →
`year` → ungruppiert. Sie ist nur aktiv, wenn die Ebene überhaupt ein `group_by:`
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

**Gruppen-Reihenfolge kippen (`toggle_group_order`, Default `o`):** Auf einer
gruppierten Flat-View kippt `o` ausschließlich die **Reihenfolge der Gruppen**
(z. B. Tages-Buckets neueste-zuerst ⟷ älteste-zuerst) — Granularität
(`bucket:`) und die Item-Reihenfolge _innerhalb_ der Gruppen bleiben unberührt
(letztere steuert `S`, siehe unten). Gleiche Gate-Bedingung wie `zg` (die Ebene
muss ein `group_by:` haben); View-State, nicht persistiert. `o` wird nur
beansprucht, solange die View keine `record_detail:`-Split anbietet — dort
behält `o` die Detail-Split-Bedeutung (siehe oben). Die Status-Leiste zeigt die
aktuelle Richtung an (`order ↓` = absteigend / `order ↑` = aufsteigend).

**Item-Sortierung innerhalb der Gruppen (`sort`, Default `S`):** `S` öffnet
einen zweistufigen Picker (Spalte → Richtung) über die vom Adapter via
`sortable_columns()` gemeldeten Spalten und sortiert die **einzelnen Items**.
Die Sortierung ist **adapter-getrieben**: jede sortierbare Spalte deklariert
eine `SortKind` (`Text` lexikografisch / `Number` numerisch / `DateTime`
chronologisch), und der Adapter wendet sie über den generischen Helper
`apply_sort` an — _vor_ einer etwaigen Gruppierung, deren Bucket-Sortierung
stabil ist, sodass die gewählte Item-Reihenfolge innerhalb jeder Gruppe
erhalten bleibt. Nicht parsebare Zellen einer typisierten Spalte (leeres
`ended`, das Literal `running`) sortieren ans Ende. Welche Sortierung
tatsächlich angewandt wurde, meldet der Adapter über `ListResult::applied_sort`
zurück (Fußzeilen-Indikator). Adapter, die server-seitig sortieren (Jira via
JQL `ORDER BY`, Taiga), ignorieren `SortKind` und übersetzen die `SortKey`s in
ihre Backend-Sprache.

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

#### `collapsed_source:` — anderes Metadaten-Feld am eingeklappten Knoten

Eine Spalte kann ein **anderes Metadaten-Feld** rendern, solange ihre Zeile ein
**eingeklappter** Tree-Knoten ist (hat Kinder, ist nicht aufgeklappt). Auf
aufgeklappten Knoten, Blättern und in Flat-Listen zeigt sie unverändert ihr
`source:`/`key:`-Feld.

```yaml
columns:
  - key: tracking
    label: Tr
    # Eingeklappter Knoten → zeigt das Roll-up-Feld des Adapters statt des
    # eigenen `tracking`; aufgeklappt/Blatt/flat → wieder `tracking`.
    collapsed_source: tracking_rollup
```

**Wofür?** Marker, die ein Zustand _im Teilbaum_ sind und beim Einklappen sonst
verschwinden würden. Beispiel Tasks-Tree: der `⏱`-Tracking-Marker hängt am
laufenden Task; klappt man dessen Eltern zu, läge der Marker im verborgenen Ast.
Der Adapter liefert zusätzlich ein Roll-up-Feld (`tracking_rollup` = `⏱`, wenn
der Knoten **oder ein Nachfahre** getrackt ist), und `collapsed_source` zeigt es
genau dann, wenn der Knoten eingeklappt ist — der Marker „bubbelt" sichtbar an
den zugeklappten Elternknoten.

- **Warum nicht immer hochpropagieren?** Die Engine kennt den **Einklapp-Zustand**
  (`tree.expanded`), der Adapter nicht; der Adapter kennt den **Teilbaum**, die
  Engine (lazy geladen) nicht. `collapsed_source` teilt die Zuständigkeit:
  Adapter faltet das Roll-up-Feld, Engine entscheidet anhand des Einklapp-Zustands,
  ob Eigen- oder Roll-up-Feld gezeigt wird.
- **Rein additiv & generisch.** Kein Capability-Gate, keine eigene Aktion — fehlt
  das Roll-up-Feld in den Metadaten, rendert die Zelle leer (wie jedes fehlende
  Feld). Beliebig für andere Subtree-Marker wiederverwendbar (Notizen, Links …).
- **Nur im Tree.** In Flat-Listen gibt es keinen Einklapp-Zustand; dort ist
  `collapsed_source` inert und die Spalte zeigt immer ihr `source:`/`key:`-Feld.

#### `hidden:` — Spalte per default ausgeblendet, aber konfigurierbar

```yaml
columns:
  - { key: description, source: label, sizing: fit }
  - { key: tag_names, label: Tags, hidden: true } # da, aber nicht gezeigt
```

Eine Spalte mit `hidden: true` ist Teil der Spaltenliste des Levels, wird aber
im **Default-Layout nicht gerendert**. Sie taucht im `c`-Spalten-Konfig-Popup
als verfügbare, **nicht angehakte** Zeile auf — der User kann sie dort
einblenden. Genau dafür gibt es das Flag: gelegentlich nützliche, aber das
Standard-Layout zumüllende Spalten (z. B. `tag_names` im Tasks-Tree, das die
ausgeschriebenen Tag-Namen neben der kompakten Symbol-Spalte zeigt) sollen
abrufbar sein, ohne immer Platz zu kosten.

- **Default vs. Override.** `hidden:` wirkt nur, solange für das Level **kein**
  Spalten-Override gesetzt ist. Sobald der User im Popup etwas an-/abwählt,
  zählt ausschließlich seine Auswahl (ein eingeblendetes `hidden`-Feld bleibt
  dann sichtbar). Wählt er exakt den Default-Sichtbar-Satz wieder an (versteckte
  Spalten aus), wird der Override **gelöscht** — sauberer Reset.
- **Die `tree_label`-Spalte ignoriert `hidden:`** — sie trägt den Baum und ist
  nie ausblendbar.
- **Rein additiv.** Default `false`; bestehende Views ohne `hidden:` sind
  unverändert.

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

#### `unread_style:` / `unread_marker:` — Ungelesen-Hervorhebung (Chat-Adapter)

Chat-Adapter (Stoat) markieren ungelesene Einträge: ein Kanal/eine Kategorie mit
ungelesenen Nachrichten bekommt im Tree vor dem Label einen **Marker-Glyph** und
Marker + Name in der Ungelesen-Farbe; die Kopfzeile (Autor/Zeit) einer
ungelesenen Nachricht in der Nachrichtenliste wird in derselben Farbe gemalt. Der
Träger ist ein `unread`-Metadatenfeld (`"true"`/leer) am Knoten — der Adapter
setzt es, die Ansicht malt. Ohne dieses Feld haben beide Optionen keine Wirkung.

```yaml
views:
  - name: servers
    tree_label: name
    unread_style: unread # optional; sonst Theme-Farbe `unread`
    unread_marker: "💬" # optional; Default 💬, "" = nur Farbe, kein Glyph
```

- **`unread_style`** — ein Theme-Farbname (`unread`, `accent`, … — dasselbe
  Vokabular wie bei einer Spalten-`style:`). Ohne Angabe gilt die globale
  Theme-Farbe `unread` (`tui.yaml`). Warum pro View: die Ungelesen-Betonung
  konkurriert mit den eigenen Akzenten der Ansicht (Selektion, Fuzzy-Treffer,
  Gruppen-Header); ein dichter Server-Tree und eine flache Nachrichtenliste
  wollen sie unterschiedlich stark.
- **`unread_marker`** — der führende Glyph; Default `💬` (Sprechblase). Ein
  leerer String unterdrückt den Marker (nur Farbe). Hinweis: ein Emoji-Marker
  ist **zwei Zellen breit** — die Tree-Einrückung rechnet die gerenderte Breite
  mit ein. Warum konfigurierbar: Emoji vs. Nerd-Font-Glyph rendern je nach
  Terminal/Font verschieden; manche bevorzugen einen ruhigen ASCII-Punkt.
- Der Fuzzy-Treffer-Highlight gewinnt über die Ungelesen-Farbe: bei aktiver
  Suche bleibt der getroffene Teilstring in der Treffer-Farbe, der Rest des
  Labels in der Ungelesen-Farbe.

#### `deleted`-Metadatenfeld — soft-gelöschte Zeilen ausgegraut

Ein Adapter, der gelöschte Datensätze als Kontext stehen lässt (statt sie hart
zu entfernen), kann sie **dimmen**: Trägt ein Knoten ein `deleted`-Metadatenfeld
mit Wert `"true"`, malt die Engine **jede Zelle** der Zeile in der Theme-Farbe
`text_dim` — die Zeile liest sich als „da, aber inaktiv". Auf segmentierten
Zellen (Tree-Label, Taskpath) dimmt das den Text, während die strukturellen
Glyphen (Connector, Separator) ihre eigene Farbe behalten.

Das ist ein **reines Styling-Signal ohne View-Konfiguration** (kein Key, keine
Opt-in-Flag): der Adapter setzt das Feld, die Ansicht dimmt. Es greift in
**allen** Render-Pfaden — der ungruppierten flachen Liste, dem **gruppierten**
Flat-View (`── Tag ──`-Header) und dem Tree. Anders als `unread` ist die Farbe
**nicht** pro View überschreibbar; gelöscht-vs-aktiv soll überall gleich
aussehen.

Sichtbar wird eine gelöschte Zeile nur, wenn der Query sie **erfasst** — die
Adapter laden das volle inkl.-gelöscht-Universum, und der Query ist der
alleinige Filter (s. „Query = einziger Filter"). Mit dem Default-Query
(`[deleted, =, false]`) bleibt alles Gelöschte unsichtbar; erst ein Query, der
gelöschte einschließt, zeigt sie ausgegraut. Tasks und Trackings nutzen
dasselbe Signal.

#### `mark_read_on_reach_end:` — Aktion bei Cursor auf der letzten Zeile

Ein generischer Engine-Hook auf Drill-Ebene (`children:`): erreicht der Cursor
**zum ersten Mal** die letzte (unterste) Zeile einer flachen Liste **und** ist
diese Zeile noch ungelesen, ruft die Engine genau **einmal** die benannte
Aktion auf dem selektierten Knoten auf.

```yaml
children:
  - name: messages
    node_type: "stoat:message"
    mark_read_on_reach_end: mark-read # Aktions-ID auf dem Nachrichtenknoten
```

- **Wert** — die `id` einer `invoke_action`, die der Adapter auf dem Zeilen-
  knoten versteht (bei Stoat ackt `mark-read` den Kanal bis zu dieser Nachricht
  und merkt sich den Lesestand lokal, worauf der Ungelesen-Marker verschwindet).
  Fehlt das Feld, ist der Hook aus.
- **Zwei Gates, damit es ehrlich bleibt:** _Ankunft_ — der Cursor muss neu auf
  der letzten Zeile landen (nicht schon dort sein), sodass bloßes Öffnen der
  Liste oder ein Tastendruck am Listenende nichts auslöst. _Ungelesen_ — die
  Zeile muss das `unread`-Metadatenfeld tragen, damit der Hook nach dem durch
  das Ack ausgelösten Reload (die Zeile gilt dann als gelesen) nicht erneut
  feuert. Beides zusammen macht ihn idempotent.
- **Warum generisch statt adapter-spezifisch:** „Cursor erreicht das Listenende"
  ist ein reines View-Ereignis (die Engine kennt Selektion und Zeilenzahl), das
  Acken dagegen eine Adapter-Aktion (nur der Adapter kennt das REST-`ack`). Der
  Hook verbindet beide über eine Aktions-`id`, ohne dass die Engine den Chat-
  Begriff „lesen" kennen muss — jeder Adapter kann ihn für ein „bis hierher
  gesehen"-Semantik nutzen.
- **Nur im Flat-Modus** (Listen). In Tree-Ansichten ohne klar definiertes
  „Ende" wird das Feld ignoriert.

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
- **Laufzeit-Chords `zm` / `zr`:** In einem Tree-Pane klappt
  `tree_collapse_all` (Default `zm`) auf genau diese konfigurierte
  Initialtiefe zurück — ein aufgeklappter Pfad bleibt nur, solange seine
  Tiefe `< expand_depth` ist; tiefere manuelle Expansionen fallen weg
  (`expand_depth: 0`/weggelassen → zurück auf die Wurzeln, bisheriges
  Verhalten). `tree_expand_all` (Default `zr`) ist der Gegenpart: es schärft
  dieselbe Kaskade mit unbegrenzter Zieltiefe und klappt den ganzen Baum
  auf, lazy nachladend wie bei `expand_depth: all`. Geladene Children bleiben
  in beiden Fällen im Cache, ein erneutes Auf-/Zuklappen ist also billig.
  Beide Chords sind nur auf Tree-Panes registriert (Wurzel-`ViewDef` mit
  `tree_label`).
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

#### Tree-Spalten-Vererbung — `columns:` einmal an der Wurzel

Alle Zeilen eines Trees rendern in **ein** gemeinsames Spaltenraster. Eine
`ChildDef`, die den Tree fortsetzt (`tree_label` gesetzt) und **keinen**
eigenen `columns:`-Block deklariert, erbt darum die Spalten der nächsten
darüberliegenden Ebene, die welche hat. Ein Tree muss seine Spalten also nur
**einmal an der Wurzel** deklarieren statt sie auf jeder Tiefe identisch zu
wiederholen (was unweigerlich auseinanderdriftet).

Die Vererbung läuft **einmalig direkt nach dem Parse** (`inherit_tree_columns`),
bevor der Validator und jede Laufzeit-Spaltenabfrage die Config lesen — beide
sehen also bereits einen vollständig gefüllten Satz und brauchen keine eigene
Vererbungslogik. Geltungsbereich bewusst eng:

- **Nur Tree-Fortsetzungs-Ebenen erben** (Gate: `tree_label` gesetzt). Ein
  reiner Drill-Child ohne `tree_label` bleibt leer und behält den
  Auto-Fallback aus den Item-Metadaten (z. B. die Postgres-Rows-Ebene, siehe
  unten).
- **Eine Ebene mit eigenem `columns:` bleibt unangetastet** und wird selbst
  zur Vererbungsquelle für darunterliegende Fortsetzungs-Ebenen — wer
  bewusst abweichen will, deklariert eigene Spalten.
- **Separate Views erben nicht über die View-Grenze** (eine flache
  Listen-`ViewDef` neben dem Tree bleibt unabhängig und deklariert ihre
  Spalten selbst).

```yaml
views:
  - name: tasks
    tree_label: description
    columns: # einmal hier deklariert …
      - { key: status, label: St }
      - { key: description, label: Task, source: label }
    children:
      - name: subtasks
        tree_label: description
        recursive: true
        # … kein columns: — erbt St/Task von der Wurzel.
```

#### Tree-Action-/Shortcut-Vererbung — `inherit:` pro Eintrag

Analog zu den Spalten lassen sich auch `actions:`- und `shortcuts:`-Einträge
die Tree-Fortsetzungs-Ebenen **hinunter vererben**, damit der rekursive
Branch sie nicht wortgleich wiederholen muss. Die Vererbung ist **fein
granular und pro Eintrag opt-in**, nicht alles-oder-nichts:

- Ein `actions:`-Eintrag wird vererbt, wenn er `inherit: true` trägt.
- Ein `shortcuts:`-Eintrag wird vererbt, wenn er die ausführliche Form
  `{ action: <name>, inherit: true }` statt der Kurzform `<key>: <name>`
  benutzt (siehe [Per-Node-Aktionen](#per-node-aktionen-shortcuts)).

Die Vererbung läuft **einmalig direkt nach dem Parse** (`inherit_tree_actions`,
neben `inherit_tree_columns`), bevor Validator und Laufzeit die Config lesen.
Geltungsbereich bewusst eng — dieselben drei Regeln wie bei den Spalten plus
eine **Override-pro-Feld**-Regel:

- **Nur Tree-Fortsetzungs-Ebenen erben** (Gate: `tree_label` gesetzt).
- **Override per Key, pro Feld:** Deklariert die Kind-Ebene denselben Key
  (Action-`key` bzw. Shortcut-Char) selbst, gewinnt der lokale Eintrag — der
  geerbte wird für genau diesen Key nicht kopiert. So lässt sich gezielt
  _eines_ erben und _ein anderes_ überschreiben.
- **Geerbte Einträge behalten ihre Vererbbarkeit** und kaskadieren weiter
  nach unten (relevant bei mehr als einer Fortsetzungs-Ebene).
- **Separate Views erben nicht über die View-Grenze** (eine flache
  Listen-`ViewDef` neben dem Tree bindet ihre Keys selbst).
- **Die Single-Level-Suchfamilie wird nie vererbt:** `fuzzy_filter`,
  `search` und `tree_find` sind von der Vererbung ausgenommen (auch wenn
  `inherit: true` gesetzt würde), weil der Validator sie ohnehin auf die
  Tree-Wurzel beschränkt.

```yaml
views:
  - name: tasks
    tree_label: description
    actions:
      - { name: edit, key: e, type: edit, id: edit, inherit: true }
      - {
          name: add,
          key: a,
          type: create,
          id: add,
          under_selection: true,
          inherit: true,
        }
      - { name: fuzzy filter, key: f, type: fuzzy_filter } # nicht vererbbar
    shortcuts:
      d: { action: delete, inherit: true } # vererbt
      s: toggle-tracking # Kurzform → NICHT vererbt
    children:
      - name: subtasks
        tree_label: description
        recursive: true
        # kein actions:/shortcuts: — erbt edit/add + `d` von der Wurzel.
        # `s` (Kurzform) und `f` (Suchfamilie) werden nicht vererbt.
```

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
- **Tree-Ebenen mit gleichem Spaltensatz teilen sich einen Override.** Da
  alle Ebenen eines Trees in **ein** Raster rendern, wäre ein Override pro
  Tiefe widersinnig: `c` auf der Wurzel würde die Kinder nicht erfassen, und
  zwei Tiefen könnten auseinanderlaufen. Darum klappt der Override-Key über
  Tree-Ebenen, die den **identischen** Spaltensatz zeigen, auf die
  spaltendeklarierende Vorfahr-Ebene zusammen — das ist genau der Fall, den
  die [Tree-Spalten-Vererbung](#tree-spalten-vererbung--columns-einmal-an-der-wurzel)
  erzeugt (geerbte Ebene == Wurzel), und es faltet auch jede Rekursionstiefe
  (alle resolven zur selben `ChildDef`) auf **einen** Key. `c` auf
  irgendeiner Tiefe konfiguriert damit den ganzen Tree. Eine Ebene, die
  bewusst **abweichende** `columns:` deklariert, behält ihren eigenen
  Per-Level-Key und bleibt unabhängig konfigurierbar. (Folge: alte, vor
  dieser Regel pro Tiefe gespeicherte Overrides eines uniformen Trees passen
  nicht mehr auf die neuen Keys und werden ignoriert — der Tree zeigt dann
  wieder den YAML-Default, bis er neu konfiguriert wird.)
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

### Jump-Mode (`jump_mode`, Default `J`)

Vimium-artiger Direkt-Sprung über die sichtbaren Zeilen — Parität zum
nativen Tasks-Tab (dort auf `p`). Die Aktion `jump_mode` ist in
`keybindings.yaml` unter `content:` bindbar; Default ist `J` (großes J),
damit der Adapter-Tab `p` für ein `paste`/`paste-move`-Shortcut frei
behält (der native Tab nutzt weiterhin `p` über `common.jump_mode`).

Ablauf:

1. `J` öffnet den Sprung-Overlay (Phase 1).
2. Ein beliebiges Zeichen tippen → jede sichtbare Zeile, die das Zeichen
   enthält, bekommt ein Label (Phase 2). Gibt es nur einen Treffer,
   springt der Cursor sofort dorthin; bei keinem Treffer schließt der
   Overlay.
3. Das Label tippen → der Cursor springt in die zugehörige Zeile.
   `Esc` bricht jederzeit ab.

Das Label-Alphabet stammt aus `navigation.jump_chars` (geteilt mit dem
nativen Tab). Der Sprung wirkt nur auf das fokussierte Pane; in Splits
gilt er für das gerade aktive Pane.

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

Ein Shortcut-Wert kennt **zwei Formen**: die Kurzform `<key>: <action>`
(oben) und die ausführliche Map-Form `<key>: { action: <action>, inherit:
<bool> }`. Beide binden denselben Action-Namen; die Map-Form trägt
zusätzlich das `inherit`-Flag (Default `false`), das den Shortcut die
Tree-Fortsetzungs-Ebenen hinunter vererbt (siehe
[Tree-Action-/Shortcut-Vererbung](#tree-action-shortcut-vererbung--inherit-pro-eintrag)).
Der `parent:`-Prefix funktioniert in beiden Formen.

```yaml
shortcuts:
  d: delete # Kurzform — nur auf dieser Ebene
  s: { action: toggle-tracking, inherit: true } # erbt nach unten
```

**`under_selection` auf `type: create`-Actions:** Standardmäßig legt eine
`create`-Action das neue Kind im aktuell gedrillten Container an (Wurzel →
Top-Level, in einen Task gedrillt → dessen Kind). Mit `under_selection:
true` wird stattdessen die **markierte Zeile** zum Ziel-Parent — in einem
Tree nistet der Create damit unter dem Cursor, ohne vorher hineinzudrillen.
Ist nichts selektiert (leerer Tree), löst die Engine den Parent auf den
Adapter-Root auf, sodass beide Fälle zu einem Top-Level-Create werden. So
realisiert der Tasks-Tab `a` (Kind der Selektion / Top-Level via Adapter-ID
`add`) und `A` (Sibling via Adapter-ID `add-sibling`).

**`on_container` auf `type: custom`-Actions:** Eine Aktion, die auf der
**ganzen Liste/Ebene** wirkt (statt auf einer Zeile), wird als `actions:`-
Eintrag mit `on_container: true` deklariert — nicht als `parent:`-Shortcut.
Der Unterschied ist Sichtbarkeit und Erreichbarkeit am flachen Wurzel-Level:
ein `parent:`-Shortcut löst sein Ziel aus dem Nav-Stack auf, der am noch
nicht hineingedrillten Root **leer** ist → der Hint verschwindet und die
Taste tut nichts. Eine `on_container`-Action baut ihren Hint dagegen
**statisch** aus der Config (ist also immer sichtbar) und dispatcht gegen
`adapter.root()` über den `invoke_action`-Pfad (nicht die Popup-/`execute`-
Schiene). So ist der zurückgegebene `ActionDispatch` — z. B. ein `Confirm`
— wirksam. Heute nutzt nur `type: custom` dieses Flag; Beispiel: der
Trackings-Tab `A restore all` (stellt die gelöschten Trackings **im aktiven
Query** wieder her — siehe Set-Scoping unten —, fragt vorher per `(y/n)`
nach). Der Adapter-Action-Name kommt aus dem `id:`-Feld, das der Wurzel-Node
in `invoke_action` behandelt.

```yaml
actions:
  - name: restore all
    key: A
    type: custom
    id: restore-all # Node::invoke_action("restore-all", …) auf adapter.root()
    on_container: true
```

Was `Node::invoke_action(name, ctx)` zurückgibt, beschreibt das
[`ActionDispatch`](../not-yet-done-content/src/lib.rs)-Enum
(`OpenEditor`, `ExecuteQuery`, `CreateChild`, `DeleteSelf`, `Reload`,
`Confirm`, `Noop`, `Error`). Die TUI übersetzt das in den passenden
View-Flow — ein Editor öffnet sich, eine Query landet in einem paginierten
Result-Pane, ein Delete spawnt einen Confirm-Popup. `Confirm { prompt }` ist
der **generische** Bestätigungs-Mechanismus: der Adapter gibt ihn beim
ersten Aufruf zurück (wenn `ActionContext.confirmed == false`), die TUI
zeigt den `(y/n)`-Prompt, und auf „y" wird dieselbe Aktion am selben Node
mit `confirmed: true` erneut invoked — dann führt der Adapter die (oft
irreversible) Arbeit aus. Anders als `DeleteSelf` (dessen Confirm-/Execute-
Split im Delete-Plumbing der TUI lebt) kann sich so **jede** Aktion hinter
einer Bestätigung absichern, und der Adapter formuliert den Text, weil nur
er weiß, was die Aktion anrichtet (z. B. wie viele Nachfolge-Intervalle ein
Restore purged).

#### Set-scoping: mengen­wirksame Aktionen folgen dem aktiven Query

**Vertrag:** Jede Aktion, die auf **mehr als den aufrufenden Knoten** wirkt
— eine Container-/listenweite Aktion (`restore-all`), ein Bulk-Delete, eine
Aggregat-Operation — MUSS auf die **sichtbare Menge** des Panes scopen, nie
auf das gesamte (inkl. gelöschter Zeilen geladene) Universum des Adapters.

Der Kanal dafür ist `ActionContext.query: Option<String>`: die TUI legt den
**aktiven Query-Text des Panes** hinein — exakt denselben Filter-String, den
sie dem Adapter ohnehin bei jedem `list()` über `LoadParams.query` reicht.
Der Adapter löst die sichtbare Menge daraus selbst neu auf (z. B. via
`find_filtered`), genau wie beim Listen-Load. Das ist **kein** Zurückfüttern
gerenderter Inhalte (keine Id-Liste, keine Tabellenzeilen) — nur die
_Identität_ des aktiven Filters, die der Adapter ohnehin kennt. `None`/leer
heißt „kein Filter" → die ganze Liste ist im Scope (= was das Pane zeigt).

Damit ist der Query der **einzige** Hebel: Gelöschte erreicht eine
`restore-all` nur, wenn der Query sie sichtbar macht (der Query ist der
alleinige Filter, ein `deleted=false` ist nirgends eingebacken — siehe
„Query = einziger Filter"). Will der Nutzer alle gelöschten wiederherstellen,
nimmt er sie in den Query auf, statt dass eine Aktion am Filter vorbei die
ganze DB anfasst.

Einzel-Knoten-Aktionen (eine Zeile löschen/restoren, Toggle) ignorieren
`ctx.query` — ihr Ziel ist bereits der aufrufende Knoten. Natürliche
Grenze: `task.undelete` (Tasks-Tab) stellt den **zuletzt gelöschten** Task
wieder her — ein Undo-Schritt, keine Mengen­operation; es scopt bewusst nicht
über den Query, weil es per Definition genau einen, den jüngsten, betrifft.

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
| `script`       | Externes Script mit Node-JSON auf stdin starten  | ❌         |
| `tag`          | Tag-Verwaltungs-Menü für den selektierten Task   | ✅ (modal) |
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
(sie dient dort nur als Schalter, der den Filter scharf macht).

**Eager-Trees laden beim Öffnen des Filters den ganzen Teilbaum:** Auf
Adaptern mit `supports_eager_subtree` (lokale Tasks/Trackings) zieht das
Öffnen des Filters einmalig den kompletten Baum (`list_subtree(u32::MAX)`)
nach und klappt ihn auf. So matchen auch Knoten in eingeklappten oder noch
nicht paginierten Ästen — das „der Filter sieht den ganzen Wald"-Verhalten
des nativen Tabs. Der Aufklapp-Zustand vor dem Filter wird gestasht und
beim Leeren des Filters wiederhergestellt (der Baum klappt exakt in seine
vorherige Form zurück). Auf Remote-Trees ohne diese Capability bleibt die
Suche auf die aktuell geladenen/aufgeklappten Knoten beschränkt: ein
Treffer in einem ungeladenen Ast wird erst sichtbar, wenn dieser Ast
geladen ist.

**Der matchende Teilstring wird hervorgehoben** (Parität zum nativen
Tasks-Tab): Im Tree-Mode werden die getroffenen Runs im **Label** der
`tree_label`-Spalte in der Theme-`accent`-Farbe (fett) gezeichnet — der
Box-Connector behält seine eigene `tree_connector`-Farbe. Im Flat-Mode
bekommen die durchsuchten Spalten (`fields`, bzw. alle bei leerer Liste)
ihre Treffer-Runs ebenfalls in `accent`. Jeder Whitespace-getrennte Token
wird einzeln gematcht; die getroffenen Zeichen-Indizes werden vereinigt und
zu zusammenhängenden Bereichen verschmolzen. Matcht ein Token nicht im
Label/in der Spalte (die Zeile überlebte den Filter über ein anderes Feld),
bleibt dort nichts markiert.

### Script Actions (`type: script`)

```yaml
actions:
  - name: script
    key: x
    type: script # öffnet das Script-Menü; Scripts liegen unter
    #   <data>/not_yet_done/scripts/<tab>/<view-node-type…>/
```

Eine `script`-Action sammelt die Scripts aus dem für Tab + View-Ebene
konventionellen Verzeichnis und übergibt sie als Auswahlmenü. Das gewählte
Script wird als Fremdprozess gestartet und bekommt ein **JSON auf stdin**.
Mutierende Scripts (non-interactive) lösen danach einen Pane-Reload aus.

**`scope:` — was das Script auf stdin bekommt.** Default ist `node`:

```yaml
- { name: script, key: x, type: script } # scope: node (default)
```

| `scope`        | stdin-JSON                                                                                                                                           |
| -------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| `node`         | `{"node": {id, label, node_type, tab, fields:{…}}}` — der **eine** selektierte Knoten                                                                |
| `filtered_set` | `{"tracking_ids": […], "filter_min_date": …, "filter_max_date": …}` — **alle** aktuell gefilterten Zeilen-IDs + die Datumsgrenzen der aktiven Query  |
| `table`        | `{"rows": [{id, label, fields:{…}}, …], "query": …, "selected_index": …, "selected_field": …}` — die **ganze angezeigte Tabelle** mit Cursor-Kontext |

```yaml
- { name: script, key: x, type: script, scope: filtered_set }
```

`scope: filtered_set` ist für **Batch-/Aggregat-Scripts** gedacht, die über
die ganze sichtbare Liste laufen (z. B. ein Stundenreport über den gefilterten
Zeitraum). Die Engine sammelt:

- **`tracking_ids`** — alle IDs, die der User gerade sieht: bei aktivem
  Fuzzy-Filter exakt die Treffermenge, sonst die komplette query-gefilterte
  Liste der Pane.
- **`filter_min_date` / `filter_max_date`** — die Datums-Untergrenze/-Obergrenze
  der aktiven gespeicherten Query (relative Angaben wie `last month` sind zum
  Lauf-Zeitpunkt aufgelöst, RFC3339; ohne Grenze `null`).

Der stdin-Schlüssel heißt aus Backcompat-Gründen `tracking_ids` — der
Engine-Pfad selbst ist generisch, sodass die historischen Trackings-Scripts
(`daily_report.py`, `hours_report.py`, …) unverändert über den
Adapter-Tab laufen.

```yaml
- { name: script, key: x, type: script, scope: table, default_field: name }
```

`scope: table` reicht die **ganze aktuell angezeigte Tabelle samt
Cursor-Kontext** weiter — gedacht für Scripts, die auf einer Zeile/Zelle
operieren und dabei die Nachbarzeilen oder den Query sehen wollen. Funktioniert
auf jeder Content-Tabelle, auch auf dem transponierten Record-Detail-Split
(`o`), wo jede „Zeile" ein Feld/Wert-Paar des Datensatzes ist — damit deckt
**ein** Scope sowohl „kompletter Datensatz + selektiertes Feld" (Detail) als
auch „alle Zeilen + Query + Cursor" (Liste) ab. Die Engine sammelt:

- **`rows`** — jede sichtbare Zeile als `{id, label, fields:{…}}` (dieselbe
  Form wie ein einzelner `node`), in Anzeige-Reihenfolge; bei aktivem
  Fuzzy-Filter exakt die Treffermenge.
- **`query`** — der aktive Query-Text der Pane (`null`, wenn keiner anliegt,
  z. B. im Detail-Split).
- **`selected_index`** — Index der Cursor-Zeile in `rows`.
- **`selected_field`** — der Spalten-Key unter dem Spalten-Cursor; ist der
  Spalten-Cursor aus, greift das konfigurierte **`default_field`** der Action
  (sonst `null`).

**Script-Shortcuts (`ctrl+s` im Menü).** Wie im Query-Menü lässt sich im
Script-Menü einem Script per **`ctrl+s`** eine Taste zuweisen. Der erfasste
Chord wird in der DB-Tabelle `query_shortcut(scope, name, shortcut)` unter dem
Scope `script:<tab>/<view-node-type…>` (gleiche Ableitung wie das
Script-Verzeichnis) für den Dateinamen abgelegt. Liegt der Fokus danach auf
einer Ebene, die eine `type: script`-Action anbietet, startet der Chord das
Script direkt — genau so, als hätte man das Menü geöffnet und auf dem Eintrag
Enter gedrückt (gleicher `scope:`/`default_field`-Kontext). Der Chord wird
gegen alle in seinem Tab aktiven Tasten geprüft (inkl. Chord-Präfixe) und bei
Kollision abgelehnt. Belegte Shortcuts erscheinen im Menü als `[chord]`-Suffix.

### Tag-Verwaltung (`type: tag`)

```yaml
actions:
  - name: tags
    key: T
    type: tag
```

Eine `tag`-Action öffnet das **globale Tag-Verwaltungs-Menü** (`:tag`),
angeheftet an den aktuell selektierten Knoten der Pane. Es ist dasselbe
Menü wie auf dem nativen Tasks-Tab — der Action-Typ verdrahtet es generisch
an jeden Content-/Adapter-Tab:

- **Enter** auf einem Tag: weist es dem Task zu / entfernt es (Toggle). Der
  Ist-Zustand wird beim Öffnen frisch geladen, nicht aus einem Cache.
- **Name tippen + Enter**: legt einen neuen Tag an und weist ihn dem Task zu.
- **ctrl+e**: öffnet das YAML-Formular eines Tags (Symbol / Name / Farbe).
- **ctrl+d**: löscht den Tag (von allen Tasks).

Nach jeder Änderung wird die Pane neu geladen, sodass die `tag_symbols`- /
`tag_names`-Spalten den neuen Stand zeigen.

Tags sind ein Task-Konzept: Der selektierte Knoten muss eine Task-ID tragen
(der `tasks`-Adapter liefert die nackte UUID als Node-ID). Auf einem Knoten
ohne Task-ID quittiert das Menü mit einer Notiz statt zu öffnen.

Konvention: Shortcut **`T` (shift+t)**, weil `t` auf dem Tasks-Tab den
Tree-/List-View-Wechsel belegt. Im Tree mit `inherit: true` deklarieren,
damit die Action auf jeder Subtask-Ebene greift.

> **Status:** `type: tag` hing am host-seitigen `tag_service` und war die
> Altlast, die mit dem DB-Split abgebaut wird (C5). **Togglen _und_
> Anlegen/Umbenennen/Löschen** sind jetzt vollständig auf den
> adapter-getriebenen `type: option_menu` migriert (siehe unten, Felder
> `toggle`/`create`/`rename`/`delete`) — der Tasks-Tab nutzt ausschließlich
> `option_menu`. `type: tag` und das Cmdline-`:tag`-Menü sind damit nur noch
> für Hosts ohne migrierten Adapter relevant.

### Option-Menü (`type: option_menu`)

```yaml
actions:
  - name: tags
    key: T
    type: option_menu
    option_menu:
      source: tags # Schlüssel für `list_values` auf dem Adapter
      marker: tag_ids # verstecktes Knoten-Feld mit den gesetzten Werten
      toggle: toggle-tag # Adapter-Action, die auf Enter feuert
      create: create-tag # optional: ctrl+n legt einen Eintrag an (fragt Text)
      rename: rename-tag # optional: ctrl+e benennt den fokussierten um
      delete: delete-tag # optional: ctrl+d löscht ihn (y/n-Confirm)
      title: Tags # Popup-Titel (optional; Default = Action-Name)
```

Ein **host-seitiges, adapter-agnostisches** Auswahl-Menü, das Werte am
selektierten Knoten togglet (z. B. Tags). **Warum es existiert:** Eine an eine
GUI-Form (Picker, Formular) gekoppelte Action zwingt den Adapter, die
Host-Oberfläche zu kennen. Statt dessen liefert der Adapter eine flache Liste
wählbarer Werte über `list_values(source)` und nimmt den gewählten Wert über
ein normales `invoke_action` (`ActionContext.value`) entgegen — das Menü selbst
ist reine Host-Logik und steht in der Config. So weiß der Adapter nichts vom
Menü, und derselbe Action-Typ bedient Tags, Status-Sets, Label u. Ä. ohne neue
Vertrags-Form.

Ablauf:

- Beim Öffnen lädt der Host die Optionen via `list_values(source)` und liest
  die aktuell gesetzten Werte aus dem `marker`-Metadatenfeld des Knotens
  (kommaseparierte stabile IDs). Gesetzte Optionen werden mit **★** markiert.
- **Enter** auf einer Option: feuert die `toggle`-Action mit dem gewählten Wert
  in `ActionContext.value`. Der Adapter entscheidet selbst assign-vs-unassign
  anhand der Ist-Zugehörigkeit und gibt einen `ActionDispatch` zurück (Unsinns-
  Werte kommen als `ActionDispatch::Error` zurück).
- Das Popup **bleibt offen** (Mehrfach-Toggle); der ★-Marker kippt sofort live,
  während die Pane im Hintergrund neu lädt.
- **Verwalten der Werteliste** (optional, je nach gesetztem Feld):
  - **`create`** (Default `ctrl+n`): öffnet einen Inline-Text-Prompt. Enter
    feuert die `create`-Action mit dem eingegebenen Namen in
    `ActionContext.text` (kein `value`); leerer Text bricht ab.
  - **`rename`** (Default `ctrl+e`): öffnet denselben Prompt, vorbefüllt mit dem
    Label der fokussierten Option. Enter feuert die `rename`-Action mit der
    stabilen ID der Option in `ActionContext.value` **und** dem neuen Namen in
    `ActionContext.text`.
  - **`delete`** (Default `ctrl+d`): zeigt einen Inline-`(y/n)`-Confirm. `y`
    feuert die `delete`-Action mit der ID der fokussierten Option in
    `ActionContext.value`; `n`/Esc bricht ab.
  - Diese Verben sind **reine Daten-Operationen auf der Werteliste** (nicht am
    Knoten) — der selektierte Knoten ist nur das Dispatch-Vehikel. Nach Erfolg
    lädt das Menü die Optionsliste neu (Prompt schließt, Popup bleibt offen);
    eine `ActionDispatch::Error`-Rückgabe wird als Hinweis angezeigt, ohne das
    Menü zu schließen.
- **Esc** schließt.

Felder:

| Feld     | Pflicht | Bedeutung                                                                 |
| -------- | ------- | ------------------------------------------------------------------------- |
| `source` | ja      | Schlüssel an `list_values(source)`; mappt auf `Vec<ValueOption>`.         |
| `marker` | ja      | Verstecktes Knoten-Feld mit den gesetzten Werten (z. B. `tag_ids`).       |
| `toggle` | ja      | Adapter-Action-ID, die auf Enter mit dem Wert aufgerufen wird.            |
| `create` | nein    | Adapter-Action-ID für „anlegen" (ctrl+n; Name → `ActionContext.text`).    |
| `rename` | nein    | Adapter-Action-ID für „umbenennen" (ctrl+e; id → `value`, Name → `text`). |
| `delete` | nein    | Adapter-Action-ID für „löschen" (ctrl+d, y/n-Confirm; id → `value`).      |
| `title`  | nein    | Popup-Titel; Default = Name der Action.                                   |

Tastenbindungen teilt sich das Menü mit dem Tag-Menü (`tag_menu`-Section:
Toggle / Create / Edit / Delete / Next / Prev / Close), weil die Menü-Form
identisch ist. Ein `create`/`rename`/`delete`-Feld ohne gesetzte Action lässt
die jeweilige Taste inaktiv.

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
