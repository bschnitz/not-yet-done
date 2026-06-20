// App-shell repositories that stayed in core.
mod link_repository;
mod query_shortcut_repository;
mod saved_query_repository;
mod settings_repository;

pub use link_repository::{LinkRepository, LinkRepositoryImpl, LinkRepositoryImplParameters};
pub use query_shortcut_repository::{QueryShortcutRepository, QueryShortcutRepositoryImpl, QueryShortcutRepositoryImplParameters};
pub use saved_query_repository::{SavedQueryRepository, SavedQueryRepositoryImpl, SavedQueryRepositoryImplParameters};
pub use settings_repository::{SettingsRepository, SettingsRepositoryImpl, SettingsRepositoryImplParameters};

// C2 bridge: task-domain repositories moved to not-yet-done-task-core; re-export
// them so `not_yet_done_core::repository::{TaskRepository, …}` keeps resolving.
pub use not_yet_done_task_core::repository::{
    compute_path, task_short_id, ProjectRepository, ProjectRepositoryImpl,
    ProjectRepositoryImplParameters, ResolvedTag, TagRepository, TagRepositoryImpl,
    TagRepositoryImplParameters, TagStyle, TagStylePatch, TaskOp, TaskRepository,
    TaskRepositoryImpl, TaskRepositoryImplParameters, TrackingRepository, TrackingRepositoryImpl,
    TrackingRepositoryImplParameters,
};
