# Architektur

Überblick über die Crate-Struktur, die zentralen Datenflüsse und die
Verantwortlichkeiten der Komponenten von **not-yet-done** — einer
terminal-basierten Task- und Zeiterfassung mit TUI, CLI und
Waybar-Anbindung.

Dieses Dokument beschreibt den _Ist-Zustand_ und vor allem die
_Begründungen_ hinter den Schnitten. Gewichtige Einzel-Entscheidungen
liegen als ADRs unter [`docs/decisions/`](decisions/) (z. B.
[0001 — Render-Loop Dirty-Gating](decisions/0001-render-loop-dirty-gating.md)).

## Leitprinzipien

Diese Prinzipien (auch in der `CLAUDE.md` festgehalten) erklären, _warum_
die Crates so geschnitten sind:

- **Separation of Concerns** — jeder Tab/View besitzt seinen eigenen
  State. Shared State auf App-Ebene wird minimiert.
- **Open/Closed** — neue Tabs/Features sind hinzufügbar, ohne bestehenden
  Tab-Code zu ändern. Geteilte Fähigkeiten laufen über Traits.
- **Kapselung** — Views verwalten ihre Popups, Filter, Favoriten und
  Suche intern. Kommunikation mit der App nur über Message-/Request-Enums.
- **Adapter-Isolation** — Content-Backends (Jira, Taiga, Postgres,
  Confluence, Stoat, local) hängen ausschließlich an `not-yet-done-content`,
  nie am TUI. Dadurch bleibt der Abhängigkeitsgraph azyklisch und ein neuer
  Adapter berührt keinen Kern-Code.
- **Eine Adapter-Verdrahtung für alle Frontends** — `not-yet-done-host` ist die
  einzige Crate, die den Vertrag _und_ alle Adapter kennt. TUI, CLI (`nyd`) und
  Waybar bauen Adapter byte-gleich über `host::resolve_adapter`, statt die
  Factory-Auswahl zu duplizieren. Begründung in
  [ADR 0005](decisions/0005-host-crate-und-lifecycle-hooks.md).

## Crate-Landschaft

Der Workspace teilt sich in fünf Schichten: **Frontends** (was der Nutzer
startet), die **Host-Schicht** (eine Crate, die Adapter für alle Frontends
gleich baut), die **Content-Adapter-Schicht** (austauschbare Backends), die
**UI-Bausteine** (rendering-nahe Hilfs-Crates) und den **Datenkern**
(Legacy-`core` + die ausgelagerte Task-Domäne).

```mermaid
flowchart TD
    subgraph Frontends
        TUI[not-yet-done-tui]
        NYD["nyd<br/>not-yet-done-cli"]
        NYDT["nyd-t<br/>not-yet-done-task-cli"]
        WAYBAR[not-yet-done-waybar]
    end

    subgraph Host
        HOST["not-yet-done-host<br/>Factory-Registry · host_context<br/>discover/resolve · hooks"]
    end

    subgraph "Content-Adapter-Schicht"
        CONTENT[not-yet-done-content<br/>ContentAdapter-Trait + Auth]
        LOCAL[not-yet-done-local-adapter<br/>Tasks/Trackings/Projects]
        JIRA[not-yet-done-jira-adapter]
        TAIGA[not-yet-done-taiga-adapter]
        PG[not-yet-done-postgres-adapter]
        CONF[not-yet-done-confluence-adapter]
        STOAT[not-yet-done-stoat-adapter]
        TRANSPORT[not-yet-done-transport<br/>SSH-Tunnel]
    end

    subgraph "UI-Bausteine"
        FOREST[not-yet-done-forest]
        TABLE[not-yet-done-table]
        NYDRATATUI[not-yet-done-ratatui<br/>Editor/Widgets]
        GRID[not-yet-done-grid-core]
    end

    subgraph Datenkern
        CORE[not-yet-done-core<br/>nyd.db: Settings/Queries/Links/Tags]
        TASKCORE[not-yet-done-task-core<br/>tasks.db: Task-/Tracking-Domäne]
        FILTER[not-yet-done-filter<br/>Filter-DSL]
        MACROS[not-yet-done-macros]
    end

    TUI --> HOST
    TUI --> CORE
    TUI --> LOCAL
    TUI --> PG
    TUI --> FOREST
    TUI --> TABLE
    TUI --> NYDRATATUI
    NYD --> HOST
    NYD --> TASKCORE
    NYDT --> TASKCORE
    WAYBAR --> HOST

    HOST --> CONTENT
    HOST --> LOCAL
    HOST --> JIRA
    HOST --> TAIGA
    HOST --> PG
    HOST --> CONF
    HOST --> STOAT

    LOCAL --> CONTENT
    LOCAL --> TASKCORE
    JIRA --> CONTENT
    TAIGA --> CONTENT
    CONF --> CONTENT
    STOAT --> CONTENT
    PG --> CONTENT
    PG --> TRANSPORT
    TRANSPORT --> CONTENT

    TASKCORE --> FILTER
    FOREST --> TABLE
    NYDRATATUI --> GRID
    CORE --> MACROS
```

