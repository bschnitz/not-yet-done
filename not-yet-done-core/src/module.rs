use shaku::module;

use crate::repository::{ProjectRepositoryImpl, TagRepositoryImpl, TaskRepositoryImpl, TrackingRepositoryImpl};
use crate::service::{ProjectServiceImpl, TagServiceImpl, TaskServiceImpl, TrackingServiceImpl};

module! {
    pub AppModule {
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
