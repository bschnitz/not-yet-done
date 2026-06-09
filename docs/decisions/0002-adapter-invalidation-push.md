# 0002 — Adapter-Invalidation-Push (Streaming-Adapter)

- **Status:** akzeptiert, umgesetzt (Stoat Phase 2)
- **Datum:** 2026-06-04
- **Betrifft:** `not-yet-done-content` (`Invalidation`,
  `ContentAdapter::subscribe_invalidations`), `not-yet-done-tui`
  (`LoadMsg::AdapterInvalidation`, `spawn_content_invalidation_watcher`,
  `handle_adapter_invalidation`), `not-yet-done-stoat-adapter` (Gateway)

## Kontext

Alle bisherigen Adapter sind **Pull-only**: das Frontend fragt via
`list()`/`get_by_id()`, der Adapter antwortet. Chat (Stoat) bricht das
Modell — neue Nachrichten, Edits, Deletes und Reactions treffen als
WebSocket-Events ein und müssen **out-of-band** einen Reload der
betroffenen View auslösen, ohne dass der User etwas tut.

Wichtige Vorarbeit: Der Render-Loop ist seit ADR `0001` (Variante 1b)
event-getrieben (`tokio::select!` über u. a. `load_rx`). Ein eintreffendes
Push-Signal weckt den Loop also sofort — kein Poll-Intervall mehr im Weg.
Und es existiert bereits ein Adapter→TUI-Push-Präzedenzfall:
`subscribe_status()` (`watch<AdapterStatus>`), dessen Updates ein
Hintergrund-Watcher in den `load_tx`-Kanal pumpt.

## Entscheidung

Live-Updates sind **dieselbe Mechanik wie `subscribe_status`,
generalisiert**: statt „Status hat sich geändert" → „Knoten X ist stale".

- Neuer adapterneutraler Typ `Invalidation` + Trait-Methode
  `ContentAdapter::subscribe_invalidations()` in `not-yet-done-content`,
  **No-op-Default** (ein nie-sendender Receiver, dessen Sender für die
  Prozesslaufzeit lebt). Pull-only-Adapter müssen **nichts** anfassen
  (Open/Closed).
- Das Gateway pusht `Invalidation::Node{id: <channel>}` bei Message-/
  Reaction-Events und `Invalidation::All` bei jedem `Ready` (erster
  Connect **und** Reconnect-Resync).
- Pro View spawnt die TUI neben dem Status-Watcher einen
  Invalidation-Watcher, der den Receiver in den **bestehenden**
  `load_tx`-Kanal umpumpt (`LoadMsg::AdapterInvalidation`). `poll_load`
  lädt die betroffenen Panes auf ihrem aktuellen Level neu.

## Optionen (und warum verworfen)

1. **Neuer dedizierter Push-Kanal pro Adapter bis in den Loop.**
   Verworfen: `load_tx` + der 1b-`select!`-Loop transportieren bereits
   async-Ergebnisse und wecken den Loop. Ein zweiter Kanal verdoppelt die
   Verdrahtung ohne Gewinn.
2. **`watch` statt `broadcast` für die Invalidations** (wie
   `subscribe_status`). Verworfen: `watch` hält nur den **letzten** Wert
   — zwei schnell aufeinanderfolgende Invalidations für verschiedene
   Channels würden zu einer koaleszieren, Zwischenstände gingen verloren.
   Invalidations sind diskrete **Events**, kein Latest-Value-State.
3. **`mpsc` statt `broadcast`** (so die ursprüngliche Plan-Skizze).
   Verworfen: `mpsc` ist Single-Consumer. Eine Adapter-Instanz kann
   **mehrere** Views speisen (zwei Tabs/Splits auf denselben Adapter),
   die je unabhängig `subscribe_invalidations()` rufen müssen —
   `broadcast` fächert auf, `mpsc` nicht.
4. **App-weiter `NodeRef` als Payload** (statt adapter-interner Node-ID).
   Verworfen: Der Watcher ist schon an eine konkrete View gebunden
   (`view_index`); routen muss er nicht. Er braucht nur „welches Level
   innerhalb der View" — also die rohe Parent-Node-ID, die das Pane in
   `parent_node_id()` ohnehin hält. Ein `NodeRef` (`<typ>/<instanz>/<id>`)
   müsste der Adapter erst bauen und der Watcher wieder zerlegen — Kopplung
   an die Frontend-Pfadkodierung ohne Nutzen.
5. **Bei jedem Event `All` pushen** (alles neu laden). Verworfen: Eine
   Nachricht in einem nicht-offenen Channel würde jede View dieses Adapters
   sinnlos per REST neu ziehen. `Node{id}`-Matching gegen `parent_node_id`
   lädt nur das tatsächlich sichtbare Channel-Level neu.

## Konsequenzen

- **Erster `Ready` pusht `All`** ⇒ der initial leere Stoat-Baum füllt sich
  jetzt **ohne** manuelles `r`. Reconnect resynct ebenso automatisch.
- Reload setzt den Pane-Cursor auf Standardverhalten zurück (kein „an der
  Leseposition bleiben") — für Phase 2 akzeptiert.
- Bei `broadcast`-`Lagged` (Frontend kurz zu langsam) resynct der Watcher
  konservativ mit `All` — kein Update geht verloren, nur vergröbert.
- **Verbleibende Grenze:** **Strukturelle** Live-Events
  (`ChannelCreate`/`Delete`/`Update`, `Server*`) sind noch nicht
  inkrementell auf `StoatState` angewendet — ein neu angelegter/umbenannter
  Channel erscheint erst nach einem Reconnect. Grund: das bräuchte
  inkrementelles Event-Apply (eigener Concern) und die exakten Wire-Shapes
  dieser Events sind — anders als die Message-Events — noch nicht per
  `curl` verifiziert. Folgearbeit.
- Das Muster ist **wiederverwendbar** für künftige Push-Backends (jeder
  Streaming-Adapter implementiert nur `subscribe_invalidations` + füttert
  seinen `broadcast::Sender`).
