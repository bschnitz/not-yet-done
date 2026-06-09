# Plan: Unified Action Bar Refactor

Status: ready to execute (plan agreed 2026-04-30).

## Goal

One reusable `ActionBarComponent` used by **all three** views (TasksView,
TrackingsView, ContentView). Today there are two parallel implementations
(`components/action_bar.rs` and `components/content_action_bar.rs`) which
diverge in features and visuals. The Jira tab is missing favorites, search,
and the unified visual language.

End state:

- Single `ActionBarComponent` in `components/action_bar.rs`.
- **View-owned**: each view holds its own bar instance and pushes state to it
  via direct method calls. App knows nothing about bar internals.
- Visual design = current Tasks bar (fuzzy label + `│` separator + hints +
  active filter name + favorites with `[shortcut]`).
- Features supported by the bar: hints, active editor highlight, fuzzy filter
  input, search input, cmdline input, favorites list, active-filter-name,
  tracking-active highlight. Views opt in by calling the relevant setter; not
  calling = feature not displayed.

## Non-goals

- Cmdline in Jira tab (not needed for now).
- Migrating Tasks/Trackings to the YAML config-driven design (separate effort
  later — keep their bar config view-internal for now).

## Architecture

```
TasksView                TrackingsView             ContentView
 ├── ActionBarComponent   ├── ActionBarComponent    ├── ActionBarComponent
 ├── bar.set_*(...)       ├── bar.set_*(...)        ├── bar.set_*(...)
 │   on state change      │   on state change       │   on state change
 └── render_action_bar()  └── render_action_bar()   └── render_action_bar()
                                  ▲
                                  │
                            App: only renders the active view in its
                            allocated area; queries view for bar height.
                            App never reads BarState.
```

App responsibilities (whole list):

- Layout: ask active view `view.action_bar_height(width)`.
- Render: call `view.render_action_bar(frame, bar_area)` and
  `view.render_body(frame, body_area)`.
- Nothing else. App must not call any `set_*` on the bar.

View responsibilities:

- Construct an `ActionBarComponent` in its constructor.
- Call `bar.set_*(...)` whenever the corresponding state changes.
- Expose `action_bar_height(width) -> u16` and
  `render_action_bar(frame, area)` as pass-through methods.

Bar = pure renderer. No upward events; user input continues to flow through
the existing keypath (App → View → updates view state → triggers
`bar.set_*`).

## ActionBarComponent API

```rust
impl ActionBarComponent {
    pub fn new(theme: Arc<Theme>) -> Self;

    // Hints displayed in normal mode.
    pub fn set_hints(&mut self, hints: Vec<(String, String)>); // (key_label, description)

    // Highlights the hint matching this description with bold+underline.
    pub fn set_active_editor(&mut self, name: Option<&str>);

    // Fuzzy filter mode (takes over the whole bar).
    pub fn set_fuzzy(&mut self, active: bool, query: &str, cursor: usize);

    // Text search mode (takes over the whole bar).
    pub fn set_search(&mut self, active: bool, query: &str, cursor: usize,
                      current: usize, total: usize);

    // Cmdline mode (takes over the whole bar).
    pub fn set_cmdline(&mut self, active: bool, query: &str, cursor: usize);

    // Favorites with shortcuts shown after hints.
    pub fn set_favorites(&mut self, items: Vec<(String, String, bool)>); // (name, shortcut, is_active)

    // Active saved-query/filter name shown after hints.
    pub fn set_active_filter_name(&mut self, name: Option<String>);

    // Highlights the "track" hint when the selected task is being tracked.
    pub fn set_tracking_active(&mut self, active: bool);

    pub fn required_height(&self, width: u16) -> u16;
    pub fn view(&mut self, frame: &mut Frame, area: Rect); // via MockComponent
}
```

Render priority: `cmdline > search > fuzzy > normal`. In normal mode, layout
order: `[hints] │ [active_filter_name] │ [favorites]`. Visual style copies
the current `components/action_bar.rs` exactly (colors, separators, brackets
around shortcuts).

Internally the component holds an opaque struct with all fields. The mode
enum from the old component goes away — render code branches on
`cmdline.active`, `search.active`, `fuzzy.active`. Only one can be active
at a time; views are responsible for not calling multiple as active.

## Per-view changes

### TasksView

