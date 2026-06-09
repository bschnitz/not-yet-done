# 0001 — Render-Loop: Dirty-Gating statt 60-fps-Dauer-Redraw

- **Status:** akzeptiert, umgesetzt (Variante 1a → **1b**)
- **Datum:** 2026-06-03 (1a), 2026-06-04 (1b)
- **Betrifft:** `not-yet-done-tui` — `main.rs::run_loop`, `App::poll_*`/`tick_*`

## Kontext

Die TUI verbrauchte im Leerlauf spürbar CPU. Ursache: der Render-Loop
pollte mit **16 ms** Timeout (`events::poll_event`) und rief **pro
Iteration bedingungslos** `App::sync_components()` + `terminal.draw()` auf.

`sync_components()` ist nicht billig — es rebuildet die aktive
Content-Tabelle (`rebuild_table`), ggf. die Task-Tabelle, sammelt die
Subtab-Labels **aller** Content-Views in frische `Vec<(String, bool)>`
und baut die Status-Bar-Hints neu (weitere String-Allokationen). Das lief
~60×/s, auch wenn keine Taste gedrückt wurde und keine Hintergrund-Message
ankam — also Dauer-Last ohne sichtbare Änderung.

Wichtige Eigenschaft der Codebasis, die die Lösung entschärft:
Redraw-Bedarf hängt **nicht** an tiefen Komponenten, sondern an den
wenigen Eintrittspunkten des Loops. Sichtbare Änderungen entstehen nur
durch (1) einen Tastendruck, (2) eine eintreffende async-`LoadMsg`,
(3) einen Timer (Busy-Banner-Sekunde, aktive-Tracking-Dauer), (4) eine
Editor-/Script-Rückkehr.

## Optionen

1. **Globales `redraw`-Flag**, das beliebige Komponenten setzen.
   Verworfen: verteilt mutablen Zustand quer durch den Baum; jede neue
   Stelle, die etwas ändert, muss ans Flag denken; schlecht zu testen.
2. **1a — Bubble per Rückgabewert (gewählt).** Die Änderungs-Quellen
   (`poll_load`, `tick_active_trackings`, `tick_animations`,
   `poll_live_editor`, `poll_editor_close`, `poll_commit_result`,
   `poll_detached_script`) geben `bool`/`Option` zurück; der Loop ODER-t
   sie zu einem lokalen `dirty` und ruft `sync_components()`+`draw()` nur
   bei `dirty`. Tiefe Komponenten bleiben unberührt. Kein globaler State.
3. **1b — voll event-getriebener `select!`-Loop.** Ein `tokio::select!`
   über crossterm-`EventStream`, `load_rx`, `commit_rx` und einen 1-Hz-
   `interval` (nur armiert, solange ein Banner/Tracking lebt). Eintreffen
   einer Message _ist_ das Redraw-Signal; Idle = geparkt = 0 % CPU.

## Entscheidung

Zuerst **1a**. Es entfernt den Großteil der Idle-CPU (kein
`sync`/`draw` mehr ohne Änderung), ist niedriges Risiko und fasst weder
die Terminal-Event-Mechanik noch das Kitty-Protokoll- bzw.
Editor-Suspend/Restore-Handling an. Der Poll-Timeout steigt von 16 ms auf
**200 ms**: `event::poll` kehrt sofort bei Tastendruck zurück (also keine
Eingabe-Latenz), der Timeout begrenzt nur Idle-Wakeups; eine Idle-Wakeup,
die nichts findet, macht keine `sync`/`draw`-Arbeit. Async-Messages
erscheinen mit ≤ 200 ms Latenz — unmerklich.

Die einzige Zeit-getriebene Sonderbehandlung ist das Busy-Banner: sein
Sekundenzähler kommt aus `SystemTime::now()` zur Render-Zeit, also würde
er ohne Anstoß zwischen Events einfrieren. `App::tick_animations()` gibt
darum ~1×/s `true` zurück, solange `has_live_banner()` (= irgendeine
Content-View im `AdapterStatus::Busy`) gilt. `Connecting`/`Failed`/Retry
sind statischer Text und brauchen das nicht. Aktive-Tracking-Dauerzellen
laufen über `tick_active_trackings` auf dessen adaptivem Intervall.

## Konsequenzen