| Crate                               | Verantwortung                                                                      | Workspace-Deps                                                       |
| ----------------------------------- | ---------------------------------------------------------------------------------- | -------------------------------------------------------------------- |
| **not-yet-done-core**               | Legacy-DB (`nyd.db`): Settings, Saved Queries, Links, Tags; Config                 | macros                                                               |
| **not-yet-done-task-core**          | Task-/Tracking-Domäne (`tasks.db`): Entities, Services, Bootstrap, Backup          | filter                                                               |
| **not-yet-done-filter**             | Filter-DSL (YAML-Query-Sprache, AST → Query, Tree-Operatoren)                      | —                                                                    |
| **not-yet-done-host**               | Adapter-Verdrahtung für alle Frontends: Factory-Registry, `resolve_adapter`, Hooks | content, alle Adapter                                                |
| **not-yet-done-tui**                | Terminal-UI: Event-Loop, Views, App-State                                          | core, content, filter, host, local, postgres, forest, table, ratatui |
| **not-yet-done-cli** (`nyd`)        | Generisches Adapter-Frontend (CLI) + Built-ins `tag`/`backup`/`config`             | host, content, core, task-core, filter                               |
| **not-yet-done-task-cli** (`nyd-t`) | Native Domänen-CLI für Tasks/Trackings (typed JSON, graded exit codes)             | task-core                                                            |
| **not-yet-done-waybar**             | Waybar-CFFI-Modul (aktives Tracking in der Statusbar)                              | content, host                                                        |
| **not-yet-done-content**            | `ContentAdapter`-Trait + `Node`/`Content`-Abstraktion + Auth-Orchestrierung        | —                                                                    |
| **not-yet-done-local-adapter**      | Tasks/Trackings/Projects als ContentAdapter (über `task-core`)                     | content, task-core, filter                                           |
| **not-yet-done-jira-adapter**       | Jira-Tickets als Content-Baum                                                      | content                                                              |
| **not-yet-done-taiga-adapter**      | Taiga-Items als Content-Baum                                                       | content                                                              |
| **not-yet-done-postgres-adapter**   | Postgres-DBs/Schemas/Tabellen/DB-Skripte als Content-Baum                          | content, transport                                                   |
| **not-yet-done-confluence-adapter** | Confluence-Spaces/Pages/Kommentare/Attachments                                     | content                                                              |
| **not-yet-done-stoat-adapter**      | Chat (Stoat/Revolt-Fork) als Streaming-Content-Baum                                | content                                                              |
| **not-yet-done-transport**          | SSH-Tunnel-Support für Adapter (z. B. Postgres hinter Bastion)                     | content                                                              |
| **not-yet-done-forest**             | Baum/Forest → flache Zeilenliste (Tree-Rendering)                                  | table                                                                |
| **not-yet-done-table**              | Spalten-Layout & Tabellen-Rendering-Primitive                                      | —                                                                    |
| **not-yet-done-ratatui**            | Ratatui-Erweiterungen (Inline-Editor, Widgets)                                     | grid-core                                                            |
| **not-yet-done-grid-core**          | Grid-Layout-Kern                                                                   | —                                                                    |
| **not-yet-done-macros**             | Proc-Macros (`ColumnRegistry` etc.)                                                | —                                                                    |
| **ratatui_form_widgets**            | Form-Widget-Komponenten (Workspace-Member, aktuell von keinem Crate eingebunden)   | —                                                                    |
| **grid-render-sim**                 | Render-Simulation/Testbett für Grid                                                | —                                                                    |

