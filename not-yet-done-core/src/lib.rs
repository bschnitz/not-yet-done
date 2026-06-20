pub mod config;
pub mod db;
pub mod entity;
pub mod module;
pub mod repository;
pub mod service;

// --- C2 bridge -----------------------------------------------------------
// The task/tracking/project/tag domain (entities, repositories, services,
// the SeaORM filter binding, the domain-event bus, task-path helpers and
// the task error type) moved into `not-yet-done-task-core`. These
// re-exports keep every historic `not_yet_done_core::…` path valid for
// existing consumers (TUI, CLI, Waybar, local-adapter) so C2 churns no
// call sites. C3 removes the bridge and re-points consumers at
// `not_yet_done_task_core::…` directly.
//
// `entity`, `repository` and `service` are *partial* re-exports: those
// modules still own the app-shell items that stayed here (link /
// saved_query / settings / query_shortcut / backup) and re-export the
// task-domain items from task-core (see their `mod.rs`). `error`,
// `events`, `filter`, `local_context` and `task_path` moved wholesale.
pub use not_yet_done_task_core::error;
pub use not_yet_done_task_core::events;
pub use not_yet_done_task_core::filter;
pub use not_yet_done_task_core::local_context;
pub use not_yet_done_task_core::task_path;
