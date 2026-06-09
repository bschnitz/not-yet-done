# Plan: Benannte Editor-Profile + Auswahl pro Action

## Ziel

Heute gibt es genau **einen** Editor-Block (`editor:` in `tui.yaml`,
`EditorConfig`). Wir wollen **mehrere benannte Profile** definieren können
und pro Action wählen, welches verwendet wird. Anwendungsfall: Stoat-Chat
`send`/`edit` sollen den Editor in einem **horizontalen Kitty-Split unten**
(über die ganze Breite) öffnen, während überall sonst der bisherige
vsplit-Editor greift.

### Warum (Begründung für spätere Leser)

- Verschiedene Aufgaben wollen verschiedene Editor-Geometrien: ein kurzer
  Chat-Compose passt besser in einen schmalen Split unten, ein längeres
  Ticket-Edit in einen vollen vsplit.
- Der Editor ist immer ein **Fremdprozess** (dein `$EDITOR` via Kitty);
  ein Pane kann ihn nicht aufnehmen (kein PTY-Embedding). Der Split wird
  vom **Terminal** (Kitty) gemacht, nicht vom TUI-internen Pane-System.
  Deshalb ist ein „Compose unten" zwangsläufig **über die ganze Breite**,
  nicht nur unter dem rechten Pane — das ist akzeptiert.

### Bewusst NICHT in diesem Plan (Follow-ups)

- 20:80-Split beim Channel-Enter (rein Config, separat).
- Edit-Editierbarkeits-Vorabprüfung (Author == self) für Stoat.
- Benannte **Script**-Profile (`ScriptConfig` bleibt Einzel-Block).

## Entscheidungen (mit dem User abgestimmt)

1. **Auswahl pro Action** (`editor:`-Feld am `ActionDef`).
2. Schema: Top-Level-Block `editors:` mit Pflichtschlüssel `default:` +
   beliebig vielen benannten Profilen. Der **alte `editor:`-Key entfällt
   komplett** (kein Alias). Der heutige Inhalt wird zu `editors.default`.
3. Unbekannter Profilname → **harter Fehler beim Config-Laden** (Validator).
4. Auflösung: `action.editor` → sonst `editors.default`. Keine weiteren
   Fallback-Ebenen (kein View-/Adapter-Scope).

## Ziel-Schema

```yaml
editors:
  default: # ← Pflicht; entspricht dem bisherigen `editor:`-Block
    command: "kitty @ goto-layout splits; kitty @ launch --location=vsplit sh -c '{env}nvim {file}; mv {file} {file}.done'"
    inline: false
    pause_tui: true
  compose-below: # ← weiteres Profil
    command: "kitty @ launch --location=hsplit sh -c '{env}nvim {file}; mv {file} {file}.done'"
    inline: false
    pause_tui: true
```

```yaml
# in einer View (z. B. stoat.yaml), pro Action:
actions:
  - {
      name: send,
      key: a,
      type: create,
      id: send_message,
      editor: compose-below,
    }
  - { name: edit, key: e, type: edit, id: edit_message, editor: compose-below }
```

Fehlt `editor:` → `editors.default`. (Der Key `a`/`e` ist orthogonal zum
Profil; ein Wechsel auf `n` wäre eine Ein-Zeilen-YAML-Änderung.)

## Phasen

### Phase 0 — Config-Schema `EditorsConfig`

`not-yet-done-tui/src/config/editor.rs`:

- Neue Struct:
  ```rust
  #[derive(Debug, Clone, Deserialize)]
  pub struct EditorsConfig {
      pub default: EditorConfig,
      #[serde(flatten)]
      pub named: std::collections::HashMap<String, EditorConfig>,
  }
  ```
- `impl EditorsConfig`:
  - `pub fn resolve(&self, profile: Option<&str>) -> &EditorConfig` —
    `None`/`Some("default")` → `&self.default`; sonst
    `named.get(name).unwrap_or(&self.default)` (Validator garantiert
    Existenz; `unwrap_or` ist nur defensiv).
  - `pub fn contains(&self, name: &str) -> bool` —
    `name == "default" || self.named.contains_key(name)`.
- `impl Default for EditorsConfig` → `{ default: EditorConfig::default(), named: HashMap::new() }`.
- Doc-Kommentar von `EditorConfig` aktualisieren (Beispiel zeigt jetzt
  `editors.default`, nicht mehr Top-Level `editor:`).

`not-yet-done-tui/src/config/tui_config.rs`:

- `pub editor: EditorConfig` → `pub editors: EditorsConfig` (Zeile 53).
- `Default`: `editor: Default::default()` → `editors: Default::default()` (Zeile 111).

### Phase 1 — `editor:`-Feld auf Actions durchreichen

- `ActionDef` (in `config/view_config.rs`): `#[serde(default)] pub editor: Option<String>`.
- `ViewRequest::OpenContentEditor` **und** `CreateContentChild`
  (`views/mod.rs:153` / `:420`): Feld `editor_profile: Option<String>` ergänzen.