- New field: `bar: ActionBarComponent`.
- Hints constructed from `KeyBindingConfig` (the list currently hardcoded in
  `ActionBarComponent::new`):
  ```
  ("queries", common.SavedFilterSelect)
  ("add",     tasks.FormAdd)
  ("edit",    tasks.FormEdit)
  ("edit node", tasks.FormEditNode)
  ("notes",   tasks.OpenNotes)
  ("track",   common.TrackingToggle)
  ```
- Update calls:
  - On apply_query_filter: `bar.set_active_filter_name(name)`,
    `bar.set_favorites(...)`.
  - On editor open/close: `bar.set_active_editor(Some("add"|"edit"|...))`.
  - On fuzzy state change: `bar.set_fuzzy(active, query, cursor)`.
  - On search state change: `bar.set_search(...)`.
  - On cmdline state change: `bar.set_cmdline(...)`.
  - When tracked-ids change for the selected task: `bar.set_tracking_active(...)`.
- Subview switch (List/Tree): hints stay the same; fuzzy state must be read
  from the active subview (List vs Tree DataTable).
- Expose `action_bar_height(width)` and `render_action_bar(frame, area)`.

### TrackingsView

Analogous to TasksView with the trackings hint set:

```
("queries",     common.SavedFilterSelect)
("new script",  trackings.TrackingScript)
("run",         trackings.TrackingScriptRun)
("track",       common.TrackingToggle)
```

No "add"/"edit"/"notes" hints. Same setter pattern.

### ContentView

- Replace `action_bar: ContentActionBar` with `action_bar: ActionBarComponent`.
- `set_hints` is already driven by `action_bar_hints()` — keep that builder.
- Add favorites: build from `db_saved_queries.iter().filter(|s|
s.shortcut.is_some())` and call `bar.set_favorites(...)` whenever
  saved queries reload.
