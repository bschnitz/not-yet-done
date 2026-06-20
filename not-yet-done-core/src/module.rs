use shaku::module;

// App-shell components live in core; task-domain components moved to
// not-yet-done-task-core (C2). The combined AppModule still wires both so
// consumers keep resolving every service from one module. C3 splits this
// into CoreModule + TaskDomainModule once the bridge is removed.
use crate::repository::{
    LinkRepositoryImpl, QueryShortcutRepositoryImpl, SavedQueryRepositoryImpl,
    SettingsRepositoryImpl,
};
use crate::service::BackupServiceImpl;
use not_yet_done_task_core::repository::{
    ProjectRepositoryImpl, TagRepositoryImpl, TaskRepositoryImpl, TrackingRepositoryImpl,
};
use not_yet_done_task_core::service::{
    ProjectServiceImpl, TagServiceImpl, TaskServiceImpl, TrackingServiceImpl,
};

module! {
    pub AppModule {
        components = [
            TaskRepositoryImpl,
            ProjectRepositoryImpl,
            TagRepositoryImpl,
            TrackingRepositoryImpl,
            SavedQueryRepositoryImpl,
            QueryShortcutRepositoryImpl,
            SettingsRepositoryImpl,
            LinkRepositoryImpl,
            TaskServiceImpl,
            ProjectServiceImpl,
            TagServiceImpl,
            TrackingServiceImpl,
            BackupServiceImpl,
        ],
        providers = []
    }
}