## Datenkern: `core` + `task-core` + `filter`

Der Datenkern wurde in Block C aufgeteilt. Die **Task-/Tracking-Domäne** liegt
nicht mehr in der Legacy-DB, sondern in einer eigenen `tasks.db`, betreut von
`not-yet-done-task-core`; `not-yet-done-core` behält nur noch die
TUI-Querschnitts-Daten (`nyd.db`); die Filter-DSL ist in `not-yet-done-filter`
ausgelagert, damit sowohl Domäne als auch Adapter sie ohne `core` nutzen können.
Begründung des Splits in
[`adapterize-tasks-trackings.md`](adapterize-tasks-trackings.md).

- **`not-yet-done-task-core` (`tasks.db`)** — die eigentliche Domäne: Entities
  (`Task`, `Tracking`, `Project`, Tags), Services, `bootstrap`
  (`default_task_dsn()`, `open_module()` — connect + Schema-Sync + DI) und das
  suffix-bewusste `backup`-Modul. _Eine_ Quelle der Wahrheit für DSN und
  Backup-Dir, die der local-Adapter (TUI) **und** `nyd-t` teilen. Default-DSN
  `<data-local>/not_yet_done/tasks.db`, Override `NYD_TASKS_DB`.
- **`not-yet-done-core` (`nyd.db`)** — frontend-agnostischer Legacy-Kern für
  Settings, Saved Queries, Query-Shortcuts, Links und (noch) globale Tags, plus
  `BackupServiceImpl` für den täglichen `nyd.db`-Backup beim Start. Ablage:
  `dirs::data_local_dir()/not_yet_done/nyd.db`, Config
  `dirs::config_dir()/not_yet_done/config.yaml`, Override `DATABASE_URL`.
- **`not-yet-done-filter`** — die YAML-Filter-DSL mit
  natürlichsprachlichen Datumsausdrücken: AST → Query inkl. baumspezifischer
  Operatoren. Keine Workspace-Deps; von `task-core`, `local-adapter`, `cli`
  und `tui` genutzt.

Schema-Sync läuft bei beiden DBs beim Start automatisch; punktuelle Migrationen
behandeln Altdaten (z. B. Tag `color` → `fg_color` + `bg_color` + `symbol`).

> **Gotcha:** SeaORM-`Uuid`-PKs landen in SQLite als `BLOB(16)`, nicht als
> Text — ein manueller `TEXT`-Insert schlägt still beim Decode fehl.

## Content-Adapter-Schicht (`not-yet-done-content`)

Externe Backends werden über ein gemeinsames Trait-Paar als navigierbarer
**Baum aus `Node`s** abstrahiert. Das ist der Open/Closed-Hebel: jeder
neue Adapter implementiert nur diese Traits und wird im TUI per
Factory + YAML registriert — kein Kern-Code ändert sich.

### `ContentAdapter` (Adapter-Ebene)

- `root()` / `get_by_id(id)` — Einstieg bzw. Direktzugriff auf einen Knoten
- `list(params)` — Kinder mit Sort/Pagination
- `actions_for_type(node_type)` — welche Aktionen ein Knotentyp anbietet
  (synchron; speist die Shortcut-Hints in der Action-/Statusbar)
- `execute_custom_query(query, ctx)` — adapter-native Abfragen (z. B. SQL),
  inkl. Cursor-Pagination
- `search_in_tree(query, limit)` — serverseitige Baumsuche (z. B.
  Confluence CQL), liefert Treffer **mit Ancestor-Pfad** für lazy
  Expand-to-Hit
- `subscribe_status()` — `watch::Receiver<AdapterStatus>` für Live-Auth-/
  Verbindungsstatus (`Idle`/`Connecting`/`Ready`/`Busy`/`NeedsCreds`/`Failed`)
- `submit_credentials(fields)` / `try_refresh_session()` — interaktive bzw.
  stille Credential-Pflege
- `saved_query_store()` — adapter-eigene Persistenz gespeicherter Queries
- `child_process_env(node)` — Env-Variablen für Editor-/Skript-Kindprozesse

