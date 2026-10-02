# Plan: editor templates for create actions

Status: planned, not started.

## Goal

A create action (`type: create`, an `InputSpec::Editor` adapter action such as
the tasks adapter's `add`) can start from one of several **stored templates**
instead of the adapter's single built-in buffer. Templates are picked from a
menu — the shared query menu, a third mode beside saved queries and node
scripts — and each template can get its own chord there (ctrl+s), stored in
`query_shortcut` like query and script chords.

First user: tasks. `a t` opens the template menu for the same flow as `a a`
(create under the selection, `inherit: true`); the template `todo` pre-sets the
tag `todo` and is bound to `a o`. deadlines-widget' deadline corner then shows open tasks
that have a deadline **or** the `todo` tag (union).

## Decisions (agreed with the user, 2026-10-02)

1. Templates belong to an **adapter instance and an action**: one namespace per
   `(instance, action_id)`, because an adapter can have several editor actions
   (`add`, `add-sibling`, …) whose buffers differ.
2. A template is a **complete buffer** in the adapter's own format. The adapter
   merges it into the context (e.g. tasks sets `parent:` from the selection;
   a `parent:` written in a template is ignored).
3. **Create only.** Editing an existing node keeps its current buffer; a
   template there would overwrite content.
4. `a t` behaves exactly like `a a` (same YAML create action, same parent
   resolution, `inherit`, `commit_on_save`), only the starting buffer differs.
5. deadlines-widget (separate repo) filters `due set OR tag todo`.

## Design

### Protocol (`not-yet-done-content/src/lib.rs`)

- `NodeAction` gains `templates: bool` + builder `.templated()`: "this editor
  action accepts a stored template as its starting buffer". The TUI shows no
  template menu for an action without it (capability, not guesswork).
- `Node::prepare_templated(action_id, args, template: &str) -> Result<EditorPrep>`,
  default `NotSupported`. The adapter returns the merged buffer; the frontend
  keeps passing `prep.template` back as `original`, so the existing
  "unchanged buffer = aborted" rule (`parse_new_task`, editor_templates.rs:155)
  still means "saved without typing anything → nothing created".
- `TemplateStore` trait (list/load/save/delete/path — same shape as
  `SavedQueryStore` at lib.rs:3221, deliberately a distinct type like
  `ExtendedQueryStore`) implemented by `FsQueryStore` (lib.rs:3316).
- `ContentAdapter::template_store(action_id) -> Option<Box<dyn TemplateStore>>`,
  default built on demand like `extended_query_store` (lib.rs:2925):
  `FsQueryStore::new(instance_data_dir()/templates/<action_id>, template_suffix())`.
  `template_suffix()` default `.txt`; tasks `.md`. Decorators forward
  `instance_data_dir`, so they need not forward this.
- A new template is seeded from the adapter's plain buffer: `root().prepare(action_id, {})`
  (root `add` writes `# parent:`, so no stale uuid lands in a stored template).

### Tasks adapter (`not-yet-done-local-adapter`)

- `editor_templates.rs`: add `tags:` (comma list of tag names) to `new_task`
  (:39) and `parse_new_task` (:154); unknown keys are ignored today (~:207).
- `task.rs`: mark root `add`/`add-sibling` (:954, :960) and item ones (:980,
  :985) `.templated()`; implement `prepare_templated` for root (:1902) and item
  (:2039): take the template, replace/insert the `parent:` front-matter line
  with the context's parent, keep everything else.
- `execute_add` (:1221-1281): pass the first tag to `task_service.add_task`
  (its `tag: Option<String>` resolves name/id, task_service.rs:97-139,
  `resolve_tag` :328 — check whether an unknown name errors; it should, with a
  clear message), further tags via `edit_task(add_tag)`.
- `template_suffix()` → `.md`.

### TUI (`not-yet-done-tui`)

- View YAML (`config/view_config.rs`, `ActionDef` :2021): optional
  `templates: { menu_key: "a t" }` on a `type: create` action; validate it is
  only on `create`, and claim the key (`keymap.rs`, new
  `KeySource::TemplateMenuKey` beside `YamlMenuKey`).
- `QueryMenuMode::EditorTemplates { action_id }` (content_view.rs:1235); route
  every `QueryMenuMessage` in `handle_query_popup_key` (:9933-10115), modelled
  on the `NodeScripts` arms: Apply → create with template; EditExisting →
  edit the file (`FileEditSession` via `TemplateStore::path`); Delete;
  Duplicate; CreateNew → seed + edit; EditShortcut/ClearShortcut → bind.
  Open it with `open_without_default` (no ★ default for templates).
- Create path: `ViewRequest::CreateContentChild` (views/mod.rs:569, emitted at
  content_view.rs:7922-7966) gains `template: Option<String>` (the name);
  app/mod.rs:10019-10081 loads it from the store and builds the edit session;
  `edit_session/node_action.rs:76-130` calls `prepare_templated` instead of
  `prepare` when a template is set (:110).
- Shortcuts: `ShortcutTarget::Template` (app/mod.rs:1023) and
  `KeySource::TemplateShortcut { action_id, template }` (keymap.rs:116);
  `query_shortcut` row with `kind = "template"`, view scope
  (`content_view.rs:8260-8271`), name `<action_id>/<template>`. Loading next
  to `reload_content_saved_queries` (app/mod.rs:12260-12375); dispatch in
  `dispatch_view_claim` (content_view.rs:12388) finds the view's create action
  with that `id` and a `templates:` block and runs it with the template.
  Bind flow as for node scripts: `begin_shortcut_bind` (:12575),
  `write_shortcut_target` (:12593), `set_db_shortcut_as` (:12892).

### Docs and tests

- `docs/generic-view-spec.md` ("Editor templates", :2165-2247),
  `docs/examples/views/tasks.yaml` (`a t` next to `a a`, :364).
- Unit tests: tasks merge (parent replaced, tags parsed, unchanged = aborted),
  view-config validation, keymap claim.
- `docs/smoke-tests.md`: new section "Editor templates".

### User config (after the build)

- `~/.config/not_yet_done/views/tasks.yaml`: `templates: {menu_key: "a t"}` on
  the `a a` action (tree :249-254, flat :458-465); commit in that repo.
- Create the tag `todo` if missing; create template `todo` (`tags: todo`) in
  the menu and bind `a o`.
- deadlines-widget: `sources/nyd-deadlines.sh` selects `due set OR tag_names has todo`
  (filter in jq — saved-query FilterExpr has no tag column); open todos without
  a deadline sort after the dated ones, oldest first, get level 1 and no time;
  menu group "Ohne Deadline" before "Erledigt". Update
  `docs/how-to/show-your-deadlines.md` there.

## Steps

1. Protocol + tasks adapter + unit tests; build; commit.
2. TUI: YAML key, menu mode, create path; build; commit.
3. TUI: template shortcuts; build; commit.
4. Docs, smoke-test section; install; user config; deadlines-widget script.
