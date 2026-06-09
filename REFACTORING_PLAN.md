# View Component Refactoring Plan

## Goal
Each View is an autonomous MockComponent that owns its data, services, and key handling.
The App only orchestrates: tab switching, editor lifecycle, popup overlays, terminal management.

## Architecture Target

```
App
├── TasksView (MockComponent)
│   ├── TasksState (owns: forest, task_rows, load_state, filter)
│   ├── Arc<dyn TaskService>
│   ├── Arc<dyn SavedFilterRepository>
│   ├── TasksListView (MockComponent, owns DataTable)
│   └── TasksTreeView (MockComponent, owns DataTable)
├── TrackingsView (MockComponent)
│   ├── TrackingsState (owns: rows, display_rows, condensed_rows, tree_rows)
│   ├── Arc<dyn TrackingRepository>
│   ├── Arc<dyn SavedFilterRepository>
│   ├── TrackingsNormalView (MockComponent, owns DataTable)
│   ├── TrackingsCondensedView (MockComponent, owns DataTable)
│   └── TrackingsTreeView (MockComponent, owns DataTable)
├── TabBar, ActionBar, StatusBar, NotificationBar
└── Popups (overlays, managed by App)
```

## Steps

### Phase 1: Move State into Views
- [x] Step 1.1: Move `TasksState` from App into `TasksView`
  - All `app.tasks_state` → `app.tasks_view.state`
  - Added `refresh_from_own_state()` to avoid borrow conflicts
- [x] Step 1.2: Move `TrackingsState` from App into `TrackingsView`
  - All `app.trackings_state` → `app.trackings_view.state`
  - TrackingsView.handle_key() uses own state internally

### Phase 2: Move Services into Views
- [x] Step 2.1: Give `TasksView` an `Arc<dyn TaskService>`
  - Service injected at construction, not yet used directly
- [x] Step 2.2: Give `TrackingsView` an `Arc<dyn TrackingRepository>`
  - Repo injected at construction, not yet used directly

### Phase 3: Move Editor Processing into Views
- [x] Step 3.1: Define `EditorResult` interface
  - `EditorProcessResult` enum in `views/editor_result.rs`
  - App runs the editor (terminal lifecycle)
  - App gives the result back to the view via `process_editor_result(content)`
  - View parses templates, calls services, updates state
- [x] Step 3.2: Move task editor processing into `TasksView`
  - `process_create_task`, `process_edit_task` → `TasksView::process_editor_result()`
  - Notes handling (TaskNotes action)
  - Old App methods removed (process_create_task dead code cleaned up)
- [x] Step 3.3: Move tracking editor processing into `TrackingsView`
  - `process_tracking_query_filter_close`, script handling → `TrackingsView::process_editor_result()`
  - TrackingsView now has `saved_filter_repo` + `settings_repo`
  - Old App methods removed (save_tracking_script, process_tracking_query_filter_close)

### Phase 4: Make TrackingsView Autonomous
- [x] Step 4.1: Move `rebuild_trackings_table` + `active_tracking_columns` into `TrackingsView`
  - View owns its table rebuild logic, called via `rebuild_table(column_config)`
  - App's `rebuild_trackings_table()` is now a thin wrapper
- [x] Step 4.2: TrackingsView handles toggles internally
  - Condensed/tree/order/normal toggles handled in `handle_key()`
  - Removed "notify" signal hack — proper ViewRequest variants instead
  - `handle_key()` takes `column_config` + `forest` parameters
  - TrackingsView has `saved_filter_repo` + `settings_repo`
- [x] Step 4.3: Cleanup dead code
  - Removed `toggle_tracking_tree_mode` from App
  - `handle_trackings_action` now delegates toggle logic to view methods
  - Separate sub-view components not needed (shared DataTable + state)

### Phase 5: Cleanup
- [ ] Step 5.1: Remove dead code from App (old handle_common_action tracking branches, etc.)
- [ ] Step 5.2: ActionBar/StatusBar hints come from views, not sync_components
- [ ] Step 5.3: Remove `tasks_sub_view` from App (lives in TasksView)

## Current Status
**Phase 4 complete** — TrackingsView is autonomous. Next: Phase 5 (Cleanup)