### `Node` / `Content` (Knoten-Ebene)

- `list(params)` — Kindknoten; `content()` — Lese-Body (für Preview/Detail)
- `list_subtree(params, depth)` — ganzer Teilbaum (`depth + 1` Ebenen) in
  **einem** Call. Default rekursiert über `list()` (ein Call pro Knoten);
  In-Memory-Adapter (Tasks, Trackings) überschreiben mit Snapshot-Walk ohne
  I/O. Engine-getrieben nur bei Capability `supports_eager_subtree` — ersetzt
  dann die O(N²)-Kaskade beim Initial-/Reload-Aufklappen. Siehe
  `docs/generic-view-spec.md` → Eager-Subtree.
- `actions()` — Aktionen dieses konkreten Knotens
- `invoke_action(name, ctx)` → `ActionDispatch` — Shortcut-getriebener
  Aktions-Dispatch, der eine UI-Flow-Absicht zurückgibt
- `prepare(action)` / `picker_options(action)` / `execute(action, input)` —
  der dreistufige Aktions-Flow (Editor-Buffer rendern bzw. Picker-Optionen
  liefern → Nutzereingabe → finalisieren)

Implementierungen: `jira-`, `taiga-`, `postgres-`, `confluence-`,
`stoat-adapter`.

### Streaming-Adapter (Gateway-Pattern)

Die meisten Adapter sind **Pull-only**: sie antworten synchron-via-`await`
auf `list()`/`get_by_id()`. Chat (Stoat, Fork von Revolt) bricht das Modell
und ist der erste **Streaming-Adapter** — Referenzmuster für künftige
Push-Backends:

- **Bootstrap ist push-only.** Die Server-/Channel-Liste gibt es _nur_ über
  das WebSocket-`Ready`-Event, nicht über REST. Ein
  **`StoatGateway`** (einzelner Hintergrund-Tokio-Task) ist die einzige
  Stelle mit WS-Logik: `connect → Authenticate → Ready → Event-Stream`,
  Heartbeat-Ping, Reconnect mit Backoff. `Ready` füllt **`StoatState`**
  (`Arc<RwLock>`, In-Memory-Source-of-Truth für den Baum) — bewusst
  **nicht** in SQLite gecacht (Chat-State ist hochvolatil; persistiert wird
  nur Session-Token + View-Sort).
- **`Node::list()` liest synchron aus `StoatState`** (kein Netz-`await` für
  die Baum-Struktur); Message-History bleibt REST-Pull (paginiert).
- **Status vereinheitlicht.** Der Adapter besitzt **einen** eigenen
  `watch<AdapterStatus>`-Kanal. Die Login-Phase wird aus dem
  `AuthOrchestrator` hineingeforwardet (dessen `Ready` wird unterdrückt),
  die Socket-Phase publiziert das Gateway (`Connecting`/`Ready`/`Failed`).
  So spiegelt das Banner Login **und** Verbindung Ende zu Ende.
- **Live-Push (Phase 2, umgesetzt).** Laufende WS-Events werden
  out-of-band als generische `Invalidation` in den `select!`-Loop
  gespeist — derselbe Mechanismus wie `subscribe_status`, nur „Knoten X
  ist stale" statt „Status geändert". Bausteine:
  - `Invalidation`-Enum + `ContentAdapter::subscribe_invalidations()` in
    `not-yet-done-content` (No-op-Default → Pull-only-Adapter bleiben
    unangetastet, Open/Closed). Rückgabe ist ein **`broadcast::Receiver`**
    (diskrete Events, nicht Latest-Value wie `watch`; eine Adapter-Instanz
    kann mehrere Views speisen).
  - Das Gateway pusht `Invalidation::Node{id: <channel>}` bei Message-/
    Reaction-Events und `Invalidation::All` bei jedem `Ready` (erster
    Connect **und** Reconnect-Resync).
  - Pro View spawnt die TUI neben dem Status-Watcher einen
    **Invalidation-Watcher**, der den Receiver in den **bestehenden**
    `load_tx`-Kanal umpumpt (`LoadMsg::AdapterInvalidation`). `poll_load`
    lädt die betroffenen Panes auf ihrem aktuellen Level neu (`All` → alle
    Panes; `Node{id}` → nur Panes, deren `parent_node_id` dieser Channel
    ist). Bei `Lagged` resynct der Watcher mit `All`.
  - Vorbedingung ist der event-getriebene Render-Loop (1b, siehe unten);
    Designdetails in ADR `0002`.
  - Grenze: **strukturelle** Live-Events (Channel/Server angelegt/gelöscht)
    sind noch nicht inkrementell auf `StoatState` angewendet — sie
    erscheinen erst nach einem Reconnect. Folgearbeit.

