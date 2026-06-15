# Smoke Tests

Zentrale Sammlung manueller Smoke-Tests für not-yet-done. Bei jedem
neuen Feature oder größeren Refactor: passende Tests hier ergänzen,
nicht in separaten Dokumenten. Erledigt-Marker (`[x]` / `[ ]`)
bleiben stehen, damit man sieht, was schon einmal grün war.

Bei Fund eines Bugs: stoppen, Diagnose, Fix oder festhalten BEVOR
weiter. Findings, die zu separaten Tasks werden, kurz unter dem
Punkt notieren.

## Jira ContentView — Issue-Level (Phase 1)

- [x] Liste lädt (`assignee = currentUser() ORDER BY updated DESC`)
- [x] `e` editiert Issue (Action `edit_full`, `InputSpec::Editor`) →
      3b-Template, beliebige Änderung, `:wq` → "X updated"
- [ ] Summary leeren → `:wq` → Reopen mit Error-Banner; das vorherige
      Summary wird automatisch wiederhergestellt (kein blanker Buffer
      mehr — User muss nicht blind tippen)
      → Reopen-Suffix-Bug (`.md` statt `.jira`) gefixt in `main.rs`,
      neu testen
- [x] Konkurrenter Browser-Edit auf disjunktem Feld → auto-merge
      Notification
- [x] Konkurrenter Browser-Edit auf gleicher Zeile → Reopen mit
      Conflict-Markern
- [x] `:q!` ohne Änderung → "Edit cancelled"
- [ ] `t` öffnet Transition-Picker (Action `transition`,
      `InputSpec::Picker`) → Optionen werden geladen, Auswahl mit
      Enter → "X transitioned"
      → User-YAML war veraltet (`custom_action: transition` statt
      `id: transition`). Migriert, neu testen.
- [x] `r` reload → Liste neu
- [x] `f` fuzzy-filter → tippen filtert, `enter` schließt
- [ ] `/` text-search → tippen springt, `n`/`N` next/prev
      → User-YAML hatte keine `search`-Action. Hinzugefügt, neu testen.
      → In Tasks/Trackings ist `/` ein älterer, separater Bug —
      eigener Task.
- [x] `q` öffnet Query-Menü, `Q` öffnet Query-Editor

## Jira ContentView — Drill-down (Phase 1)

- [x] `c` (navigate) drillt in Comments → Liste lädt
- [ ] `e` editiert Comment (`edit_full`) → "Comment updated"
      → Funktioniert. Erweitert in Phase 2: Edit-Action wird bei
      Comments fremder Autoren ausgeblendet (siehe unten).
- [ ] `a` (create) öffnet Editor (`create_comment`) → Body eingeben,
      `:wq` → Drill-down-Liste refresht, neuer Kommentar sichtbar
      → User-YAML war veraltet (`type: create` ohne `id`). Migriert,
      neu testen.
- [x] `backspace` zurück zu Issues
- [ ] `a` (navigate) drillt in Attachments → read-only Liste, keine
      Edit-Action
- [ ] Attachment markieren, `o` → Datei wird heruntergeladen nach
      `$TMPDIR/not_yet_done/jira_attachments/<id>-<filename>` und in
      `xdg-open` gestartet (background, keine TUI-Pause). Notification
      "opened &lt;filename&gt;". Zweites `o` auf demselben Attachment
      öffnet ohne Re-Download.

## Jira ContentView — `edit_with_comments` (Phase 2)

`Shift+e` auf einem Issue: öffnet 3b-Header + alle Kommentare in einem
Buffer (newest→oldest). Eigene Comments inline editierbar / per `del`
löschbar; neue Comments per `--- add ---`-Block. Fremde Comments
werden read-only gerendert; Konflikte landen im Banner-Reopen.

- [ ] `Shift+e` öffnet Buffer mit Header + allen Kommentaren in
      newest→oldest-Reihenfolge
- [ ] Eigenen Comment-Body editieren → `:wq` → Notification
      "X updated, comments: ~1" (oder mit `+`/`-`/`~`-Counts wenn
      mehrere Operationen)
- [ ] Eigenen Comment per `del` (oder `delete`, case-insensitive,
      einziger non-blank line) löschen → `:wq` → DELETE-Request,
      "comments: -1"
- [ ] `--- add ---` + Body am Ende → POST neuer Comment, Drill-down
      zeigt ihn anschließend
- [ ] Fremden Comment editieren → Banner-Reopen
      `# ─── COMMENTS CHANGED UPSTREAM ───` mit Bullet pro Foreign-
      Edit-Versuch + Restore aus fresh
- [ ] Konkurrenter Browser-Edit auf Comment während Editor offen →
      Banner-Reopen mit re-rendered fresh comments + User-Edit als
      Banner-Bullet
- [ ] Header-Edit + eigener Comment-Edit gleichzeitig → beides geht
      durch, eine kombinierte Notification
- [ ] Im JiraCommentNode-Drilldown bei fremdem Comment: Edit-Action
      ist nicht in der Action-Bar (nur eigene Comments editierbar)
- [ ] Reopen → User akzeptiert Foreign (löscht eigene Edit, lässt
      fresh stehen) → nächster `:wq` geht ohne Banner durch

## Schema-Validation (Phase 1, strikt)

`ActionDef` hat `#[serde(deny_unknown_fields)]`; `validate()` läuft
beim App-Start auf jeder als View-Config erkannten YAML-Datei (hat
`tab` + `adapter`). Bei Fehler `exit(1)` mit Diagnose.

- [ ] `create`-Action ohne `id:` im YAML → App startet nicht, Meldung
      "type='create' requires `id` (e.g. id: create_comment)"
- [ ] `custom`-Action ohne `id:` im YAML → App startet nicht, Meldung
      "type='custom' requires `id`"
- [ ] `navigate` ohne `navigate_to:` → App startet nicht, Meldung
      "type='navigate' requires `navigate_to`"
- [ ] Legacy-Felder (`edit:`, `custom_action:`, `query_template:`)
      im YAML → App startet nicht, serde-Fehler "unknown field"
- [ ] Adapter-Credential-File (kein `tab:`/`adapter:`) im
      Views-Verzeichnis → wird silent geskippt (kein App-Crash)

## Action-Bar / Status-Bar

- [ ] `e` (edit), `f` (fuzzy_filter), `/` (search), `q` (queries),
      `Q` (edit query), `Shift+e` (edit + comments) erscheinen in der
      Action-Bar
      → `/` jetzt im User-YAML konfiguriert, neu testen.
- [x] `r` (reload), `c`/`a` (navigate), `t` (custom-transition)
      erscheinen nur in der Status-Bar
- [ ] Beim drill-down ändern sich die Hints zur Child-Level-Konfig
      (Comments: `e`/`a`/`f`; Attachments: keine Edit-Aktionen)
      → `a` bei Comments-Child jetzt mit `id: create_comment`,
      neu testen.

### Aktiv-Markierung der Action-Bar-Hints

