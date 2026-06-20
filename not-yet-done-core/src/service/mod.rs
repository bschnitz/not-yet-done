// App-shell services that stayed in core.
mod backup_service;

pub use backup_service::{BackupService, BackupServiceImpl};

// C2 bridge: task-domain services moved to not-yet-done-task-core; re-export
// them so `not_yet_done_core::service::{TaskService, …}` keeps resolving.
pub use not_yet_done_task_core::service::{
    annotate_error, edit_global_template, edit_project_template, new_tag_template, normalize,
    parse_draft, strip_error_block, DaySummary, ExportOptions, ExportedTracking, GravityDirection,
    MoveOptions, MovedTracking, ProjectService, ProjectServiceImpl, SortDirection, SplitTracking,
    StoppedTracking, Summary, TagDraft, TagItem, TagService, TagServiceImpl, TaskService,
    TaskServiceImpl, TaskSummary, TrackingService, TrackingServiceImpl,
};