### Auth-Orchestrierung

Der `AuthOrchestrator` in `not-yet-done-content` entkoppelt Adapter von der
Credential-Beschaffung:

1. **Value-Provider** (literal, env, file, command, keyring) lösen
   synchron bei Bedarf auf.
2. **Prompt-Felder** werden zu einem `AdapterStatus::NeedsCreds`-Formular
   gebündelt, über den Status-Channel publiziert und warten auf
   `submit_credentials(...)` aus dem TUI.
3. Eine adapter-gelieferte **Login-Funktion** verbraucht die aufgelösten
   Credentials und liefert einen Session-Blob.
4. Ein **Session-Cache** (`SessionStore` + `SessionCachePolicy`)
   persistiert den Blob (TTL, Refresh-Token …).
5. Nebenläufige `ensure_session`-Aufrufe serialisieren über einen internen
   Mutex.

Dadurch ist die TUI nie blockiert: ein Adapter, der Credentials braucht,
meldet `NeedsCreds` über den Status-Channel; die UI öffnet das Formular,
ohne den Render-Loop zu stoppen.

## TUI (`not-yet-done-tui`)

### Render- und Event-Loop

`main.rs::run_loop` ist **event-getrieben und dirty-gated** (Variante 1b):
ein `tokio::select!` über die crossterm-`EventStream`, `load_rx`,
`commit_rx` und einen **bedingten** 200-ms-`interval` (nur armiert, solange
ein Editor/Skript pending ist, ein Busy-Banner läuft oder ein aktives
Tracking tickt — sonst parkt der Loop und Idle ist ~0 % CPU). Das Eintreffen
einer Message _ist_ das Redraw-Signal; `sync_components()` +
`terminal.draw()` laufen weiterhin nur, wenn diese Iteration etwas geändert
hat (`dirty`). Jede Änderungsquelle (`poll_*`/`tick_*`/`handle_*_msg`) meldet
per `bool`-Rückgabewert, ob sie sichtbaren State berührt hat. Das ist
zugleich die Vorbedingung für out-of-band Adapter-Invalidation
(Streaming-Adapter, oben). Begründung, Heikel-Punkte (`EventStream` ↔
Editor-Suspend) und Konsequenzen stehen in
[ADR 0001](decisions/0001-render-loop-dirty-gating.md).

```mermaid
flowchart TD
    START([run_loop]) --> POLL["poll_load / tick_* / poll_*<br/>jede Quelle gibt bool zurück"]
    POLL --> OR{dirty?}
    OR -- ja --> DRAW["sync_components()<br/>terminal.draw(render)"]
    OR -- nein --> WAIT
    DRAW --> WAIT["poll_event(200ms)"]
    WAIT -- Taste --> KEY["handle_key → EditorRequest<br/>dispatch_editor_request"]
    WAIT -- Timeout --> LOOPBACK
    KEY --> LOOPBACK([nächste Iteration])
    LOOPBACK --> POLL
```

### Nachrichten-/Request-Enums

Die Kommunikation zwischen Views und App läuft ausschließlich über Enums —
kein direkter Methodenzugriff über Tab-Grenzen, kein geteilter mutabler
State.

```mermaid
flowchart LR
    KEY[Tastendruck] --> VIEW[View::handle_key]
    VIEW -- ViewRequest --> APP[App]
    VIEW -- SubViewMessage --> APP
    APP -- EditorRequest --> EDITOR[Editor/Skript-Dispatch]
    APP -- spawn async --> BG[Tokio-Task:<br/>Adapter-Call]
    BG -- LoadMsg via load_rx --> APP
    EDITOR -- CommitMsg via commit_rx --> APP
    APP --> RENDER[render::render]
```