- **Positiv:** Idle-CPU sinkt drastisch (kein Tabellen-Rebuild/Repaint
  60×/s mehr). Kein globaler Redraw-Zustand; die Information fließt als
  Rückgabewert nach oben, lokal im Loop aggregiert — gut testbar
  (`is_busy_tracks_adapter_status`).
- **Negativ / offen:** Es bleibt ein Poll-Loop — im Leerlauf ~5 triviale
  Wakeups/s (kein Draw), nicht echte 0 %. Async-Display-Latenz ≤ 200 ms.
- **Folge-Schritt 1b — umgesetzt (2026-06-04):** `run_loop` ist jetzt ein
  `tokio::select!` über crossterm-`EventStream`, `load_rx`, `commit_rx` und
  einen bedingten 200-ms-`interval`. Idle = im `select!` geparkt; eine
  eintreffende `LoadMsg`/`CommitMsg` weckt den Loop sofort (kein 200-ms-
  Deckel mehr), was zugleich die Vorbedingung für den geplanten
  Adapter-Invalidation-Push (Streaming-Adapter, siehe Stoat-Plan) ist. 1a
  war eine echte Teilmenge — kein Wegwerf-Code; das Dirty-Gating bleibt.

## 1b — wie die Heikel-Punkte gelöst wurden

- **`EventStream` ↔ Editor-/Script-Suspend (Haupt­risiko).** crossterm
  0.29s `EventStream` startet einen Hintergrund-Thread, der **nur dann**
  blockierend auf stdin liest, wenn gerade kein Event anliegt; sein `Drop`
  weckt diesen Thread über den internen Waker, ohne ein stdin-Byte zu
  konsumieren. Wir nutzen das: vor **jedem** Suspend-Punkt (Inline-/
  Launch-Editor, interaktives Script, `Reopen` nach Validierungsfehler)
  wird der Reader gedroppt und danach via `EventStream::new()` neu
  erzeugt. Damit besitzt das Kindprozess-stdin allein. Das
  Kitty-Protokoll-Enable/Disable bleibt unverändert in den Editor-Dispatch-
  Funktionen (reine stdout-Writes, kein Konflikt mit dem Reader).
- **Poll-basierte Quellen ohne Waker.** `poll_live_editor`,
  `poll_editor_close`, `poll_detached_script`, `tick_animations`
  (Busy-Banner-Sekunde) und `tick_active_trackings` (Tracking-Dauer) haben
  keinen Channel. Sie laufen im `interval`-Zweig, der per
  `App::needs_periodic_tick()` **nur armiert** ist, solange eine dieser
  Quellen lebt (Editor/Script pending, Busy-Banner aktiv, aktives
  Tracking). Lebt nichts Zeitgetriebenes, ist der Zweig deaktiviert und der
  Loop parkt rein auf Events/Channels → echte ~0-%-Idle.
- **`poll_load`/`poll_commit_result` aufgeteilt.** `recv()` im `select!`
  konsumiert genau eine Nachricht; diese wird über die neu extrahierten
  `App::handle_load_msg` / `App::handle_commit_msg` verarbeitet, danach
  drainen wir den Rest (`poll_load` mit `try_recv`). So geht keine
  vorab-`recv()`-te Nachricht verloren.
- **Resize.** Terminal-Resize-Events markieren jetzt explizit `dirty`
  (vorher implizit über den 200-ms-Repaint abgedeckt).

## Konsequenzen 1b

- **Positiv:** Idle ist jetzt wirklich geparkt (kein periodisches Wakeup
  mehr, solange nichts Zeitgetriebenes pending ist). Async-Anzeige­latenz
  ohne 200-ms-Deckel. Saubere Basis für out-of-band Push (Adapter-
  Invalidation).
- **Negativ / offen:** crossterm-`EventStream`-Reader-Thread wird pro
  Suspend-Zyklus neu erzeugt (vernachlässigbar, da nur bei
  Editor-/Script-Aufrufen). Sehr enges Race-Fenster: ein exakt zwischen
  „Editor öffnen" und abgeschlossenem `drop(reader)` gedrückter Tastendruck
  könnte vom alten Reader verschluckt werden — praktisch irrelevant, da der
  Editor erst danach sichtbar wird (gleiche Klasse wie die bisherigen
  Mode-Switch-Races).