- `content_view.rs::execute_action` (Zeilen 3873 + 3932): jeweils
  `editor_profile: action.editor.clone()` setzen.
- `app/mod.rs` (5143 + 5322): Profil in `NodeActionEditSession::new(...)` reichen.
- `edit_session/node_action.rs`: Feld `editor_profile: Option<String>`
  speichern; Konstruktor-Signatur erweitern.
- `EditSession`-Trait (`edit_session/mod.rs`): Default-Methode
  `fn editor_profile(&self) -> Option<&str> { None }`; in
  `NodeActionEditSession` überschreiben. Alle anderen Sessions (Task,
  Tracking, `:config`, Query, DB-Script) erben `None` → `default`-Profil.

### Phase 2 — Profil bei Open + Reopen auflösen

- `app/editor.rs::open_session` (169): statt `&self.config.editor` →
  `let editor = self.config.editors.resolve(session.editor_profile());`
  (aufgelöst, solange `session` noch geborgt ist, vor dem Move in App).
- `main.rs::reopen_editor_with_errors`: dasselbe Profil über
  `pending_session.editor_profile()` auflösen (`pending_session` ist da).
- `editor.rs:252` (`.indent`, Task-Restructure-Editor): nutzt
  `editors.default` — diese Action hat kein per-Action-Profil.
- Kein gespeichertes `active_editor` nötig: jede der drei Stellen löst
  direkt auf. **Befund:** `busy_timeout_secs` (Editor) wurde nirgends in
  der Logik konsumiert → totes Feld, ersatzlos entfernt (Script-Config
  behält ihr eigenes).

### Phase 3 — Validierung (harter Fehler)

- Im Config-Validator (`config/view_config.rs`, bestehender `validate`-Pfad):
  über alle `ViewDef`/`ChildDef`-Actions laufen; für jede mit
  `editor: Some(name)` prüfen `config.editors.contains(name)`.
- Bei unbekanntem Profil: **harter Fehler** mit View-/Action-Name und der
  Liste verfügbarer Profile. Validator-Signatur ggf. um Zugriff auf
  `EditorsConfig` erweitern.

### Phase 4 — Configs migrieren + Profil anwenden

- `~/.config/not_yet_done/tui.yaml`: `editor:` → `editors: { default: {…} }`
  - Profil `compose-below` (Kitty `--location=hsplit`).
- Doc-Beispiel-Config (unter `docs/examples/`): gleiche Migration.
- `docs/examples/views/stoat.yaml` **und** `~/.config/.../views/stoat.yaml`:
  `editor: compose-below` an die `send`- und `edit`-Action in **beiden**
  `messages`-Blöcken (uncategorized + unter Kategorie).
- README + `docs/`-Referenz: `editors:`-Schema, Profile, das per-Action
  `editor:`-Feld dokumentieren — inkl. **Warum** (verschiedene Geometrien,
  Fremdprozess/kein PTY → Split via Kitty, Compose unten = volle Breite).
- `docs/generic-view-spec.md`: neues Action-Feld `editor:` dokumentieren.

### Phase 5 — Tests, Build, Doku-Politur

- Unit-Tests:
  - `EditorsConfig` Deserialize (nur `default`; `default` + benannte).
  - `resolve()`: `None` → default; `Some("default")` → default;
    bekannter Name → Profil; unbekannter Name → default (defensiv).
  - `contains()`.
  - Validator lehnt Action mit unbekanntem `editor:` ab.
  - `ActionDef` parst `editor:`-Feld.
- `cargo build --release`, `cargo test`, `cargo install --path not-yet-done-tui --force`.
- `npx prettier --write` auf geänderte Markdown-Dateien.
- Privacy-Sweep des Diffs (keine echte Domain/Credentials).

## Betroffene Dateien (Überblick)

| Datei                                    | Änderung                                     |
| ---------------------------------------- | -------------------------------------------- |
| `config/editor.rs`                       | `EditorsConfig` + `resolve`/`contains`       |
| `config/tui_config.rs`                   | Feld `editor` → `editors`                    |
| `config/view_config.rs`                  | `ActionDef.editor` + Validator               |
| `views/mod.rs`                           | 2 `ViewRequest`-Varianten + `editor_profile` |
| `views/content_view.rs`                  | `execute_action` reicht Profil durch         |
| `app/mod.rs`                             | 2 Session-Konstruktionen                     |
| `app/editor.rs`                          | `open_session` löst auf, `active_editor`     |
| `edit_session/mod.rs` + `node_action.rs` | Trait-Methode + Feld                         |
| `main.rs`                                | Reopen-Pfad auf aufgelöstes Profil           |
| `tui.yaml` (User + Beispiel)             | `editors:`-Migration + `compose-below`       |
| `stoat.yaml` (Beispiel + deployed)       | `editor: compose-below` an send/edit         |
| `README.md`, `docs/*`                    | Schema + Begründung dokumentieren            |
