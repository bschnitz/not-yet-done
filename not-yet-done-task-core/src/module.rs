use shaku::module;

use crate::repository::{
    ProjectRepositoryImpl, TagRepositoryImpl, TaskRepositoryImpl, TrackingRepositoryImpl,
};
use crate::service::{ProjectServiceImpl, TagServiceImpl, TaskServiceImpl, TrackingServiceImpl};

// The task/tracking/project/tag domain wired as a self-contained Shaku
// module. Hosts that only need the task domain (Waybar, the CLI, the
// local-adapter) build this alone; the TUI builds it alongside
// `not_yet_done_core::module::CoreModule` for the app-shell repositories.
module! {
    pub TaskDomainModule {
        components = [
            TaskRepositoryImpl,
            ProjectRepositoryImpl,
            TagRepositoryImpl,
            TrackingRepositoryImpl,
            TaskServiceImpl,
            ProjectServiceImpl,
            TagServiceImpl,
            TrackingServiceImpl,
        ],
        providers = []
    }
}