Die obere Action-Bar markiert jeden Shortcut, der gerade _aktiv_
(„scharf") ist, mit Akzentfarbe + fett + unterstrichen. Jeder
`ActionHint` trägt sein `active`-Flag selbst — die Komponente kennt
keine Sonderfälle mehr.

- [ ] **Jump**: `J` (bzw. konfigurierter `jump_mode`-Key) drücken →
      `jump`-Hint wird markiert, solange der Hop-Overlay offen ist;
      nach Auswahl/`Esc` erlischt die Markierung. Über alle
      Content-Tabs (Tasks (A), Trackings (A), Jira, …) und in allen
      Ansichten (Liste/Tree/Condensed).
- [ ] **Track**: in Tasks (A) / Trackings (A) ein Tracking starten
      (`t`/`s`) → `track`-Hint bleibt markiert, solange ein Tracking
      läuft; Stop → Markierung weg.
- [ ] **Cut**: einen Knoten mit `C` (mark-move) auf das Move-Clipboard
      legen → `cut`-Hint markiert, bis Paste/Abbruch/Tab-Wechsel.
- [ ] **Editor**: einen Editor öffnen (`e`/`a`) → der zugehörige Hint
      (`edit`/`add`/…) ist markiert, solange die Edit-Session offen ist.
- [ ] Rebinding-Test: `jump_mode` in `tui.yaml` auf eine andere Taste
      legen → Hint zeigt die neue Taste UND markiert weiterhin korrekt
      (Identität über die konfigurierte Taste, nicht hartcodiert).

## EditSession — Jira (Refactor Phase 7)

- [x] Issue editieren via `e` → 3b-Layout sehen, beliebiges Feld
      ändern, `:wq` → App bleibt responsive während Save (Jira ist
      langsam, gut beobachtbar), Notification "X updated"
- [x] Während Save (5–30 s Fenster) nochmal `e` drücken →
      Notification "Saving previous edit, please wait…"
- [x] Issue editieren, Summary löschen → `:wq` → Error-Banner
      ("Summary is required") im Reopen
- [x] Issue editieren, im Browser ein _anderes_ Feld ändern, lokal
      `:wq` → auto-merge: kein Reopen, Notification "X updated
      (auto-merged with upstream changes)". Auch disjunkte Body-
      Zeilen (Zeile 1 lokal, Zeile 5 upstream) gehen automatisch.
- [x] Issue editieren, im Browser _dieselbe Zeile_ anders ändern,
      lokal `:wq` → Reopen mit Banner + git-style Markern
      (`<<<<<<< ours`, `=======`, `>>>>>>> theirs`) genau auf der
      konfligierenden Zeile. Resolve durch Löschen einer Seite +
      Marker, save → "X updated"
- [x] Beim Reopen Marker stehen lassen und `:wq` → Error-Banner
      "unresolved conflict marker — keep one side and remove the
      markers"
- [x] Issue editieren, `:q!` ohne Änderung → Notification "Edit
      cancelled"
- [x] Comment via `c` (ContentChildCreate) anlegen →
      Drill-down-Liste refresht

## EditSession — Tasks

- [x] `n` Add-Task in Tree-Subview → parent wird vererbt
- [x] `n` Add-Task in List-Subview → kein parent
- [x] `e` Edit-Task → Tracking-Toggle in Form ändern, save →
      active_trackings aktualisiert, action_bar reflektiert das
- [x] `r` Restructure → Subtree editieren, mehrere `:w` während
      Editor offen → live_apply läuft, IDs werden korrekt erkannt
      (kein Doppel-Insert)
- [x] Restructure mit Parse-Fehler → query_error-Bar zeigt Fehler,
      bei nächstem erfolgreichen Save verschwindet er
- [x] Notes editieren (`o`) → speichert; mit leerem Buffer save →
      Datei wird gelöscht (Re-Test nach Fix: TaskNotesSession::new
      legte vorher eine 0-Byte-Datei an, dadurch matchte der leere
      Save mit dem leeren Template und triggerte cancel-detection
      statt commit→delete)

## EditSession — Trackings

- [x] Tracking-Script anlegen → wird unter
      `<data>/not_yet_done/tracking/scripts/` mit `chmod 755`
      abgelegt
- [x] Script ausführen (background / capture / interactive) →
      jeweiliger Modus funktioniert, bei capture öffnet sich
      Output-Editor (read-only)
- [x] Tracking-Query-Filter editieren via Query-Menu → live-apply
      während `:w`, save mit Name → favorite-Shortcut-Prompt erscheint

## `:script` fuzzy menu (Trackings + Tasks + Content)

- [ ] Trackings-Tab `x` öffnet das Menü mit den Scripts unter
      `<data>/not_yet_done/tracking/scripts/`; `X` ist nicht mehr
      gebunden (entfernt)
- [ ] Enter auf bestehendes Script → führt es aus (JSON-Argument
      enthält `tracking_ids` + `filter_min_date` + `filter_max_date`
      wie bisher)
- [ ] Typischen Namen ohne Treffer + Enter → öffnet leeren Editor
      auf neuem Script unter dem passenden Scripts-Dir
- [ ] `+name` als Eingabe + Enter → erzwingt CreateNew auch wenn
      `name` einen Treffer matcht
- [ ] Ctrl+E → öffnet das selektierte Script im Editor
- [ ] Ctrl+D → löscht das selektierte Script (mit Notification)
- [ ] Tasks-Tab (list **und** tree, beide Sub-Views): `x` öffnet das
      Menü mit den Scripts unter `<data>/not_yet_done/scripts/tasks/`
      (flach, geteilter Pool). Per-View-Title ist „Scripts · Tasks".
- [ ] Tasks-Tab `x` ohne Selektion → Notification „No task selected",
      Menü öffnet sich nicht.
- [ ] Tasks-Tab Run eines Scripts → JSON-Argument hat Form
      `{"task": {"id": "<uuid>", "description": "<desc>", "parent_id":
"<uuid>"|null, "ancestors": [{"id":..,"description":..}, …]}}`.
      `ancestors` ist root→parent (Self exklusiv). Root-Task hat
      `parent_id: null` und `ancestors: []`.
- [ ] Tasks-Tab `:script` (cmdline) → identisches Menü wie `x`.
- [ ] Tasks-Tree, Cursor auf Task in tiefem Pfad (z.B.
      `Work/Clients/acme/Tickets/#42 - …`): `ancestors` enthält
      genau die 4 Eltern in Root→Parent-Reihenfolge.
- [ ] Tasks-Tab, Script in `# mode: commands` emittiert
      `focus-node Taiga:items /ref|<slug>#<n>` → Tab wechselt nach
      Taiga, Cursor parkt auf dem Ticket (Rückrichtung des
      `goto_task.py`-Flows).
- [ ] `:script` in einem Content-Tab mit selektiertem Node → Menü
      mit Scripts unter `<data>/not_yet_done/scripts/<tab>/<view-path>/`,
      Run liefert JSON `{node: {ref, id, label, node_type, tab, instance,
fields}}` (`label` = Anzeige-Label der Zeile, z. B. die Task-Beschreibung)
- [ ] Taiga `items`-View mit gemischten Knotentypen (issue +
      userstory + task + epic): das Skript-Menü zeigt **immer
      dieselbe** Liste unabhängig vom selektierten Knoten — Pfad
      `scripts/taiga/taiga_item/`. Im JSON-`node.node_type` steht
      dennoch der Item-Typ (`taiga_issue` / `taiga_userstory` / …)
- [ ] Drill-Down in eine ChildDef (z.B. `taiga:item` → `taiga:comment`):
      Skript-Menü zeigt nun Skripte aus
      `scripts/taiga/taiga_item/taiga_comment/`, JSON enthält
      die Felder des selektierten Comments
- [ ] Per-View `actions: - {name: script, key: x, type: script}` in
      einer View-YAML → drücken von `x` triggert das Menü; ohne
      diesen Eintrag passiert auf `x` nichts (kein globaler Default
      auf Content-Tabs)
- [ ] Interactive-Skript mit `{json_file}` Placeholder im
      `interactive_command` → wird von beiden Pfaden (Trackings +
      Content) bedient (alter `{tracking_json_file}` ist
      umbenannt → tui.yaml einmal anpassen)
- [ ] Taiga `items`-View, Cursor auf einem Ticket mit `ref` wie
      `acme#42`, `:script` → `goto_task.py` ausführen → TUI springt
      auf den **Adapter**-Tab „Tasks (A)" (NICHT den Legacy-Tasks-Tab)
      und expandiert/parkt auf dem Task im Pfad
      `/work/.../<slug>/tickets/<…42…>`. Das Skript emittiert genau
      ein `tree-find "Tasks (A)" id:<uuid>`.
- [ ] Taiga `items`-View, Auto-Create-Pfad: Cursor auf einem
      Ticket, dessen lokaler Task NOCH NICHT existiert (z.B. neue
      Ticket-Nummer). `:script` → `goto_task.py`:
  - Skript ruft via CLI `task add` auf, legt `#<n> - <subject>`
    unter dem `tickets`-Parent an und löst danach dessen `id` neu auf.
  - `tree-find` erzwingt einen frischen Reload des Adapter-Tabs, **bevor**
    gesucht wird → der eben angelegte Task ist sofort sichtbar
    (Parität zum alten `reload-tasks`), Cursor parkt darauf.
  - Wiederholtes Ausführen ist idempotent (Task existiert dann
    schon → nur jump+focus). Tree zeigt KEINE Duplikate.
  - Wenn der Parent-Path (`/work/.../<slug>/tickets`) gar
    nicht existiert: Modal-Fehler aus dem Skript (stderr).
- [ ] `:tree-find` direkt (ohne Skript): `:tree-find "Tasks (A)" <text>`
      (Beschreibungs-Substring) springt auf Tasks (A) und parkt auf
      dem ersten Treffer; `n`/`N` zykeln weitere. `:tree-find "Tasks (A)"
id:<uuid>` parkt exakt auf diesem Knoten. Modal-Fehler bei
      unbekanntem Tab/View oder wenn die aktive View kein Baum ist
      (Hinweis auf `:focus-node`).

## `:query apply` — saved-query activation via cmdline

- [ ] Auf einem Content-Tab mit mind. einer in YAML definierten
      Saved Query, `:query apply <name>` (ohne `-t`) → die genannte
      Query wird im aktiven View aktiv (Action-Bar zeigt
      `Filter: <name>`), Rows reloaded, vorheriger Cursor verloren
      ist OK.
- [ ] `:query apply foo bar baz` mit Whitespace im Namen → Name
      wird komplett als ein Token interpretiert (Whitespace bleibt
      Teil des Match-Strings, Case-insensitive Vergleich).
- [ ] `:query apply -t Taiga:items <name>` von einem anderen Tab
      aus → wechselt zuerst auf Taiga:items, dann Query
      aktivieren + Reload. Wenn `<name>` nur YAML-Default ist,
      funktioniert das auch bei einem nie besuchten Tab.
- [ ] `:query apply -t Taiga:nonexistent foo` → Modal-Fehler
      „unknown view 'nonexistent' for tab 'Taiga' (available: …)",
      kein Tab-Wechsel.
- [ ] `:query apply unknown-name` → Modal-Fehler mit Liste der
      verfügbaren Saved Queries.
- [ ] Auf einem Tasks- oder Trackings-Tab ohne `-t`:
      Modal-Fehler „not on a content tab".
- [ ] Command-Chain aus einem `# mode: commands` Skript:
      `query apply -t Taiga:items <q>` gefolgt von
      `focus-node -i Taiga:items /ref|<slug>#<num>` → die Saved
      Query ist beim `focus-node`-Schritt bereits aktiv, der
      Cursor parkt auf dem Ticket (synchroner Reload zwischen
      den beiden Schritten).
- [ ] `:query` ohne Subkommando → Modal-Fehler mit Hinweis auf
      `:query apply`. `:query foo` → Modal-Fehler „unknown
      subcommand 'foo'".

## `:query edit/new/delete` — saved-query body management

- [ ] Auf einem Content-Tab mit Jira- oder Taiga-Adapter,
      `:query new Test foo` → `$EDITOR` öffnet sich auf leerem
      Buffer mit Suffix `.yaml`. Inhalt eingeben + Speichern +
      Editor schließen → Notification „Saved query 'Test foo'",
      Datei taucht unter
      `<XDG_DATA_HOME>/not_yet_done/<adapter>/<instance>/queries/Test foo.yaml`
      auf, Q-Menü (Taste `q`) zeigt sie.
- [ ] Direkt danach `:query new Test foo` nochmal → Modal-Fehler
      „'Test foo' already exists (use :query edit to modify)",
      kein Editor.
- [ ] `:query edit Test foo` → Editor öffnet sich mit dem zuvor
      gespeicherten Inhalt. Inhalt ändern, speichern, schließen →
      Notification, Datei aktualisiert.
- [ ] `:query edit unknown` → Modal-Fehler „no saved query named
      'unknown' (use :query new to create)".
- [ ] `:query delete Test foo` → Datei und ggf. Shortcut-Eintrag
      sind weg, Notification „Deleted saved query 'Test foo'",
      Q-Menü listet sie nicht mehr.
- [ ] `:query delete unknown` → keine Aktion, leise (idempotent,
      kein Modal), Notification trotzdem.
- [ ] Auf Tasks- oder Trackings-Tab: `:query edit/new/delete foo` →
      Modal-Fehler „not on a content tab".
- [ ] Auf Postgres-Tab (Adapter ohne `saved_query_store()` → noch
      nicht migriert) → Modal-Fehler „adapter 'postgres' has no
      saved-query store".
- [ ] Q-Menü sortiert die Liste so, dass neue Queries direkt
      auftauchen ohne Restart (Adapter-Store wird vor jedem
      Q-Menü-Aufruf neu gelesen).

## `:query apply` — Variablen + Popup

- [ ] Taiga-Saved-Query mit `project=${proj:alpha}` als YAML-Default:
      Shortcut-Taste (z.B. `1`) → Popup öffnet sich mit Feld
      `proj` und vorbelegtem `alpha`. Enter → Reload mit
      `project=alpha`. Esc → kein Reload, Popup zu, alte Rows
      bleiben.
- [ ] Gleiche Query: Popup öffnen, Wert auf `beta` ändern, Enter
      → Reload mit `project=beta`. Action-Bar zeigt
      `Filter: <name>` wie zuvor.
- [ ] Query mit `${proj}` (kein Default, also required): Shortcut
      → Popup zeigt Label `proj (required)`, leeres Feld. Enter
      mit leerem Feld → Inline-Fehler „'proj' is required",
      Popup bleibt offen. Wert eingeben + Enter → Reload.
- [ ] Query-Menü `Apply`-Action auf einer Query mit Variablen →
      gleicher Popup, gleiches Verhalten wie Shortcut (immer
      Popup).
- [ ] `:query apply --var proj=alpha -t Taiga:items <name>` mit
      derselben Query → kein Popup, direkter Reload mit
      `project=alpha`. Geeignet für Scripts.
- [ ] `:query apply --var proj=alpha --var x=42 -t Taiga:items
<name>` mehrere `--var` werden alle vorbelegt. Reihenfolge
      zwischen `--var` und `-t` egal.
- [ ] `:query apply -t Taiga:items <name>` ohne `--var` auf einer
      Query mit nur optionalen Variablen (alle haben Defaults) →
      kein Popup (CLI-Pfad), Reload mit Defaults.
- [ ] `:query apply -t Taiga:items <name>` auf einer Query mit
      einer required Variable und ohne `--var` für sie → Popup
      öffnet, weil mind. eine required nicht abgedeckt ist.
- [ ] `:query apply --var=oops -t ...` (kein `=` im Wert) →
      Modal-Fehler „--var expects k=v".
- [ ] Saved-Query ohne Variablen (kein `${...}`) → Verhalten
      unverändert, kein Popup, direkter Reload.
- [ ] Tab-Wechsel/Tabwechsel und andere Popups: Query-Var-Popup
      verhält sich wie andere Modals — solange offen, schluckt
      es alle Keys außer Esc.

## EditSession — Query-Menu (alle drei Tabs)

- [x] Tasks-Tab: Filter neu anlegen, save → DB persist +
      favorite-Prompt
- [x] Trackings-Tab: bestehenden Filter editieren → kein Prompt
- [x] ContentView: Query editieren mit/ohne save_name

## EditSession — Editor-Pfade

- [x] Launch-Modus (User-Default): kitty-Split öffnet, `:wq` schließt
      Split, App ist responsive
- [x] Inline-Modus (`editor.inline: true` in tui.yaml): TUI pausiert,
      Editor bekommt Terminal, Resume nach `:wq` — funktioniert
      weiter wie vorher (sync-await, kann blockieren bei langsamem
      Backend)

## Tasks/Trackings — `/` text-search

- [ ] Tasks-Tab: `/` öffnet die Such-Leiste in der Action-Bar (`/ ` +
      Cursor + "type to search…"-Placeholder)
- [ ] Tippen filtert die Auswahl auf den ersten Match. Zeile springt
      automatisch zum ersten Treffer
- [ ] `n`/`N` springen zum nächsten/vorigen Match
- [ ] `enter` schließt die Such-Leiste, Auswahl bleibt auf dem Treffer
- [ ] `esc` mit leerer Query schließt die Leiste; mit nicht-leerer
      Query → erst clearen, dann schließen (zwei `esc`)
- [ ] Trackings-Tab: dasselbe Verhalten

## Jira — Free-text search (`s`)

- [ ] Jira-Tab Root-Level: `s` öffnet die Action-Bar mit Prompt `? `
      und dem in `jira.yaml` konfigurierten Placeholder (`Jira-Suche`,
      kein `[n/m]`-Counter)
- [ ] Tippen lässt nichts lokal passieren (kein Filter, keine
      Selektions-Sprünge — Eingabe geht erst beim Submit raus)
- [ ] Plain-Text-Eingabe + `enter` → aktive Query wird zu
      `text ~ "<input>"` (kein `ORDER BY`, `{key_or}` bleibt leer);
      Reload läuft, Query-Name wird geleert (anonyme Query). Treffer
      sind nach Lucene-Score sortiert (best match zuerst), nicht nach
      Datum
- [ ] Eingabe einer Issue-Key-Form (`ABC-123`) + `enter` → Query wird
      zu `issuekey = "ABC-123" OR text ~ "ABC-123"`; das genannte
      Ticket erscheint im Ergebnis
- [ ] `esc` mit leerer Query schließt die Leiste; mit nicht-leerer
      Query → erst clearen, dann schließen
- [ ] Eingabe mit `"`/`\`-Zeichen → JQL bleibt valide (Escape via
      `\"` / `\\`); Suche läuft ohne 400er
- [ ] Prompt-Override: wenn `prompt:` im YAML entfernt wird, fällt der
      Placeholder zurück auf `free-text search…`
- [ ] `q` (Query-Menü) und `Q` (Query-Editor) funktionieren weiterhin
      — `s` darf den anderen Such-Pfaden nichts wegnehmen
- [ ] **Bug-Regression**: in einem Content-Tab `s` drücken triggert
      KEIN Tracking-Toggle mehr (vorher: Last-Tracking aus Trackings
      wurde gestartet)

## Jira — Toggle Watch (`w`)

- [ ] Auf einem nicht-gewatchten Issue `w` drücken → Status-Bar zeigt
      `<KEY>: watching`; nach kurzem Reload taucht das Ticket im
      Saved-Query "Watched Tickets" (`ctrl+w`) auf
- [ ] Auf einem gewatchten Issue `w` drücken → Status-Bar zeigt
      `<KEY>: no longer watching`; nach Reload ist das Ticket aus dem
      `ctrl+w`-View verschwunden
- [ ] Fehlerfall (z.B. Auth-Status nicht ok / Issue nicht erreichbar)
      → Status-Bar zeigt `Action failed: …`, kein Crash, View bleibt
      stehen

## Jira — Saved Queries (Ctrl-Shortcuts)

Bodies liegen unter
`<XDG_DATA_HOME>/not_yet_done/jira/<instance>/queries/<name>.yaml`,
Shortcuts in der DB-Tabelle `query_shortcut`
(Scope `jira:<instance>:tickets`).

- [ ] `ctrl+i` lädt "My Tickets" (`assignee = currentUser()`)
- [ ] `ctrl+w` lädt "Watched Tickets" (`watcher = currentUser()`) —
      darf NICHT mit dem `w`-Toggle-Watch kollidieren
- [ ] `ctrl+m` lädt "Mentioned In" — darf NICHT als `enter` interpretiert
      werden (Kitty-Protokoll erforderlich, sonst kollidiert es mit der
      Selektions-Aktion)
- [ ] `q`-Menü listet exakt die Bodies aus dem queries-Verzeichnis
      des Adapters (keine YAML-`saved:`-Reste mehr). Action-Bar zeigt
      die Shortcuts an der Saved Query — `My Tickets [ctrl+i]` etc.
- [ ] `:query delete <name>` löscht Body **und** Shortcut-Row; nach
      App-Restart sind beide weg.

## Jira — Labels / Assignee / Mentions im Edit-Template

- [ ] `e` auf einem Issue: editierbare Sektion zeigt jetzt
      `summary:`, `labels:` (CSV mit `ll-…`-Slugs), `assignee:`
      (`uu-…`-Slug oder leer)
- [ ] Am Ende des Templates erscheint `#### CACHE / available labels
& users (do not edit) ####`-Block mit den Slugs aus dem Cache
- [ ] Label hinzufügen/entfernen via `ll-…`-Slug → speichert korrekt
      (Liste wird ersetzt, nicht ergänzt)
- [ ] Assignee ändern via `uu-…`-Slug → speichert; leerer Wert
      un-assigned das Issue
- [ ] Unbekanntes `ll-foo` oder `uu-foo` → Reopen mit Banner-Fehler
      ("unknown label slug …" / "unknown user slug …")
- [ ] `Shift+E` (`edit_with_comments`): in jedem Comment-Body wird
      `[~JDOE1]` als `@uu-jane-doe` angezeigt
- [ ] Comment unverändert speichern → kein Change-Event (Roundtrip
      sauber, kein Update an Jira)
- [ ] In `--- add ---` Block ein `@uu-…` schreiben → kommt bei Jira
      als `[~KEY]`-Mention an
- [ ] Unbekanntes `@uu-…` in Comment → Reopen mit Banner-Fehler
- [ ] Kollidierende Slugs (zwei Labels normalisieren auf gleichen
      Wert) bekommen `-2`, `-3` Suffix; deterministisch über Restarts

## Jira — Merge-Only User/Label Cache (Issue-basiert)

Hintergrund: alter Bulk-Pull (`/rest/api/2/user/search?username=.`) ist
kaputt — Server cappt unentdeckt bei 100 Treffern. Stattdessen wird der
Cache jetzt rein über tatsächlich geladene Issues gefüttert
(assignee + reporter + creator + Comment-Authoren + Labels) und ist
strikt additiv: was einmal drin ist, bleibt drin; existierende Einträge
bekommen bei Re-Merge nur ihren `display_name` aktualisiert.

- [ ] Erster App-Start nach Update: stderr zeigt einmalig
      `nyd: cleaned up N orphan jira_user and M orphan jira_label
row(s) from previous schema` (legacy-Rows aus dem alten
      `run_sync`-Pfad mit anderer connection_id)
- [ ] Zweiter Start: Meldung kommt **nicht** mehr (nichts mehr zu
      räumen)
- [ ] Issue mit bekanntem Reporter/Creator öffnen, der bisher nicht in
      der CACHE-Liste stand → CACHE-Sektion am Buffer-Ende führt ihn
      jetzt mit `uu-…`-Slug
- [ ] DB-Inspektion: nach Issue-Open ist der Reporter/Creator als
      Zeile in `jira_user` (gleiche `connection_id` wie vorhandene
      Einträge — UUID v5 vom Jira-URL)
- [ ] Issue mit Comment-Author, der bisher nicht im Cache war →
      Author taucht als `uu-…`-Slug in CACHE-Sektion auf, im Comment-
      Body wird sein `[~KEY]` → `@uu-…` aufgelöst
- [ ] User wird im Jira umbenannt → nach erneutem Issue-Open für
      ein Ticket, in dem er auftaucht: `display_name` ist im Cache
      aktualisiert (in DB und in der CACHE-Sektion); `username`
      bleibt stabil
- [ ] User wird im Jira deaktiviert → bleibt im Cache (Merge-only,
      es wird nie gelöscht); Slugs zu alten Comments funktionieren
      weiter
- [ ] App-Restart → Cache wird aus DB hydratisiert, sofort beim ersten
      Issue-Open ist die CACHE-Sektion gefüllt (kein Bulk-API-Call mehr
      nötig — kein "ladend" Zustand)
- [ ] `[~KEY]` in einem Comment, der KEY noch nicht im Cache:
      `Shift+e` löst die KEY per `/rest/api/2/user?username=KEY`
      auf, mergt in Cache + DB, rendert `@uu-…`-Slug in dem Comment
- [ ] `[~UNKNOWN]` (KEY existiert auch in Jira nicht) → Lookup gibt
      Fehler, KEY bleibt im Render verbatim als `[~UNKNOWN]` stehen
      (kein Crash)
- [ ] CLI-Export `nyd content list jira:user` → API-Call, Result wird
      zusätzlich in den Cache + DB gemergt (nächste Session findet
      die User dort)
- [ ] Alte YAML mit `cache: { preload: true, label_ttl: 86400,
user_ttl: 86400 }` → App startet ohne Validation-Fehler, das
      Block wird stillschweigend ignoriert (Felder sind tot)

## Sort-Hint Mode (Phase 6)

Default-Keybinding `S`. Zwei Phasen, beide rendern das Overlay direkt
in den Tabellen-Headern (kein ActionBar-Overlay). Spaltenbreiten
bleiben über alle Phasen hinweg stabil.

- [ ] Tasks: `S` aktiviert Sort-Mode. In den sortierbaren Headern
      erscheint an Position 0 ein Label-Buchstabe (`a`, `b`, `c`, …),
      die ersten Zeichen des Originalheaders werden überschrieben
      (z. B. `Status` → `atatus`, `Pri` → `bri`, `Task` → `cask`).
      Nicht-sortierbare Spalten (z. B. `Tr`, `N`) sind gedimmt.
- [ ] Tasks: Bereits sortierte Spalten behalten beim `S` ihren
      Sortpfeil neben dem Label (`Status ▲` → `atatus ▲`); die
      Spaltenbreite ändert sich beim Eintritt in Sort-Mode nicht.
- [ ] Tasks: Drücken eines Label-Buchstabens schaltet auf Phase 2 um.
      Über dem gewählten Header erscheint als Overlay
      `(d)esc/(a)sc/(c)lear` (in der Akzent-Farbe). Der darunter
      liegende Header behält die Originalbreite — andere Spalten
      verschieben sich **nicht**, das Overlay kann benachbarte
      gedimmte Header optisch überdecken.
- [ ] Tasks: `a` (asc) / `d` (desc) / `c` (clear) führen die Aktion aus.
      Sortierte Spalte zeigt anschließend `▲` bzw. `▼` neben dem
      Original-Header.
- [ ] Tasks: Multi-column Sort ist additiv. Wenn nach Spalte `Status`
      sortiert wurde und anschließend `S` → `Pri` → `a` gedrückt wird,
      werden **beide** Sorts angewendet. Beide Spalten zeigen einen
      Pfeil mit Index-Subscript (`Status ▲₁`, `Pri ▲₂`).
- [ ] Tasks: `c` auf einer der sortierten Spalten entfernt nur diese
      eine Sort-Ebene aus dem Stack; die übrigen Sorts bleiben aktiv.
- [ ] Tasks: Sort persistiert über Restart (`settings`-Tabelle Key
      `tasks.sort`); Pfeil bleibt nach Restart sichtbar.
- [ ] Tasks: in Tree-Mode bleibt Reihenfolge der Geschwister konsistent
      mit der gewählten Sort-Spalte.
- [ ] Jira ContentView: `S` zeigt Labels in den Adapter-Headern
      (sortierbare Felder per YAML); Auswahl + Direction triggern
      Reload mit neuer Sort.
- [ ] Jira/Taiga ContentView: Sort persistiert pro `query_scope` über
      Restart (eigene `jira_view_sort_state` / `taiga_view_sort_state`
      Tabellen in der Adapter-DB).
- [ ] `Esc` in beiden Phasen schließt den Mode ohne Änderung; Header
      kehren in Originaldarstellung zurück.
- [ ] Tab-Wechsel während Sort-Mode aktiv → Mode schließt automatisch.
- [ ] Trackings: `S` zeigt **keinen** Sort-Hint in der Status-Bar
      (Trackings ist bewusst ausgenommen).

## Auth — Explicit invalidation (Phase 5)

- [ ] Cmdline `:invalidate-session` auf einem Content-Tab:
      Status-Bar meldet _"Session invalidated, re-authenticating…"_,
      `auth_session`-Row für die Connection ist weg, nächster List-Call
      triggert Re-Auth (Cookie-Script läuft / JWT wird neu gezogen),
      Liste lädt durch.
- [ ] Cmdline `:invalidate-credentials` auf einem Content-Tab mit
      `prompt`-Provider (Taiga): Status-Bar meldet
      _"Credentials invalidated, re-authenticating…"_, Resolver-Caches
      und Prompt-Cache sind leer, Credentials-Popup erscheint erneut.
- [ ] Cmdline `:invalidate-session` auf einem Nicht-Content-Tab
      (Tasks/Trackings) → Modal _"… only works on a content tab"_,
      keine Aktion.
- [ ] YAML-Action `type: invalidate_session` mit Keybinding feuert
      identisch zur Cmdline-Variante. Tipp:
      `- { name: forget session, key: I, type: invalidate_session }`
      in `views[].actions`.

## Taiga — Notifications subtab (#149–#152)

Voraussetzung: `~/.config/not_yet_done/views/taiga.yaml` enthält den
`notifications`-View (`node_type: taiga:notification`, `key: n`,
Actions inklusive `mark as read` und `open ticket`).

- [ ] Auf Taiga-Tab: `n` schaltet auf den Notifications-Subtab um,
      die Liste lädt; Spalten _Read/Event/Ref/Project/Actor/Created/
      Subject_ sind sichtbar; Default-Sort ist zweispaltig _read asc,
      created desc_ — alle ungelesenen erscheinen als Block oben
      (innerhalb des Blocks neueste zuerst), darunter der Read-Block in
      gleicher Datums-Reihenfolge. `i` schaltet zurück auf _items_.
- [ ] Wenn die Liste fehlschlägt (z. B. abgelaufenes JWT), erscheint
      ein roter Banner _"Fetch failed: …"_ am oberen Rand der
      Content-Fläche statt eine wortlos leere Tabelle (Regression-
      Schutz: vorher wurde `fetch_error` nirgends gerendert).
- [ ] Pagination-Footer zeigt _N total_, blättern via Next/Prev
      funktioniert (sofern mehr als die Default-Page-Size existiert).
- [ ] Sort-Hint Mode (`S`) zeigt die Notification-Sortspalten
      (created, event, project, actor, read, subject); Asc/Desc-
      Wechsel sortiert in-place ohne Reload.
- [ ] `m` auf einer ungelesenen Zeile → Status-Bar zeigt _"Notification
      #X marked as read"_, View lädt nach, der _Read_-Wert dieser
      Zeile wechselt auf _read_, weiteres `m` auf einer bereits
      gelesenen Zeile → _"Action `mark_as_read` not exposed by node"_
      (oder ähnlich; kein API-Call).
- [ ] `e` (open ticket) auf einer Notification → Edit-Editor öffnet
      sich für das _verlinkte_ Ticket (nicht für die Notification);
      Speichern wirkt auf das Ticket; Schließen kehrt zur
      Notifications-Liste zurück.
- [ ] `e` auf einer Notification mit unbekanntem `content_type`
      (z. B. wiki*page) → Notify *"… target*id … empty"* (Action
      ist no-op statt Crash).
- [ ] `:invalidate-session` auf dem Notifications-Subtab feuert
      identisch zum items-Subtab; nächste Liste lädt nach Re-Auth
      durch.

## Postgres adapter — Phase A (databases-only)

Voraussetzung: `~/.config/not_yet_done/views/postgres.yaml` aus dem
Repo (gibt den Tab Postgres mit dem Subtab _databases_ vor) und ein
selbst gepflegtes `~/.config/not_yet_done/views/postgres-adapter.yaml`
mit Transport- + Postgres-Block. Beispiel-Skelett:

```yaml
# Optional: hard deadline for each postgres call. On timeout, the
# session + transport are torn down and the next call reconnects
# lazily. Omit for libpq-style "wait forever".
query_timeout_secs: 7

transport:
  mode: ssh_tunnel # or: direct
  # `ssh` is a list of hops. The first entry connects via local TCP;
  # every subsequent entry runs a fresh SSH handshake over a
  # direct-tcpip channel of the previous hop, so its `host:port` is
  # resolved by the predecessor (e.g. `localhost:2222` on hop #2 means
  # localhost relative to hop #1). The **last** hop opens the
  # direct-tcpip channel to `target`.
  ssh:
    - host: bastion.example.invalid
      port: 22
      user: alice
      auth:
        kind: public_key # or: password | agent
        identity: ~/.ssh/id_ed25519
        passphrase: # optional, only if key is encrypted
          type: command
          script: pass ssh/bastion-key
    # Optional second hop (DBeaver-style jump server). Drop this entry
    # for a single-hop tunnel.
    - host: localhost
      port: 2222
      user: someone
      auth:
        kind: password
        password:
          type: keyring
          service: nyd-ssh-jump
          account: someone
  target:
    host: db.internal.invalid
    port: 5432

postgres:
  user: dbuser
  password:
    type: keyring
    service: nyd-postgres-prod
    account: dbuser
  admin_database: postgres # optional; default `postgres`
  sslmode: prefer # optional; one of disable | prefer | require
```

- [ ] **Direct mode**: `mode: direct`, `target` zeigt direkt auf einen
      lokal erreichbaren Postgres → Tab _Postgres_ erscheint, Subtab
      _databases_ lädt; Spalten _Name / Owner / Encoding_ sind
      gefüllt; Templates (`template0`, `template1`) sind nicht
      enthalten; sortiert alphabetisch.
- [ ] **SSH-tunnel mode mit `kind: agent`**: `ssh-add -l` zeigt
      mindestens eine Identität → Tab lädt ohne Passwortprompt,
      `ss -lntp | grep 127.0.0.1:` zeigt einen ephemeren Listener,
      Subtab _databases_ lädt durch. Beim Schließen des Tabs
      verschwindet der Listener.
- [ ] **SSH-tunnel mit `kind: public_key`** und encrypted key:
      `passphrase`-Provider feuert genau einmal (z. B. `pass`-Aufruf
      einmal), Folge-Listings nutzen die gecachte Sitzung
      (kein erneuter Aufruf).
- [ ] **SSH-tunnel mit `kind: password`** (Bastion) und
      `password.type: keyring`: Login zieht das SSH-Passwort aus dem
      keyring; falsches Passwort → Tab zeigt Fehler-Banner _"ssh auth
      failed: server rejected credentials"_ statt leerer Liste.
- [ ] **Zwei-Hop-Kette (Jump-Server, DBeaver-äquivalent)**: `ssh:`
      enthält zwei Einträge — z. B. Public-Key auf Hop #1 und Passwort
      auf Hop #2. Beim Tab-Öffnen authentifizieren beide Hops; auf der
      Bastion zeigt `ss -tnp | grep <hop2-port>` einen Connect-Out vom
      sshd-Forked-Prozess. Falsches Passwort auf Hop #2 → Banner _"ssh
      auth failed (hop #1): …"_ (Index 1 = zweiter Eintrag). Hop-#1
      OK + falscher Hop-#2-Host → Banner _"ssh channel error: open hop
      #1 (…): …"_.
- [ ] Postgres-Auth fehlerhaft (z. B. falsches `postgres.password`):
      Tab zeigt Fehler-Banner _"postgres connect: password
      authentication failed for user …"_; nach Korrektur und Reload
      lädt die Liste.
- [ ] Tunnel-Drop unter Last: SSH-Session vom Server kappen
      (z. B. `pkill -f "sshd: alice"`) → nächster Reload reconnectet
      stillschweigend (lazy reconnect in `PostgresClient`).
- [ ] **`query_timeout_secs: N` (Adapter-Top-Level, optional)**:
      Mit z. B. `query_timeout_secs: 7` in
      `views/postgres-adapter.yaml`. Halbgeschlossener Tunnel
      simulieren (z. B. lokales `iptables -A OUTPUT -p tcp --dport <forward> -j DROP`
      auf dem Forward-Port, oder Bastion-sshd Prozess pausieren mit
      `kill -STOP`). Beim nächsten Reload zeigt der Banner einen
      Countdown `… (0s/7s) → (1s/7s) → …`. Nach 7s: Fehler-Banner
      _"… : timed out after 7s; connection reset"_, gefolgt von
      `Ready`. Erneutes `r` → frischer Tunnel + Session werden
      lazy aufgebaut.
- [ ] **View-Retries (`retries: N` auf einer View in
      `views/postgres.yaml` o.ä.)**: Mit `retries: 2` und
      `query_timeout_secs: 7`. Tunnel wie oben blockieren. Erwartet:
      Banner zeigt `Connecting/Busy` Countdown des 1. Versuchs; nach
      7s flippt der Text auf
      _"Retrying (2/3) — list databases (0s/7s): … timed out after
      7s; connection reset"_; nach weiteren 7s `Retrying (3/3) — …`;
      nach insgesamt ~21s wird der Fehler sticky als
      _"Fetch failed: …"_-Banner und der Retry-Status verschwindet.
      Tunnel freigeben, dann `r` → erfolgreicher Reload räumt
      `fetch_error` weg.
- [ ] **`retries: 0` (Default)**: Gleiches Setup ohne `retries:` —
      Banner zeigt nur einen Versuch, sofort _"Fetch failed: …"_
      ohne `Retrying`.
- [ ] **Manual-Connect (`adapter.manual_connect: true`)**: Auf
      `views/postgres.yaml` im `adapter:` Block
      `manual_connect: true` setzen. TUI starten. Erwartet: Postgres-
      Tab zeigt sofort _"Auto-connect disabled — press `r` to
      connect"_ als Banner; **keine** Connection-Versuche, keine
      Timeouts in Logs/Wartezeit. Subtab-Wechsel `d`/`t`/`s` triggert
      ebenfalls **keinen** Load — jeder Subtab zeigt denselben
      Banner. Erstes `r` startet den ersten Load (Banner wird zu
      `Connecting/Busy` Countdown). Nach erfolgreichem Load
      verschwindet der Banner, normales Verhalten ist
      hergestellt. Subtab-Switch auf einen bereits geladenen Subtab
      zeigt Cache; auf einen noch nicht geladenen zeigt erneut
      _"Auto-connect disabled …"_.
- [ ] **Manual-Connect ohne reload-Action**: Auf einer View mit
      `manual_connect: true` aber **ohne** `type: reload` in
      `actions:`. Banner liest
      _"Auto-connect disabled — no `reload` action configured for
      this view"_; der Tab bleibt dauerhaft leer (Soft-Fehler, kein
      Crash).
- [ ] **Manual-Connect off (Default)**: `manual_connect` nicht
      setzen / `false` → Tab lädt automatisch beim Start wie
      bisher; keine Regression.
- [ ] **Ohne `query_timeout_secs`**: Bisheriges Verhalten — Banner
      zeigt nur Elapsed (`… (3s)`), kein Auto-Reset. (Optional, nur
      für Regression.)
- [ ] Subtab _databases_ hat **keine** Aktionen (nur Default-Reload
      via `r` falls global gebunden); `e` / `c` / `a` zeigen
      _"Action … not exposed by node"_.
- [ ] `transport.mode: ssh_tunnel` ohne `ssh:` Block → Tab erscheint
      mit roter Banner _"Invalid Postgres config: transport.mode=
      ssh_tunnel requires an `ssh:` block"_; Liste leer.
- [ ] Unbekanntes Feld unter `postgres:` (z. B. `schema:`) → Tab
      bleibt leer mit Banner _"Invalid Postgres config: unknown field
      `schema` …"_ statt stillem Akzeptieren.

### Postgres — Tabellen-Rows-Drilldown (`o` → split right)

Voraussetzung: `views/postgres.yaml` enthält den `Rows`-Child mit
`key: o`, `split: right ratio 0.8`, `pagination: server page_size 100`.
Der Drilldown nutzt `query_rows` mit `ORDER BY ctid` und `LIMIT/OFFSET`.

- [ ] Auf einer Tabellenzeile `o` drücken → neuer Pane öffnet rechts
      (Verhältnis 1:4: links schmal, rechts breit), Akzent-Border auf
      dem rechten Pane, gedimmter Border links. Im rechten Pane laden
      bis zu 100 Rows.
- [ ] Spalten werden automatisch aus den Daten abgeleitet (kein
      `columns:`-Eintrag im YAML), alle gleich breit, Header zeigt die
      Postgres-Spaltennamen.
- [ ] NULL-Werte erscheinen als `(null)`; nicht-Text-Spalten
      (z. B. `int`, `timestamptz`, `bool`, `jsonb`) werden korrekt als
      Text dargestellt.
- [ ] `>` lädt nächsten 100er-Block (Footer zeigt `100–199`),
      `<` zurück zum vorigen. Das funktioniert auch auf einer Tabelle
      mit mehr als 200 Rows.
- [ ] Auf einer kleinen Tabelle (< 100 Rows): `>` ist no-op
      (kein has_next).
- [ ] Tabelle mit ungewöhnlichem Spaltennamen (z. B. mit `"` darin) →
      Query darf nicht crashen, Spalte wird korrekt gequotet.
- [ ] `backspace` schließt den rechten Pane / drillt zurück
      (je nach Pane-Lage); Liste der Tables im linken Pane bleibt
      erhalten.

## Split-Pane (Phase 2 + 3 + 4)

Phase 2 hat manuelle Splits eingebaut, Defaults jetzt `wv`/`ws`/`wq`
(Leader `w`, früher `ctrl+w`). Phase 3 öffnet Splits über `split:` in
der ChildDef beim Drilldown. Phase 4 trägt jedem Pane einen
Buchstabentag und einen `<leader><letter>`-Switcher nach.
`pane_tags`-Alphabet wird automatisch gegen die Action-Keys (v/s/q
in den Defaults) gefiltert, damit nichts kollidiert.

### Phase 2 — Manuelle Splits

- [ ] Beliebigen Content-Tab öffnen (Jira/Taiga/Postgres) → `wv`
      → zweiter Pane rechts, Akzent-Border auf rechtem Pane,
      gedimmter Border auf linkem; Fetch feuert für neuen Pane.
- [ ] `wq` auf rechtem Pane → schließt, linker Pane bekommt
      Fokus zurück, Border verschwindet (Single-Leaf).
- [ ] `ws` → Split unten; in jedem Pane unabhängig navigieren.
- [ ] In linkem Pane in Postgres-DB drillen; mit `wv` splitten;
      im rechten Pane anders weiterdrillen → beide behalten ihren
      Drilldown-State.
- [ ] Subtab-Wechsel (z. B. Taiga Tickets ↔ Notifications) bei
      offenem Split im aktuellen Subtab → der andere Subtab-Tree
      (Single-Leaf) erscheint, ursprünglicher Split-State beim
      Zurückwechseln intakt.
- [ ] Sort-Overlay (`S` in Jira/Taiga) erscheint nur auf fokussiertem
      Pane.
- [ ] Action-Bar / Breadcrumb passen sich nach Split an fokussierten
      Pane an.
- [ ] Input-Mode-Guard: bei aktivem fuzzy-Filter (`f`), Suche (`/`)
      oder Cmdline (`:`) → `w` wird als normales Zeichen ins
      Input-Feld geschrieben, NICHT als Leader interpretiert (sonst
      wäre `w` im Suchtext nicht tippbar).

### Phase 3 — `split:` auf ChildDef

- [ ] In einer YAML-View einen `children:`-Eintrag mit
      `split: { direction: right, ratio: 0.5 }` konfigurieren →
      Drill-Key (Enter / Open) öffnet die Child-Ebene als neuen Pane
      rechts vom aktuellen, Fokus auf neuem Pane.
- [ ] `direction: bottom` → neuer Pane unten.
- [ ] `direction: left` / `direction: top` → neuer Pane links / oben
      vom Source.
- [ ] `ratio: 0.7` → neuer (gedrillter) Pane bekommt 70 % des Platzes.
- [ ] Ohne `split:` → klassischer In-Place-Drilldown (kein neuer Pane).
- [ ] Im neuen Split-Pane mit Back-Key (`Esc`/`h`) → kehrt zur Parent-
      Liste zurück (NavFrame mit den Parent-Items wurde beim Split-
      Drill auf den neuen Pane übertragen).
- [ ] `wq` auf dem Split-Drill-Pane → schließt zurück zur
      Single-Pane-Ansicht; Source-Pane ist unverändert.
- [ ] Adapter-Cache: zweimal hintereinander split-drillen (erst
      schließen, dann erneut) → zweite Anfrage liefert sofort aus
      dem Cache, keine doppelten HTTP-Calls (der `Arc<dyn ContentAdapter>`
      wird geteilt).

### Phase 4 — Letter-Tags + `w<letter>`

Default-Alphabet `asdfghjkl`, mit Auto-Filter gegen v/s/q (= Action-
Keys der Default-Window-Bindings) → effektives Alphabet `adfghjkl`.

- [ ] Single-Pane: kein Border, kein sichtbarer Tag.
- [ ] `wv` → Split rechts. Im linken Pane steht oben links im
      Border `a`, im rechten `d` (jeweils gestylt: fokussierter
      Pane in Akzentfarbe + bold, ungehfokussierter in dim). `s`
      wird übersprungen, weil es als Action-Key reserviert ist.
- [ ] `wd` → Fokus springt auf Pane `d`; Bordertitel wechselt die
      Farben (`d` → bold/Akzent, `a` → dim). Action-Bar zeigt jetzt
      Aktionen des `d`-Panes.
- [ ] Im fokussierten Pane noch ein Mal `wv` → dritter Pane mit
      Tag `f` (nächster freier Buchstabe nach `a` und `d`).
- [ ] `wa` → Fokus zurück auf den ersten Pane. Tag-Zuweisungen
      bleiben stabil (kein Re-Layout der Buchstaben).
- [ ] Pane `d` schließen (`wq`) → Tag `d` ist wieder frei. Den
      verbleibenden Pane mit `f` erneut splitten → der neue Pane
      bekommt `d` (recyclt) statt `g`.
- [ ] `w` gefolgt von einem nicht zugewiesenen Buchstaben (z. B.
      `z`, oder ein Buchstabe der zwar im Alphabet ist aber kein
      Pane trägt) → keine Aktion, Chord wird sauber abgebrochen
      (`window_pending` zurück auf `None`).
- [ ] In `tui.yaml` `pane_tags: "qwert"` setzen → nach Restart bekommen
      Panes Tags aus `wert` (`q` ist Close-Action und wird gefiltert).
- [ ] In `tui.yaml` `window:` Bindings auf `gv`/`gs`/`gq` ändern →
      nach Restart funktioniert der Switcher mit Leader `g`
      (`gv` = split right, `ga` = switch zu Pane `a`). Default-
      Alphabet wird gegen v/s/q gefiltert (selber Filter-Output).
- [ ] Subtab-Wechsel mit aktiven Splits: Tag-Belegung des einen Trees
      beeinflusst den anderen nicht (jeder Tree hat eine eigene
      Allokation, die mit `a` startet).

### Phase 4 follow-up — Chord-Precedence + Action-Bar-Mode

- [ ] **Chord-Vorrang über andere Handler**: Pane `s` (mit Tag `s`)
      lässt sich tatsächlich nicht entstehen, weil `s` als Action-Key
      reserviert ist — daher als Stellvertreter ein Subtab-Switch
      ausprobieren: in einer View mit Subtab-Key `i` (Taiga _items_)
      `w` drücken, dann `i` → der Subtab wechselt **nicht**, der
      Chord wird sauber als _ungebunden_ aufgelöst (Action-Bar wieder
      normal). Vorher hätte der Subtab-Switch das `i` weggeschnappt.
- [ ] Mit gesetzten Saved-Query-Shortcuts (z. B. `1` lädt "My Bugs")
      `w1` drücken → keine Saved-Query wird geladen, Chord wird
      verworfen.
- [ ] **Action-Bar-Mode-Indikator**: `w` drücken → Action-Bar zeigt
      links bold + Akzentfarbe `WINDOW  │  v split right  s split down
q close pane` (bei mehreren Panes zusätzlich `<a/d/…>  switch
pane`).
- [ ] Beliebigen Chord auflösen (z. B. `wv`) → Action-Bar fällt zurück
      auf die normale Hint-Liste (ohne `WINDOW`-Label).
- [ ] Während eines aktiven Inputs (`f` fuzzy / `/` search / `:` cmdline)
      `w` drücken → keine `WINDOW`-Anzeige, `w` wird ins Input-Feld
      geschrieben.

## Coupled-Split (Phase 1)

Voraussetzung: `~/.config/not_yet_done/views/postgres.yaml` setzt am
`Rows`-Child `split.coupled: true`. Eltern-Pane = Tabellenliste,
Child-Pane = Rows-View.

- [ ] Tabellenliste fokussieren, `o` auf Tabelle T1 → rechter Split
      öffnet, lädt Rows von T1, Fokus liegt auf dem neuen Rows-Pane.
- [ ] Zurück in den Tabellen-Pane wechseln (`wa` o. ä.), eine andere
      Tabelle T2 selektieren, `o` → der bestehende Rows-Pane lädt um
      auf T2 (kein neuer Split entsteht). Fokus bleibt auf dem
      Tabellen-Pane.
- [ ] Mehrere Wechsel hintereinander (T1 → T2 → T3) → immer derselbe
      Rows-Pane, Spalten passen sich der jeweiligen Tabelle an
      (auto-derived columns).
- [ ] Im Rows-Pane `wq` (manuell schließen) → Backlink im Tabellen-Pane
      löst sich; nächstes `o` öffnet wieder einen neuen Split.
- [ ] Tabellen-Pane (Eltern) `wq` schließen → Rows-Pane (Kind) wird
      mit zugemacht (Cascade), Fokus geht auf einen verbleibenden
      Pane (z. B. Schema-Pane bei tieferer Drill-Hierarchie).
- [ ] Cascade-Kollisions-Schutz: Wenn nur die zwei gekoppelten Panes
      offen sind (Tabellen + Rows, sonst nichts), `wq` auf
      Tabellen-Pane → Close wird verworfen (Tree würde sonst leer);
      User muss erst Child manuell schließen.
- [ ] Anderer ChildDef ohne `coupled: true` (oder ohne `split:`) →
      Verhalten unverändert; klassischer Split-Drill bzw. In-Place-
      Drilldown.

## Action-Chains (Phase 2)

Voraussetzung: in einer Content-Tab-YAML (z. B. `postgres.yaml`) ist auf
ChildDef-Ebene ein Chain definiert, etwa:

```yaml
- name: Rows
  key: o
  node_type: "postgres:row"
  split: { direction: right, ratio: 0.8, coupled: true }
  action_chains:
    "ctrl+n":
      [window.focus_parent, common.list_next, content.open, window.focus_child]
    "ctrl+p":
      [window.focus_parent, common.list_prev, content.open, window.focus_child]
```

- [ ] `ctrl+n` im gekoppelten Rows-Pane → Fokus springt auf Eltern-
      Pane (Tabelle/Schema), nächste Zeile selektiert, `content.open`
      hot-replaced den Rows-Pane, anschließend springt der Fokus per
      `window.focus_child` zurück in den Rows-Pane mit den Daten der
      nächsten Zeile. Reihenfolge wirkt atomar (kein Flicker).
- [ ] `ctrl+p` analog rückwärts.
- [ ] Globale Chain in `tui.yaml` unter `key_bindings.action_chains:`
      eintragen, z. B. `"ctrl+]": [common.list_next]`. In jedem Tab
      drückt `ctrl+]` einen Schritt nach unten — auch in Tasks/
      Trackings, weil die Chain im global-Scope landet.
- [ ] ChildDef-Chain überschreibt globale Chain: gleiche Taste in
      ChildDef + global definieren → ChildDef-Chain läuft, globale
      bleibt stumm.
- [ ] Chain auf einer Ebene deaktivieren: `"ctrl+n": ~` in einer
      ViewDef → in dieser Subtab tut `ctrl+n` nichts (auch wenn global
      eine Chain definiert ist; kein Fall-through).
- [ ] Abbruch bei Fehler: Chain enthält `[window.focus_parent,
content.open]` und im Fokus ist kein Eltern-Pane verlinkbar →
      Notification mit `chain ctrl+x: step N aborted: …`, Folgesteps
      werden nicht ausgeführt.
- [ ] Validation am Config-Load: `[global.quit]` als Chain-Eintrag →
      App-Start bricht mit `not chainable in V1`-Fehler ab.
- [ ] Validation am Config-Load: `[content.warp]` als Chain-Eintrag →
      App-Start bricht mit `unknown content action`-Fehler ab.
- [ ] Validation am Config-Load: `[list_next]` (ohne `common.`-Präfix)
      → App-Start bricht mit `missing <section>.`-Fehler ab.
- [ ] Chain-Bindings greifen NICHT, solange ein Popup oder Mode-Input
      aktiv ist (Cmdline `:`, Search `/`, Fuzzy `f`, Saved-Query-Menu,
      Adapter-Creds-Popup). Eine Chain-Taste zwischendurch tippen →
      Popup schluckt sie wie gewohnt.

## Column-Cursor

Per-View / per-ChildDef Opt-in (`column_cursor: true`). Liefert eine
Spalten-Selektion zusätzlich zur Zeilen-Selektion: Zeile via
`RowSelected`-Style, Spalte via `ColumnSelected`, Kreuzungs-Zelle via
`CellSelected`. Navigation: `ColumnLeft`/`ColumnRight` (Default
`left`/`h`, `right`/`l`). Aktuell aktiviert für Postgres `Rows` (und
nur dort).

Voraussetzung: `not-yet-done-tui` aus aktuellem `master` installiert
und `~/.config/not_yet_done/views/postgres.yaml` enthält
`column_cursor: true` auf der `Rows`-Child.

- [ ] Postgres-Tab → Database → Schema → Table → `o` (Rows). Im
      Rows-Pane: Zeilen-Highlight wie gewohnt; zusätzlich erste Spalte
      hervorgehoben; Zelle (Zeile 0, Spalte 0) im Schnitt nochmal anders.
- [ ] `l` und `right` schieben den Spalten-Cursor nach rechts; clamped
      am letzten Cell der Zeile (kein Wrap).
- [ ] `h` und `left` schieben nach links; clamped bei 0.
- [ ] Coupled-Chain-Wechsel (`ctrl+n` / `ctrl+p`): Nach `content.open
  - window.focus_child` ist der Spalten-Cursor wieder bei 0
    (frischer Drill in einen column_cursor-Child startet auf 0).
- [ ] Drill heraus (z.B. `backspace` aus Rows in Tables): Spalten-
      Highlight verschwindet (Tables hat `column_cursor: false`).
- [ ] Drill wieder rein in Rows: Spalten-Cursor steht auf 0 (frischer
      Drill, NavFrame-Wiederherstellung greift nur, wenn beide Ebenen
      Cursor an haben).
- [ ] Page-Wechsel via `>` / `<` im Rows-Pane: Spalten-Position
      bleibt erhalten (z.B. Cursor auf Spalte 3, Page 2 zeigt immer
      noch Cursor auf Spalte 3, sofern die Zeile mindestens so viele
      Spalten hat — sonst geclampt).
- [ ] Andere Views (Tasks, Trackings, Jira, Taiga): Kein Spalten-
      Highlight. `h`/`l` machen nichts (oder verhalten sich gemäß
      etwaiger anderer User-Mappings, aber jedenfalls keine Spalten-
      bewegung).
- [ ] User-Override im `tui.yaml` unter `key_bindings.common`:
      `column_left: ["a"]` → `a` bewegt den Spalten-Cursor; `left` /
      `h` werden überschrieben.

## Configurable Row-Nav (Bug-Fix)

ContentView routet `j`/`k`/`g`/`G` nicht mehr hardcoded; alle vier
Tasten gehen über `CommonAction::ListNext/Prev/First/Last`.

- [ ] In `tui.yaml` `key_bindings.common.list_next: ["x"]` setzen → in
      Postgres-Tab und in Tasks bewegt jetzt `x` nach unten; `j` und
      `down` reagieren nicht mehr (außer per zusätzlichem Default-Eintrag).
- [ ] Default ohne User-Override: `j`/`k`/`down`/`up` funktionieren in
      allen Tabs (Tasks, Trackings, Postgres, Jira, Taiga).
- [ ] `h` öffnet keine `back`-Aktion mehr und `l` keinen `open`-Drill
      (es sei denn, der User bringt sie via `key_bindings.content.back`
      oder `.open` zurück).

## Horizontal Scroll (Column-Cursor coupled)

Wenn der Spalten-Cursor auf eine Spalte wandert, deren rechte Kante
außerhalb der Pane-Breite liegt, scrollt die Tabelle horizontal mit
(Snap an Spaltengrenze). Indikatoren `‹` / `›` in der Header-Zeile
melden verborgene Spalten links/rechts. Aktiv nur bei
`column_cursor: true` (Variante A, gekoppelt am bestehenden Flag).

Voraussetzung: Postgres-Tab mit einer Tabelle, deren Zeilenbreite die
Pane-Breite übersteigt — geeignet sind Tabellen mit vielen oder
breiten Spalten. Der Rows-Pane des Default-`postgres.yaml`-Splits ist
80 % breit; sehr breite Tabellen reichen aus, ggf. das Terminal
schmaler ziehen.

- [ ] Postgres → Database → Schema → Table → `o` (Rows): Cursor steht
      links bei Spalte 0; kein `‹`-Indikator; `›` erscheint, wenn rechte
      Spalten verborgen sind.
- [ ] `l` mehrfach drücken: sobald die Cursor-Spalte rechts aus dem
      sichtbaren Bereich rausläuft, scrollt die Ansicht (Header und
      Zeilen synchron) um eine Spalte mit. Cursor-Highlight bleibt
      vollständig sichtbar.
- [ ] Linker Indikator `‹` taucht auf, sobald `scroll_col_offset > 0`,
      verschwindet wieder beim Zurückscrollen auf 0.
- [ ] `h` zurück: Ansicht scrollt zurück (Snap an Spaltengrenze, keine
      Halbspalten). Bei Cursor auf 0 ist `‹` weg.
- [ ] Coupled-Chain (`ctrl+n` / `ctrl+p`): nach Drill auf neue Row und
      `window.focus_child` startet der Cursor auf Spalte 0 → Scroll
      ist auf 0 zurückgesetzt.
- [ ] Drill heraus (Tables ohne `column_cursor`): kein Indikator, keine
      Scroll-Spuren — Tabelle wird wie zuvor abgeschnitten gerendert.
- [ ] Drill wieder rein (Rows): Cursor und Scroll fangen frisch bei 0
      an.
- [ ] Page-Wechsel `>` / `<` mit Cursor weit rechts: Cursor-Position
      bleibt; Scroll-Offset wird in `set_rows` zurückgesetzt und
      `view()` re-snapt sofort, sodass die Cursor-Spalte wieder
      sichtbar ist.
- [ ] Terminal sehr schmal ziehen (1–2 Spalten passen): Cursor + Scroll
      bleiben kohärent, kein Crash, kein leerer Render.
- [ ] Terminal sehr breit ziehen (alle Spalten passen): kein Indikator,
      Scroll-Offset bleibt 0 oder schnappt zurück auf 0.
- [ ] Views ohne `column_cursor` (Tasks, Trackings, Jira, Taiga,
      Datenbanken-Liste): keine `‹`/`›`-Indikatoren, kein
      Horizontal-Scroll, identisches Render-Verhalten wie vor dem
      Feature.

## Auto Column Sizing (Postgres Rows)

Für Tabellen mit dynamischem Schema (`current_columns`-Auto-Fallback —
v.a. Postgres-Rows) baut der Adapter `ColumnDef { sizing: "auto" }`. Im
Sizer wird `width = clamp(max(header_w, content_max), min, max)` mit
Defaults `min=5`, `max=11`. Auto-Spalten respektieren das pane-Budget
nicht — H-Scroll fängt Overflow.

- [ ] Postgres → Datenbank → Schema → Tabelle drillen, in Tabelle mit
      vielen Spalten landen. **Vorher** Symptom: `2…  …  …  …  …`.
      **Nachher**: jede Spalte zeigt entweder Header (wenn ≤ 11 chars)
      oder Inhalt bis 11 chars; kein `…`-Spam mehr.
- [ ] Spalte mit Header länger als 11 (z.B. `transaction_timestamp`):
      Header wird auf 11 chars gekappt (`transactio…` o.ä. via
      fit_aligned), Inhalt entsprechend.
- [ ] Spalte mit kurzem Header und sehr kurzem Inhalt (`id` mit Werten
      `1`, `2`): Spalte ist mindestens 5 Zeichen breit (Min-Floor).
- [ ] H-Scroll greift, wenn Spalten in Summe nicht in die Pane passen:
      `‹`/`›`-Indikatoren erscheinen, `l`/`h` (oder konfigurierte
      Column-Cursor-Keys) navigieren über Spaltengrenzen.
- [ ] Per-Column-Override im YAML: in einer `ChildDef.columns`-Liste
      eine Spalte mit `sizing: "auto(3, 30)"` setzen. Spalte respektiert
      die neuen Bounds (min 3, max 30).
- [ ] Tabellen mit explizit konfigurierten Spalten (Database / Schema /
      Table-Listen mit `sizing: "max"`): unverändert, kein Auto-Verhalten.
- [ ] Tabellen ohne `column_cursor` (Tasks, Jira) bleiben unverändert —
      Auto-Fallback greift dort nicht (alle haben `columns:`-Listen).

## Postgres Query Editor (`Q`)

`Q` auf einer Postgres-Tabelle (Drill-Down `Database → Schema →
Tables → <table>`, im Rows-Level) öffnet einen externen `$EDITOR`
mit einer `.sql`-Datei. Layout:

```
-- Scratch area: notes, helper SELECTs. Lines above the marker
-- below are ignored on every :w.

-- ▼ THIS SQL WILL BE EXECUTED ON SAVE ▼

SELECT * FROM "<schema>"."<table>";
```

Auf jedem `:w` wird der Bereich **unter** dem Marker gegen den
Adapter ausgeführt. Erfolgreiche Resultsets ersetzen die Items im
aktiven Pane (dynamische Spalten greifen das Auto-Sizing). Fehler
landen in der bekannten Query-Error-Bar — die Datei wird **nicht**
verändert. Auf `:wq` mit zuletzt-gefehlertem Run wird die Datei mit
einem Comment-Banner-Block reopen't (`-- ─── ERRORS ───`).
Erfolgreich ausgeführte Buffer werden unter
`<XDG_DATA_HOME>/not_yet_done/postgres/<instance_id>/queries/<schema>/<table>.sql`
persistiert (Crash-resistent, überlebt Restart).

- [ ] Postgres-Tab → Database → Schema → Tables → eine Tabelle.
      Action-Bar zeigt `Q edit query` (oder vergleichbare Hint).
- [ ] `Q` öffnet `$EDITOR`. Default-Buffer enthält Marker-Zeile +
      `SELECT * FROM "<schema>"."<table>";`. Vorgegebene Quotes
      sind `"…"`, schema/table identisch zur Drill-Down-Quelle.
- [ ] `:w` ohne Änderungen → Items werden mit dem Default-`SELECT *`
      neu geladen. Status-Bar zeigt `N row(s)` o.ä.
- [ ] Query in `WHERE id = …` ändern und `:w` → Pane filtert auf
      die getroffenen Zeilen, Spalten passen sich an.
- [ ] Syntaxfehler einbauen (`SELEC * …`) und `:w` → Query-Error-Bar
      zeigt Postgres-Fehler. Items im Pane bleiben unverändert.
      Datei (im Editor) zeigt **noch keinen** Banner.
- [ ] Mit dem fehlerhaften Query `:wq` → Editor öffnet sich neu mit
      vorangestelltem `-- ─── ERRORS ───`-Block. Banner verschwindet
      bei nächstem `:w` mit korrigierter Query (kein Stacking).
- [ ] Multi-Statement-Query, z.B. `BEGIN; UPDATE … ; ROLLBACK;`. Auf
      `:w` zeigt Status-Bar das Resultat des **letzten** Statements
      (`ROLLBACK` → `0 row(s) affected`). Bei finalem `SELECT * FROM …`
      werden dessen Rows gerendert.
- [ ] `UPDATE …` ohne `RETURNING` als einziges Statement → Pane
      bleibt unverändert (keine Items), Status-Bar zeigt
      `<n> row(s) affected`. Nach erneutem `:w` mit `SELECT` werden
      die Items aktualisiert sichtbar.
- [ ] Marker-Zeile aus dem Buffer entfernen + `:w` → der **gesamte**
      Buffer (inkl. des "Scratch"-Kommentars) wird ausgeführt.
      Postgres ignoriert SQL-Kommentare, also läuft effektiv der
      `SELECT`. Marker zurückschreiben → Scratch-Schutz wirkt wieder.
- [ ] Editor schließen (`:wq` mit erfolgreichem letzten Run) → Done,
      keine Reopen-Schleife. TUI kehrt zur Ergebnisliste zurück.
- [ ] `Q` erneut auf derselben Tabelle: persistierter Buffer (inkl.
      letzter `WHERE`-Klausel) erscheint, **nicht** der Default.
      Datei-Pfad
      `<XDG_DATA_HOME>/not_yet_done/postgres/<instance_id>/queries/<schema>/<table>.sql`
      existiert.
- [ ] `Q` auf einer **anderen** Tabelle: eigener Buffer, eigene
      Datei. Tabellen-Editoren beeinflussen sich nicht.
- [ ] Auf Root-Level (Datenbank-Liste, kein Drill-Down): `Q`
      triggert **nicht** den SQL-Editor (existierende JQL/JSON-
      Editor-Pfad bleibt unverändert).
- [ ] Andere Adapter (Jira / Taiga): `Q` öffnet **keinen** SQL-Editor
      (Notify "Adapter does not support custom queries" oder die
      bisherige Logik je nach Level).

### Multi-Instance (zwei Postgres-Tabs)

Voraussetzung: zwei View-Configs in `~/.config/not_yet_done/views/`
mit demselben `adapter.type: postgres`, aber unterschiedlichen
`adapter.id:` (z.B. `id: prod` und `id: staging`), die jeweils auf
die gleiche Datenbank zeigen können.

- [ ] App-Start: beide Tabs laden, kein Duplicate-ID-Fehler.
- [ ] In Tab `prod` Tabelle `public.users` → `Q` → eigene Query
      eintippen + `:w`.
- [ ] In Tab `staging` dieselbe Tabelle `public.users` → `Q` → eine
      andere Query eintippen + `:w`. Beide Pane-Inhalte zeigen den
      jeweils eigenen Filter.
- [ ] Auf Disk: zwei separate Pfade existieren —
      `…/postgres/prod/queries/public/users.sql` und
      `…/postgres/staging/queries/public/users.sql`.
- [ ] App neu starten, `Q` in beiden Tabs → jeweils der zuletzt für
      diesen Tab gespeicherte Buffer wird geladen, kein Cross-Talk.

## Tree-Mode (Phasen 0–7)

Voraussetzung: `~/.config/not_yet_done/views/postgres.yaml` hat
`tree_label: name` auf der `databases`-View **und** auf den ChildDefs
`Schema` + `Table`. `Rows` bleibt ohne `tree_label` (Leaf → Split).
Eine erreichbare Postgres-Instanz ist konfiguriert.

### Render + Expand/Collapse (Phasen 1–3)

- [ ] App-Start, Postgres-Tab → `databases`-Subtab. Cursor auf Zeile 0,
      Glyph `▶` vor dem DB-Namen, alle anderen Spalten (`Owner`,
      `Encoding`) gefüllt — Cursor-Ebene = root.
- [ ] `Enter` (oder `l`) auf einer DB → Glyph wechselt zu `▼`, darunter
      eingerückte Schema-Zeilen mit eigenem `▶`. Header-Spalten
      switchen **nicht** (Cursor steht noch auf root).
- [ ] Cursor mit `j` auf eine Schema-Zeile → Header wechselt zu
      Schema-Spalten (`Name`, `Owner`), die Schema-Zeile zeigt jetzt
      Inhalt in beiden Spalten. Die DB-Zeile oberhalb hat nur noch
      Inhalt in der `Name`-Spalte (mit `▼`), andere Spalten leer.
- [ ] `Enter` auf einer Schema-Zeile → Tabellen erscheinen eingerückt,
      Cursor bleibt auf Schema, kann mit `j` runter auf Table.
- [ ] Cursor auf Table → Header wechselt zu Table-Spalten
      (`Name`/`Owner`/`Rows (est.)`).
- [ ] `Enter` (oder `l`) auf einer Table-Zeile → Split-Pane rechts mit
      `Rows`-Inhalt (Tree-Pane bleibt links erhalten). Tree-Pane behält
      seinen Cursor / sein Expanded-Set.
- [ ] Zweiten Subtree öffnen: andere DB expanden während die erste noch
      offen ist — beide Subtrees gleichzeitig sichtbar.
- [ ] `Enter` auf einer expanded Zeile (mit `▼`) → kollabiert,
      Children verschwinden, Glyph zurück zu `▶`.

### Back-Key (Phase 4)

- [ ] Cursor auf eine Schema-Zeile unter expanded DB → `gh` (oder
      konfigurierter Back-Key) → Cursor springt zurück zur DB-Zeile
      **und** die DB collapsed (`▼` → `▶`, Children weg).
- [ ] Cursor auf DB-Zeile (Depth 0) → `gh` → no-op (kein
      Window-Close, keine Pane-Schließung).

### Smart-Collapse `c`

- [ ] Cursor auf expanded DB-Zeile (mit `▼`) → `c` → DB kollabiert
      (`▼` → `▶`, Children weg), **Cursor bleibt auf derselben Zeile**.
- [ ] Cursor auf eine Schema-Zeile (Depth 1), die selbst **nicht**
      aufgeklappt ist → `c` → Eltern-DB kollabiert und Cursor springt
      hoch auf die DB-Zeile (gleiches Verhalten wie `gh`/Back).
- [ ] Cursor auf expanded Schema-Zeile (Depth 1, mit `▼`) → `c` →
      Schema kollabiert, Cursor bleibt auf der Schema-Zeile.
- [ ] Cursor auf eine kollabierte Top-Level-DB (Depth 0, `▶`) → `c`
      → no-op (kein Window-Close, kein Beep).
- [ ] `c` außerhalb von Tree-Modus (Tasks-/Trackings-Tab) → öffnet
      weiterhin das ColumnConfig-Popup (unverändert). In nicht-Tree
      ContentView-Panes (z. B. `tables`-Subtab oder Rows-Split-Pane)
      ist `c` ohne Effekt — keine TreeCollapse-Aktion mehr aktiv.

### Pagination innerhalb Tree (Phase 5)

Voraussetzung: eine DB mit mehr Schemas als die konfigurierte
`page_size` (oder Schema mit mehr Tables als `page_size`).

- [ ] DB mit vielen Schemas expanden → unter den geladenen Schemas
      erscheint als letzte Zeile der Placeholder `… N weitere`
      (Glyph `…`).
- [ ] Cursor auf Placeholder → `Enter` lädt nächste Page und appendet
      sie **oberhalb** des Placeholders. Sind weitere Pages verfügbar,
      bleibt der Placeholder als letzte Zeile sichtbar; sonst
      verschwindet er.
- [ ] Während Pagination-Load nicht doppelt klicken — Placeholder
      darf nicht zweimal ausgelöst werden (Cursor wandert vorher zur
      ersten neu geladenen Zeile).

### Filter / Search im Tree (Phase 6)

Voraussetzung: `databases`-View hat `actions.fuzzy_filter` (key `f`)
und `actions.search` (key `/`) — beide nur an EINEM Tree-Level
definiert (per Validator-Constraint).

- [ ] `f` auf root-Ebene → Eingabe `pub` (oder anderes Snippet, das
      mind. eine DB matched) → nur matchende DBs sichtbar; bereits
      expanded Subtrees ihrer matchenden DBs bleiben offen. DBs die
      nicht matchen verschwinden samt ihren Children.
- [ ] Filter aktiv, jetzt eine DB neu expanden (Children noch nicht im
      Cache) → async Load läuft, neue Schemas erscheinen. Filter ist
      auf der **root**-Ebene definiert, also greift er für Schemas
      nicht (Schemas zeigen alle).
- [ ] (Optional, falls `fuzzy_filter` zum Test temporär auf
      `Schema`-ChildDef verschoben) Filter aktiv mit Snippet das nur
      ein Schema matched → Expand einer neuen DB → neu geladene
      Schemas werden gegen den aktiven Filter geprüft, nicht-matchende
      bleiben versteckt. (Unit-Test:
      `tree_apply_children_respects_active_filter_at_load_time`.)
- [ ] Filter leeren (`Esc` / Backspace bis leer) → alle Zeilen zurück.
- [ ] `/` Search → Eingabe → Cursor springt auf nächste Treffer-Zeile.
      Search überspringt Pagination-Placeholder; n/N wechselt durch
      Treffer.

### Keymap pro Cursor-Ebene (Phase 7)

- [ ] Action-Bar zeigt auf root-Ebene Aktionen der `databases`-View
      (mind. `f` filter, `/` search).
- [ ] Cursor `j` runter auf Schema → Action-Bar wechselt auf Schema-Level
      Aktionen (sofern auf Schema-ChildDef welche definiert sind).
      Globale Aktionen (`fuzzy_filter`, `search`, `text_search`) der
      root-View bleiben verfügbar.
- [ ] Cursor weiter runter auf Table → Action-Bar wechselt erneut.

### Refresh + Action-Chains

- [ ] `r` auf Tree-Pane (root-Cursor) → Root-Liste reloaded, Expanded-Set
      bleibt erhalten (wo möglich).
- [ ] Im Split-Modus (Tree links, Rows rechts): `ctrl+n` / `ctrl+p`
      (`window.focus_parent, common.list_next, content.open,
window.focus_child`) navigiert von Rows-Pane zur nächsten Row in
      der Tabelle oben. Tree-Pane bleibt unverändert.

## Multi-Tree-Continuation + DB-Level Scripts (MT-1 … MT-4)

Voraussetzung: `~/.config/not_yet_done/views/postgres.yaml` hat unter
der `databases`-View **zwei** tree-continuing Children:

- `Schema` (`node_type: "postgres:schema"`, `tree_label: name`)
- `DB Script` (`node_type: "postgres:db_script"`, `tree_label: script`)

Beide auf gleicher Ebene, unterschiedliche `node_type`s → Validator
akzeptiert (Rule 3 der MT-1a).

### Validator-Check (MT-1a)

- [ ] App-Start mit obiger Config → kein Validator-Fehler, Tab lädt
      normal.
- [ ] Zwei tree-Children mit **identischem** `node_type` in eine View
      kopieren (z. B. zweimal `node_type: "postgres:schema"`) → App
      lehnt Reload ab mit Fehler _"ambiguous tree continuation —
      duplicate node_type 'postgres:schema' used by both
      tree-continuing children …"_.

### Multi-Branch Expand (MT-1b/c/d + MT-2)

Vorbereitung: lege per Hand mindestens ein DB-Script an, damit der
DB-Scripts-Branch beim Expand etwas Sichtbares hat:

```sh
mkdir -p ~/.local/share/not_yet_done/postgres/<instance_id>/db_scripts/<db>
printf '%s\n%s\n%s\n' '-- scratch' '-- ▼ THIS SQL WILL BE EXECUTED ON SAVE ▼' 'SELECT 1;' \
  > ~/.local/share/not_yet_done/postgres/<instance_id>/db_scripts/<db>/hello.sql
```

- [ ] App-Start, Postgres-Tab → `databases`-Subtab → DB expanden
      (`Enter`/`l`). **Zwei** Branches erscheinen unter der DB,
      Reihenfolge wie in YAML: zuerst Schemas, dann das DB-Script
      `hello`. Beide jeweils mit eigenem `▶`-Glyph.
- [ ] Während des Loads (kurz) → Banner zeigt _"loading"_ bis beide
      Branches geladen sind. Erscheinen erst beide gleichzeitig, nicht
      nur einer.
- [ ] Branches in der Reihenfolge der YAML-Children (Schema vor DB
      Script), unabhängig davon, welcher Adapter-Call früher fertig
      war.
- [ ] Schema-Zeile weiter expanden → klassischer Schema → Table-Pfad
      funktioniert weiterhin.
- [ ] DB-Script-Zeile ist Leaf → kein weiteres `▶`, `Enter` macht
      nichts (kein Drilldown definiert).
- [ ] DB collapsen (`c` oder `gh`) → **beide** Branches verschwinden.
- [ ] DB erneut expanden → beide Branches kommen wieder, gleicher
      Inhalt.

### Mixed Scripts-Subtab (MT-3)

- [ ] `s` (scripts-Subtab) → Liste enthält sowohl Table-Level-Scripts
      (z. B. `default` aus `queries/<db>/<schema>/<table>/`) als auch
      DB-Level-Scripts (z. B. `hello` aus `db_scripts/<db>/`).
- [ ] DB-Level-Zeile hat die `Schema`/`Table`-Spalten **leer**,
      `Database` und `Script` sind gefüllt.
- [ ] Table-Level-Zeile hat alle vier Spalten gefüllt.

### Storage-Layout (MT-2)

- [ ] Auf Disk: DB-Scripts liegen unter
      `<instance_data_dir>/db_scripts/<db>/<script>.sql`, separat
      vom existierenden `queries/<db>/<schema>/<table>/<script>.sql`.
- [ ] Nicht-`.sql` Dateien im DB-Scripts-Verzeichnis (`notes.txt`
      o. ä.) werden ignoriert, keine Crash.
- [ ] Fehlendes `db_scripts/`-Verzeichnis ⇒ leerer DB-Scripts-Branch
      (kein Fehler).

## Cursor pagination & per-node actions (CP-1 … CP-9)

Voraussetzung: ein konfigurierter Postgres-Tab mit der DB-Scripts-
Branch wie in [Multi-Tree-Continuation](#multi-tree-continuation--db-level-scripts-mt-1--mt-4).
Im `postgres.yaml` sind die per-node Shortcuts gesetzt:

```yaml
- name: Scripts
  node_type: "postgres:db_scripts"
  shortcuts:
    a: add
  children:
    - name: DB Script
      node_type: "postgres:db_script"
      shortcuts:
        x: execute
        e: edit
        d: delete
      children:
        - name: DB Script Result
          node_type: "postgres:db_script_result"
          split: { direction: right, ratio: 0.8, coupled: true }
          pagination: { mode: cursor, page_size: 100 }
          column_cursor: true
          keybindings: { back: null }
```

### Add + edit (CP-9)

- [ ] Cursor auf den **Scripts**-Gruppen-Node (oder direkt auf einen
      bestehenden DB-Script-Eintrag) setzen, `a` drücken → Cmdline
      öffnet sich pre-filled mit `:db-script-new <database> ` (mit
      Trailing-Space). User tippt nur den Namen, drückt Enter →
      Notification "Created DB script '<name>'", Editor öffnet sich
      automatisch auf die neue Datei.
- [ ] Datei landet unter
      `<instance_data_dir>/db_scripts/<database>/<name>.sql` mit dem
      Default-Template (Scratch-Hinweis, `▼ THIS SQL WILL BE EXECUTED
ON SAVE ▼` Marker, `SELECT 1;` Body).
- [ ] `:db-script-new` mit ungültigem Namen (mit `/`, `\`,
      Whitespace, führendem `.`, leer) → Modal-Fehler, kein File
      angelegt.
- [ ] `:db-script-new <db> <vorhandener-name>` → Modal-Fehler
      "already exists"; File auf Disk unverändert.

### Execute + cursor result pane (CP-8 + CP-4 … CP-6)

- [ ] Auf einem DB-Script `x` drücken → rechts daneben öffnet sich
      ein Result-Pane (80/20 split). Spalten ergeben sich dynamisch
      aus dem SELECT.
- [ ] Body mit `INSERT … SELECT * FROM generate_series(1, 500)` o.ä.
      bauen, dann SELECT, `x` → erste 100 Zeilen erscheinen. `>` →
      nächste 100. `<` → Cursor re-opened auf Start (NO SCROLL).
- [ ] Multi-Statement-Body: `CREATE TEMP TABLE t(x int); INSERT INTO
t VALUES (1),(2),(3); SELECT * FROM t;` → läuft, paginiert über
      den finalen SELECT.
- [ ] DDL-only Body (`VACUUM`, `ANALYZE`) → derzeit Notify
      "unpaged ExecuteQuery not implemented yet" (CP-9 nicht
      ausgeliefert).
- [ ] Result-Pane schließen (`wq`/`Esc` auf Pane) → `pg_stat_activity`
      zeigt **keinen** verbleibenden idle-in-tx Eintrag mit dem
      Cursor-Statement.
- [ ] Während ein Cursor-Pane offen ist eine andere Long-Running-
      Query starten → Timeout (`query_timeout_secs`) räumt den
      gesamten Pool ab; Cursor-Pane zeigt beim nächsten `>` einen
      "cursor lost"-Banner (kein Crash).
- [ ] Auf einem DB-Script `Enter` drücken (statt `x`) → identisches
      Verhalten wie `x`: Result-Pane öffnet, Pagination greift.
      Funktioniert sowohl im Flat-Mode (Scripts-Subtab) als auch im
      Tree-Mode (databases → Database → DB Scripts → Script-Row).
      Regression-Bait: vor dem Fix wurde der synthetische Child
      `postgres:db_script_result` durch den generischen Drill-Pfad
      angesteuert und endete mit `Fetch failed: Node type
'postgres:db_script_result' not available on …`. Routing
      jetzt via `enter_action: execute` auf der `DB Script`
      ChildDef in `postgres.yaml`. Zusätzlich nutzt `current_children`
      im Tree-Mode jetzt den `node_type_chain` der selektierten
      Row (statt First-Chain-Walk), sonst würde der Split in den
      falschen Branch (z. B. Schemas → Schema → Table) abbiegen.

### Edit (CP-8)

- [ ] Auf DB-Script `e` drücken → SQL-Editor öffnet sich mit dem
      gespeicherten Body. `:w` persistiert **ohne** Re-Execute; ein
      eventuell offenes Result-Pane ändert sich nicht. User muss
      explizit `x` drücken, um die neue Version zu sehen.
- [ ] Im **Tree-Mode** (über `databases` → Database expandiert →
      `DB Scripts`-Branch expandiert → Cursor auf einzelner
      Script-Row) ebenfalls `e` drücken → derselbe Editor öffnet
      sich. Während der kurzen Adapter-Vorabfrage zeigt die
      Status-Bar evtl. den `list databases (Ns/Ms)`-Busy-Banner
      (das ist erwartet — `get_by_id` validiert den DB-Namen);
      der Editor öffnet sich **danach**, nicht stattdessen.
      Regression-Bait: vor dem Fix wurde der vom Async-Dispatch
      zurückgegebene `EditorRequest` in `poll_load` verschluckt.

### Delete (CP-9)

- [ ] Auf einem DB-Script `d` drücken → Confirm-Popup "Delete DB
      script '<name>' in database '<db>'? (y/n)". `y` → Notify
      "Deleted DB script '<name>'", Row verschwindet (Pane reloaded),
      Datei ist von Disk entfernt.
- [ ] Wiederholtes `d` auf eine bereits gelöschte/vermisste Datei
      ist idempotent — kein Fehler, Notify trotzdem.
- [ ] `n`/`Esc` im Confirm-Popup → Datei bleibt, Row bleibt sichtbar.

### DB-Script Folders (DSF)

Voraussetzungen: Postgres-Tab, Tree-Mode (`d` auf Tab), Database
expandiert → `Scripts`-Branch sichtbar. `postgres.yaml` enthält den
DSF-Cutover (DB Script Dir + recursive: true). User-Config:
`~/.config/not_yet_done/views/postgres.yaml`.

#### DSF-1/2 Adapter

- [ ] Auf `Scripts` `a` → Cmdline pre-filled `db-script new ` —
      Skript-Name tippen + Enter → Notify "Created DB script
      '<name>'", neue Row erscheint unter Scripts.
- [ ] Auf `Scripts` `A` → Cmdline `db-script new-dir ` — Ordnername
      tippen + Enter → Notify "Created DB-script folder '<name>'",
      neue Folder-Row mit `▶`-Glyph erscheint unter Scripts.
- [ ] Folder-Row mit `Enter`/`l` expandieren → leer (keine Children).
      `a` → Cmdline `db-script new ` — Skript erstellen unter dem
      Folder. Filesystem-Check: `<instance_data_dir>/db_scripts/<db>/<folder>/<script>.sql`.

#### DSF-3 Recursive ChildDef

- [ ] Folder-Row → `A` → neuen Sub-Folder erstellen. Sub-Folder ist
      seinerseits expandierbar (`▶`). `A` darin → noch eine Ebene.
      Tiefen ≥ 3 funktionieren ohne YAML-Änderung — `recursive: true`
      auf der `DB Script Dir`-ChildDef macht sie zu ihrem eigenen
      Tree-fortsetzenden Kind.

#### DSF-4 Mark/Paste-Move

- [ ] Auf einem Skript `m` → Status-Bar Pill "⚓ marked: move:
      <db>/db_scripts/.../script" + Notify "Marked '...' for move".
- [ ] Cursor auf eine Folder-Row → `p` → Notify "Moved '<src>' →
      '<dst>' in <db>"; die Source-Row verschwindet aus ihrem alten
      Parent, die Folder-Row enthält jetzt das verschobene Skript.
      Pill verschwindet.
- [ ] Esc nach `m` → Notify "DB-script move cancelled", Pill weg.
- [ ] Folder mit `m` markieren, auf anderen Folder `p` → ganzer
      Folder-Subtree wird verschoben (rekursiv, mit Inhalt).
- [ ] Cross-DB-Paste-Versuch (Skript aus DB1 marken, in DB2 paste-en)
      → Notify-Error "Cross-database move not supported (DB1 → DB2)".
      Mark bleibt erhalten, damit der User einen passenden Target
      wählen kann.

#### DSF-4 Delete-Dir

- [ ] Auf einer leeren Folder-Row `d` → Confirm-Popup "Delete empty
      DB-script folder '<rel_path>' in '<db>'? (y/n)". `y` → Notify
      "Deleted DB-script folder '<rel_path>'", Row verschwindet.
- [ ] Auf einer **nicht-leeren** Folder-Row `d` → Confirm `y` →
      Notify-Error "Delete folder failed: not empty (N entries)".
      Folder + Inhalt bleiben unverändert.
- [ ] `n`/`Esc` im Confirm → Folder bleibt.

#### DSF-5 Cmdline-Namespace

- [ ] `:db-script` ohne Subcommand → Modal-Error "expects a
      subcommand (new | new-dir | rename | move | delete)".
- [ ] `:db-script unknown` → Modal-Error mit Liste der gültigen
      Subcommands.
- [ ] `:db-script new` ohne Name → Modal-Error "expects <name>".
- [ ] `:db-script rename foo/bar` → Modal-Error "invalid name
      'foo/bar' (no slashes or leading dot)".
- [ ] `:db-script move /` (absolut-root) auf markiertes Skript →
      verschiebt das Skript an die Wurzel des `db_scripts/<db>/`-
      Verzeichnisses.
- [ ] `:db-script move foo` (relativ) bei Cursor in Folder `bar` →
      Zieldir wird `bar/foo`.
- [ ] `:db-script delete` mit Cursor auf der `Scripts`-Group-Row
      → Modal-Error "selected row is the group node".

#### DSF-3 Validator

- [ ] In `postgres.yaml` `recursive: true` ohne `tree_label` setzen
      und Tab reloaden → Validator-Fehler "recursive: true requires
      tree_label" (granular reload, alter Stand bleibt aktiv bis
      gefixt).

### DB-Script Table-Name Completions (TC-1 … TC-5)

Bedingung: Postgres-Tab, mindestens eine DB mit ein paar Basis-Tabellen
(`pg_class.relkind = 'r'`, schemas außer `pg_catalog`/`information_schema`).

- [ ] Auf einem DB-Script `e` drücken → Editor öffnet sich. Am Ende des
      Buffers steht eine einzelne Zeile der Form
      `-- table completions: tt_public__users, tt_public__orders, …`.
      Reihenfolge: alphabetisch nach `(schema, table)`. Keine Tabelle
      aus `pg_catalog`/`information_schema`/`pg_…` ist gelistet.
- [ ] Token kopieren oder von Hand tippen: `SELECT * FROM tt_public__users;`
      über den `QUERY_MARKER` schreiben, `:w`, dann `x` auf der Row →
      Result-Pane zeigt die Zeilen der `public.users`. Substitution
      hat `tt_public__users` zu `"public"."users"` ersetzt.
- [ ] Tabelle mit einfachem Unterstrich im Namen (z. B. `user_orders`)
      verifiziert Boundary-Match: `tt_public__user_orders` wird zu
      `"public"."user_orders"`. `tt_public__user` (falls existent)
      würde **nicht** den `user_orders`-Token partial mitkonsumieren
      (Regex-`\b` an `_`-Boundaries).
- [ ] Unbekannter Token (`tt_xxx__yyy` mit nicht-existenter Tabelle)
      bleibt unverändert. Postgres meldet `syntax error at or near
"tt_xxx__yyy"` — der literale Token ist im Fehler-Banner
      sichtbar, sodass der User den Tippfehler sofort findet.
- [ ] `:w` ohne Änderung → Datei auf Disk enthält **keine**
      `-- table completions:` Zeile (cat `<instance_data_dir>/db_scripts/<db>/<script>.sql`
      verifizieren). Beim erneuten Open mit `e` ist der Completion-Block
      wieder am Ende — aus der aktuellen Tabellen-Liste neu generiert,
      nicht aus dem File gelesen.
- [ ] Completion-Zeile manuell im Editor verändern oder löschen, dann
      `:w` → das beeinflusst die Persistenz nicht (Strip arbeitet auf
      Prefix-Match `-- table completions: `). Nächstes Open zeigt die
      frisch berechnete Zeile.
- [ ] Adapter ohne Tabellen (leere DB) → Completion-Zeile wird gar
      nicht angehängt (kein orphaner Header). Editor öffnet sich wie
      gewohnt.
- [ ] Substitution feuert nur, wenn der Query-Body `tt_` enthält
      (Fast-Path) — die Schritte oben sollen keinen messbaren Mehr-
      Round-Trip gegen die DB triggern, wenn der User Tokens nicht
      benutzt. Regression-Bait: vor dem Feature gab es überhaupt
      keinen `tt_`-Pfad, der Adapter führte die Query 1:1 aus.

### Shortcut-Resolver (CP-1)

- [ ] In einem Rows-Pane (`postgres:row`) `Q` drücken → öffnet den
      Q-SQL-Editor des Eltern-Table-Nodes (via
      `shortcuts: { Q: "parent:edit_sql" }`).
- [ ] Tasten, die _kein_ Shortcut sind und _keine_ View-Aktion
      sind, gehen wie gehabt durch (Cursor-Movement, etc.).
- [ ] YAML mit leerer Action-ID (`shortcuts: { x: "" }`) oder mit
      Key-Kollision zur `actions:`-Liste → Validator-Fehler beim
      Reload, der Tab geht in den Broken-State.

## Cross-app Linking (L1–L11)

Voraussetzungen: mindestens ein konfigurierter Jira-View und ein
Taiga-View, ein paar Tasks in der Tasks-DB. Default-Keybinds für die
Aktionen (alle unter dem `gl`-Prefix): `glm` (mark), `glp` (paste),
`glo` (popup öffnen), `glb` / `glf` (jump back/forward), `:linkprune`
(cmdline).

### Mark + Paste (L5/L6)

- [ ] In Tasks-Tab Cursor auf eine Task → `glm` → Status-Bar zeigt
      links den Pill _"⚓ marked: tasks/<uuid>"_; Notification _"Link
      mark armed: tasks/<uuid>"_.
- [ ] Tab wechseln (z. B. nach Jira) → Pill bleibt sichtbar.
- [ ] In Jira auf ein Issue → `glp` → Notification _"Linked:
      jira/<inst>/<KEY> → tasks/<uuid>"_; DB-Check `select * from
link;` zeigt die Zeile.
- [ ] Nochmal `glp` auf eine andere Jira-Zeile → zweite Link-Row,
      Mark bleibt erhalten.
- [ ] `Esc` (außerhalb von Popups/Modals) → Pill verschwindet,
      Notification _"Link mark cleared"_.
- [ ] `glp` ohne Mark → _"No link mark armed (press M on a row first)"_;
      kein DB-Write. (Die Notification-Wording stammt aus L5 vor dem
      Rebind; Funktion stimmt.)
- [ ] `glm` + `glp` auf dieselbe Zeile → _"Cannot link a node to
      itself"_; kein DB-Write.
- [ ] Postgres-Tab + `glm` → _"Nothing to mark for linking"_ (Postgres
      hat keine stabilen IDs).

### `glo`-Popup (L7)

- [ ] Auf der zuvor gepasteten Jira-Zeile `glo` → Popup _"Links ·
      jira/…"_; eine Zeile _"← tasks/<uuid>"_ (incoming).
- [ ] `Enter` darauf → Tab springt nach Tasks, gepastete Task ist
      fokussiert.
- [ ] Wieder dort `glo` → Popup zeigt _"→ jira/<inst>/<KEY>"_
      (outgoing).
- [ ] Tippen filtert die Liste; `↑`/`↓` bewegt; `Esc` schließt; `d`
      löscht die Selektierte und refresht das Popup. Wenn nichts übrig
      bleibt → Popup schließt + Notification _"No more links for this
      node"_.
- [ ] `glo` auf einer Zeile ohne Links → Notification _"No links for
      this node"_.

### Stale-Link Confirm (L8)

Vorbereitung: einen kaputten Link manuell einfügen, z. B.

```sh
sqlite3 ~/.local/share/not_yet_done/not_yet_done.db <<SQL
INSERT INTO link (id, source_ref, target_ref, created_at)
VALUES (lower(hex(randomblob(4)))||'-1111-1111-1111-111111111111',
        'tasks/<live-task-uuid>',
        'jira/<live-inst>/MISSING-9999',
        datetime('now'));
SQL
```

- [ ] `glo` auf der Live-Task → Popup zeigt _"→
      jira/<inst>/MISSING-9999"_.
- [ ] `Enter` → Confirm-Modal _"Stale link …\n(no content tab … oder
      Stale: ticket not found … etc.)\nDelete from link table? (y/n)"_.
- [ ] `y` → Notification _"Stale link deleted"_, DB-Row weg,
      `🔗`-Marker verschwindet bei nächstem Rebuild.
- [ ] Mit zweitem Stale-Insert (z. B. `target_ref = 'nope/whatever'`):
      `glo` + `Enter` → Confirm-Modal, `n` (oder beliebige andere
      Taste) → Notification _"Cancelled"_; Zeile bleibt.
- [ ] Mit `target_ref = 'postgres/main/qrow:1'`: `glo` + `Enter` →
      _kein_ Confirm-Modal, sondern Notification _"Link open failed:
      …NotSupported…"_ (Postgres ist v1-NotSupported, kein Stale).

### Has-Links Column (L9)

- [ ] Nach erfolgreichem Mark+Paste oben: Tasks-Tab hat in der
      `🔗`-Spalte für die gelinkte Task einen Haken, andere Tasks nicht.
- [ ] Trackings-Tab: laufende/historische Trackings _dieser_ Task
      zeigen ebenfalls `🔗` (Fallback über `tasks/<uuid>`).
- [ ] Trackings-Tab Tree-Mode: Task-Knoten leuchtet, Eintrags-Knoten
      ebenfalls (gleicher Fallback-Lookup).
- [ ] Jira-View ohne `source: has_links` Spalte → nichts ändert sich.
- [ ] In `~/.config/not_yet_done/views/jira.yaml` einer Issue-Liste
      eine Spalte ergänzen:
      `yaml
  - key: links
    label: "🔗"
    source: has_links
    sizing: fixed(2)
    `  TUI neu starten → gelinktes Jira-Issue zeigt`🔗`, andere nicht.
- [ ] `glo` + `d` auf der gelinkten Row → `🔗`-Marker verschwindet
      beim nächsten Rebuild (Tab-Switch reicht).
- [ ] Postgres-Tab: `🔗`-Spalte leuchtet nie auf, egal ob Stale-Row in
      der DB steht (Postgres ist excluded by design).

### Jump-History `glb` / `glf` (L10)

- [ ] In Tasks-Tab `glo` → `Enter` auf einer outgoing Jira-Link →
      Tab wechselt nach Jira, Issue fokussiert, Notification fehlt
      (kein expliziter "jumped"-Toast — gewollt).
- [ ] `glb` → Tab wechselt zurück nach Tasks, Cursor steht auf der
      ursprünglichen Task, Notification _"← tasks/<uuid>"_.
- [ ] `glf` → Tab wechselt wieder zu Jira, Issue fokussiert,
      Notification _"→ jira/<inst>/<KEY>"_.
- [ ] `glb` aus dem "Anfangszustand" (keine Link-Jumps gemacht) →
      Notification _"No back-history"_; kein Tab-Wechsel.
- [ ] Mehrere Hops: `glo`/`Enter` von A→B, dann von B→C → `glb`
      zurück nach B, `glb` nochmal zurück nach A; `glf` zweimal
      bringt wieder C.
- [ ] Nach `glb` zu B: dort _neuen_ Link-Jump B→D ausführen →
      Forward-Branch (C) ist verworfen, `glf` von D bringt _kein_ C
      mehr (_"No forward-history"_).
- [ ] Tab-Wechsel über `1`/`2`/`3` schiebt _nichts_ in die History.
- [ ] Stale Jump (vorher gelinktes Issue serverseitig gelöscht oder
      Adapter-Instance umbenannt): `glb` darauf → Notification
      _"Back-jump failed: …"_, Eintrag wird aus dem Stack verworfen
      (nächstes `glb` greift den darunter liegenden Eintrag).

### `:linkprune` (L11)

Vorbereitung: 2–3 Live-Links plus mindestens einen offensichtlich
staleen (z. B. wie in L8, oder einen tasks-Link auf eine soft-gelöschte
Task).

- [ ] `:` → Cmdline öffnet sich, `linkprune` tippen, `Enter`.
- [ ] Modal erscheint: _"N of M link(s) are stale:\n tasks/… → jira/…
      (reason)\n …\nDelete all? (y/n)"_; Liste enthält max. 5
      Sample-Refs, danach _"… and X more"_.
- [ ] `y` → Modal schließt, Notification _"Pruned N stale link(s)"_;
      DB-Check zeigt nur noch die Live-Links; `🔗`-Spalte aktualisiert
      sich sofort (kein Tab-Switch nötig).
- [ ] `:linkprune` erneut → Modal _"Scanned M link(s). None are stale."_
      (mit M = aktuelle Live-Anzahl).
- [ ] Bei leerer link-Table: `:linkprune` → Modal _"No links in the
      database."_.
- [ ] `:linkprune extra-arg` → Modal _":linkprune takes no arguments"_,
      keine Aktion.
- [ ] Mit absichtlich kaputter DB-Connection (oder dem link*repo
      offline): `:linkprune` → Modal *"link scan failed: …"\_,
      kein Confirm-Modal, kein Delete.
- [ ] Soft-deleted Tasks/Trackings zählen als stale: Task löschen
      (lower-d) → `:linkprune` listet die Links darauf; nach `y`
      verschwinden sie.

## In-App Config-Editor (`:config`)

Cross-cutting Feature: YAML-Configs unter
`~/.config/not_yet_done/` im externen `$EDITOR` öffnen und nach Save
in-process neu laden — ohne TUI-Restart.

### 1. Fuzzy-Picker

- [ ] `:config` öffnet ein SearchablePopup mit allen `*.yaml` Dateien
      (rekursiv) unter `~/.config/not_yet_done/`. Labels sind
      relativ zur Config-Root (z. B. `tui.yaml`,
      `views/jira.yaml`, `views/jira-adapter.yaml`).
- [ ] Tippen filtert die Liste (Fuzzy, gleicher Matcher wie
      `gl`/`gs`-Popups).
- [ ] `Enter` öffnet die selektierte Datei im `$EDITOR`. `Esc`
      schließt den Picker.
- [ ] `:config jira` (mit Argument) öffnet die Picker-Liste bereits
      gefiltert; wenn das Argument exakt eine Datei matcht, wird der
      Editor direkt geöffnet ohne Zwischenschritt.

### 2. Edit + Save + Reload — Granular (View-YAML)

- [ ] Eine View-YAML editieren (z. B. `views/jira.yaml`: `tab.name`
      ändern), speichern und `$EDITOR` schließen.
- [ ] Notification "Reloaded view jira.yaml" erscheint.
- [ ] Der Tab-Name in der Tab-Bar zeigt den neuen Wert.
- [ ] Andere Tabs (Tasks, Trackings, Taiga, Postgres) sind unverändert
      — keine Daten verloren, keine Cursor-Sprünge.

### 3. Edit + Save + Reload — Voll (tui.yaml / adapter-yaml)

- [ ] `tui.yaml` editieren (z. B. ein Theme-Color ändern), speichern.
- [ ] Notification "tui.yaml reloaded"; neues Theme greift sofort.
- [ ] Tasks- und Trackings-Tab: Daten werden neu geladen
      (spawn_load lief), Selection auf Default zurück — akzeptiert.
- [ ] Eine Adapter-YAML editieren (z. B. `views/jira-adapter.yaml`:
      Subdomain ändern), speichern.
- [ ] Notification "All views reloaded after views/jira-adapter.yaml
      change"; Jira-Tab nutzt sofort den neuen Adapter (z. B. neue
      Auth-Domain).

### 4. Fehlerfall — Parse Error

- [ ] In einer YAML eine offene Klammer einfügen
      (`queries: [unclosed`), speichern.
- [ ] Editor **schließt nicht** — wird mit dem gleichen Buffer
      sofort wiedereröffnet, oben drüber ein Error-Banner
      (`# ─── ERRORS ───` … `# • YAML parse: …` … `# ─────────────────`).
- [ ] Die Datei auf Disk wurde **nicht** überschrieben (alter Inhalt
      bleibt). Alte Config läuft normal weiter.
- [ ] Den Banner-Block belassen und ein zweites Mal speichern → nur
      ein Banner-Block bleibt am Ende sichtbar (kein Stapeln).
- [ ] Fehler beheben + speichern → Editor schließt, Reload-Message
      erscheint normal.

### 5. Fehlerfall — Validation Error (semantisch)

- [ ] In `views/jira.yaml` einen `adapter.type: nonexistent` setzen,
      speichern.
- [ ] Datei wird auf Disk geschrieben (anders als beim Parse-Error —
      Syntax war ja OK).
- [ ] Editor öffnet sich wieder mit Error-Banner
      (`Reload … failed: no adapter factory registered for type
'nonexistent'`).
- [ ] Im Hintergrund: Jira-Tab läuft **mit der alten Config weiter**
      (alter Adapter aktiv). User kann tabben und sehen, dass nichts
      verloren ging.

## Cmdline-Shortcuts (`cmdline_shortcuts:`)

In `tui.yaml` lassen sich beliebige `:command`-Strings auf einzelne
Tasten/Chords binden — ohne über den `:`-Prompt zu gehen.

```yaml
cmdline_shortcuts:
  F2: "config tui"
  "<c-comma>": "config"
```

- [ ] Mit obigem Eintrag: `F2` öffnet sofort `tui.yaml` im Editor,
      `Ctrl+,` öffnet den `:config` Picker.
- [ ] Shortcut feuert nur, wenn die Taste **kein** typed Action
      bindet — Standard-Keys (`q`, `j`, `k`, …) bleiben unverändert.
- [ ] Shortcut feuert **vor** dem Chord-Prefix-Fallback: ein
      single-char Shortcut auf `m` würde `gl`-Chord-Erkennung nicht
      kaputt machen (weil `m` kein Prefix von `gl` ist), aber wäre
      auch nicht verfügbar wenn `m` bereits eine Action wäre.
- [ ] Nach `:config tui` → Edit eines Shortcuts → Save: neue
      Shortcuts greifen sofort (tui.yaml Reload schließt sie ein).
- [ ] **Built-in default** `T` → `tag`: Bei tui.yaml ohne
      `cmdline_shortcuts:` Section (oder mit Section, die `T` nicht
      definiert und vorher kein eigener `cmdline_shortcuts:`-Eintrag
      existierte) öffnet `T` das Tag-Menü. Sobald der User
      `cmdline_shortcuts:` explizit setzt, ersetzt sein Block die
      Defaults — er muss `T: tag` selbst eintragen, falls er es
      behalten will.

## Tasks-Tree Expand/Collapse

Aufklappbarer Tree im Tasks-Tab. Bindings: `vt`/`vl` (Tree/List
sub-view), `enter` (toggle Cursor-Node), `zr` (alles ausklappen),
`zm` (auf `default_expand_depth` zurück; **nicht** voll bis Roots,
dafür `default_expand_depth: 0` setzen). Config:

```yaml
tasks:
  tree:
    default_expand_depth: 2 # 0 = nur Roots, 1 = + direkte Kinder, ...
```

- [x] In Tasks-Tab: `vt` → Tree-Sub-View aktiv; `vl` → List-Sub-View
      aktiv (Rename von `t`/`l`).
- [x] Tree zeigt initial nur die ersten `default_expand_depth+1`
      Ebenen — tiefere Nodes sind versteckt; ihre direkten Eltern
      tragen `▶` + `(N)` Suffix (N = Anzahl direkter Kinder).
- [ ] `enter` auf einem collapsed Parent → klappt auf, Glyph
      wechselt zu `▼`, Kinder werden sichtbar.
- [ ] `enter` auf einem expanded Parent → klappt zu, Glyph wechselt
      zu `▶`, Kinder verschwinden, `(N)` Suffix erscheint.
- [ ] `enter` auf einem Leaf-Node → No-Op (Cursor bleibt, Tree
      unverändert).
- [ ] `zr` → alle Branches ausgeklappt; alle Parents zeigen `▼`,
      keine `(N)` Suffixes.
- [ ] `zm` → Tree zurück auf `default_expand_depth` (z.B. 2 Ebenen
      offen, tiefer wieder zu); per-Node `enter`-Toggles werden dabei
      verworfen.
- [ ] Mit `:config tui` `default_expand_depth: 0` setzen, dann `zm` →
      jetzt sind nur die Roots sichtbar (vim-style full collapse).
- [ ] Fuzzy-Filter aktiv (`f` + Text) → Expand-State wird ignoriert;
      jedes Match (plus Ancestors) erscheint, keine `(N)` Suffixes.
- [ ] Filter clear → vorheriger Expand-State ist zurück.
- [ ] `:config tui` → `default_expand_depth: 3` → Save → Tree
      rendert sofort mit der neuen Tiefe (über Reload-Pipeline).
- [ ] Restart der TUI: Expand-State ist **nicht** persistiert; Tree
      ist wieder auf `default_expand_depth` zurück.
- [ ] List-Sub-View (`vl`) zeigt weder Glyphen noch `(N)` —
      Listenmodus unangetastet.
- [ ] Trackings-Tab → Tree (`t`) zeigt **keine** Glyphen und
      **keine** `(N)`-Suffixes (Trackings-Tree ist nicht aufklappbar
      konfiguriert).
- [ ] Action-Bar (im Tree-Sub-View) zeigt Hint `↵ expand/collapse`.
- [ ] Default `dismiss_notifications` ist jetzt `Z` (vorher `z`,
      kollidierte mit `zr`/`zm`). Trigger eine Notification (z.B.
      Fehler-Befehl), drücke `Z` im Normal-Mode → Notification
      verschwindet. `:dismiss-notifications` ohne Argument tut
      dasselbe. `:dismiss-notifications foo` → Modal _":dismiss-notifications
      takes no arguments"_.

### `/`-Suche durch eingeklappte Branches

`/` durchsucht im Tree-Sub-View jetzt **alle** Tasks (auch versteckte).
Springt der Cursor mit `n`/`N` auf einen Match in einem eingeklappten
Branch, klappt sich nur die Ancestor-Kette des aktuellen Treffers auf.
Beim nächsten `n`/`N` kollabiert der vorherige Pfad wieder; eine
beliebige andere Taste (`j`, `k`, `<space>`, Enter zum Schließen, …)
"committet" den aktuellen Pfad — er bleibt offen, der nächste `n` ist
dann wieder eine frische Auto-Expansion.

- [ ] Tree mit `default_expand_depth: 2` und Sub-Tasks tiefer als 2.
      `/` + Tippen eines Strings, der nur in einem tiefen Sub-Task
      vorkommt → der Pfad zum Treffer öffnet sich automatisch, Cursor
      landet auf dem Match.
- [ ] Weiter tippen (Buchstaben anhängen) → bei jedem Query-Change
      kollabiert der alte Pfad und der neue Pfad zum ersten Match
      öffnet sich.
- [ ] Mehrere Treffer in verschiedenen Sub-Branches: `n` springt zum
      nächsten Match → vorheriger Branch kollabiert wieder, neuer
      Branch öffnet sich. `N` rückwärts analog.
- [ ] Während `n`/`N`-Sequenz: `j` (oder beliebige andere Taste) →
      aktuell offener Pfad **bleibt** offen. Anschließend nochmal `n`
      → neuer Branch öffnet sich, der eben "fixierte" Pfad bleibt
      auch offen (zwei sichtbare Pfade).
- [ ] `Enter` (Suche annehmen) commit'tet den aktuell offenen Pfad
      ebenso.
- [ ] `Esc` (Suche abbrechen, leere Query) → Auto-Expansion wird
      verworfen, Tree fällt auf den vorigen Expand-State zurück.
- [ ] `Esc` bei nicht-leerer Query → Query wird gelöscht, Auto-
      Expansion verworfen, Suche bleibt aktiv.
- [ ] Fuzzy-Filter aktiv (`f` + Text) + `/`-Suche → Suche durchsucht
      nur die vom Fuzzy-Filter sichtbaren Tasks (keine Auto-Expansion
      nötig, alles ist bereits sichtbar).
- [ ] Vor `/`: einen Top-Level-Branch manuell mit `enter` zugeklappt.
      `/` matched einen Task in diesem Branch → Branch öffnet sich
      transient. Commit → Branch bleibt offen, `enter` darauf
      schließt ihn wieder normal (transient wurde sauber in `flipped`
      promotet, nicht doppelt geflippt).

## Tasks-Tree Cut / Paste (`:cut-node` / `:paste-node`)

Tasks im Tree umhängen ohne Edit-Form: erst `mc` (oder
`:cut-node`) auf der Quelle, dann Cursor auf den neuen Parent
bewegen, dann `mp` (oder `:paste-node`). Das DB-Update läuft
**ausschließlich** bei Paste — vorher wird nichts angefasst.

- [ ] Tasks-Tab, irgendeinen Task wählen, `mc` → Notification "Cut:
      … — paste with :paste-node (mp)". Tree unverändert.
- [ ] Anderen Task wählen, `mp` → Notification "Moved: …". Tree
      rebuildet, Quelle hängt jetzt unter dem Ziel.
- [ ] `mc` ohne Auswahl in Tasks → Modal _":cut-node — no task selected"_.
- [ ] `mp` ohne vorheriges `mc` → Modal _":paste-node — nothing cut (use :cut-node / mc first)"_.
- [ ] `mc` auf einem Task A, dann nochmal `mc` auf Task B → letzter
      Cut gewinnt (Notification mit Beschreibung von B).
- [ ] `mc` auf A, `Esc` → Notification "Cut cancelled".
      Nachfolgendes `mp` → Modal _"nothing cut"_.
- [ ] `mc` auf A, Cursor auf A selbst, `mp` → Modal _"cannot paste a
      task onto itself"_. A bleibt cut (zweiter Versuch möglich).
- [ ] `mc` auf A, Cursor auf einen Nachfahren von A, `mp` → Modal
      _"cannot move a task into its own subtree"_. Tree unverändert.
- [ ] `mc` auf A, Cursor auf den aktuellen Parent von A, `mp` →
      Notification _"already a child of the target"_, kein DB-Write,
      Cut gelöscht.
- [ ] `mc` auf A, `mp` auf Root-Task B → A wird Kind von B,
      Notifikation "Moved: …".
- [ ] `mc` aus Tasks-Tab raus in Trackings/Content-Tab gewechselt,
      dann `mp` → Modal _":paste-node only works on the Tasks tab"_.
- [ ] `:cut-node foo` → Modal _":cut-node takes no arguments"_.
      Analog für `:paste-node foo`.
- [ ] Default-Bindings: `m` allein landet nicht direkt (wird als
      Chord-Prefix gestasht); erst `mc` / `mp` feuert.

## `:jump` und `:focus-task`

Programmatic Navigation für Skripte und Power-Cmdline-User.
`:jump <Tab>[:<sub>]` schaltet Tab/Subtab, `:focus-task /a/b/c`
sucht im Tasks-Tree den passenden Knoten und expandiert den Pfad.

- [ ] `:jump Tasks` → schaltet auf Tasks-Tab, Subtab unverändert.
- [ ] `:jump Tasks:tree` → Tasks-Tab, Subtab Tree. Aus dem List-
      Subtab heraus: Selektion bleibt erhalten (set_pending_focus).
- [ ] `:jump Tasks:list` analog.
- [ ] `:jump Tasks:foobar` → Modal _":jump — unknown Tasks sub-view 'foobar' (list|tree)"_.
- [ ] `:jump Trackings:condensed` → Trackings-Tab, Condensed-Subtab.
      (Trackings rebuildet via `rebuild_trackings_table`.)
- [ ] `:jump <name-eines-content-tabs>` (case-insensitive) →
      schaltet auf den passenden Content-Tab.
- [ ] `:jump nichtvorhanden` → Modal _":jump — unknown tab 'nichtvorhanden'"_.
- [ ] `:jump` ohne Argument → Modal _":jump expects one argument, e.g. :jump Tasks:tree"_.

- [ ] Im Tasks:tree, `:focus-task /seg1/seg2/...` mit eindeutiger
      Pfadkette → Pfad expandiert sich, Cursor parkt auf dem
      tiefsten Knoten. Default-Match ist **case-sensitive
      substring**.
- [ ] `:focus-task /Work` (Groß-W) trifft `work`-Task NICHT mehr
      (Default sensitive); `:focus-task -i /Work` findet ihn.
- [ ] `:focus-task -i /…` mit gemischter Groß/Kleinschreibung in den
      Segmenten matched trotzdem, sowohl für Substring- als auch
      `re:`-Segmente.
- [ ] `:focus-task /work/clients/acme/tickets/re:\b42\b` →
      matched ein Task mit "42" in der description, NICHT 420 oder 421. (Word-Boundary-Trennung über Ziffern.)
- [ ] `:focus-task /…/re:[broken(` (kaputtes Regex) → Modal
      _"invalid regex 're:…' — …"_, Tree unverändert.
- [ ] `:focus-task -x /…` (unbekannter Flag) → Modal _"unknown flag
      '-x' (only -i is supported)"_, Tree unverändert.
- [ ] `:focus-task /unbekannt` → Modal _"no task matching 'unbekannt' at root level"_, Tree unverändert.
- [ ] `:focus-task /work/unbekannt` → Modal _"… under 'work'"_, Tree unverändert.
- [ ] `:focus-task work/x` (ohne führenden `/`) → Modal _"expects a /-rooted path …"_.
- [ ] `:focus-task /` → Modal _"path is empty"_.
- [ ] `:focus-task -i /` → Modal _"path is empty"_ (Flag konsumiert, leerer Pfad).
- [ ] `:focus-task /<ambig>` wenn mehrere Root-Tasks matchen → Modal
      _"'<ambig>' is ambiguous: 'task A', 'task B', …"_.
- [ ] `:focus-task /…` im Tasks:list-Subtab → Modal _"only works in the Tasks:tree sub-view"_.
- [ ] `:focus-task /…` aus Trackings/Content-Tab → Modal _"only works on the Tasks tab"_.

## `:reload-tasks`

Synchroner Refetch der `task_rows` aus der DB. Hauptzweck: in einer
Command-Chain aus einem Skript (siehe nächster Abschnitt) zwischen
einem externen `nyd task add` und einem nachgelagerten
`:focus-task` einklemmen, damit der neu angelegte Task gesehen wird.

- [ ] In TUI Tasks-Tab; in zweitem Terminal `nyd task add 'Smoke
reload' --parent <id-eines-existierenden-tasks>` → neue Row
      ist im laufenden TUI noch NICHT sichtbar (selbst nach
      Tab-Wechsel hin und zurück, weil `set_active_tab` nur bei
      `Idle` reloaded). `:reload-tasks` → Row erscheint sofort im
      Tree (Parent ggf. auto-expandiert, falls vorher offen).
- [ ] `:reload-tasks` aus Trackings-Tab → Tasks werden im
      Hintergrund neu geladen, aktiver Tab bleibt Trackings (kein
      Auto-Switch).
- [ ] `:reload-tasks` aus einem Content-Tab → dito, kein Tab-Wechsel.
- [ ] `:reload-tasks foo` → Modal _"takes no arguments"_.
- [ ] Tasks-Filter aktiv (z.B. fuzzy oder gespeicherter Filter) →
      Reload respektiert den aktiven Filter (gleiche Args wie
      `spawn_load`); neue Rows, die den Filter nicht erfüllen,
      tauchen nicht auf.
- [ ] DB nicht erreichbar während Reload → Modal _"reload-tasks
      — …"_; aktive `task_rows` bleiben unverändert (kein Wipe).

## `:focus-node`

Content-View-Pendant zu `:focus-task`. Schaltet auf den genannten
Content-Tab (+ optional Sub-View) und parkt den Cursor auf der ersten
Zeile, deren Spalte das Pattern matched. Default: case-sensitive
substring; `-i` foldet Case; `re:` opt-in zu Regex. Single-segment
only (Drill-Down später).

- [ ] In einem beliebigen Tab; `:focus-node Taiga:items /ref|acme#42`
      → Tab wechselt zu Taiga, Subview items, Cursor parkt auf der
      Zeile mit `ref = acme#42`. Nachfolgende n/N-Navigation
      respektiert die neue Position.
- [ ] `:focus-node Taiga:items /acme#42` (kein column-hint) →
      matched, weil das Pattern in der konkatenierten label+fields
      vorkommt; falls Collisions (z.B. mit subject), darüber direkt
      `ref|...` schreiben.
- [ ] `:focus-node Taiga:items /id|userstory:4242` → matched via
      composite_id. (Skripten unbequem, weil die `composite_id` nicht
      aus dem ref/slug ableitbar ist; aber als Form unterstützt.)
- [ ] `:focus-node Taiga:items /label|<exakter subject-Anfang>` →
      matched substring im NodeSummary.label.
- [ ] `:focus-node Taiga:items /ref|re:\bacme#42\b` → Regex mit
      Word-Boundary, matched nicht auch `acme#420`.
- [ ] `:focus-node -i Taiga:items /ref|ACME#42` → case-insensitive
      matched eine Zeile mit `ref = acme#42`.
- [ ] `:focus-node Taiga:items /foo|x` (foo existiert in keiner
      Metadaten-Spalte) → Modal _"unknown column 'foo' (available:
      …)"_, Cursor unverändert.
- [ ] `:focus-node Taiga:items /ref|nope` → Modal _"no row matching
      'ref|nope'"_.
- [ ] `:focus-node Taiga:items /ref|acme` (matched mehrere
      Tickets) → Modal _"'ref|acme' is ambiguous: 'userstory:1',
      'userstory:2', …"_.
- [ ] `:focus-node Taiga:items /a/b` (zwei Segmente) → Modal
      _"multi-segment drill-down paths are not yet supported"_.
- [ ] `:focus-node Taiga:items ref|x` (ohne führenden `/` im Pfad)
      → Modal _"expects a /-rooted path …"_.
- [ ] `:focus-node Taiga:items /` → Modal _"path is empty"_.
- [ ] `:focus-node Tasks:tree /work` → Modal _"…not a content tab"_
      (Tasks ist kein Content-Tab; für Tasks gilt `:focus-task`).
- [ ] `:focus-node nichtvorhanden:items /ref|x` → Modal _"'…' is not
      a content tab …"_.
- [ ] `:focus-node Taiga:foobar /ref|x` → Modal _"unknown view
      'foobar' for tab 'Taiga' (available: items, notifications)"_.
- [ ] `:focus-node -x Taiga:items /ref|x` (unbekannter Flag) → Modal
      _"unknown flag '-x' (only -i is supported)"_.
- [ ] Wenn die Items-View noch nie geladen wurde (manual*connect):
      `:focus-node Taiga:items /ref|acme#42` findet 0 Zeilen →
      Modal *"no row matching …"\_; User muss erst `r` (reload)
      triggern.

## CLI `task show --path`

Pfad-basiertes Lookup, teilt Semantik mit `:focus-task`. Erfolg →
JSON `{"id","description","parent_id"}` auf stdout, exit 0. Fehler
→ Meldung auf stderr, exit ≠ 0.

- [ ] `nyd task show --path /Work/Clients/Acme/Tickets` →
      stdout JSON mit `id` + `description` + `parent_id`, exit 0.
- [ ] `nyd task show -i --path /work/clients/acme/tickets` →
      gleicher Hit wie oben, exit 0.
- [ ] `nyd task show --path /nope` → stderr _"no task matching
      'nope' at root level"_, exit 4.
- [ ] `nyd task show --path 'work/foo'` (kein leading `/`) →
      stderr _"path must start with '/'"_, exit 2.
- [ ] `nyd task show --path '/re:[broken('` → stderr _"bad
      segment …"_, exit 3.
- [ ] `nyd task show --path /<ambig>` wenn mehrere Root-Tasks
      matchen → stderr _"…is ambiguous (n candidates):"_ mit
      Liste, exit 5.
- [ ] `nyd task show --path /` → stderr _"path is empty"_, exit 2.
- [ ] Pipe-bar: `nyd task show --path … | jq -r '.id'` liefert
      eine valide UUID (smoke fürs JSON-Schema).

## Script `mode: commands` (script → TUI command relay)

`# mode: commands` Skripte schreiben in `$NYD_OUTPUT_FILE` JSON
`{"commands": [...]}` und steuern darüber die TUI (z.B. `jump`,
`focus-task`, `tag`, ...). `interactive+commands` macht dasselbe,
nur dass das Skript zusätzlich das Terminal bekommt.

- [ ] Background-Variante: Skript schreibt
      `{"commands": ["jump Tasks:tree"]}` in `$NYD_OUTPUT_FILE`,
      Tab wechselt nach Tasks:tree.
- [ ] Mehrere Commands hintereinander: `["jump Tasks:tree",
"focus-task /work/foo"]` → Jump + Path-Expand + Focus
      laufen der Reihe nach durch.
- [ ] Skript schreibt nichts → Notification "Script finished", keine
      Commands ausgeführt (no-op statt Fehler).
- [ ] JSON ist nicht parsebar → Modal _"Script output is not valid JSON: …"_.
- [ ] JSON ohne `commands`-Array → Modal _"missing `commands` array"_.
- [ ] Eintrag in `commands` ist kein String → Modal _"command entry is not a string"_, weitere Einträge laufen trotzdem.
- [ ] Eintrag mit führendem `:` (`":jump Tasks:tree"`) wird genauso
      akzeptiert wie ohne.
- [ ] Skript exitet mit non-zero status → Modal _"Script exited with …"_,
      Commands werden NICHT ausgeführt.
- [ ] Vorwärtskompatibilität: JSON `{"commands": [...], "version": 1,
"metadata": {…}}` wird akzeptiert, unbekannte Keys ignoriert.
- [ ] Stderr aus dem Skript landet weiterhin als Notification.
- [ ] `interactive+commands`: Skript läuft interaktiv (TUI yielded
      Terminal), nach Beendung werden Commands aus dem detached-
      output_file ausgeführt.
- [ ] Das output-file in `/tmp` ist nach dem Lauf weg (cleanup).

## SQ-8 — Postgres-Script-Shortcuts via `query_shortcut` (DB)

Postgres-pro-Tabelle-Skripte hängen ihren Hotkey nicht mehr in einer
`.shortcuts.yaml` neben dem Skript-`.sql`, sondern in der
`query_shortcut`-Tabelle (Scope = NodeRef-Pfad
`postgres/<inst>/<db>/schemas/<schema>/tables/<table>`, Name =
Skript-Datei-Stem, Shortcut = Chord). Apply-on-Chord ist symmetrisch zu
Jira/Taiga: globaler Auslöser solange das fokussierte Pane auf genau
dieser Tabelle steht.

- [ ] **Bestand:** Migration der 6 jira/taiga-Zeilen mit `:`-Separator auf
      NodeRef-`/`-Form:
      `sh
sqlite3 ~/.local/share/not_yet_done/nyd.db \
    "UPDATE query_shortcut SET scope = REPLACE(scope, ':', '/') WHERE scope LIKE '%:%';"
`
      Danach erscheinen Jira/Taiga-Saved-Query-Hotkeys (`ctrl+i`, `ctrl+m`,
      `ctrl+w` etc.) im Tab wieder als highlighted Apply-Hint und feuern
      die zugehörige Query.
- [ ] Postgres-Tab → eine konkrete Table fokussieren (Drilldown auf Schema
      `public` → Cursor auf `users`) → `q queries` → ein existierendes
      Skript hat `[chord]` in der Liste, wenn vorher gebunden.
- [ ] `Ctrl+e` in der Skript-Liste → Modal „Press a shortcut key" → eine
      freie Taste drücken → Notification „Bound shortcut" oder still.
      Anschließend `sqlite3 ~/.local/share/not_yet_done/nyd.db
"SELECT scope, name, shortcut FROM query_shortcut WHERE scope LIKE
'postgres/%';"` zeigt den Eintrag mit NodeRef-Pfad.
- [ ] Popup schliessen, Cursor bleibt auf derselben Tabelle → den
      gebundenen Chord drücken → der Skript-Result öffnet sich im
      Rows-Split (gleiches Verhalten wie Enter-on-Apply aus dem Menü).
- [ ] Selber Chord, Cursor auf ANDERER Tabelle → der Chord ist NICHT
      claimed (`PostgresTableScriptShortcut` ist auf den Table-NodeRef
      konditioniert), Default-Verhalten der Taste bleibt aktiv.
- [ ] Im Q-Menü `d` (delete) auf ein Skript mit Shortcut → Datei UND
      `query_shortcut`-Zeile sind weg (sqlite3-Check).
- [ ] Restart TUI → die Bindings überleben (DB-persistent).
- [ ] Filesystem-Check: Es gibt nach Bind/Delete KEINE
      `.shortcuts.yaml`-Datei mehr in
      `~/.local/share/not_yet_done/postgres/*/queries/**`.

## Jira Multi-Hop Workflow-Transitions (TR-1 … TR-7)

Hintergrund: Der Transition-Picker schreibt jede beobachtete Workflow-Kante
in den Cache (`jira_workflow_edge`) und enumeriert daraus Mehrschritt-Ketten,
sodass User von `Ready → In Progress → Done` in einem Schritt durchgehen
können, ohne die Zwischenstati einzeln klicken zu müssen.

### Setup

- Backing-DB ist `~/.local/share/not_yet_done/jira-cache.sqlite`. Tabelle
  `jira_workflow_edge` wird beim ersten Adapter-Start automatisch angelegt
  (SeaORM auto-sync).
- Cache nie manuell zurückgesetzt — Hop-Limit ist 4, Self-Loops werden
  aufgezeichnet aber nicht traversiert.

### Smoke (Cold-Start, leerer Cache)

- [ ] DB-Tabelle leeren: `sqlite3 ~/.local/share/not_yet_done/jira-cache.sqlite "DELETE FROM jira_workflow_edge;"`
      vor TUI-Start.
- [ ] Auf einem Jira-Ticket Transition-Picker öffnen → Optionen entsprechen
      genau den direkten Transitions. Label = nur Ziel-Status, kein `*`
      (alle direkt). Kein doppelter Eintrag wenn zwei Transitions zum
      selben Status führen — erste gewinnt.
- [ ] Picker schließen → in der DB existieren Edges für aktuellen Status:
      `sqlite3 ... "SELECT from_status_name, transition_name, to_status_name FROM jira_workflow_edge;"`

### Smoke (Snowball wächst, Multi-Hop erscheint)

- [ ] Ticket in Status `Ready` öffnen → Picker → direkte Transitionen
      (keine `*`).
- [ ] Andere Tickets in `In Progress` und in `Review` öffnen, Picker je
      einmal aufrufen, dann Esc (kein Transition!).
- [ ] Zurück zum ersten Ticket → Picker → erscheint jetzt zusätzlich
      Eintrag `Done*` (oder ähnlich), erreichbar via Multi-Hop. Das `*`
      markiert "nicht direkt"; die Zwischenstati erscheinen NICHT mehr
      im Label.
- [ ] Gibt es sowohl eine direkte Transition nach `Done` als auch eine
      Multi-Hop-Kette zu `Done`, erscheint nur ein Eintrag `Done` (ohne
      `*`) — der direkte Pfad gewinnt.

### Smoke (Chain-Erfolg)

- [ ] Picker → Multi-Hop-Eintrag wählen (z.B. 2-Hop) → Enter.
- [ ] Status-Bar: `<KEY> → <Endstatus>`.
- [ ] Ticket-Detail zeigt Endstatus; Jira-Web bestätigt dass beide
      Transitionen durchliefen.

### Smoke (Chain-Fehler mit Refresh)

- [ ] Workflow mit `required field` an zweitem Hop konstruieren (z.B.
      Transition fordert `resolution` als pflicht). Test-Ticket auf
      Startstatus zurücksetzen.
- [ ] Picker → Multi-Hop wählen, dass den Pflicht-Feld-Hop enthält → Enter.
- [ ] Status-Bar: `Chain stopped at step 2/3 (now in <Zwischenstatus>):
<Jira-Errorbody>`.
- [ ] Ticket-Detail in TUI zeigt aktuellen Zwischenstatus (Hop 1 erfolgreich
      persistiert, Hop 2 abgebrochen) — nicht den ursprünglichen Startstatus.

### Smoke (Picker-Hint-Bar)

- [ ] Beliebige Picker-Action öffnen (`transition`, oder auch andere wie
      Postgres-Cell-Picker falls existent) → Hint-Bar zeigt `Enter apply
| Esc close` am Popup-Footer.
- [ ] Im Detail-Pane: Style passt zu anderen Menü-Hints (Query-Menü,
      Tag-Menü). Kein doppelter Style, kein Layout-Bruch.

### Edge Cases

- [ ] Ticket-Key ohne `-` (sollte nicht passieren, aber falls
      Test-Konfig kaputt ist): Recording bricht still ab, direkter
      Picker funktioniert weiter.
- [ ] `db.url` in der Jira-Adapter-YAML auf `none`/leer: Recording
      No-op, Picker zeigt nur direkte Transitionen, kein Crash.
- [ ] Self-Loop-Transition in Workflow (z.B. `To Do → To Do` zum Anhängen
      eines Attachments): Wird als Edge geschrieben, aber NICHT als
      Pfad-Option vorgeschlagen.

## SearchablePopup — intrinsische Navigation (SP-1 … SP-7)

Hintergrund: `SearchablePopup` trägt jetzt sein eigenes Set an Bindings
für `next` / `prev` / `backspace` / `cursor_left` / `cursor_right`
(`PopupAction`-Enum, konfigurierbar unter `popup:` in `tui.yaml`,
Defaults `ctrl+j`/`ctrl+k`/`backspace`/`left`/`right` plus Pfeiltasten
als Sekundär-Binding für `Next`/`Prev`). Die intrinsischen Hints
erscheinen automatisch in der Hint-Bar — der Transition-Picker stimmt
damit visuell und funktional zu QueryMenu/TagMenu/ScriptMenu.

### Smoke (Transition-Picker bekommt jetzt sichtbare Navigation)

- [ ] Jira-Ticket mit ≥3 verfügbaren Transitions auswählen, Picker
      öffnen (`Action`-Trigger des Adapters, z.B. `t`).
- [ ] Hint-Bar am unteren Popup-Rand zeigt: `↓ next  ↑ prev  ⏎ apply
␛ close` (Icons via `key_icons`-Map; `↓/↑` für Pfeil + ggf. `ctrl+j`
      mit Slash davor).
- [ ] Pfeil-Hoch/Runter UND `Ctrl+J`/`Ctrl+K` navigieren beide.
- [ ] Tippen filtert die Liste; `Backspace` zeigt zusätzlichen Hint
      `⌫ erase` (nur wenn die Query nicht leer ist).

### Smoke (andere Picker unverändert)

- [ ] QueryMenu (`q` auf Trackings/Tasks/Content) — Hint-Bar zeigt
      jetzt zusätzlich `next`/`prev` vor den bisherigen Hints
      (`apply`/`edit`/`shortcut`/`delete`/`close`).
- [ ] TagMenu (`:tag`), ScriptMenu (`:script`) und `gl`-Link-Popup
      analog: Pfeile + `Ctrl+J/K` funktionieren, Hint-Bar zeigt sie.
- [ ] `:config` Picker funktioniert weiterhin: tippen filtert,
      Pfeil-Navigation, Enter öffnet die Datei.

### Smoke (Custom-Bindings via tui.yaml)

- [ ] In `~/.config/not-yet-done/tui.yaml` unter `keybindings.popup:`
      `next: ctrl+n` und `prev: ctrl+p` setzen.
- [ ] TUI neu starten (oder `:config` → tui.yaml → save → granularer
      Reload).
- [ ] Transition-Picker zeigt jetzt `^N next  ^P prev …` und genau
      diese Tasten navigieren.

## Shortcut Hints (SH-1 … SH-7)

Hintergrund: YAML-`shortcuts:` (z.B. `a: add`, `x: execute`,
`Q: parent:edit_sql`) werden jetzt als Action-Bar- oder Status-Bar-Hints
auf der gerade selektierten Zeile gerendert. Die Hints sind row-spezifisch
und kommen aus dem `Node::actions()`-Lookup pro `node_id`, asynchron
gefetcht und gecached. Race-frei via Cache-Key = `node_id`.

### Smoke (Postgres — Datenbank-Subtab Tree-Mode)

- [ ] Postgres-Tab → Subtab `5` (Datenbank) → in Tree-Mode hoch- und
      runter-cursorn. Cursor auf einer DB-Scripts-Gruppen-Row →
      Action-Bar zeigt `a: add`.
- [ ] Tree-expand der Scripts-Gruppe (`l`/`Enter`) → Cursor auf einer
      einzelnen DB-Script-Leaf → Action-Bar zeigt `x: execute`,
      `e: edit`, `d: delete` — `a: add` ist NICHT mehr sichtbar
      (Leaf hat kein `add` in `actions()`).
- [ ] Erster Cursor-Move auf neuer Zeile: Hints können kurz fehlen,
      erscheinen sobald die Adapter-Antwort eintrifft (max. einige
      ms). Beim zweiten Besuch derselben Zeile sofort da (Cache-Hit).

### Smoke (Rows-View mit `parent:`-Shortcut)

- [ ] Postgres-Tab → Tabelle wählen → Rows-View öffnen. Action-Bar
      zeigt `Q: edit sql` (aus dem ViewDef-Shortcut
      `Q: parent:edit_sql`), aufgelöst über den Eltern-Table-Node.
      Cursor in der Zeilen-Liste bewegen → der Hint bleibt stabil
      (Target ist das Parent, nicht die aktuelle Row).

### Smoke (Cache-Invalidation auf Reload)

- [ ] Auf einer Row mit existierenden Hints (z.B. DB-Script-Leaf,
      `x e d` sichtbar) → `r` (reload) → Liste wird neu geladen, Hints
      werden für die nun selektierte Zeile neu gefetched und
      erscheinen wieder.

### Regression-Bait

- [ ] Mehrfaches schnelles Hoch-/Runter-Cursorn auf verschiedenen
      Rows: keine Duplikat-Fetches, keine stale Hints aus einer alten
      Row (Cache key=node_id, Pending-Dedup).
- [ ] Jira-Ticket-Liste: bewegt sich der Cursor zwischen Tickets, sind
      die Shortcut-Hints der jeweiligen Row stets ihrer eigenen
      `actions()` zugeordnet (kein Ticket zeigt die Hints des
      Nachbar-Tickets).

## EIP — Edit-in-Place für DB Scripts

ChildDef-Flag `editor_in_place: true` legt das Editor-Tempfile im
Zielverzeichnis statt in `$TMPDIR` ab, damit LSPs (z. B.
`postgres-language-server`) den Projektkontext finden.

**Setup**: ein leeres oder beliebiges `postgres-language-server.jsonc`
unter `<instance_data_dir>/db_scripts/<db>/` ablegen.

- [ ] DB Scripts-Tree öffnen, mit `e` ein Skript editieren →
      vim/$EDITOR-Statuszeile zeigt einen Pfad **innerhalb** des
      `db_scripts/<db>/`-Verzeichnisses, prefix `.nyd_tmp_…`, suffix
      `.sql` (kein `/tmp/…`).
- [ ] Nach `:w` + `:q` ist das `.nyd_tmp_…`-Tempfile im Verzeichnis
      gelöscht, das echte Skript trägt den geschriebenen Inhalt.
- [ ] `postgres-language-server.jsonc` neben den Skripten wirkt auf
      die Edit-Session (LSP-Diagnose / Hover, je nach Server-Setup).
- [ ] Mit `editor_in_place: false` (oder Default) liegt das Tempfile
      wieder in `/tmp/…`.
- [ ] Ein `.py`/`.md`-Skript editieren: Tempfile-Suffix übernimmt die
      reale Endung (`.nyd_tmp_xyz.py`), kein SQL-Template eingefügt.
- [ ] TUI hart killen mitten in einer Edit-Session →
      `.nyd_tmp_…`-Datei verbleibt im Verzeichnis (Prefix ist klar
      als Junk erkennbar; manuell löschbar).

## AE — Adapter Child-Process Environment

Trait `ContentAdapter::child_process_env(node) -> HashMap<String,String>`
wird beim Spawn von Editor- und Skript-Kindprozessen abgefragt; die TUI
gibt den Inhalt opak per `Command::envs(...)` weiter. Postgres-Adapter
liefert `PGHOST`/`PGPORT`/`PGUSER`/`PGPASSWORD`/`PGDATABASE`/`PGSSLMODE`.

**Setup**: aktiver Postgres-Adapter mit `manual_connect: false` (auto-
warmup), `transport.mode: ssh_tunnel` ist der interessante Fall. Im
nvim `postgres_lsp` aktiv via `:LspInfo`. Die jsonc neben den Skripten
darf nur die Schema-Zeile enthalten — alle `db`-Keys raus.

- [ ] Auf einem `postgres:db_script` Node `e` (Edit) — nvim öffnet den
      Buffer in `db_scripts/<db>/`, `postgres-language-server` startet
      sauber (Logs: kein „pool timed out").
- [ ] Im Buffer `SELECT * FROM ` tippen → Completion-Popup mit den
      realen Tabellen/Spalten der DB der NodeRef. Vorher/nachher
      vergleichen: ohne Adapter-Env (z. B. `disableConnection: true`
      in der jsonc) liefert v0.25 0 Items.
- [ ] In einem Shell-Term im editor: `env | grep ^PG` zeigt die fünf+
      Variablen (Wert von `PGPASSWORD` nicht in den Repo paste!).
- [ ] Adapter offline (Status-Bar `Disconnected`): `e` öffnet trotzdem,
      LSP startet ohne DB-Connection und zeigt 0 Completions (graceful,
      keine Fehlermeldung).
- [ ] `:script` im Postgres-Tab auf einem Tabellen-Knoten ausführen:
      `python3 - <<'PY' …` kann `os.environ["PGPASSWORD"]` lesen
      (z. B. Skript schreibt nach `$NYD_OUTPUT_FILE` eine Liste der
      `PG*`-Variablen). `NYD_OUTPUT_FILE` darf von Adapter-Env nicht
      überschrieben werden — Adapter-`PGFOO` kommt vor `NYD_*`.
- [ ] `:script` im Tasks-Tab: empty env, kein `PG*` (no regression —
      Tasks-Skripte sehen weiterhin nur die alten Variablen).
- [ ] `:script` im Trackings-Tab: gleich, empty env.
- [ ] Tunnel manuell killen (auf der SSH-Bastion), in der TUI eine
      Query absetzen → tear_down + reconnect; nächster Editor-Start
      auf einem DB-Script hat `PGPORT` mit dem neuen lokalen Port,
      nicht dem alten.

## Confluence-Adapter (CF-3 … CF-16)

Hintergrund: Confluence-Server/DC-Adapter spiegelt die Jira-/Taiga-
Architektur — `confluence:space` als Root, `confluence:page` rekursiv
darunter, plus `confluence:attachment` und `confluence:comment` als
Leaf-Branches an jeder Seite. Alle Aktionen werden über das
adapterseitige `actions_for_type` gebunden, die View-YAML-Einträge
sind Dokumentation.

### Setup

- `~/.config/not_yet_done/views/confluence-adapter.yaml` mit Cookie-
  Skript-Pfad (`auth.bindings[].provider.script`). Skript schreibt
  eine Zeile `JSESSIONID=...; crowd.token_key=...; atlassian.xsrf.token=...`
  auf stdout.
- `~/.config/not_yet_done/views/confluence.yaml` (Beispiel:
  [`docs/examples/views/confluence.yaml`](examples/views/confluence.yaml)).
- Saved-Queries-Verzeichnis:
  `<XDG_DATA_HOME>/not_yet_done/confluence/<instance_id>/queries/`.
  Optionaler Seed:
  [`docs/examples/views/saved/confluence/recent-pages.yaml`](examples/views/saved/confluence/recent-pages.yaml).
- TUI starten → Tab `Confluence` (Default-Sub-Tab `spaces`); per
  `manual_connect: true` lädt nichts automatisch, `r` triggert den
  ersten Fetch.

### CF-3 — Spaces

- [ ] `r` auf `spaces` → Liste aller Spaces; `Key`/`Name`/`Type`
      Spalten gefüllt; Cursor-Navigation `j`/`k` funktioniert.
- [ ] `f`-Fuzzy- und `/`-Search-Aktionen filtern auf `Key`+`Name`.
- [ ] `o` auf einem Space-Row öffnet den Space im Browser (webui).

### CF-4 — Pages (rekursiv)

- [ ] Auf einer Space-Row Enter → Top-Level-Pages werden inline
      expandiert; Tree-Marker (`▼`/`▶`) sichtbar.
- [ ] Auf einer Page-Row Enter → Kindseiten + Attachments + Comments
      als drei Branches erscheinen.
- [ ] Tief drillen (3+ Ebenen) bleibt konsistent — gleicher
      rekursiver `ChildDef` an jedem Level.

### CF-5 — Preview Pane (body.storage)

- [ ] Auf einer Page-Row `p` → Preview-Pane erscheint horizontal
      gesplittet (50/50), zeigt `body.storage` (XHTML).
- [ ] Erste `p`-Toggle: spürbare Latenz (lazy-fetch
      `GET /content/{id}?expand=body.storage,...`); zweite Toggle:
      sofort (Cache).
- [ ] `p` erneut → Preview schließt.

### CF-6 — Attachments

- [ ] Page drillen → `attachments`-Branch zeigt Dateiname, Author,
      Size, Mime-Type, Created.
- [ ] `d` auf einem Attachment → Download in Tempdir + `xdg-open`
      öffnet die Datei.
- [ ] Zweiter `d` auf demselben Attachment: kein erneuter Fetch
      (cached-by-id), öffnet sofort.

### CF-7 — Comments (read-only)

- [ ] `comments`-Branch zeigt Author, Created, Body-Auszug.
- [ ] `p` toggelt Body-Preview; **kein** zweiter HTTP-Call (Body
      ridet auf `list_comments` mit `expand=body.storage,version`).

### CF-8 — CQL Search

- [ ] Sub-Tab `search` öffnen → Default-CQL aus YAML
      (`type = page AND lastModified > now("-7d") ...`) zeigt
      Treffer.
- [ ] `q` öffnet Saved-Queries-Menu; Seed `recent-pages` taucht auf.
      Enter applied; `Ctrl+f` bindet Chord-Shortcut (persistiert in
      `query_shortcut`).
- [ ] `:query new <name>` → Editor öffnet; CQL eintippen, speichern
      → erscheint im `q`-Menu.
- [ ] `:query delete <name>` entfernt Datei + DB-Shortcut.
- [ ] Drilldown auf einem Search-Treffer öffnet die gleichen drei
      Branches (pages / attachments / comments) wie via `spaces`.

### CF-9 — Edit Page + 3-Way Merge

- [ ] `e` auf einer Page → `$EDITOR` öffnet mit Title + pretty-
      printed `body.storage` (xmllint).
- [ ] Body trivial ändern (z.B. neuen Absatz einfügen), speichern,
      Editor schließen → `Updated page <Title> (v <n+1>)` Banner.
- [ ] Confluence-Web: neue Version erscheint, Body korrekt.
- [ ] **Disjoint-Merge**: vor Editor-Save in Confluence-Web die
      Page upstream ändern (eine andere Stelle als die Edits im
      Buffer). Speichern → 409 → auto-merge → `Merged on top of
v<m>` Banner; beide Änderungen sind in der Final-Version.
- [ ] **Conflict-Merge**: vor Save dieselbe Stelle upstream
      ändern. Speichern → 409 → Buffer reopent mit
      `<<<<<<< ours` / `>>>>>>> theirs` Markern + Banner
      `Merge conflict — resolve and save again`. Marker manuell
      auflösen, Save → Update geht durch.
- [ ] Parse-Error im Buffer (z.B. Title-Zeile löschen) → Reopen
      mit Error-Banner, kein PUT.

### CF-10 — Create Page

- [ ] `a` auf einer Space-Row → Editor mit `title:`-Header + leerem
      `<p></p>`-Body. Title setzen, Save → neue Top-Level-Page in
      diesem Space (Banner mit neuer ID); Reload zeigt sie unter
      dem Space.
- [ ] `a` auf einer Page-Row → analog, neue Kindseite unter dieser
      Page (Reload zeigt sie als Child).
- [ ] Title leer lassen → Reopen mit Parse-Error.

### CF-11 — Delete Page (Trash)

- [ ] `Shift+D` auf einer Page → Confirm-Popup
      `Delete '<title>'? y/n` (oder Enter/Esc).
- [ ] `y`/Enter → Page verschwindet aus Liste; Confluence-Web Trash
      enthält sie; Restore aus Web-UI funktioniert.
- [ ] `n`/Esc → Page bleibt; kein Request gefeuert.

### CF-12 — Comments CRUD

- [ ] `c` auf einer Page → leerer XHTML-Editor; Body eingeben, Save
      → neuer Comment erscheint im `comments`-Branch (Reload
      erzwingen via `r` auf der Page).
- [ ] `e` auf einem Comment → Buffer mit Body, modifizieren,
      speichern → Banner `Updated comment`; Body neu im Listing.
- [ ] **Comment-409**: parallel auf dem Web denselben Comment
      editieren → Reopen mit Error-Banner (kein 3-way merge — manuell
      neu schreiben + speichern).
- [ ] `Shift+D` auf einem Comment → generisches `ConfirmDeleteContentNode`
      Popup → Enter löscht; Comment verschwindet.

### CF-13 — Attachment-Upload

- [ ] `Shift+A` auf einer Page → FilePicker öffnet sich.
- [ ] Multi-Select: 2–3 kleine Test-Dateien wählen (invented data,
      keine echten Kunden-Files!) → Save → Banner
      `Uploaded N attachment(s) to page <Title>`.
- [ ] `attachments`-Branch der Page (nach `r`) listet alle
      hochgeladenen Files; `d` öffnet sie korrekt.
- [ ] Eine unlesbare Datei + eine lesbare auswählen → Error-Banner
      benennt explizit den fehlgeschlagenen Pfad; lesbare Datei
      trotzdem hochgeladen (`uploaded 1/2; failures: ...`).

### CF-14 — Clone Page

- [ ] `y` auf einer Page → Editor öffnet mit `title: <Original> (Clone)` + pretty-printed Body.
- [ ] Save ohne Änderung → neue Page unter demselben Parent
      (oder als Top-Level, wenn Quelle Top-Level war) im selben
      Space; Banner `Cloned page <orig> → <new> (id ...)`.
- [ ] Title-Suffix-Stacking: `y` auf der gerade geklonten Page →
      Title bleibt `<Original> (Clone)` (kein doppeltes Suffix).
- [ ] Body editieren vor Save → neue Page hat den editierten Body,
      nicht den Original-Body.
- [ ] Parse-Error (Title-Zeile löschen) → Reopen mit Banner; kein
      POST gefeuert.

### CF-Bugfix 2026-06-02 — Pages-in-Space rendern leer (tree_label-Alignment)

Symptom: eine `tree_label: name`-Space aufgeklappt → ~50 leere Zeilen unter der Space-
Zeile, obwohl der `/content/page`-Response volle `title`-Felder
liefert. Root-Cause: der Tree-Renderer (`content_view.rs::build_tree_data_rows`)
malt für jede Zeile nur dann die Label-Zelle, wenn die `tree_label`
des Zeilen-Levels mit einem `col.key` der **aktiven** Spaltenmenge
übereinstimmt — und zeigt für non-active-depth Zeilen sonst alle
Zellen leer an. Space-Level hatte `tree_label: name`, Page-Level
`tree_label: title` → keine Übereinstimmung → Page-Zeilen leer.

Fix: Page-Level in `confluence.yaml` (User-Config + Repo-Example) auf
`tree_label: name` umgestellt; Page-Column `key: title` → `key: name`
(Display-Header bleibt "Title" via `label:`). 115 Tests grün.

- [ ] Space-Whitelist aktiv mit zwei realen Keys (s. user-config),
      Cursor auf der ersten Space-Zeile lassen, `o` zum Expand →
      Pages erscheinen mit korrekten Titeln (nicht leer).
- [ ] Cursor auf eine Page-Zeile bewegen → active_depth=1, Header
      wechselt auf "Title | ID", andere Spaces zeigen leer (by design,
      siehe Konvention).
- [ ] Recursive: in eine Page mit Sub-Pages drillen → Sub-Pages
      ebenfalls korrekt benannt (kein Regress gegen die rekursive
      ChildDef).

### CF-Bugfix 2026-06-02 — Spaces zeigen alle Pages statt Top-Level

Symptom: eine Space aufgeklappt → ~50 Pages der gesamten Space, statt
nur der direkten Children des Space-Homepages wie im Confluence
"Tree browser" der Web-UI sichtbar. Root-Cause:
`/rest/api/space/{KEY}/content/page` liefert per Default `depth=all`,
also _jede_ Page der Space — nicht die Tree-Browser-Liste.

Fix: `/rest/api/space?expand=homepage` zieht jetzt die Homepage-Id
pro Space mit, `SpaceMeta::homepage_id` speichert sie. Beim Expand
einer Space ruft der Adapter `list_child_pages(homepage_id, ...)`
statt `list_top_pages(space_key, ...)`. Lookup-Pfad
(`get_by_id`-Synthesizer) fetcht `/space/{KEY}?expand=homepage`
lazy beim ersten `list()` via `OnceCell`. Wenn Confluence keine
Homepage exponiert (legacy/restricted), liefert die Listing-API
eine leere Page-Liste statt zurück auf alle-Pages-Fallback.

- [ ] Space-Whitelist aktiv, eine bekannte Space mit Tree-Browser-
      Seitenleiste im Web-Confluence öffnen → Anzahl + Titel der
      sichtbaren Pages dort gegen die Expand-Liste in der TUI
      vergleichen (sollten 1:1 matchen, Reihenfolge `position`).
- [ ] Reload (`r`) auf eine Space-Zeile → Pages erscheinen weiter
      korrekt (Cache-Pfad).
- [ ] Direkter Lookup-Pfad: nach `:focus-node` oder Cross-Tab-Link
      auf eine Space → erste Expand hängt nicht (OnceCell fetcht
      Homepage transparent), Pages-Liste matcht Web-UI.

### CF-Bugfix 2026-06-02 — Column-Reihenfolge: Tree-Spalte zuerst

YAML-Reorder in `spaces`-View: `name` (tree-label) jetzt vorne,
`key` und `type` trailing. Konvention für tree-mode Views: die
Tree-tragende Spalte gehört an Position 1, sonst kommt der
Tree-Indent erst nach den schmalen Trailing-Spalten.

- [ ] Spaces-Subtab zeigt links zuerst den Tree-Indent + Space-Name,
      rechts daneben `Key` und `Type`.

### CF-16 — Spaces-Whitelist (`space_keys`)

- [ ] `confluence-adapter.yaml` ohne `space_keys:` → spaces sub-tab
      listet wie früher alle lesbaren Spaces (Regression-Check).
- [ ] `space_keys: [BBB, AAA]` mit zwei realen Keys, die _nicht_
      alphabetisch sortiert die gewünschte UI-Reihenfolge sind →
      spaces sub-tab zeigt nur BBB und AAA, in genau dieser
      Reihenfolge (BBB zuerst).
- [ ] `space_keys: [GOOD, NOPE_TYPO]` → spaces sub-tab zeigt nur
      GOOD; kein Error-Banner, kein Crash. (silent-drop verifiziert)
- [ ] Mit `space_keys:` gesetzt: `r` (reload) im spaces sub-tab →
      keine Pagination-Affordanzen sichtbar (kein `has_next`),
      Listing zeigt nur die whitelist-Spaces.
- [ ] Drill-down in eine whitelisted Space → Pages/Attachments/
      Comments funktionieren wie ohne whitelist (Recursive ChildDef
      ist unbetroffen).

### CT-1..CT-11 — Tree-Find (spaces-tab `/`)

Voraussetzung: Spaces sub-tab geladen mit ≥2 Spaces, in denen
mehrere Pages liegen (mindestens eine mit ≥2 Verschachtelungstiefe).
Wenn `space_keys:` gesetzt: nur whitelisted Spaces dürfen auftauchen.

- [ ] `/` öffnet die Eingabezeile mit Prompt "Search pages" und
      `?`-Prefix. Tippen ändert die Anzeige nicht (kein
      Local-Filter).
- [ ] Enter mit nicht-trivialem Begriff → Toast
      `Tree find "q": N hits — n/N to navigate`. Status-Bar zeigt
      `n/N  Tree find "q": 1/N`.
- [ ] Tree expandiert automatisch bis zum ersten Hit, Cursor sitzt
      darauf.
- [ ] `n` springt zum nächsten Hit in Baum-Reihenfolge (gleicher
      Space zuerst, dann nächster YAML-Space-Eintrag); Vorfahren
      werden lazy nachgeladen wenn nötig (kurzes Flackern okay).
- [ ] `N` springt zurück (wrap-around am Anfang/Ende).
- [ ] Status-Bar-Counter aktualisiert sich bei jedem Sprung
      (`1/47`, `2/47`, …).
- [ ] Wenn der Server mehr Treffer hat als der Cap (100), zeigt der
      Counter `, truncated` an und das Toast meldet `truncated`.
- [ ] Esc auf der leeren Eingabezeile schließt sie ohne Cache.
- [ ] Esc auf gefüllter Eingabezeile vor Enter: löscht den Cache
      (n/N fällt zurück auf Local-/).
- [ ] `r` (reload) löscht den Tree-Find-Cache. Status-Bar-Hint
      verschwindet, n/N wieder Local-/.
- [ ] Erneutes `/` mit anderer Query: alter Cache weg, neuer Such-
      vorgang startet sauber (kein Mix von alten + neuen Hits).
- [ ] Mit `space_keys:` gesetzt: tree_find findet keine Pages aus
      whitelisted-out Spaces (Server filtert via `space in (...)`
      injection).
- [ ] Während ein Tree-Find aktiv ist, weiterhin manuelles
      Expandieren/Kollabieren in Spaces möglich; Cache überlebt
      diese UI-Interaktionen.
- [ ] `f` (lokaler fuzzy_filter) ist unverändert und filtert nur
      die bereits sichtbaren Space-Zeilen (nicht Pages).

### CT-12 — SpaceNode → top-level pages (statt Homepage-Kinder)

Voraussetzung: Spaces sub-tab mit ≥1 Space, dessen Homepage und/oder
Top-Level-Pages mehrere Verschachtelungsebenen haben.

- [ ] Drill in einen Space → Level 1 zeigt nicht mehr nur die
      Homepage-Kinder, sondern alle Top-Level-Pages des Space
      (inklusive der Homepage selbst).
- [ ] Tree-Find (`/`) auf eine Page, die ≥2 Ebenen unter der
      Homepage liegt → expandiert sauber bis zur Page (kein
      `Tree find: Hit's ancestor '…' at depth 1 not in loaded
children` mehr).
- [ ] Page mit `o` (open-in-browser) auf Homepage-Zeile öffnet die
      Homepage-URL.
- [ ] `a` auf Space-Zeile erstellt weiterhin eine top-level Page;
      `a` auf Homepage-Zeile erstellt ein Child unter der Homepage
      (= bisheriges Verhalten, unverändert).
- [ ] Spaces ohne erkennbare Top-Level-Pages (Edge-Case) liefern
      eine leere Liste statt eines Errors.

### CT-13 — Tree-Find Walker bleibt nach Hit "settled"

Voraussetzung: tree_find auf spaces aktiv (`type: tree_find` in
view-YAML). Vorher mindestens einmal `/` gesucht und auf einen Hit
gesprungen.

- [ ] Nach Treffer-Sprung den Cursor mit `j`/`k` woandershin bewegen,
      dann auf einem anderen, eingeklappten Knoten Enter drücken
      (lädt dessen Kinder) → Cursor bleibt auf dem geöffneten Knoten,
      springt NICHT zurück zum letzten Suchergebnis.
- [ ] `n`/`N` springt weiterhin zwischen den Hits hin und her (also
      settled wird durch next/prev korrekt zurückgesetzt).
- [ ] Neue Suche mit `/` (Search-Input erneut öffnen) verhält sich
      wie immer: erster Hit wird angesprungen.

### RD-1 — Render-Loop Dirty-Gating (CPU im Leerlauf)

Siehe `docs/decisions/0001-render-loop-dirty-gating.md`.

- [ ] App offen lassen, nichts tun (kein Key, kein Tracking aktiv):
      `top`/`htop` zeigt die CPU des Prozesses nahe 0 % (vorher: dauerhaft
      spürbare Last durch 60-fps-Repaint).
- [ ] Tippen/Navigieren fühlt sich unverändert direkt an — keine
      Eingabe-Latenz (Keys lösen sofort Redraw aus).
- [ ] Async-Last (z. B. großes Content-Listing laden, Taiga/Jira-Reload):
      Ergebnis erscheint praktisch sofort (≤ ~200 ms), Spinner/Banner
      aktualisieren sich flüssig.
- [ ] **Busy-Banner-Zähler läuft:** eine Query mit `query_timeout_secs`
      starten (Postgres) bzw. langsame Verbindung — der „…(Ns/…)"-Zähler
      im Banner zählt **ohne** weitere Eingabe sekündlich hoch (friert
      nicht ein).
- [ ] **Aktives Tracking:** ein Tracking starten → die Dauer-Spalte
      aktualisiert sich weiterhin (adaptives Intervall), App bleibt im
      Leerlauf trotzdem ruhig.
- [ ] Editor öffnen (`e`), `:w` (live-apply, falls aktiv), schließen →
      Rückkehr rendert sofort sauber; detached-Script (`x`) liefert nach
      Ende sein Ergebnis ohne Hänger.

### RD-2 — Render-Loop 1b (event-getriebener `select!`-Loop)

Siehe `docs/decisions/0001-render-loop-dirty-gating.md` (§1b). Ersetzt den
200-ms-Poll-Loop; Idle ist jetzt geparkt statt periodisch wach. **Fokus:
keine Regression an stdin-Übergabe und Zeitgetriebenem.**

- [ ] **Echtes Idle:** App offen, nichts tun → CPU 0 %. Mit `strace -fp
<pid> -e trace=poll,read` (oder `perf`) sieht man **keine**
      periodischen 200-ms-Wakeups mehr, solange kein Tracking/Banner/Editor
      lebt.
- [ ] **Key-Latenz:** Tippen/Navigieren reagiert sofort — kein Regress
      gegenüber 1a.
- [ ] **Async-Push weckt sofort:** großes Content-Listing laden → Ergebnis
      erscheint ohne wahrnehmbare Verzögerung (nicht mehr an 200-ms-Raster
      gebunden).
- [ ] **Inline-Editor stdin-Übergabe:** `e` → der Editor bekommt **jeden**
      Tastendruck (kein verschlucktes erstes Zeichen), Tippen flüssig;
      Schließen → TUI kehrt sauber zurück, erste Taste danach wirkt sofort.
- [ ] **Detached-/Launch-Editor + `:w` live-apply:** Editor (Launch-Modus)
      offen lassen, im Editor speichern → live-reload greift weiterhin;
      `.done`/Schließen → Commit läuft, Rückkehr rendert.
- [ ] **Validierungsfehler-Reopen:** einen Commit mit Fehler provozieren →
      Editor öffnet erneut mit Fehlerpuffer, stdin gehört wieder dem Editor
      (keine verschluckten Keys).
- [ ] **Interaktives Script (`x`):** Script übernimmt das Terminal voll,
      bekommt Eingaben; „Press any key" + Rückkehr funktionieren.
- [ ] **Busy-Banner-Sekunde** und **aktives Tracking** zählen wie unter
      RD-1 sekündlich/adaptiv hoch — und sobald sie enden, kehrt die App in
      echtes Idle zurück (Ticker disarmt).
- [ ] **Resize:** Terminal-Fenster vergrößern/verkleinern → sofortiger,
      korrekter Repaint (kein „erst bei nächster Taste").
- [ ] **Quit:** `q`/`:q` beendet prompt.

### Real-Data-Sweep

- [ ] Vor jedem Commit: `git diff --staged | grep -iE '<euer-firmenname>|<firmen-kurzform>|atlassian\.net|<echte JSESSIONID-Snippets>'`
      — muss leer sein. (Die Platzhalter `<…>` durch euren echten
      Firmennamen + Kurzform + interne Host-Muster ersetzen; diese
      Muster selbst gehören **nicht** ins Repo.) Beispiel-Hosts sind
      `wiki.example.invalid`, Beispiel-Cookies `JSESSIONID=synthetic`,
      Space-Keys `DEMO`.

## Stoat-Adapter (Phase 0 — Fundament)

Verbindungs-only: Login + Discovery + Gateway (WS) + Status-Spiegelung.
**Noch kein Baum** — `list()` liefert leer; Phase 1 füllt ihn. Manueller
Test gegen die private Test-Instanz (Credentials **außerhalb** des Repos,
`username`-Feld trägt die E-Mail). Voraussetzung: `stoat-adapter.yaml` +
`stoat.yaml` in `~/.config/not_yet_done/views/` (Vorlagen unter
`docs/examples/views/`), echte Basis-Domain eingetragen.

- [ ] **Discovery + Login:** Stoat-Tab öffnen → Banner `Connecting…`; falls
      Credentials nötig, erscheint das `NeedsCreds`-Formular (Feld
      `username` = E-Mail, `password`). Nach Submit läuft der Login durch.
- [ ] **Ready:** Nach erfolgreichem WS-`Authenticate`+`Ready` springt das
      Banner auf `Ready`. Der Baum ist leer (Phase 0) — das ist korrekt.
- [ ] **Falsche Credentials:** absichtlich falsches Passwort → Login
      schlägt fehl, Banner `Failed{reason}` mit lesbarer Meldung; erneuter
      Versuch (`r` / Credentials neu) möglich.
- [ ] **Token-Persistenz:** TUI beenden + neu starten → kein erneuter
      Credential-Prompt (Session-Token aus SQLite wiederverwendet), Banner
      geht direkt über `Connecting` nach `Ready`.
- [ ] **Heartbeat/Idle:** Tab offen lassen (kein Input) → Verbindung bleibt
      bestehen (Ping alle 20 s); keine Idle-CPU-Regression (vgl. RD-2 — der
      Gateway-Task schläft zwischen Pings).
- [ ] **Reconnect:** Netz kurz trennen (z. B. WLAN aus/an) → Banner fällt
      auf `Connecting…`, nach Wiederkehr automatisch zurück auf `Ready`
      (Backoff ≤ 30 s).
- [ ] **MFA-Konto:** (falls verfügbar) Login mit MFA-Konto → klare
      „MFA not supported"-Fehlermeldung statt Hänger.
- [ ] **Sauberes Beenden:** `:q` beendet prompt; der Gateway-Task wird beim
      Adapter-Drop abgebrochen (kein hängender Prozess/Socket).
- [ ] **Real-Data-Sweep:** keine echte Instanz-Domain/E-Mail/Token im Repo
      (Vorlagen nutzen `chat.example.org`).

## Stoat-Adapter (Phase 1 — Read-only Baum)

Browsen + Lesen. Baut auf Phase 0 auf (Login/Gateway/Ready müssen grün
sein). Struktur (Server/Channels) kommt aus dem WS-`Ready`-Snapshot,
Message-Bodies per REST-Pull. **Kein Live-Push** — nach Connect ggf. `r`
drücken, damit der Baum den frisch eingetroffenen `Ready`-Stand zeigt.

- [ ] **Server-Liste:** Nach `Ready` (ggf. `r`) listet der `chats`-View die
      Server. Leerer Baum direkt nach Login = `Ready` kam noch nicht / `r`
      drücken.
- [ ] **Channels:** In einen Server drillen (`Enter`/`c`) → Text-Channels in
      der Server-Reihenfolge. Voice-Channels erscheinen, sind aber nicht
      aufklappbar (kein Inhalt).
- [ ] **Messages:** In einen Text-Channel drillen → die letzten ≤ 50
      Nachrichten, **neueste unten**. Spalten: Author, Time, Message.
- [ ] **Autor-Auflösung:** Author-Spalte zeigt Benutzernamen (nicht die
      rohe ID) — auch für Autoren, die nicht im `Ready`-Snapshot waren
      (kommt über `include_users`).
- [ ] **Zeitstempel:** Time-Spalte zeigt Datum/Uhrzeit (aus der Message-
      ULID dekodiert), plausibel aufsteigend nach unten.
- [ ] **Preview:** Auf einer Message `p` → Preview-Pane zeigt den vollen
      Message-Body (mehrzeilig korrekt; Tabellen-Zeile bleibt einzeilig).
- [ ] **DMs (optional):** Eine zweite View mit Top-Level `stoat:channel`
      (auskommentiertes Beispiel in `stoat.yaml`) listet Direkt-/Gruppen-
      Nachrichten statt Server.
- [ ] **Phase-1-Grenze bewusst:** sehr lange Channels zeigen nur die
      neuesten ~50 Messages; älteres Backfill kommt später (Cursor-
      Pagination). Kein Bug.
- [ ] **Real-Data-Sweep:** keine echten Channel-/Message-/User-Daten im
      Repo (Tests/Fixtures nutzen erfundene IDs).

## Stoat-Adapter (Phase 2 — Live-Layer)

Out-of-band-Updates ohne manuelles `r`. Baut auf Phase 1 auf. Braucht
einen zweiten Client (Web/Mobile) oder einen Helfer, um Nachrichten in
einen Channel zu schicken, während die TUI offen ist.

- [ ] **Auto-Populate:** Tab frisch öffnen. Sobald der Banner `Ready`
      erreicht, füllt sich der Server-Baum **von selbst** — **ohne** `r`.
      (Phase 1 brauchte hier noch ein manuelles Reload.)
- [ ] **Live-Message:** In der TUI einen Text-Channel offen halten
      (Message-Level). Von außen eine Nachricht in **genau diesen** Channel
      posten → sie erscheint innerhalb ~1 s unten, ohne Tastendruck.
- [ ] **Kein Fremd-Reload:** Channel A offen halten, Nachricht in Channel B
      posten → A lädt **nicht** neu (kein Flackern/Cursor-Sprung). Nur das
      offene Channel-Level reagiert.
- [ ] **Edit/Delete/Reaction:** Eine sichtbare Nachricht von außen
      editieren / löschen / mit einer Reaction versehen → das offene
      Channel-Level spiegelt die Änderung nach Reload.
- [ ] **Reconnect-Resync:** Netz kurz trennen (oder Gateway-Disconnect
      provozieren) → Banner `Connecting`, dann `Ready`; danach zeigt der
      Baum wieder den aktuellen Stand, ohne `r`.
- [ ] **Cursor-Reset bekannt:** Beim Live-Reload springt der Cursor auf
      Standard (kein „an der Leseposition bleiben") — erwartetes
      Phase-2-Verhalten, kein Bug.
- [ ] **Strukturgrenze bewusst:** Ein **neu angelegter/umbenannter** Channel
      erscheint erst nach einem Reconnect (frisches `Ready`), nicht sofort —
      strukturelle Live-Events sind noch nicht verdrahtet. Kein Bug.
- [ ] **Idle-CPU:** Tab offen, keine Aktivität → CPU bleibt bei ~0 %
      (Render-Loop 1b parkt; Invalidations wecken nur bei echtem Event).

## Stoat-Adapter (Phase 2.1 — Kategorien + Tree)

Die flache Drill-Down-View ist durch eine Tree-View ersetzt:
`Server → (Kategorie | uncategorized Channel) → Channel → Messages`.

- [ ] **Tree statt flach:** Tab öffnen → Server stehen als Tree-Wurzeln da
      (Indent/Glyph). Enter/→ auf einem Server expandiert **inline** zu
      seinen Kategorien **und** uncategorized Channels (kein Tab-Wechsel).
- [ ] **Reihenfolge:** Unter einem Server kommen die **uncategorized
      Channels zuerst**, danach die Kategorien.
- [ ] **Kategorie expandieren:** Enter/→ auf einer Kategorie zeigt ihre
      Channels inline eine Ebene tiefer.
- [ ] **Channel drillt in Messages:** Enter auf einem Channel (egal ob
      unter Kategorie oder direkt unter Server) öffnet die **flache
      Message-Liste**; Back/← kehrt in den Baum an dieselbe Stelle zurück.
- [ ] **Vollständigkeit:** Summe aus „uncategorized + alle Kategorie-
      Channels" deckt alle Channels des Servers ab — nichts fehlt, nichts
      doppelt.
- [ ] **Server ohne Kategorien:** Ein Server, der keine Kategorien hat,
      zeigt alle seine Channels direkt als uncategorized (kein leerer
      Kategorie-Zweig).
- [ ] **Voice-Channel bleibt Leaf:** Voice-Channels erscheinen, sind aber
      nicht drill-/expandierbar.
- [ ] **`/` tree find:** Auf der Server-Wurzel `/` → durchsucht Channels
      quer durch den Baum (eingeklappte Knoten inklusive).
- [ ] **Live im Tree:** Eine Message in einem **drillgeöffneten** Channel
      von außen posten → die Message-Liste aktualisiert sich live (Phase-2-
      Verhalten gilt unverändert, egal wo der Channel im Baum hängt).

## Stoat-Adapter (Phase 3 — Write)

In einen Channel drillen (flache Message-Liste). Vier Aktionen:
`a` send, `e` edit, `d` delete, `+` react. Endpoints sind vorab per `curl`
gegen die echte Instanz verifiziert (gegen `SavedMessages`).

- [ ] **Senden (`a`):** In der Message-Liste `a` → leerer `$EDITOR`
      (Markdown). Text tippen, speichern/schließen → Status „Message sent";
      die neue Nachricht erscheint unten in der Liste (Reload **und** Live-
      Event). Leerer Buffer → kein Versand, keine Fehlermeldung.
- [ ] **Senden bricht ab:** `a`, Editor **ohne** Änderung schließen →
      nichts wird gesendet.
- [ ] **Markdown bleibt:** Eine Nachricht senden, die mit `#` beginnt
      (z.B. `# Überschrift`) → wird **wörtlich** gesendet (kein Header wird
      gefressen).
- [ ] **Editieren (`e`):** Eigene Nachricht auswählen, `e` → Editor zeigt
      den **rohen Body** (kein `#`-Header). Body ändern, speichern → Status
      „Message edited"; die Zeile zeigt den neuen Text + „Edited"-Flag.
      Unverändert speichern → „No changes", kein Roundtrip.
- [ ] **Editieren fremd:** Fremde Nachricht `e`, Body ändern, speichern →
      sauberer Server-Fehler (403), kein Crash.
- [ ] **Löschen (`d`):** Eigene Nachricht `d` → Bestätigung, dann weg;
      Liste aktualisiert sich.
- [ ] **Reagieren (`+`):** Nachricht `+` → Emoji-Picker (👍 ❤️ 😂 …),
      eines wählen → Status „Reacted 👍"; Reaktion erscheint in einem
      zweiten Client / nach Reload im Web-UI.
- [ ] **Live-Echo:** Senden/Editieren/Löschen löst zusätzlich das Live-
      Event aus → die Liste ist auch in einem zweiten geöffneten Pane
      aktuell.

## Stoat-Adapter (Phase 4 — Strukturelle Live-Events)

Strukturänderungen werden live, ohne Reconnect. Auslösen im **offiziellen
Stoat/Revolt-Client** (oder seit dem Create-Feature direkt in der TUI, siehe
nächster Abschnitt), in einem Server, den du administrierst — die TUI mit
dem Stoat-Tab offen und auf dem Server-Baum (oder in einem Channel) daneben
halten. Wire-Shapes vorab per WS-Capture gegen die echte 0.13.7-Instanz
verifiziert.

- [ ] **Channel anlegen:** Im Web-Client einen Text-Channel anlegen → er
      erscheint **ohne** `r` im TUI-Baum unter dem Server (uncategorized).
- [ ] **Channel umbenennen:** Channel im Web-Client umbenennen → der neue
      Name erscheint live im Baum.
- [ ] **Channel löschen:** Channel im Web-Client löschen → verschwindet
      live aus dem Baum (und aus seiner Kategorie, falls zugeordnet).
- [ ] **Kategorie anlegen:** Kategorie im Web-Client anlegen → erscheint
      live als eigener Branch unter dem Server.
- [ ] **Kategorie umbenennen:** → neuer Titel live im Baum.
- [ ] **Channel in Kategorie ziehen:** Channel einer Kategorie zuordnen →
      wandert live aus „uncategorized" in den Kategorie-Branch.
- [ ] **Kategorie löschen:** → Branch verschwindet live; die enthaltenen
      Channels rutschen zurück nach „uncategorized" (bzw. wohin der Server
      sie umhängt).
- [ ] **Server umbenennen:** → Server-Label aktualisiert sich live.
- [ ] **Cursor-Verhalten:** Eine Strukturänderung setzt den Pane-Cursor
      zurück (bekannte Reload-Grenze, kein Bug) — Baum bleibt konsistent.
- [ ] **Server beitreten/verlassen (Negativ-Check):** Erscheint/verschwindet
      **erst nach Reconnect** — bewusst nicht live (Phase-4-Scope-Grenze).

## Stoat-Adapter — Channel/Kategorie anlegen (`al` / `ay`)

Anlegen direkt aus der TUI. Voraussetzung: ein Server, den du
administrierst, und die `al`/`ay`-Actions in `stoat.yaml` (Server-View +
`categories`-ChildDef). `al`/`ay` sind **Mehrzeichen-Chords** — sie liegen
nicht in `keybindings.content`, sondern werden generisch über die
View-Keymap erkannt (`ContentView::yaml_action_chord_prefix`); das `a`
wird als Chord-Präfix gestasht, das zweite Zeichen löst aus.

- [ ] **Channel unter Server:** Cursor auf die **Server-Zeile**, `al` tippen
      → Namens-Formular (ein Feld) öffnet. Name eingeben, bestätigen →
      Channel erscheint live unter „uncategorized" (kein `r` nötig; der
      Gateway echot `ChannelCreate`).
- [ ] **Kategorie unter Server:** Cursor auf die **Server-Zeile**, `ay`
      tippen → Formular → Name → Kategorie erscheint live als eigener
      Branch (`ServerUpdate` mit voller Kategorie-Liste).
- [ ] **Channel unter Kategorie:** Cursor auf eine **Kategorie-Zeile**, `al`
      tippen → Formular → Name → Channel erscheint live **im
      Kategorie-Branch**. (Zweistufig intern: Channel anlegen + Server-PATCH
      — für den Nutzer ein Schritt.)
- [ ] **Leerer Name:** Formular mit leerem/Whitespace-Namen bestätigen →
      klare Fehlermeldung, kein namenloser Channel/Kategorie entsteht.
- [ ] **`ay` auf Kategorie-Zeile:** nicht gebunden (Kategorien gibt es nur
      unter dem Server) → `a` wird zwar als Präfix gestasht, `ay` löst
      nichts aus und fällt sauber durch (kein Hänger, kein Fehler).
- [ ] **Chord-Abbruch:** `a` tippen, dann `esc`/eine nicht-passende Taste →
      kein Effekt, normale Bedienung läuft weiter.

## Stoat-Adapter — Channel/Kategorie umbenennen (`R`)

Umbenennen aus der TUI. Voraussetzung: ein Server, den du administrierst,
und die `R`-Action in `stoat.yaml` (beide `channels`-ChildDefs +
`categories`-ChildDef). `R` ist ein **Einzelzeichen** (kein Chord) und
öffnet — wie `al`/`ay` — ein Namens-Formular mit einem Feld, vorgefüllt
ist es nicht.

- [ ] **Channel unter Server:** Cursor auf eine **uncategorized Channel-Zeile**,
      `R` tippen → Formular → neuer Name → Channel-Zeile aktualisiert sich
      live (kein `r` nötig; der Gateway echot `ChannelUpdate`).
- [ ] **Channel unter Kategorie:** Cursor auf eine Channel-Zeile **im
      Kategorie-Branch**, `R` → Formular → neuer Name → Zeile aktualisiert
      sich live (gleiche Action, `PATCH /channels/{id}`).
- [ ] **Kategorie:** Cursor auf eine **Kategorie-Zeile**, `R` → Formular →
      neuer Name → Kategorie-Header aktualisiert sich live (`ServerUpdate`
      mit voller Kategorie-Liste; nur der Titel der Zielkategorie ändert
      sich, Channel-Zuordnungen bleiben).
- [ ] **Leerer Name:** Formular mit leerem/Whitespace-Namen bestätigen →
      klare Fehlermeldung, kein Umbenennen findet statt.
- [ ] **Fremder Server (kein Admin):** `R` auf einem Channel/einer Kategorie
      ohne Rechte → der Server lehnt mit sauberer Fehlermeldung ab, der Baum
      bleibt unverändert.

## Stoat-Adapter — Channel cut/paste (`C` / `P`)

Channels zwischen Kategorien verschieben. `C` (cut) **markiert** den Channel
unter dem Cursor — es wird **nichts gelöscht**; erst `P` (paste) hängt ihn um.
Reuse der generischen `mark-move`/`paste-move`-Shortcuts (wie Tasks `m`/`p`),
über `invoke_action` + `ActionContext.marked`. Voraussetzung: ein Server, den
du administrierst. Der Move ist intern ein Voll-Listen-PATCH der Server-
Kategorien (`update_server_categories`), live via `ServerUpdate`.

- [ ] **Channel in Kategorie:** Cursor auf eine Channel-Zeile, `C` (Status:
      „Marked … for move") → Cursor auf eine **Kategorie-Zeile**, `P` → Channel
      wandert live in diese Kategorie, verschwindet aus der alten Stelle.
- [ ] **Channel → uncategorized:** Channel `C`, dann Cursor auf die
      **Server-Zeile**, `P` → Channel landet im uncategorized-Branch.
- [ ] **Paste neben Channel:** Channel A `C`, dann Cursor auf Channel B (in
      einer anderen Kategorie), `P` → A landet in B's Kategorie (bzw.
      uncategorized, wenn B uncategorized ist).
- [ ] **Abbruch per zweimal `C`:** Channel `C`, dann auf derselben Zeile noch
      einmal `C` → „Cut cancelled", kein Move bei späterem `P`.
- [ ] **Abbruch per Tab-Wechsel:** Channel `C`, Tab wechseln (z. B. `1`) →
      „Cut cancelled"; zurück auf Stoat, `P` auf einer Kategorie → nichts
      passiert (kein hängender Cut).
- [ ] **`C` löscht nie:** nach `C` ist der Channel unverändert sichtbar; nur
      `P` verändert den Baum.
- [ ] **`cut` in der oberen Leiste + Highlight:** Der `C cut`-Hint steht in der
      **oberen Action-Bar** (nicht in der Status-Leiste). Nach `C` wird er in der
      Akzentfarbe (fett + unterstrichen) hervorgehoben, solange ein Cut armiert
      ist; nach `P` oder Abbruch (zweimal `C` / Tab-Wechsel) erlischt das
      Highlight wieder.
- [ ] **Fremder Server:** Channel von Server A `C`, dann `P` auf eine
      Kategorie/Server B → saubere Fehlermeldung („different server"), kein
      Move (Kategorien sind serverlokal).

## Stoat-Adapter — Chat-Layout (`row_layout`)

Die Message-Liste rendert per `row_layout` als Chat: Meta-Zeile + Body +
Spacer. In einen Channel drillen (split öffnet die Liste rechts).

- [ ] **Drei Zeilen je Nachricht:** Jede Nachricht belegt 3 Terminalzeilen —
      Zeile 1 `author  time`, Zeile 2 der Nachrichtentext, Zeile 3 leer.
- [ ] **Hervorhebung:** Author in Akzentfarbe, Time gedimmt (kommt aus
      `style: accent` / `style: text_dim`, über `tui.yaml` überschreibbar).
- [ ] **Kein Spaltenkopf:** Im Chat-Layout wird die `Author | Time | Message`-
      Kopfzeile **nicht** angezeigt.
- [ ] **Selektion:** Mit `j`/`k` navigieren → Auswahl-Hintergrund deckt die
      Meta- und die Body-Zeile ab, **nicht** die Leerzeile dazwischen.
- [ ] **Scrollen:** Liste mit mehr Nachrichten als Bildschirmhöhe → `j` ans
      Ende scrollt sauber Block für Block; ausgewählte Nachricht bleibt
      vollständig sichtbar (nicht halb abgeschnitten).
- [ ] **Aktionen unverändert:** `e`/`d`/`+`/`p`/`n` wirken weiter auf die
      ausgewählte Nachricht (die ganze Block-Auswahl, nicht einzelne Zeilen).
- [ ] **Andere Tabs unberührt:** Jira/Taiga/Postgres-Tabs rendern weiter als
      normale einzeilige Tabellen (kein `row_layout` → altes Verhalten).

## Stoat-Adapter — Markdown-Body (`markdown: true`)

Die Content-Spalte der Messages ist `source: content` + `markdown: true`, der
Body wird mehrzeilig und soft-gewrappt gerendert (`ratatui-markdown`). In einen
Channel drillen.

- [ ] **Alle Zeilen sichtbar:** Eine Nachricht mit mehreren harten
      Zeilenumbrüchen zeigt **jede** Zeile, nicht zu einer Zeile kollabiert.
- [ ] **Soft-Wrap:** Ein langer Absatz bricht am Pane-Rand um; Pane schmaler
      ziehen → derselbe Absatz reflowt über mehr Zeilen (Row-Höhe wächst mit).
- [ ] **Inline-Styling:** `**fett**`, `*kursiv*`, `` `code` `` erscheinen
      hervorgehoben; eine `# Überschrift` und `- Listen` werden als solche
      gerendert.
- [ ] **Farben aus Theme:** Body-Text/Headings/Emphasis ziehen ihre Farben aus
      `tui.yaml` (Theme-Bridge) — kein Hardcode; Theme wechseln verändert sie.
- [ ] **Selektion = nur Hintergrund:** Auswahl der Nachricht legt den
      Auswahl-Hintergrund über Meta + alle Body-Zeilen, **ohne** die
      Vordergrundfarben (author=accent, Body-Styling) plattzumachen.
- [ ] **Leerer Body:** Eine Nachricht ohne Text (z. B. nur Attachment) bricht
      das Layout nicht — die Body-Zeile bleibt leer, Spacer intakt.
- [ ] **Scrollen mit hohen Rows:** Sehr lange Nachrichten (viele Body-Zeilen)
      scrollen sauber; ausgewählte Nachricht bleibt vollständig sichtbar.
- [ ] **Andere Tabs unberührt:** Spalten ohne `markdown:` (Jira/Taiga/Postgres)
      rendern weiter einzeilig.

> Bekannte Schnitte: `/`-Suche markiert Treffer **nicht** im gerenderten Body;
> Code-Blöcke ohne Hintergrund; Syntax-Highlighting ist optional/separat.

## Stoat-Adapter — Smooth-Scroll (`smooth_scroll: true`)

Beide `messages`-Level haben `smooth_scroll: true`. In einen Channel mit vielen
(idealerweise mehrzeiligen) Nachrichten drillen.

- [ ] **Zeilenweise statt sprunghaft:** ↓ scrollt den Inhalt um **eine
      physische Zeile** nach oben — eine hohe Nachricht oben wird dabei
      sukzessive angeschnitten, nicht als ganzer Block weggeschoben.
- [ ] **Sticky Highlight, kein Snapping:** Der Highlight bleibt auf _seiner_
      Nachricht und **gleitet mit** dem Inhalt mit (darf am Rand angeschnitten
      sein). Er springt **nicht** bei jedem Schritt an den oberen Rand.
- [ ] **Späte Übergabe:** Erst wenn die hervorgehobene Nachricht **vollständig**
      über den oberen Rand verschwindet, rückt der Highlight um genau eine
      Nachricht zur jetzt obersten sichtbaren weiter. Beim Hochscrollen
      analog am unteren Rand.
- [ ] **Aktionen treffen die hervorgehobene Nachricht:** `e`/`d`/`+`/`p`
      operieren auf genau der aktuell hervorgehobenen Nachricht (auch wenn sie
      gerade angeschnitten am Rand sitzt).
- [ ] **Halbe/ganze Seite + g/G:** `Ctrl+d`/`Ctrl+u` scrollen um eine halbe
      Pane-Höhe (in Zeilen); `G`/`g` springen ans Ende/an den Anfang und
      wählen dabei explizit die letzte/erste Nachricht.
- [ ] **Bottom-Clamp:** Am Ende lässt sich nicht über die letzte Zeile hinaus
      scrollen; die letzte Nachricht bleibt unten bündig stehen.
- [ ] **Reload/Live-Event:** Kommt eine neue Nachricht an oder wird `r`
      gedrückt, bleibt die Scroll-Position sinnvoll (kein Sprung an den Anfang).
- [ ] **Andere Tabs unberührt:** Tabs ohne `smooth_scroll` (Jira/Taiga/Tasks)
      scrollen weiterhin diskret Eintrag-für-Eintrag.

## Stoat-Adapter — @-Mentions (`@username` + Slug-Roundtrip)

Revolt kodiert Erwähnungen im Body als `<@USERID>`. Anzeige und Editieren
lösen das wie bei Jira/Taiga über `not_yet_done_content::slug::SlugTable`:
Anzeige → `@username`, Editor → `@uu-slug` + CACHE-Section, beim Speichern
zurück nach `<@ID>`. Die Completion-Liste ist **server-scoped** (`GET
/api/servers/{id}/members`, einmal pro Server gecacht). Voraussetzung: einen
Server-Channel öffnen, in dem Nachrichten mit Erwähnungen existieren.

- [ ] **Anzeige `@username`:** Eine Nachricht, die jemanden erwähnt, zeigt in
      der Liste **`@Benutzername`**, nicht den rohen `<@01ABC…>`-Code (Label-
      Zeile **und** Markdown-Body).
- [ ] **Unbekannte ID bleibt roh:** Erwähnung eines Users, der **nicht** im
      Server ist (kein Cache-Eintrag) → `<@ID>` bleibt wörtlich stehen, kein
      Crash.
- [ ] **Edit zeigt Slugs + CACHE:** Eigene Nachricht mit Erwähnung `e` →
      Editor zeigt `@uu-<name>` statt `<@ID>`, und unten die CACHE-Section
      `#### CACHE / available @mentions … ####` mit allen `@uu-…` des Servers.
- [ ] **Slug-Roundtrip (No-op):** Im Editor **nichts** ändern, speichern →
      „No changes" (die `@uu-…` werden korrekt zurück nach `<@ID>` übersetzt,
      Body bleibt identisch — kein versehentlicher Edit).
- [ ] **Neue Erwähnung einfügen:** Im Editor einen `@uu-<name>` aus der CACHE-
      Section in den Text kopieren, speichern → der Erwähnte wird im Web-Client
      tatsächlich benachrichtigt; die TUI-Zeile zeigt danach `@<name>`.
- [ ] **Unbekannter Slug → Fehler:** Im Editor `@uu-quatsch` (nicht im CACHE)
      eintippen, speichern → sauberer Fehler „unknown mention slug @uu-quatsch",
      kein Versand, kein Crash.
- [ ] **Senden (`a`) mit Mention:** In der Channel-Liste `a` → leerer Buffer +
      CACHE-Section; `@uu-<name>` einfügen, Text senden → Erwähnung kommt im
      Web-Client an, TUI zeigt `@<name>`.
- [ ] **Server-Scoping:** In zwei verschiedenen Servern hat die CACHE-Section
      **unterschiedliche** Mitgliederlisten (nur Mitglieder des jeweiligen
      Servers).
- [ ] **DM/Gruppe:** In einem Direkt-/Gruppen-Channel (kein Server) speist sich
      die Completion-Liste aus den Recipients (Ready-Snapshot), nicht aus dem
      Members-Endpoint.

> Bekannte Schnitte: Member-Liste wird einmal pro Server pro Session gecacht
> (kein Live-Refresh bei Beitritt/Austritt); Slug-Source ist der Username
> (Server-Nickname noch nicht berücksichtigt).

## Taiga-Adapter — Edit-Hang-Fix (Timeout + nicht-blockierender Editor)

Zwei Ebenen gegen das „App friert beim Edit (`e`) auf einer Taiga-Zeile
komplett ein"-Problem. Ebene 1 = HTTP-Timeout + Reconnect-Retry im Adapter;
Ebene 2 = Editor-`prepare` läuft off-thread statt blockierend.

**Ebene 1 — Timeout/Reconnect:**

- [ ] **Default-Timeout greift:** Ohne `request_timeout_secs` in
      `taiga-adapter.yaml` verhält sich alles wie bisher; eine gesunde,
      langsame Instanz antwortet weiterhin (Default 20 s).
- [ ] **Toter Socket → Fehler statt Freeze:** Während die App läuft, die
      Verbindung zur Taiga-Instanz hart kappen (z. B. Netzwerk/Tunnel
      blockieren), dann `e` auf einer Zeile. Erwartet: nach ~Timeout ein
      Reconnect-Versuch, dann saubere Fehlermeldung
      („Failed to load …" / Netzwerkfehler) — **nie** dauerhaftes Hängen.
- [ ] **Kurzer Timeout zum Testen:** `request_timeout_secs: 3` setzen, Tunnel
      blockieren → Fehler kommt nach ~6 s (2 Versuche), App bleibt bedienbar.
- [ ] **`connect_timeout_secs` separat:** Ohne Angabe = `min(request, 10)`. Mit
      explizit z. B. `connect_timeout_secs: 30` darf der Verbindungsaufbau auf
      einer langsamen Leitung länger als 10 s dauern, ohne fälschlich
      abzubrechen (gesunde, langsam verbindende Instanz).
- [ ] **Reconnect heilt transienten Abriss:** Verbindung kurz kappen und sofort
      wieder freigeben → der zweite (Retry-)Versuch geht durch, kein
      Nutzer-sichtbarer Fehler.

**Ebene 2 — nicht-blockierender Editor-Dispatch (alle Adapter):**

- [ ] **UI bleibt responsiv:** `e` auf einer Taiga-Zeile bei langsamer
      Verbindung → die Notification „⏳ Opening editor: …" erscheint sofort,
      und die TUI nimmt währenddessen weiter Input an (scrollen, Tab wechseln),
      friert also nicht ein.
- [ ] **Editor öffnet normal:** Bei gesunder Verbindung öffnet `$EDITOR` wie
      gewohnt; die „Opening editor…"-Notification verschwindet beim Öffnen
      (andere Notifications bleiben stehen).
- [ ] **Kein Doppel-Open:** Während „Opening editor…" läuft, erneut `e` →
      „Editor is already open", kein zweiter Ladevorgang.
- [ ] **Fehler-Notification:** Schlägt das `prepare` fehl (toter Socket), zeigt
      die Statuszeile den Fehler und es öffnet sich **kein** leerer Editor.
- [ ] **Andere Adapter unverändert:** Jira/Postgres/Confluence/Stoat — `e`
      (bzw. die jeweilige Editor-Aktion) öffnet weiterhin korrekt; inline- und
      pause-tui-Editor-Profile funktionieren (der `pending_editor_request`-Pfad).

> Bekannte Schnitte: kein explizites „Abbrechen" während des Ladens — der
> Wartebalken ist durch den Adapter-Timeout (Ebene 1) ohnehin begrenzt, und der
> Generation-Token verwirft eine veraltete Session, falls zwischenzeitlich neu
> geöffnet wird. Retry trifft nur Transport-Fehler, nicht HTTP-Status (4xx/5xx);
> der multipart-Upload (`upload_attachment`) hat keinen Retry (Form nicht
> klonbar), wird aber vom Timeout geschützt.

## Postgres — Retry nach fehlgeschlagenem Erst-Load (leerer Tree)

Voraussetzung: Postgres-Tab erreichbar machen/kappen, sodass der erste
`list databases`-Load fehlschlägt (z. B. Tunnel-Ziel down, oder kurzes
`query_timeout_secs`).

- [ ] Postgres-Tab öffnen, Load schlägt fehl → Banner „Fetch failed:
      list databases: …".
- [ ] **`r` drücken → Reload wird ausgelöst** (Banner wechselt auf
      „Retrying …"/„Connecting …", nicht stummes Nichts). Das war der
      Bug: im leeren Tree war keine Cursor-Zeile → die View-Actions
      (inkl. `reload`) wurden nicht aufgelöst, `r` verpuffte.
- [ ] Verbindung wieder herstellen, `r` → Datenbanken laden, Banner weg.
- [ ] Auch `f` (fuzzy filter) und `/` (search) sind im leeren Tree
      ansprechbar (gleiche Wurzel-Fallback-Logik).

## Postgres — `manual_connect` (kein Auto-Connect, nur `r`)

`adapter.manual_connect: true` in `postgres.yaml`.

- [ ] App-Start: Postgres-Tab lädt **nicht** automatisch; Banner
      „Press `r` to connect".
- [ ] Subtab-Wechsel (databases/tables/scripts) löst ebenfalls keinen
      Auto-Load aus.
- [ ] `r` baut Verbindung + SSH-Tunnel auf und lädt.

## Tab-Konstellationen + Autonummerierung

`tabs:`-Sektion in `tui.yaml` (`active: default`, `sets: { default: [...] }`).

- [ ] Tab-Bar zeigt nur die Tabs der aktiven Konstellation, in
      Listenreihenfolge, mit Ziffern `1`,`2`,`3`,… als Key-Hint.
- [ ] Ziffern wechseln den Tab (auch `7` für einen 7. Tab wie Stoat, der
      vorher keine Taste hatte). `0` = 10. Tab; ab dem 11. keine Ziffer.
- [ ] Eine nicht in der Konstellation genannte View ist verborgen (aber
      nicht entladen) — taucht wieder auf, sobald ihr Name ergänzt wird.
- [ ] `Tab` / `Shift+Tab` zykeln nur durch die sichtbaren Tabs.
- [ ] Nicht belegte Ziffer (mehr Ziffern als Tabs) tut nichts — springt
      **nicht** über eine alte feste `tab_*`-Bindung auf einen
      versteckten Tab.
- [ ] `tabs:`-Sektion entfernen → Legacy-Verhalten: alle Tabs nach
      `order`, feste Tasten `1`..`6`.
- [ ] Zwei Tabs mit gleichem `tab.name` → **harte Fehlermeldung** als
      Start-Modal; App fällt auf Legacy-Layout zurück, bleibt bedienbar.
- [ ] `:config` / `tui.yaml` editieren + Reload → Konstellation wird neu
      aufgelöst (aktiver Tab snappt auf den ersten sichtbaren, falls er
      rausfiel).

## Tab-Set-Popup (Laufzeit-Umschalten)

`Ctrl+X` (`tab_set_popup`) öffnet ein Popup mit allen Konstellationen.
Sets können `icon` + `shortcut` definieren (Voll-Form mit `tabs:`).

- [ ] `Ctrl+X` öffnet das Popup; jede Konstellation steht mit Icon und
      Shortcut-Hinweis in der Liste, die **aktive** ist markiert und
      vorausgewählt.
- [ ] Drücken eines Set-Shortcuts (z. B. `w`) wechselt sofort zu diesem
      Set, baut die Tab-Bar neu auf und schließt das Popup.
- [ ] Pfeiltasten + `Enter` wählen auch ein Set ohne Shortcut.
- [ ] `Esc` schließt ohne Wechsel.
- [ ] Eine nicht belegte Taste im Popup tut nichts (kein Durchschlagen
      auf Tab-Ziffern dahinter).
- [ ] Ohne konfigurierte `sets` → `Ctrl+X` zeigt nur eine Notification
      („No tab sets configured").
- [ ] Wechsel ist **session-only**: nach Neustart ist wieder das in
      `tui.yaml` gesetzte `active` aktiv.
- [ ] Mischung Kurz-/Voll-Form unter `sets:` parst fehlerfrei (Kurz-Form
      = ohne Icon/Shortcut, nur via Pfeil+Enter erreichbar).
- [ ] Shortcut mit mehr als einem Zeichen → Parse-Fehler (sichtbar als
      Config-Validierungsfehler).

## TaskAdapter (adapterisierter Tasks-Tab) — A1b + A1c-1 + A1c-2

Voraussetzung: `docs/examples/views/tasks.yaml` nach
`~/.config/not_yet_done/views/tasks.yaml` kopieren. Der Adapter-Tab läuft
**parallel** zum nativen Tasks-Tab (Vergleich), kein C1-Cutover.

- [ ] Tab lädt: Forest als Tree, Top-Level-Tasks sichtbar, Drill in
      Subtasks beliebig tief; `priority` rechtsbündig, `created` lokalisiert.
- [ ] `a` (add) am Root → Markdown-Buffer mit `## Description:` /
      `## Notes:`; Beschreibung eintragen, `:wq` → neuer Top-Level-Task
      erscheint, Cursor darauf.
- [ ] `a` mit `parent:`-Feld auf eine bestehende Task-UUID → Task landet
      als Subtask unter dem Parent.
- [ ] In einen Task gedrillt, `a` → neuer Task hängt als Subtask darunter
      (Buffer hat `parent:` vorbefüllt).
- [ ] `e` (edit) auf Task → Buffer zeigt aktuelle Felder + Notes;
      Beschreibung ändern, `:wq` → Zeile aktualisiert. Notes-Datei
      mitgeschrieben.
- [ ] `e` → Beschreibung leeren → `:wq` → Reopen mit Error-Banner
      (Description darf nicht leer sein).
- [ ] `e` → `status`/`priority` ändern → übernommen. Ungültiger `status`
      → Reopen mit Inline-Fehler.
- [ ] `e` → `tracking: true` → Tracking startet (im nativen Trackings-Tab
      sichtbar); bei `allow_parallel=false` werden andere aktive Trackings
      gestoppt. `tracking: false` → stoppt wieder.
- [ ] `d` (delete) auf Task mit Subtasks → Confirm → ganzer Teilbaum weg;
      Meldung „Deleted subtree (N tasks)". Notes soft-deleted.
- [ ] `u` (undelete) → zuletzt gelöschte(r) Task(s) zurück; ohne
      vorherige Löschung → „Nothing to undelete".
- [ ] `m` (mark-move) auf Task A, dann `p` (paste-move) auf Task B → A
      wird Subtask von B. „marked …"-Indikator währenddessen sichtbar.
- [ ] `m` auf A, `p` auf A selbst oder auf einen Nachfahren von A →
      Fehler (Zyklus abgelehnt), keine Änderung.
- [ ] Mutation in diesem Tab → nativer Tasks-Tab (falls offen) repaint/
      reload via DomainEvent.

### A1c-1 — Tracking-Marker-Spalte + Start/Stop-Taste

- [ ] `⏱`-Spalte zwischen Task und Status sichtbar. Tasks mit laufendem
      Tracking zeigen `⏱`, alle anderen leer.
- [ ] `t` (toggle-tracking) auf untracktem Task → `⏱` erscheint sofort
      (Reload); im nativen Trackings-Tab taucht das Tracking auf.
- [ ] `t` erneut auf demselben Task → `⏱` verschwindet, Tracking gestoppt.
- [ ] Bei `tracking.allow_parallel=false`: `t` auf Task B während A läuft →
      A's `⏱` verschwindet, B's erscheint (exklusiv, native Policy).
- [ ] Bei `tracking.allow_parallel=true`: `t` auf B lässt A's `⏱` stehen
      (beide laufen).
- [ ] Tracking via `e`-Buffer (`tracking: true`) gestartet → `⏱` erscheint
      ohne extra `t`; `t` togglet danach konsistent.
- [ ] `t` und der `tracking:`-Buffer-Toggle bleiben synchron (kein
      Stale-Marker): nach jedem Toggle spiegelt die Spalte den Live-Stand.

### A1c-2 — Saved Queries + FilterExpr-Filter (gefilterter Baum)

Voraussetzung: `tasks.yaml` mit dem `query:`-Block (Default `open tasks`:
nur nicht-`done`, nicht gelöscht). Mindestens ein `done`-Task tief im Baum
und ein offener Geschwister-Task anlegen.

- [ ] Tab lädt mit aktivem Default-Query: `done`-Tasks fehlen im Tree,
      offene Tasks da. Ein offener Task **unter** einem `done`-Parent bleibt
      sichtbar — der `done`-Parent erscheint als Vorfahr mit (nur dem
      passenden offenen Kind).
- [ ] Drill in einen gefilterten Knoten zeigt **nur** matchende Kinder
      (Filter greift auf jeder Tiefe, nicht nur an der Wurzel).
- [ ] `q` öffnet das Query-Menü: Default-Query `open tasks` gelistet.
- [ ] `:query new <name>` mit eigenem `FilterExpr`-Body (z. B.
      `[priority, ">=", 5]`) → speichern → erscheint im `q`-Menü; Apply
      filtert den Baum live.
- [ ] `:query edit <name>` → Body ändern, `:wq` → Baum re-filtert sofort
      (alter Subtree-Cache verworfen, keine Stale-Kinder).
- [ ] `:query delete <name>` → verschwindet aus dem Menü; Body-Datei unter
      `…/tasks/<id>/<view>/queries/<name>.yaml` weg.
- [ ] Query mit 0 Treffern → leerer Baum, Reload-Action (`r`) bleibt
      erreichbar (kein Dead-End).
- [ ] Query leeren / `default` droppen → ganzer Forest wieder sichtbar.
- [ ] Strukturelle Mutation (add/delete/reparent) → Baum re-snapshottet;
      Filter geht bis zum nächsten erneuten Query-Send verloren (akzeptierte
      Lifecycle-Kante, s. Plan-Box A1c-2).
- [ ] Saved-Query-Shortcut (Ctrl+f im `q`-Menü) auf eine Query → Taste
      filtert den Baum direkt; übersteht YAML-Reload (`query_shortcut`-Tabelle).

### A1c (scripts) — `:script` / `x` auf dem adapterisierten Tasks-Tab

Voraussetzung: `tasks.yaml` mit der `run script`-Action (Key `x`).

- [ ] `x` auf einem selektierten Task → Script-Menü öffnet, Verzeichnis
      `<data>/not_yet_done/scripts/tasks/task_item/` (auto-angelegt). Auch
      `:script` über die Cmdline öffnet dasselbe Menü.
- [ ] Auf jeder Drill-Tiefe (Subtask, Sub-Subtask) liefert `x` **dasselbe**
      Verzeichnis (View-Pfad stabil, ein gemeinsamer Scripts-Ordner).
- [ ] `+name<Enter>` legt ein neues Script aus dem Template an; Editor öffnet.
- [ ] Script ausführen → bekommt den Task als JSON
      `{"node": {"id": <uuid>, "label": <description>, "node_type":
"task:item", "tab": "tasks", "fields": {status/priority/tags/tracking/
created/…/ancestors}}}` (uniforme Node-Form, NICHT die native
      `{"task": …}`-Form). `fields.ancestors` ist ein JSON-Array-String
      `[{"id", "description"}, …]` Root→Parent (exklusive des Tasks selbst);
      bei einem Top-Level-Task `"[]"`.
- [ ] Selektion wechseln (anderer Task, anderer Typ-Mix) → Menü bleibt am
      selben Ordner (kein Shuffle).
- [ ] Kein Task selektiert / leerer Baum → Notification „No row selected",
      kein Crash.
- [ ] Portiertes `task_to_taiga.py` (unter `scripts/tasks/task_item/`) auf
      einem Ticket-Task (`#<n> - …` unter `<slug>/tickets/`) ausführen →
      Taiga-Tab aktiviert die Per-Project-Query und parkt den Cursor auf
      dem Item `<slug>#<n>` — identisch zum Verhalten auf dem nativen
      Tasks-Tab.

### A1c (Komfort) — Add-Child-unter-Selektion (`A`) + Un-nest (`U`)

- [ ] Im Tree-Mode einen Task selektieren, `A` → Editor-Buffer mit
      `parent:` auf den selektierten Task vorbefüllt; `:wq` → neuer Subtask
      hängt **unter** dem selektierten Task (nicht als Top-Level).
- [ ] `a` (klein) am selben Task → weiterhin Top-Level-Task am Container
      (Verhalten unverändert).
- [ ] `A` auf einem tief verschachtelten Task → Subtask landet auf der
      nächsten Tiefe darunter, Baum klappt zum neuen Knoten auf.
- [ ] `A`, dann im Buffer das `parent:`-Feld leeren → `:wq` → Task wird
      doch Top-Level (Buffer-Override gewinnt).
- [ ] `U` (Shift+U) auf einem verschachtelten Task → Task wandert auf die
      oberste Ebene (parent_id = None), erscheint als Top-Level-Knoten.
- [ ] `U` auf einem bereits Top-Level-Task → Notification „already at the
      top level", keine Änderung.
- [ ] `U` und `A` funktionieren im Tree-Mode (Root-View) und nach Drill in
      einen Task (rekursiver Branch).

## TrackingAdapter (adapterisierter Trackings-Tab) — A2a + A2b + A2c

Voraussetzung: `views/trackings.yaml` (aus `docs/examples/views/`) nach
`~/.config/not_yet_done/views/` kopiert. Der Adapter-Tab läuft neben dem
bespoke nativen Trackings-Tab (bis C1).

### A2a — Read-Path + Live-Dauern + Grouping

- [ ] Trackings-Tab (Adapter) öffnen → flache Liste, neueste zuerst; Spalten
      Marker (`⏱` nur bei laufenden), Path (gestylt `/a › b`), Task, Started,
      Ended (leer bei laufend), Duration (`H:MM:SS`, rechtsbündig).
- [ ] Auf der Tasks-Seite ein Tracking starten → die laufende Zeile zeigt
      `⏱`, leeres Ended, und die Duration **tickt adaptiv** (frisch: alle
      5 s; ab 1 min: 10 s; ab 10 min: 30 s; ab 1 h: 60 s — wie der native
      Tab; nur diese Zeile wird gepatcht, kein Voll-Reload-Flackern).
- [ ] Tracking stoppen → `⏱` weg, Ended gefüllt, Duration statisch; das
      Ticken stoppt (kein Dauer-CPU mehr).
- [ ] `zg` zykliert die Gruppierung (Day → Week → Month → Year → None) mit
      Pro-Gruppe-Summe + Footer-Gesamtsumme.
- [ ] `q` öffnet das Query-Menü; eine gespeicherte FilterExpr-Query
      (z. B. `description ~ "<wort>"`) filtert die Liste; löschen zeigt
      wieder alles.

### A2b — Mutationen

- [ ] `d` auf einer Zeile → Confirm-Dialog; bestätigen → „Tracking deleted",
      Zeile verschwindet (Zeiten bleiben in der DB erhalten).
- [ ] War die gelöschte Zeile **aktiv**, verschwindet auch der Tracking-Marker
      des zugehörigen Tasks auf dem Tasks-Tab (Cross-Tab via `TrackingChanged`).
- [ ] `t` auf einer Zeile → startet/stoppt Tracking auf dem **Task** der Zeile;
      bei deaktiviertem `allow_parallel_tracking` wird ein anderes laufendes
      Tracking zuerst gestoppt (gleiche Politik wie Tasks-Tab).
- [ ] `R` auf einer sichtbaren (nicht-gelöschten) Zeile → Notification
      „Restore failed: … not deleted" (bekannte Grenze: die Liste zeigt keine
      gelöschten Zeilen; Parität mit Native).
- [ ] `A` (restore-all) ohne gelöschte erreichbare Zeilen → Notification
      „No deleted trackings to restore".
- [ ] `x` öffnet das `:script`-Menü; ein Script gegen die selektierte Zeile
      bekommt deren JSON (`{json_file}`) übergeben.

### A2c — Condensed (verschachtelte Gruppierung M3 `then_by`)

- [ ] `v` schaltet auf den **Condensed**-Subtab; `a` schaltet zurück zur
      flachen Liste.
- [ ] Condensed zeigt pro **Tag** einen `── 2026-… ──`-Header mit Tages-Summe,
      darunter **je Task eine Zeile** mit Pfad, Task-Name und der summierten
      Dauer dieses Tasks **an diesem Tag**. Ein Task, der an zwei Tagen
      getrackt wurde, erscheint zweimal (einmal pro Tag).
- [ ] Zwei **verschiedene** Tasks mit gleichem Namen verschmelzen **nicht**
      (innere Gruppierung keyt auf `task_id`, nicht aufs Label).
- [ ] `zg` rotiert nur die **äußere** (Tag-)Ebene (Day→Week→Month→Year→None);
      die Pro-Task-Aufschlüsselung bleibt. Auf `None` → eine Zeile pro Task
      über den ganzen gefilterten Zeitraum.
- [ ] Eine Condensed-Zeile ist **selektierbar**; `d`/`t` wirken auf das
      repräsentative Tracking der Zeile (bekannte Grenze: Aktion trifft ein
      einzelnes Intervall, nicht die ganze Task-Tagessumme).
- [ ] Der Saved-Query-Filter (`q`) wirkt auch im Condensed-Subtab.

### A2c — Tree (own/cumulated, M4 `tree_aggregate`)

- [ ] `T` (Shift+t) schaltet auf den **Tree**-Subtab; `a` schaltet zurück zur
      flachen Liste. `t` (klein) bleibt toggle-tracking auf der Zeile — die
      beiden kollidieren **nicht**.
- [ ] Der Tree zeigt den **Task-Forest** (Tasks, nicht einzelne Intervalle);
      nur Tasks mit getrackter Zeit **irgendwo im Teilbaum** erscheinen
      (untracked Branches sind ausgeblendet, der Pfad zu getrackten Blättern
      bleibt sichtbar).
- [ ] Die `Duration`-Spalte zeigt zunächst die **kumulierte** Teilbaum-Summe
      (Default `cumulated`). `zt` schaltet alle `tree_aggregate`-Spalten auf
      die **eigene** Dauer des Tasks um (und zurück). (`zt` ist nur aktiv, weil
      der Trackings-Adapter `supports_tree_aggregation` meldet — das
      Capability-Gate. Ein `tree_aggregate:` in der YAML allein reicht nicht.)
- [ ] Ein Eltern-Task ohne eigenes Tracking, aber mit getrackten Kindern, zeigt
      cumulated > 0 (Eigenwert 0:00:00 nach `zt`).
- [ ] Drill-in (Enter/→) klappt die Subtasks auf; auf jeder Tiefe gilt die
      gleiche `tree_aggregate`-Spalte.
- [ ] `⏱`-Marker erscheint auf Tasks mit laufendem Tracking; `t` startet/stoppt
      Tracking auf dem selektierten Task (gemeinsame Exklusiv-Policy mit dem
      Tasks-Tab) und der Tree lädt neu.
- [ ] **Grenze:** kein Live-Tick im Tree (Dauern backen beim Load wie
      Condensed); ein `r`-Reload aktualisiert sie.

## Saved-Query-Shortcut-Validierung (Content-Tabs)

Saved-Query-Shortcuts claimen tab-weit auf der View-Claim-Ebene und würden
jede danach dispatchte Taste überschatten (Navigations-Keys, Chords, …).
Beide Prüfpfade testen:

- [ ] **Set-Time:** q-Menü öffnen, auf einer Query `ctrl+s` (Shortcut
      binden), dann `j` drücken → Modal „Shortcut 'j' is already taken by
      common.list_next!" und Re-Prompt; `v` → Konflikt mit dem Subtab-Key;
      `w` → Konflikt mit einem Window-Chord (Leader-Präfix); `z` →
      Konflikt mit `content.cycle_grouping` (Chord-Präfix); `d` →
      Konflikt mit dem YAML-`shortcuts:`-Eintrag. `esc` bricht ab.
- [ ] Ein freier Key (z. B. `M`) wird akzeptiert: „Favorite … added".
- [ ] **Load-Time:** eine kollidierende Row direkt in `query_shortcut`
      schreiben (oder eine Config-Änderung, die einen bestehenden
      Shortcut kollidieren lässt) → beim Start erscheint eine
      Notification „<Tab>: saved-query shortcut [x] ('name') shadows … —
      rebind it via the query menu"; der Shortcut bleibt aktiv.

## Column-Config (`c`) auf Content-Tabs

`c` öffnete früher auf jedem Nicht-Trackings-Tab das **native
Tasks**-Spalten-Popup (und hätte beim Anwenden dessen Settings
überschrieben). Jetzt generisch pro Level:

- [x] Adapter-Tab (z. B. „Trackings (A)"): `c` zeigt die Spalten der
      aktiven View (nicht die Tasks-Spalten); `Space` blendet eine
      Spalte aus (z. B. Taskpath) → Tabelle baut sofort ohne sie neu.
- [x] Persistenz: App neu starten → die Spalte bleibt ausgeblendet
      (Settings-Row `content_columns:<Tab>` als JSON-Map).
- [x] Reset: Spalte wieder aktivieren und per `Ctrl+D` an die
      YAML-Position schieben → Override entfernt, Settings-Row gelöscht
      (`SELECT key FROM settings WHERE key LIKE 'content_columns%'`
      ist leer).
- [x] Tree-Mode („Tasks (A)"): `c` zeigt die Spalten der Cursor-Ebene;
      die `tree_label`-Spalte (Task) ist fix (`Space` ohne Wirkung);
      andere Spalte (Created) togglen wirkt sofort + Reset wie oben.
- [x] Native Tabs (Tasks/Trackings): Popup unverändert (Display-Namen,
      Toggle, Persistenz in `tree_columns`/`tracking_columns`).
- [ ] Auto-Fallback-Level (Postgres-Rows): `c` → Notification „This
      level has no configurable columns" (Unit-Test vorhanden, live
      ungetestet — braucht verbundene Postgres-Instanz).

## Default-Query + Query-Menü-Styling

Das Query-Menü (`q`) teilt sich jetzt das Popup-Chrome mit dem
Column-Config-Popup (SearchablePopup rendert über `popup_utils`), und
`ctrl+t` markiert die selektierte Saved Query als Default, die beim
App-Start automatisch angewendet wird.

- [x] Optik: Query-Menü (Content + nativ), Script- und Tag-Menü zeigen
      das einheitliche Chrome (abgerundeter Rahmen, gewrappte
      Hint-Zeile, Cursor-Zeile hinterlegt statt Farbbalken);
      Saved-Query-Shortcuts erscheinen als `[key]`-Suffix.
- [x] Content-Tab („Trackings (A)"): `ctrl+t` auf „2 months" →
      Notification „Default query: 2 months", Settings-Row
      `default_query:<scope>` angelegt; App-Neustart → Query ist aktiv
      (Action-Bar zeigt sie), Menü zeigt `★ 2 months`.
- [x] Toggle-Off: `ctrl+t` auf der markierten Query → „Default query
      cleared", Settings-Row gelöscht.
- [x] Nativer Tasks-Tab: `ctrl+t` auf „Alle" → Neustart wendet „Alle"
      an, obwohl zuletzt „2 months" aktiv war (Default schlägt
      Last-Active-Restore); Toggle-Off stellt das alte Verhalten
      wieder her.
- [x] Postgres-Script-Menü: kein `default`-Hint, `ctrl+t` ohne Wirkung
      (Scripts sind keine Queries; via `open_without_default`).
- [ ] Default-Query mit Pflicht-Variablen (`{var}`): wird beim Start
      roh (ohne Variablen-Popup) angewendet — Verhalten dokumentiert,
      live ungetestet.

## Tree-Linien + Aufklappmarker konfigurierbar (`tree_lines` / `tree_markers`)

Pro Tree (Wurzel-`ViewDef`) sind die Box-Linien (`├──`/`└──`/`│`) und die
Aufklappmarker (`▶`/`▼`) getrennt konfigurierbar: `tree_lines: false` ersetzt
die Linien durch Einrückung (zwei Leerzeichen pro Tiefe), `tree_markers:`
überschreibt (`collapsed`/`expanded`) oder versteckt (`enabled: false`) die
Marker.

- [x] Postgres-Tab (`tree_lines: false` in der User-Config): Datenbank
      vier Ebenen tief aufklappen → Schema/Tabellen-Ebenen sind nur
      eingerückt, ohne `├──`/`└──`-Linien; die `▶`/`▼`-Marker
      erscheinen weiterhin.
- [x] Tab ohne Konfiguration („Tasks (A)"): unverändert Linien +
      Marker wie bisher (Default-Verhalten).
- [ ] `tree_markers.enabled: false` (temporär setzen): Linien bleiben,
      Marker verschwinden; Aufklappen per Enter funktioniert weiter
      (unit-getestet, live offen).
- [x] Connector-Farbe färbt bei `tree_lines: false` weiterhin den
      Marker-Lauf (im Capture: Marker in `tree_connector`-Farbe).

## Initiale Aufklapptiefe (`expand_depth`) + Listenansicht (`task:flat`)

Tasks-Adapter-Parität mit dem nativen Tab: `expand_depth: 2` auf dem
Wurzel-`ViewDef` klappt nach dem Laden Tiefe 0 und 1 automatisch auf
(One-Shot-Kaskade über den normalen Expand-Pfad, spiegelt
`tasks.tree.default_expand_depth: 2`); die zweite View `list`
(`node_type: task:flat`, Subtab-Key `v`, zurück `t`) zeigt den ganzen
Forest als flache Tabelle in DFS-Reihenfolge.

- [x] „Tasks (A)" öffnen: drei Ebenen sind direkt sichtbar (Wurzeln +
      Kinder + Enkel aufgeklappt), tiefere Ebenen bleiben zu.
- [x] Einen Knoten manuell zuklappen, dann `r` (Reload): der Knoten
      bleibt zu — die Kaskade ist one-shot und klappt nach Abschluss
      nichts mehr gegen den User auf.
- [x] `v` drücken: flache Liste aller Tasks (alle Tiefen, keine
      Marker/Einrückung, DFS-Reihenfolge); `t` wechselt zurück zum Tree,
      Aufklappstand bleibt erhalten.
- [x] In der Listenansicht: `e` öffnet die Edit-Session der selektierten
      Zeile wie im Tree (`s` toggle-tracking nutzt denselben
      invoke-Pfad; bewusst nicht live gedrückt — würde ein echtes
      Tracking starten/stoppen).
- [ ] Saved Query in der Listenansicht anwenden (`q`): nur die Treffer
      selbst erscheinen, keine Vorfahren-Zeilen (unit-getestet, live
      offen).

## Default-Query auf allen Trackings-Subtabs (`query.inherit_default`)

Der User-Default (★ im q-Menü) wird beim Start nur auf die Default-View
des Tabs gestempelt. `query.inherit_default: true` (condensed + tree in
trackings.yaml) stempelt ihn zusätzlich auf den jeweiligen Subtab; der
Tree filtert dabei adapter-seitig (Projektion wird aus den sichtbaren
Trackings neu gefaltet, `propagates_query_to_subtree`).

- [x] App mit ★-Default starten: Normal-, Condensed- UND Tree-Subtab
      zeigen den Default-Query-Namen als aktive Query in der Action-Bar
      (Grenze unverändert: ein Default mit `{var}`-Variable wird roh,
      d. h. effektiv ungefiltert, angewendet — wie auf der Default-View).
- [ ] Subtab ohne `inherit_default` (z. B. Tasks (A) Listenansicht):
      Default-Query greift dort weiterhin NICHT (Opt-in-Verhalten;
      unit-getestet, live offen).
- [x] Im Tree-Subtab `q` → Saved Query anwenden: Wurzel zeigt die
      gefilterte Summe; Expand der Äste bleibt gefiltert (nur Äste mit
      sichtbarer Zeit, identische Summen die Kette hoch bei
      Einzel-Ast-Treffern).
- [x] Nach Anwendung zurück zur flachen Liste (`a`): deren eigene Query
      unverändert (Pane-State bleibt getrennt; geteilt ist nur der
      Start-Default).

## Group-by-Menü (`u`) auf Content-Tabs (`content.group_menu`)

Direktsprung-Parität zum nativen Trackings-`u`: ein Hotkey-Popup über die
fünf `zg`-Zustände (No grouping/Day/Week/Month/Year). Nur aktiv, wenn die
Ebene ein `group_by:` konfiguriert; Wahl ist View-State (nicht
persistiert, wie `zg` — nativ persistierte via `SaveTrackingGrouping`).

- [x] Trackings (A), Normal-Subtab: Action-Bar zeigt `u group`; `u`
      öffnet das Popup „Group by" in der nativen Optik (Standard-Chrome,
      `●` markiert den aktuellen Zustand (Day), Hotkey-Buchstabe im Label
      unterstrichen, Keybinding-Legende unten).
- [x] `w` springt direkt auf Wochen-Gruppierung (Header `── W24 2026`),
      Summen pro Woche; `u` → `n` entfernt die Gruppierung (flache
      Liste; Aggregat-Spalte + Σ-Footer verschwinden, wie bei `zg` auf
      „ungruppiert").
- [x] Pfeile + Enter/Space wählen ebenfalls; Esc schließt ohne Änderung.
- [x] Condensed-Subtab: `u` → `m` rotiert nur die äußere Ebene auf
      Monat (`── 2026-06`), die innere `then_by`-Task-Ebene bleibt.
- [x] Auf einer Ebene ohne `group_by` (z. B. Tasks (A)): kein
      `u group`-Hint, `u` bleibt frei für YAML-`shortcuts:`.

## Trackings-Tree: immer ausgeklappt + ohne Marker (`expand_depth: all`)

Native Parität für den Tree-Subtab von Trackings (A): der Legacy-Tree war
immer komplett offen und hatte keine Aufklappmarker. `expand_depth: all`
(neuer Wert, Kaskade läuft bis nichts Aufklappbares übrig ist) +
`tree_markers.enabled: false` in trackings.yaml.

- [x] Trackings (A) → `t` (Tree): der gesamte Baum ist sofort komplett
      ausgeklappt — alle Ebenen sichtbar, ohne manuelles Enter.
- [x] Keine `▶`/`▼`-Marker vor den Zeilen; die Box-Connectors
      (`├──`/`└──`) bleiben.
- [x] Manuell einen Ast zuklappen (Enter), dann Subtab wechseln und
      zurück: Zustand bleibt — die Kaskade ist one-shot und klappt nichts
      gegen den User wieder auf.
- [x] Saved Query anwenden (`q`): der gefilterte Baum ist ebenfalls
      sofort voll ausgeklappt (neue Query re-armiert die Kaskade).
      Auch mit Cursor tief im Baum + vorherigem manuellen Auf-/Zuklappen
      (Regression: Out-of-Range-Cursor brach den Tabellen-Rebuild ab →
      stale Anzeige).

### Tiefe Bäume klappen vollständig auf (Kaskade bleibt scharf)

Bugfix: die `expand_depth: all`-Kaskade wird pro asynchron eintreffender
Kind-Ebene einmal gepumpt. Bei mehreren Geschwister-Ästen, die parallel
laden, konnte ein Ast „auslaufen" (ein Blatt landet) **während** ein anderer
noch in der Luft war — der Pump für das Blatt fand nichts mehr und
ent-schärfte die Kaskade voreilig. Folge: nur die obersten ein/zwei Ebenen
klappten auf, tiefere Äste blieben zu. Fix: die Kaskade ent­schärft erst,
wenn keine bereits-expandierte Ebene mehr auf ihre Kinder wartet.

- [ ] Trackings (A) → `t` (Tree) mit einem **mehrstufigen** Task-Baum
      (≥3 Ebenen, mehrere Geschwister mit unterschiedlich tiefen Ästen):
      der Baum ist nach dem Laden **komplett** offen bis zum letzten
      getrackten Blatt — nicht nur die obersten beiden Ebenen. Gleichviel
      sichtbar wie im nativen Trackings-Tab.
- [ ] Auch der gruppierte Tree (Tages-Buckets) klappt jeden Bucket-Teilbaum
      vollständig auf, nicht nur die erste Task-Ebene.

### `s` (toggle-tracking) aktualisiert die Ansicht sofort

Bugfix: `s` aktualisierte die TUI in den Trackings-Tabs (flach / condensed /
Tree) meist **nicht** sofort. Grund: der Toggle gab `Noop` zurück und
verließ sich auf Bridge-Row-Patches bzw. (im Tree) auf einen
`PatchRow`-Dispatch. Beide trafen die sichtbare Zeile oft nicht — ein
_Start_ erzeugt ein neues, noch unsichtbares Intervall (keine Zeile zum
Patchen), und `patch_row` durchsucht nur die Tiefe-0-Zeilen, sodass tiefere
Tree-Knoten gar nicht aktualisiert wurden. Die `Noop`/`PatchRow`-Lösung
existierte nur, weil ein voller `Reload` früher die O(N²)-Expand-Kaskade
auslöste (langsam, blockierte Eingabe).

Fix: Mit der Eager-Subtree-Verbesserung (`supports_eager_subtree`) erneuert
ein `Reload` den ganzen aufgeklappten Baum in **einem** `list_subtree`-Call.
Der Toggle gibt deshalb in allen drei Views schlicht `Reload` zurück
(identisch zur Tasks-(A)-Logik) — re-foldet Own/Cumulated, Vorfahren-Aggregate
und Marker konsistent. Der `PatchRow`-Dispatch entfällt ganz.

> Beim Smoke-Test **kein** echtes Tracking auf echten Zeilen togglen —
> eine Wegwerf-Aufgabe anlegen und auf der tracken.

- [ ] Trackings (A) → Tree, tiefer/voll aufgeklappter Baum: `s` auf einer
      **verschachtelten** Zeile flippt deren `⏱`-Marker **sofort** (an beim
      Start, weg beim Stopp); der Baum bleibt voll aufgeklappt, der Reload
      ist flott (kein sekundenlanges Zusammenklappen/Eingabe-Freeze), die
      Selektion bleibt auf der Zeile stehen. Kumulierte Sekunden der
      Vorfahren stimmen ohne extra `r`.
- [ ] Trackings (A) → flache Liste (`a`) und condensed (`v`): `s` auf einer
      laufenden Zeile stoppt sie (`⏱` weg, Dauer eingefroren); `s` auf einer
      gestoppten Zeile startet ein neues Intervall, das sofort sichtbar wird.
- [ ] Tasks (A) → Tree: `t` (toggle-tracking) flippt den `⏱`-Marker der
      Zeile sofort (unverändert — nutzte schon `Reload`).

## Trackings-Tree: Gruppierung via Adapter (`group_by_via_adapter`)

Native Parität, Punkt (3): der Legacy-Tree gruppierte nach Tag (ein
Gruppenkopf pro Tag, darunter der Task-Baum mit den Durations nur dieses
Tages). Generischer Mechanismus: Engine reicht das aktive `group_by` im
Root-`list()` durch, Adapter liefert `tracking:tree-group`-Bucket-Knoten
mit per-Bucket gefalteten Teilbäumen; `zg`/`u` = Reload.

- [ ] Trackings (A) → `t` (Tree): Tages-Gruppen als `── label`-Header-Zeilen
      (nicht selektierbar, Header-Style, Label wie in der gruppierten
      Flat-List: `W24 2026-06-08 Mon`), neuester Tag zuerst. Die Task-Zeilen
      darunter starten bei Einrückung 0 (keine Extra-Ebene unter dem
      Header). Voll aufgeklappt (expand_depth-Kaskade), Aufbau flott (kein
      sekundenlanger Aufbau — Folds + Query-Auflösung pro Snapshot
      memoisiert).
- [ ] Teilbaum unter einem Header: Durations sind die des jeweiligen
      Tages (derselbe Task unter zwei Tagen zeigt unterschiedliche
      Werte). Spalten wie nativ: `⏱`, Task, Own, Cumulated; zusätzlich
      schließt eine **Total**-Spalte jeden Tag auf seiner letzten Zeile
      (Stundenzettel-Layout). Cursor überspringt die Header-Zeilen.
- [ ] `zg` rotiert Day → Week → Month → Year → No grouping → Day; jeder
      Schritt lädt neu. „No grouping" zeigt den ungebucketeten Task-Baum
      ohne Header und ohne Total-Spalte (wie vor diesem Feature). `u`-Menü
      springt direkt, `●` markiert den aktiven Zustand.
- [ ] Saved Query (`q`) auf gruppiertem Tree: Buckets + Teilbäume
      re-falten aus den sichtbaren Trackings; leere Buckets verschwinden.
      Gruppierungszustand überlebt das Query-Apply.
- [ ] `s` (toggle-tracking) auf einer Task-Zeile im Bucket funktioniert;
      auf einer Bucket-Zeile ist `s` nicht belegt (read-only Aggregat).
      ⚠ im Smoke-Test nur auf einem Wegwerf-Task togglen.

### `s` im gruppierten Tree aktualisiert nur den Now-Bucket

Im **gruppierten** Tree (z. B. nach Tag) ist jeder Bucket ein eigenständig
aggregierter Teilbaum. Ein `s` (Start/Stopp) verschiebt nur die Totals des
Buckets, in den **„jetzt"** fällt — bei Tages-Gruppierung der heutige Tag,
generell der Bucket der gerade laufenden/zuletzt berührten Buchung. Statt
den ganzen Forst neu zu falten lädt das Frontend deshalb **nur diesen einen
Bucket** neu: Der Adapter sendet das payload-freie `Invalidation::NowAnchored`,
das Frontend fragt `bucket_for_now(spec)` (jüngstes Tracking → dessen Bucket),
holt Header + Teilbaum dieses Buckets und spleißt sie in-place ein; alle
anderen Buckets (inkl. deren Auf-/Zugeklappt-Zustand) bleiben unangetastet.
Ein _Start_, der den ersten Eintrag der Periode anlegt, erzeugt einen
brandneuen Bucket → das Frontend fällt dann auf einen vollen Pane-Reload
zurück (damit der neue Bucket in Sortier-Position erscheint).

> ⚠ im Smoke-Test nur auf einem Wegwerf-Task togglen, nie auf echten Zeilen.

- [ ] Trackings (A) → `t` (Tree), nach Tag gruppiert, mehrere Tage
      aufgeklappt: `s` auf einer Task-Zeile im **heutigen** Bucket flippt
      deren `⏱`-Marker und aktualisiert das Tages-Total dieses Buckets
      **sofort** — die **anderen** Tages-Buckets flackern nicht, klappen
      nicht zu und ihre Totals bleiben unverändert. Selektion bleibt stehen.
- [ ] Ein `s`, das die **erste** Buchung des heutigen Tages anlegt (vorher
      kein heutiger Bucket sichtbar): der neue Tages-Header erscheint in
      korrekter Sortier-Position (Fallback voller Reload), restliche Buckets
      bleiben aufgeklappt.
- [ ] „No grouping" (ungebucketeter Tree): `s` lädt wie gehabt den ganzen
      (einen) Baum neu — kein Now-Bucket-Spezialfall, keine Regression.

### Live-Tick im gruppierten Tree zählt nur den Now-Bucket hoch

Der **statische** Tree-Fold bäckt alle Dauern gegen den Snapshot-Zeitpunkt —
ein bloßes Neuladen desselben Snapshots tickt also _nicht_. Damit die Dauern
im gruppierten Tree live hochzählen, faltet der Adapter pro Timer-Tick **nur
den Now-Bucket** frisch gegen die aktuelle Uhrzeit: der neue Hook
`live_group_rows(spec, query)` liefert den Bucket-Header (Total neu aufsummiert)
plus die **laufende Kette** (laufende Task + ihre Vorfahren, deren kumulierte
Dauer mitwächst) als `Invalidation::Row`-Patches — nur Zeilen, die sich
tatsächlich bewegen, alle übrigen bleiben unberührt. Der Framework-Timer
feuert dabei nur noch ein payload-freies `LiveTick`; die Faltung passiert erst
im Frontend.

**Hintergrund-Tab-Verhalten (bewusst):** Ein Tick eines _nicht aktiven_ Tabs
hat **keine** Auswirkung auf den aktuellen Tab — er wird nicht neu gezeichnet.
Der Tick wird nur als Flag (`pending_live_refresh`) vermerkt und **erst beim
Zurückschalten** auf seinen Tab ausgewertet, und zwar **coalesced**: egal wie
viele Ticks in der Abwesenheit anfielen, beim Zurückschalten läuft genau eine
Faltung gegen den dann aktuellen Stand.

> ⚠ im Smoke-Test nur auf einem Wegwerf-Task togglen, nie auf echten Zeilen.

- [ ] Trackings (A) → `t` (Tree), nach Tag gruppiert: auf einem Wegwerf-Task
      `s` starten. Im **heutigen** Bucket zählen Task-Zeile, deren Vorfahren
      und das Tages-Total **sekündlich/live hoch** — die **anderen** Buckets
      stehen still, flackern nicht und klappen nicht zu. Selektion bleibt.
- [ ] Während die Buchung läuft, auf einen **anderen** Tab wechseln und ein
      paar Sekunden bleiben: der aktuelle Tab zeichnet **nicht** wegen des
      Trackings-Ticks neu. Zurückschalten → die Dauern springen **in einem
      Schritt** auf den jetzt korrekten Wert (kein Nachholen jedes einzelnen
      verpassten Ticks).
- [ ] Idle (keine laufende Buchung): es passieren **keine** Live-Patches —
      der Tree bleibt ruhig, kein unnötiges Neuzeichnen.

## Live-Frische Tasks/Trackings (A): Marker sofort, externe Starts, adaptives Ticken

Drei Frische-Fixes für die Adapter-Tabs: (1) ein Root-Reload erneuert jetzt
auch alle **aufgeklappten** Tree-Ebenen (vorher blieben deren gecachte
Children stehen → `⏱` erschien auf verschachtelten Tasks nicht sofort);
(2) neuer Trait-Hook `revalidate()` — beim Tab-Wechsel diffen Task-/
Tracking-Adapter die laufenden Trackings gegen die DB und laden bei Drift
neu (externe Starts/Stops via CLI/waybar); (3) die Live-Dauer tickt
adaptiv statt sekündlich (5 s → 10 s → 30 s → 60 s, native Parität).

- [ ] Tasks (A), Baum aufgeklappt: `s` auf einem **verschachtelten** Task
      → `⏱` erscheint sofort auf der Zeile (kein Zuklappen/Neuladen
      nötig); nochmal `s` → Marker sofort weg.
      ⚠ nur auf einem Wegwerf-Task togglen.
- [ ] Trackings (A) Flat-List: laufende Zeile tickt erst alle 5 s, nach
      einer Minute spürbar seltener (10 s-Sprünge); CPU bleibt ruhig.
      Nach Stop hört das Ticken auf.
- [ ] Extern ein Tracking starten (z. B. CLI `task track …` / waybar),
      während ein anderer Tab aktiv ist → auf Tasks (A) wechseln: `⏱`
      ist da; auf Trackings (A) wechseln: neue laufende Zeile da und
      tickt. Extern stoppen → Tab-Wechsel zeigt den Stop.
- [ ] `r` auf Tasks (A) bzw. Trackings (A) holt dieselbe externe Änderung
      manuell — auch im **Tree** mit aufgeklappten Ebenen (vorher blieb
      dort alter Stand stehen).
- [ ] Trackings (A) Tree/Condensed nach Toggle/Reload: Durations
      konsistent frisch (auch unter Gruppen-Headern).

## Eager-Subtree (`supports_eager_subtree`, `list_subtree`)

In-Memory-Adapter (Tasks (A), Trackings (A)) liefern bei `expand_depth: all`
bzw. `expand_depth: N` den ganzen erwarteten Teilbaum in **einem**
`list_subtree`-Call statt der per-Knoten-Kaskade. Resultat muss optisch
**identisch** zur Kaskade sein — gleiche Zeilen, gleiche Reihenfolge, gleiche
Aufklapp-Tiefe — nur ohne das ebenenweise Nachladen.

- [ ] Tasks (A) Tree-View (`expand_depth: all`): nach dem Laden ist der
      komplette Forest sofort offen — kein sichtbares Ebene-für-Ebene-
      Nachklappen. Tiefe ≥ 3 Ebenen testen (Task → Subtask → Sub-Subtask).
- [ ] Selektion/Collapse: Cursor auf einen tiefen Knoten, `zc`/Collapse
      und wieder aufklappen → Zustand stimmt (Pfad-Schema == Kaskade).
- [ ] Trackings (A) Tree-View: dito, voll aufgeklappt in einem Rutsch.
- [ ] `:tree-find "Tasks (A)" id:<uuid>` (z. B. via `goto_task`) landet
      weiterhin auf dem richtigen Knoten — der eager geladene Baum ist
      vollständig durchsuchbar.
- [ ] `r`-Reload auf dem eager Tree erneuert alle Ebenen (z. B. neu
      gestartetes Tracking zeigt `⏱` auf verschachteltem Task).
- [ ] Gegenprobe Remote (Postgres/Confluence-Tree, `supports_eager_subtree:
false`): klappt weiterhin **progressiv** auf (Ebene für Ebene), UI
      friert nicht ein — der eager Pfad greift dort bewusst nicht.

## Fuzzy-Filter — Teilstring-Highlight (Tasks/Trackings (A))

Parität zum nativen Tasks-Tab: der gematchte Teilstring wird hervorgehoben
(Theme-`accent`, fett). Im Tree-Mode im **Label** der `tree_label`-Spalte (der
Box-Connector behält seine `tree_connector`-Farbe), im Flat-Mode in den
durchsuchten Spalten.

- [ ] Tasks (A) Tree-View: `f` + Teil-Text eines Task-Titels tippen → in den
      verbleibenden Zeilen ist genau der getroffene Teilstring farbig/fett
      hervorgehoben; die Box-Connectors (`├──`/`└──`/`▶`) behalten ihre
      eigene Farbe.
- [ ] Mehrere Tokens (`foo bar`) → beide Treffer-Runs im selben Label sind
      hervorgehoben.
- [ ] Eine Zeile, die nur über ein anderes Feld (z. B. Tag) matcht, zeigt im
      Label **keine** Markierung (kein falsches Highlight).
- [ ] Filter leeren (`esc`) → Highlight verschwindet, Labels normal.
- [ ] Flat-View (`v`): `f` + Text → Treffer in den durchsuchten Spalten
      hervorgehoben; nicht durchsuchte Spalten bleiben unmarkiert.
- [ ] Sehr schmale Spalte / langes Label: Highlight bleibt korrekt geclamped
      (kein Panic, kein Übermalen des Connectors).

## Jump-Mode (`J`) auf Content-Tabs (`content.jump_mode`)

Parität zum nativen Tasks-Tab-Sprung (dort `p`), hier auf `Shift+J`.
Default-Binding ist `J`; konfigurierbar über `keybindings.content.jump_mode`.
Das Label-Alphabet kommt aus `navigation.jump_chars`.

- [ ] Tasks (A): `Shift+J` drücken → Sprung-Overlay aktiv (Action-Bar zeigt
      `J jump`). Ein Zeichen tippen, das in mehreren sichtbaren Zeilen
      vorkommt → jede Treffer-Zeile bekommt ein Label, Nicht-Treffer sind
      gedimmt.
- [ ] Label tippen → Cursor springt in die zugehörige Zeile.
- [ ] Zeichen, das nur in **einer** sichtbaren Zeile vorkommt → sofortiger
      Sprung ohne Label-Phase.
- [ ] Zeichen ohne Treffer → Overlay schließt sich, keine Auswahländerung.
- [ ] `esc` während des Overlays → Abbruch, Cursor unverändert.
- [ ] In einem Split: `Shift+J` wirkt nur auf das **fokussierte** Pane; nach
      Pane-Wechsel funktioniert der Sprung auch dort (neu erzeugtes Pane).
- [ ] Trackings (A) (Liste/Condensed/Tree): `Shift+J` verhält sich gleich.
- [ ] Nativer Tasks-Tab unverändert: `p` öffnet dort weiterhin den Sprung.

## Stoat: Ungelesen-Hervorhebung (`unread_style` / `unread_marker`)

Channels/Kategorien mit ungelesenen Nachrichten + ungelesene
Nachrichten-Header werden hervorgehoben (Marker-Glyph + Theme-Farbe
`unread`, beide per View überschreibbar). Quelle ist der Revolt-Read-State
(`sync/unreads` + Acks); Live-Reload bei jeder eintreffenden Nachricht.

Voraussetzung: Stoat-Tab geöffnet, Gateway `Ready`, in einem Server mit
mindestens einem Channel, der ungelesene Nachrichten enthält.

- [ ] Channel mit ungelesenen Nachrichten zeigt im Tree den Marker (Default
      💬) **vor** dem Channel-Namen, Name in der `unread`-Farbe (Default
      `#89b4fa`, fett).
- [ ] Die Kategorie, die einen solchen Channel enthält, ist ebenfalls
      markiert (OR über ihre Channels).
- [ ] In die Nachrichtenliste drillen: ungelesene Nachrichten haben einen
      hervorgehobenen Header (Autor/Zeit-Zeile) in derselben Farbe; gelesene
      Nachrichten normal.
- [ ] Marker-Breite stimmt: das Emoji (2 Zellen) verschiebt Einrückung/
      Folgespalten nicht, kein abgeschnittener Connector.
- [ ] Fuzzy-Filter (`/`) auf einem ungelesenen Channel: die Treffer-Runs
      bleiben in der Fuzzy-Match-Farbe (gewinnt über die Unread-Farbe), der
      Rest des Labels in der Unread-Farbe.
- [ ] In einem ungelesenen Channel eine Nachricht **senden** → Channel- und
      Kategorie-Marker verschwinden (Ack-on-send), ohne manuelles `r`.
- [ ] Eine neue Nachricht trifft in einem anderen Channel ein → dieser
      Channel + seine Kategorie werden live markiert (kein manuelles `r`).
- [ ] `unread_marker: ""` in der View gesetzt → kein Glyph, aber Name/Header
      weiterhin in der Unread-Farbe.
- [ ] `unread_style:` auf einen anderen Theme-Farbnamen gesetzt → Marker +
      Name/Header in dieser Farbe.

### Ack bei Cursor auf der neuesten Nachricht (`mark_read_on_reach_end`)

Der Channel-Marker verschwindet auch, wenn man den Cursor auf die unterste
(neueste) Nachricht der Liste bewegt — ohne zu senden und ohne manuelles `r`.
Konfiguriert über `mark_read_on_reach_end: mark-read` auf der Nachrichten-Ebene
(beide Branches in `stoat.yaml`: Channels im und außerhalb einer Kategorie).

- [ ] In einen ungelesenen Channel drillen, der **mehrere** ungelesene
      Nachrichten hat → die ungelesenen Header sind hervorgehoben, der Cursor
      steht oben/auf einer der oberen Zeilen; Channel-/Kategorie-Marker bleiben
      noch sichtbar (kein Auto-Ack beim Öffnen).
- [ ] Cursor mit `j`/Pfeil-runter bis auf die **unterste** (neueste) Zeile
      bewegen → Channel- und Kategorie-Marker im Tree verschwinden (Ack), die
      Header-Hervorhebung der Liste klingt nach dem Reload ab.
- [ ] Erneut am Listenende eine Taste drücken / wieder hoch- und runterfahren →
      kein erneutes Ack-Flackern (idempotent: Zeile ist nun gelesen).
- [ ] In einem bereits gelesenen Channel drillen und ans Ende fahren → keine
      Änderung (nichts zu acken).

## Refinements / Deferred Tasks

Punkte, die in Smoke-Tests aufkamen aber nicht zum jeweiligen Refactor
gehören. Werden in eigenen Sessions adressiert.

- Validator (keymap.rs) kennt die Autonummerierungs-Ziffern noch nicht;
  in Konstellations-Modus könnten feste `tab_*`-Bindings als
  Schein-Kollision auftauchen bzw. eine View-Ziffer-Bindung wird nicht
  als global geclaimt geführt. Niedrige Priorität (Ziffern als
  View-Action-Keys sind selten).
- Persistenz des Tab-Set-Wechsels: aktuell session-only (nicht zurück in
  `tui.yaml` geschrieben). Falls gewünscht, optionaler Write-back.

## Quellen

- Plan Content-Actions: [`plan-content-actions-unification.md`](plan-content-actions-unification.md)
- Plan EditSession-Refactor: [`plan-edit-session-refactor.md`](plan-edit-session-refactor.md)
