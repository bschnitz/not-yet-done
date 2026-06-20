// App-shell entities that stayed in core.
pub mod link;
pub mod query_shortcut;
pub mod saved_query;
pub mod settings;

// C2 bridge: task-domain entities moved to not-yet-done-task-core; re-export
// them so `not_yet_done_core::entity::{task, tracking, …}` keeps resolving.
pub use not_yet_done_task_core::entity::{
    global_tag, granularity, project, project_tag, task, task_global_tag, task_project,
    task_project_tag, tracking,
};
