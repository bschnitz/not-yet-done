// App-shell repositories. The task-domain repositories live in
// not-yet-done-task-core (C3 of the DB-split).
mod link_repository;
mod query_shortcut_repository;
mod settings_repository;

pub use link_repository::{LinkRepository, LinkRepositoryImpl, LinkRepositoryImplParameters};
pub use query_shortcut_repository::{
    QueryShortcutRepository, QueryShortcutRepositoryImpl, QueryShortcutRepositoryImplParameters,
};
pub use settings_repository::{
    SettingsRepository, SettingsRepositoryImpl, SettingsRepositoryImplParameters,
};
