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