| Enum               | Ort                    | Rolle                                                                                                                  |
| ------------------ | ---------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| **ViewRequest**    | `views/mod.rs`         | View → App: Editor öffnen, Service-Calls, Popups, Content laden, Drill-Down                                            |
| **SubViewMessage** | `views/mod.rs`         | Sub-View → Eltern-View: Hints, Selektion, Request-Weiterleitung                                                        |
| **LoadMsg**        | `app/mod.rs`           | async-Ergebnisse via `load_rx` (Content-Items, Preview, Aktions-Resultat, Adapter-Status, Tree-Children, Custom-Query) |
| **EditorRequest**  | `app/editor.rs`        | App → Editor-Dispatch: Inline / Launch / Script / None                                                                 |
| **CommitMsg**      | `app/editor.rs`        | Editor-Speicherergebnis via `commit_rx` (z. B. Reopen mit Konflikt-Buffer)                                             |
| **NodeAction**     | `not-yet-done-content` | adapter-deklarierte Aktion pro Knotentyp (Quelle der Shortcut-Hints)                                                   |

Async-Ergebnisse treffen über zwei `tokio::mpsc::Unbounded`-Channels ein:
`load_rx` (Adapter-Listen/Previews/Status) und `commit_rx`
(Editor-Commits). Beide werden im Loop pro Iteration gedrained — ihr
`recv()` ist zugleich der Enabler für den geplanten `select!`-Loop (ADR 0001).

### Views-Schicht

Eine einzige View-Familie, die ihren State selbst besitzt:

- **ContentView** (`content_view.rs`) — generischer, adapter-getriebener
  Baum-View, einer pro konfiguriertem Adapter. Pro Drill-Down-Kontext ein
  **ContentPane** mit eigenem `nav_stack`, `items`, Such-, Sort- und
  Pagination-State; Split-Panes verschalten mehrere Panes. Auch Tasks und
  Zeiterfassung (Trackings) laufen über ContentView — als adapter-getriebene
  Tabs mit live aktualisierter Dauer-Spalte (adaptives Tick-Intervall),
  nicht mehr als eigene View-Familie.

Tree-Darstellung läuft über `not-yet-done-forest` (verschachtelte Knoten →
flache Zeilenliste) auf Basis von `not-yet-done-table` (Spalten-Layout).

Die Tree-Ebene jeder Zeile (Spalten, Label-Spalte, Aktionen, Preview) wird
über die `node_type_chain` der Zeile aufgelöst, nicht über ihre Tiefe — so
rendern auch Multi-Branch-Bäume mit unterschiedlich tiefen Zweigen korrekt.
Die Label-Spalte wird einmal aus der Cursor-Ebene bestimmt; jede Zeile malt
ihr Label dorthin. Einzige (vom Validator erzwungene) Regel: `tree_label`
muss ein Spalten-Key der **eigenen** Ebene sein. Begründung und verworfene
Alternativen in
[ADR 0003](decisions/0003-tree-level-resolution-by-chain.md).

### Konfiguration

Views sind datengetrieben: pro View eine YAML-`ViewDef` mit rekursiven
`ChildDef`s (Knotentyp-Kette), Spalten, Aktions-Shortcuts und
Preview-Optionen. Theme/Farben kommen aus `ThemeConfig` + user-`tui.yaml`
— Farben werden nie hartkodiert. Details zum View-Format in
[`generic-view-spec.md`](generic-view-spec.md), zum Adapter-Vertrag in
[`content-adapter-spec.md`](content-adapter-spec.md).

## Host-Schicht (`not-yet-done-host`)

`host` ist die **einzige** Crate, die den `ContentAdapter`-Vertrag _und_ alle
konkreten Adapter-Crates kennt — die gemeinsame Adapter-Verdrahtung, die TUI,
`nyd` und Waybar teilen, damit jedes Frontend Adapter exakt gleich baut. Sie
exportiert:

- `factories()` / Factory-Registry — `adapter:`-Typ → Factory. Einen Adapter
  zum Produkt hinzufügen heißt: hier **einmal** eintragen; alle Frontends erben
  ihn.
