# TODO

## 1. Misc

- [x] ctrl+j/ctrl+k für ab/auf in allen listen/tree (Default: Pfeiltasten, konfigurierbar)
- [x] Spacing von 2 Zeichen zwischen den Tabellenspalten in Tree und Liste
- [x] Tracking auf "A new subitem" flackert zwischen beiden hin und her
- [x] Priority Spalte per default zwischen Status und Tracking
- [x] Multi-Choice mit Ordering Funktion erweitern (Ctrl+Up/Down, show_order mit Padding)
- [x] Spaltenselect und Ordering mit Multi-Choice bestimmbar (Als Setting speichern)
      (c-Taste, Popup mit Checkbox+Ordering, DB-Persistenz)
- [x] delete ist keine Form, sondern ein binding, was das aktuelle element
      deleted (falls node: man muss mit 'yes' bestätigen) + undelete mit u
- [x] Highlighting von add, edit, edit node geht nicht sofort weg, wenn man den
      editor schließt (sync_components vor jedem draw)
- [x] Bars / Keybindings aufräumen. Oben sollten nur Elemente stehen, bei denen
      man bei Aktivierung etwas in der Bar sehen kann:
      - Fuzzy Filter
      - search
      - add
      - edit
      - edit node
      - track => highlighten wenn mindestens ein Tracking läuft
- [x] Wenn die Elemente nicht mehr in die Bar passen, weil das Terminal zu klein
      ist, dann sollte es einen automatischen Umbruch geben. Die Komponente
      sollte hier wissen, wieviel Zeilen sie benötigt und der Parent sollte das
      abfragen. (required_height + dynamisches Layout)
- [x] Bug: Wenn man im Edit mode ein item editiert, dann wird der Tree richtig
      geupdated. Aber die Cursor Position nicht. (pending_focus_id nach Reload)

## 2. Tracking-View

- [x] Liste aller Trackings, search und fuzzy filter Funktionen wie bei den tasks
- [ ] Summary option ähnlich wie timewarrior
- [ ] Öffnen von Trackings und Summary mit edit mode ähnlich wie in der Tree
      ansicht, der es erlaubt Trackings zu editieren und zu verschieben
- [ ] Speichern von Trackings und Summary
- [ ] Plugins, die es erlauben Operationen auszuführen und custom summaries zu
      erstellen

## 3. Post Tracking-View

- [ ] Shortcut t (konfigurierbar) zeigt alle trackings zu einer Node + Childs im
      Tree, bzw. zu einem Task in der Liste

## 4. Minor

- [x] Speichern in edit aktualisiert direkt die Liste (live-reload bei :w)
- [x] Speichern in add aktualisiert direkt die Liste (CreateTask → EditTask
      Konvertierung nach erstem :w)
