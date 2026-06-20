use shaku::module;

use crate::repository::{
    LinkRepositoryImpl, QueryShortcutRepositoryImpl, SavedQueryRepositoryImpl,
    SettingsRepositoryImpl,
};
use crate::service::BackupServiceImpl;

// The app-shell domain (link / saved_query / settings / query_shortcut +
// backup) wired as a Shaku module. The task domain lives in its own
// `not_yet_done_task_core::module::TaskDomainModule` (C3 of the DB-split).
// A host that needs both — the TUI — builds both modules and resolves each
// service from the module that owns it.
module! {
    pub CoreModule {
        components = [
            SavedQueryRepositoryImpl,
            QueryShortcutRepositoryImpl,
            SettingsRepositoryImpl,
            LinkRepositoryImpl,
            BackupServiceImpl,
        ],
        providers = []
    }
}
