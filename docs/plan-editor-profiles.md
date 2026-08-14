# Plan: named editor profiles + per-action selection

## Goal

Today there is exactly **one** editor block (`editor:` in `tui.yaml`,
`EditorConfig`). We want to be able to define **several named profiles** and
pick per action which one is used. Use case: the Stoat chat `send`/`edit`
actions should open the editor in a **horizontal Kitty split at the bottom**
(across the full width), while everywhere else the existing vsplit editor
applies.

### Why (rationale for later readers)

- Different tasks want different editor geometries: a short chat compose fits
  better into a narrow split at the bottom, a longer ticket edit into a full
  vsplit.
- The editor is always an **external process** (your `$EDITOR` via Kitty); a
  pane cannot host it (no PTY embedding). The split is made by the **terminal**
  (Kitty), not by the TUI-internal pane system. That is why a "compose at the
  bottom" is necessarily **across the full width**, not just below the right
  pane — this is accepted.

### Deliberately NOT in this plan (follow-ups)

- 20:80 split on channel enter (pure config, separate).
- Up-front editability check (author == self) for Stoat edits.
- Named **script** profiles (`ScriptConfig` stays a single block).

## Decisions (agreed with the user)

1. **Selection per action** (`editor:` field on `ActionDef`).
2. Schema: top-level block `editors:` with a mandatory `default:` key plus any
   number of named profiles. The **old `editor:` key is dropped entirely** (no
   alias). Today's content becomes `editors.default`.
3. Unknown profile name → **hard error at config load** (validator).
4. Resolution: `action.editor` → otherwise `editors.default`. No further
   fallback levels (no view or adapter scope).

## Target schema

```yaml
editors:
  default: # ← mandatory; corresponds to the previous `editor:` block
    command: "kitty @ goto-layout splits; kitty @ launch --location=vsplit sh -c '{env}nvim {file}; mv {file} {file}.done'"
    inline: false
    pause_tui: true
  compose-below: # ← another profile
    command: "kitty @ launch --location=hsplit sh -c '{env}nvim {file}; mv {file} {file}.done'"
    inline: false
    pause_tui: true
```

```yaml
# in a view (e.g. stoat.yaml), per action:
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

If `editor:` is missing → `editors.default`. (The key `a`/`e` is orthogonal to
the profile; switching to `n` would be a one-line YAML change.)

## Phases

### Phase 0 — config schema `EditorsConfig`

`not-yet-done-tui/src/config/editor.rs`:

- New struct:
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
    `None`/`Some("default")` → `&self.default`; otherwise
    `named.get(name).unwrap_or(&self.default)` (the validator guarantees
    existence; `unwrap_or` is purely defensive).
  - `pub fn contains(&self, name: &str) -> bool` —
    `name == "default" || self.named.contains_key(name)`.
- `impl Default for EditorsConfig` → `{ default: EditorConfig::default(), named: HashMap::new() }`.
- Update the doc comment on `EditorConfig` (the example now shows
  `editors.default`, no longer a top-level `editor:`).

`not-yet-done-tui/src/config/tui_config.rs`:

- `pub editor: EditorConfig` → `pub editors: EditorsConfig` (line 53).
- `Default`: `editor: Default::default()` → `editors: Default::default()` (line 111).

### Phase 1 — thread the `editor:` field through actions

- `ActionDef` (in `config/view_config.rs`): `#[serde(default)] pub editor: Option<String>`.
- `ViewRequest::OpenContentEditor` **and** `CreateContentChild`
  (`views/mod.rs:153` / `:420`): add a field `editor_profile: Option<String>`.
- `content_view.rs::execute_action` (lines 3873 + 3932): set
  `editor_profile: action.editor.clone()` in both places.
- `app/mod.rs` (5143 + 5322): pass the profile into `NodeActionEditSession::new(...)`.
- `edit_session/node_action.rs`: store a field `editor_profile: Option<String>`;
  extend the constructor signature.
- `EditSession` trait (`edit_session/mod.rs`): default method
  `fn editor_profile(&self) -> Option<&str> { None }`; override it in
  `NodeActionEditSession`. All other sessions (task, tracking, `:config`, query,
  DB script) inherit `None` → the `default` profile.

### Phase 2 — resolve the profile on open and reopen

- `app/editor.rs::open_session` (169): instead of `&self.config.editor` →
  `let editor = self.config.editors.resolve(session.editor_profile());`
  (resolved while `session` is still borrowed, before the move into App).
- `main.rs::reopen_editor_with_errors`: resolve the same profile via
  `pending_session.editor_profile()` (`pending_session` is available there).
- `editor.rs:252` (`.indent`, the task restructure editor): uses
  `editors.default` — this action has no per-action profile.
- No stored `active_editor` needed: each of the three sites resolves directly.
  **Finding:** `busy_timeout_secs` (editor) was never consumed anywhere in the
  logic → dead field, removed without replacement (the script config keeps its
  own).

### Phase 3 — validation (hard error)

- In the config validator (`config/view_config.rs`, the existing `validate`
  path): walk all `ViewDef`/`ChildDef` actions; for each one with
  `editor: Some(name)` check `config.editors.contains(name)`.
- On an unknown profile: **hard error** naming the view and action plus the list
  of available profiles. Extend the validator signature to give it access to
  `EditorsConfig` if needed.

### Phase 4 — migrate configs + apply the profile

- `~/.config/not_yet_done/tui.yaml`: `editor:` → `editors: { default: {…} }`
  - profile `compose-below` (Kitty `--location=hsplit`).
- Example config in the docs (under `docs/examples/`): same migration.
- `docs/examples/views/stoat.yaml` **and** `~/.config/.../views/stoat.yaml`:
  `editor: compose-below` on the `send` and `edit` actions in **both**
  `messages` blocks (uncategorized and under a category).
- README + the `docs/` reference: document the `editors:` schema, profiles and
  the per-action `editor:` field — including the **why** (different geometries,
  external process/no PTY → split via Kitty, compose at the bottom = full
  width).
- `docs/generic-view-spec.md`: document the new action field `editor:`.

### Phase 5 — tests, build, doc polish

- Unit tests:
  - `EditorsConfig` deserialize (`default` only; `default` + named).
  - `resolve()`: `None` → default; `Some("default")` → default; known name →
    profile; unknown name → default (defensive).
  - `contains()`.
  - The validator rejects an action with an unknown `editor:`.
  - `ActionDef` parses the `editor:` field.
- `cargo build --release`, `cargo test`, `cargo install --path not-yet-done-tui --force`.
- `npx prettier --write` on the changed Markdown files.
- Privacy sweep of the diff (no real domain or credentials).

## Affected files (overview)

| File                                     | Change                                      |
| ---------------------------------------- | ------------------------------------------- |
| `config/editor.rs`                       | `EditorsConfig` + `resolve`/`contains`      |
| `config/tui_config.rs`                   | field `editor` → `editors`                  |
| `config/view_config.rs`                  | `ActionDef.editor` + validator              |
| `views/mod.rs`                           | 2 `ViewRequest` variants + `editor_profile` |
| `views/content_view.rs`                  | `execute_action` passes the profile through |
| `app/mod.rs`                             | 2 session constructions                     |
| `app/editor.rs`                          | `open_session` resolves, `active_editor`    |
| `edit_session/mod.rs` + `node_action.rs` | trait method + field                        |
| `main.rs`                                | reopen path uses the resolved profile       |
| `tui.yaml` (user + example)              | `editors:` migration + `compose-below`      |
| `stoat.yaml` (example + deployed)        | `editor: compose-below` on send/edit        |
| `README.md`, `docs/*`                    | document the schema + the rationale         |
