use shaku::module;

use crate::repository::{
    LinkRepositoryImpl, ProjectRepositoryImpl, QueryShortcutRepositoryImpl,
    SavedQueryRepositoryImpl, SettingsRepositoryImpl, TagRepositoryImpl, TaskRepositoryImpl,
    TrackingRepositoryImpl,
};
use crate::service::{
    BackupServiceImpl, ProjectServiceImpl, TagServiceImpl, TaskServiceImpl, TrackingServiceImpl,
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