- `host_context()` — baut den `HostContext` (In-Process-Event-Bus, Pfade).
- `discover_instances()` — liest die View-Files und parst je einen
  **`ViewFileHead`** (nur `adapter:` + optional `hooks:`; den Rest des
  View-Files geht den Host nichts an).
- `resolve_adapter(instance, ctx)` — Instanz → fertiger
  `Box<dyn ContentAdapter>`.

Vor Block D lebte diese Logik im TUI-Binary; CLI und Waybar konnten sie nicht
nutzen, ohne das ganze TUI zu ziehen. Die eigene Crate bricht das auf, hält den
Graph azyklisch (Frontends → `host` → Adapter → `content`) und behebt u. a. den
Waybar-Bug, der nach dem DB-Split die falsche DB las. Details in
[ADR 0005](decisions/0005-host-crate-und-lifecycle-hooks.md).

### Lifecycle-Hooks

Ein **Hook** ist ein benannter Punkt in der Lebenszeit eines Adapters, den eine
Frontend-Config zu einer Action-Invocation macht. Der Adapter _deklariert_ seine
Hook-Ids (`ContentAdapter::hooks()`, Default leer; der local-Adapter:
`["connected"]`, gefeuert direkt nach erfolgreichem Bau — beim In-Process-Adapter
also jeder Programmstart). Die Instanz-Config bindet pro Hook eine throttle-bare
Adapter-Action:

```yaml
hooks:
  connected:
    - run: backup # Adapter-Action-Id
      when: { throttle: 24h } # höchstens einmal pro Fenster (s/m/h/d)
```

Der Host feuert Hooks aus jedem Frontend — `fire_hook` gegen einen schon
gebauten Adapter (CLI, direkt nach `resolve_adapter`), `fire_connected_hooks`
beim TUI-Start (prüft den Throttle _vor_ dem Adapter-Bau, sodass ein Launch im
Fenster nichts konstruiert). Der Throttle-Zustand liegt in einer host-globalen,
adapter-unabhängigen Datei `~/.local/state/not_yet_done/hooks.json`
(`"<instanz>:<hook>:<action>"` → letzter Fire). So wird das frühere hartkodierte
tägliche `tasks.db`-Backup ein bloßer Spezialfall: `backup` an `connected` mit
24 h Throttle — frontend-übergreifend, sodass auch reine `nyd`-Nutzung das
tägliche Backup auslöst. Best-effort: schlechte Config, unbekannter Hook,
scheiternde Action oder unschreibbares State-File brechen den Aufrufer nie ab.

## Frontends neben dem TUI

- **`nyd`** (`not-yet-done-cli`) — generisches Frontend über das
  `ContentAdapter`-Protokoll: `nyd <instanz> <verb>` spricht jeden
  konfigurierten Adapter gleich an (baut ihn über `host::resolve_adapter`).
  Terse Alltagsformen sind Aliase (`cli.yaml`); `tag`/`backup`/`config` bleiben
  Built-ins.
- **`nyd-t`** (`not-yet-done-task-cli`) — native Domänen-CLI direkt auf
  `task-core`, mit typisiertem, domänengeformtem JSON und abgestuften
  Exit-Codes (Stabilkontrakt für Batch-Scripts). Adapter sind Interop-Grenzen,
  `nyd-t` ist die eigene Domäne in ihrem Idiom — siehe
  [ADR 0004](decisions/0004-zwei-cli-binaries-adapter-vs-domain.md).
- **Waybar** (`not-yet-done-waybar`) — CFFI-`.so`, zeigt das aktive Tracking in
  der Statusbar. Dünnes Protokoll-Frontend: löst über den Host denselben
  In-Process-`trackings`-Adapter auf wie TUI und `nyd` (statt selbst die DB zu
  öffnen) und liest damit die adapter-konfigurierte `tasks.db`, nicht mehr die
  `core`-DB.

## Weiterführend

- [`decisions/`](decisions/) — ADRs (Kontext, Optionen, Entscheidung,
  Konsequenzen) für gewichtige Einzel-Entscheidungen.
- [`content-adapter-spec.md`](content-adapter-spec.md) — vollständiger
  Adapter-Vertrag.
- [`generic-view-spec.md`](generic-view-spec.md) — YAML-View-Format.
- [`smoke-tests.md`](smoke-tests.md) — manuelle Testszenarien.