- Add active-filter-name: from `active_query_name`.
- Wire fuzzy state (already done).
- Wire search state (new — see "Search in ContentView" below).
- Cmdline: not used (don't call `set_cmdline`).
- `tracking_active`: not relevant here.
- Move favorites and `Q` shortcut **out** of `status_bar_hints()` (currently
  in `views/content_view.rs:1057-1067`); they belong in the action bar.

## Search in ContentView

Configurable per view in the YAML view-config, analogous to fuzzy_filter.

### YAML schema

Extend `ActionDef` with a new `search` action type:

```yaml
actions:
  - name: search
    key: /
    type: search
    search:
      fields: [summary, description, key] # fields to match against; empty/absent = all
```

`search` is a takeover-bar action like `fuzzy_filter` —
`shows_in_action_bar()` returns true for it.

### Code changes

- `config/view_config.rs`: add `SearchConfig { fields: Vec<String> }` and
  `pub search: Option<SearchConfig>` field on `ActionDef`. Update
  `shows_in_action_bar()` to include `"search"`.
- `views/content_view.rs`: add `search: SearchComponent` (the existing
  shared component). Implement `Searchable` trait (already defined in
  `views/mod.rs`).
- ContentView::handle_key: when search is active, route keys to search
  component; on Esc, deactivate; on Enter, jump to first match.
- Search target: rows in the current `DataTable`. Match against the fields
  specified in `SearchConfig.fields`, falling back to label if empty.
- Push state to bar via `bar.set_search(...)` whenever search state changes.

### Rendering priority interaction

`fuzzy` and `search` are mutually exclusive (both takeover the bar). Views
must enforce this: opening one closes the other.

## App layout / render changes

### `render.rs`

Today (simplified):

```rust
let show_action_bar = !matches!(app.active_tab, Tab::Content(_));
if show_action_bar { ... app.action_bar.view(frame, ...) ... }
// Content tabs render their bar internally.
```

New:

```rust
// Always have a bar; height comes from active view.
let bar_height = active_view.action_bar_height(area.width);
let chunks = split(area, bar_height + body + status);
active_view.render_action_bar(frame, bar_chunks);
active_view.render_body(frame, body_chunks);
```

Pseudo-trait:

```rust
trait ViewWithBar {
    fn action_bar_height(&self, width: u16) -> u16;
    fn render_action_bar(&mut self, frame: &mut Frame, area: Rect);
}
```

`active_view` is one of `TasksView | TrackingsView | ContentView` —
realize via `match app.active_tab` in App, since trait objects don't fit
the existing structure cleanly.

### `app/mod.rs`

- Remove `app.action_bar: ActionBarComponent`.
- Remove all `self.action_bar.set_*(...)` calls scattered through the file
  (search for `action_bar.set_` to find them; there are several in
  `sync_components` and event handlers). Their state-update intent moves
  into the views.
- `sync_components()` simplifies — bar updates happen view-internally.

## Removals

- `components/content_action_bar.rs` — delete entirely.
- `components/mod.rs` — drop the `pub mod content_action_bar;` line.
- ContentView `set_action_bar_*` indirections (if any) — collapse into
  direct `self.bar.set_*` calls.
- Status-bar favorites + `Q` hint in `content_view.rs status_bar_hints()` —
  remove (now in action bar).
- `ActionBarMode::Trackings`, `ActionBarMode::Hidden`, hardcoded
  `tracking_hints` and `action_hints` from `ActionBarComponent` constructor.

## Phase order

1. **Bar component** (`components/action_bar.rs`): strip hardcoded hints,
   add granular setters, unify render code. `cargo build` + existing call
   sites broken — that's fine, fix per-phase.
2. **TasksView**: add bar field, wire setters, expose
   height/render passthroughs. Update `app/mod.rs` to call them. Remove
   App-level bar.
3. **TrackingsView**: same as TasksView with trackings hints.
4. **ContentView**: swap `ContentActionBar` for `ActionBarComponent`,
   wire favorites and active-filter-name. Move favorites out of status
   bar. (No search yet.)
5. **`render.rs`**: unify the Tab branching — all tabs use
   `active_view.render_action_bar`.
6. **Delete `content_action_bar.rs`** + `mod.rs` cleanup.
7. **Search YAML schema**: extend `view_config.rs` with `SearchConfig`.
8. **Search in ContentView**: wire `SearchComponent`, route keys, push
   state to bar. Update example YAML in `docs/examples/views/jira.yaml`.
9. **Verify**: `cargo build --release`, `cargo install --path
not-yet-done-tui --offline`, manual smoke test of all three tabs:
   hints, fuzzy, search (where applicable), favorites, active-filter,
   editor highlight, tracking highlight.

Each phase compiles cleanly before moving on.

## Files touched (estimate)

- `not-yet-done-tui/src/components/action_bar.rs` — major rewrite (~300 LoC)
- `not-yet-done-tui/src/components/content_action_bar.rs` — delete
- `not-yet-done-tui/src/components/mod.rs` — small
- `not-yet-done-tui/src/views/tasks_view.rs` — moderate (~80 LoC added)
- `not-yet-done-tui/src/views/trackings_view.rs` — moderate (~80 LoC added)
- `not-yet-done-tui/src/views/content_view.rs` — moderate
- `not-yet-done-tui/src/render.rs` — small (unify branches)
- `not-yet-done-tui/src/app/mod.rs` — moderate (remove `set_*` chains)
- `not-yet-done-tui/src/config/view_config.rs` — small (SearchConfig)
- `docs/generic-view-spec.md` — document `search` action
- `docs/examples/views/jira.yaml` — example search action

Total: ~600-800 lines net diff.

## Verification checklist

After completion, manually verify in each tab:

| Feature                         | Tasks | Trackings | Jira |
| ------------------------------- | :---: | :-------: | :--: |
| Hints render                    |   ✓   |     ✓     |  ✓   |
| Fuzzy filter (open/type/accept) |   ✓   |     ✓     |  ✓   |
| Text search (open/type/n/N)     |   ✓   |     ✓     |  ✓   |
| Cmdline                         |   ✓   |     ✓     |  —   |
| Favorites with shortcut hint    |   ✓   |     ✓     |  ✓   |
| Active-filter name shown        |   ✓   |     ✓     |  ✓   |
| Editor-active hint highlighted  |   ✓   |     ✓     |  ✓   |
| Tracking-active highlight       |   ✓   |     ✓     |  —   |
| Status bar no longer has favs   |  n/a  |    n/a    |  ✓   |

## Open questions for the implementer

None remaining. The user has confirmed:

- Bar belongs to view (not App). App stays oblivious to bar contents.
- Visual design = current Tasks bar.
- Search is per-view YAML config (mirrors fuzzy_filter).
- Favorites move from status bar to action bar in Jira.
- Cmdline not needed in Jira (will work for views that opt in via setter).
