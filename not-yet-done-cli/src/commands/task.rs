use tusks::tusks;

#[tusks()]
#[command(about = "Manage tasks")]
pub mod cli {
    pub use crate::cli as parent_;

    /// Add a new task
    pub fn add(
        #[arg(help = "Task description")] description: String,
        #[arg(long, help = "Assign to project (name or ID)")] project: Option<String>,
        #[arg(long, help = "Set parent task (ID)")] parent: Option<String>,
        #[arg(long, help = "Assign a tag (name, ID, global-tag:<id> or project-tag:<id>)")] tag: Option<String>,
    ) -> u8 {
        let result = crate::run_async(|module| async move {
            use shaku::HasComponent;
            use not_yet_done_core::service::TaskService;
            let service: &dyn TaskService = module.resolve_ref();
            service.add_task(description, project, parent, tag).await
        });
        match result {
            Ok(task) => { println!("✓ Task created: [{}] {}", task.id, task.description); 0 }
            Err(e)   => { eprintln!("Error: {e}"); 1 }
        }
    }

    /// List tasks
    pub fn list(
        #[arg(long, help = "Filter by project (name or ID)")] project: Option<String>,
    ) -> u8 {
        let result = crate::run_async(|module| async move {
            use shaku::HasComponent;
            use not_yet_done_core::service::TaskService;
            let service: &dyn TaskService = module.resolve_ref();
            service.list_tasks(project).await
        });
        match result {
            Ok(tasks) if tasks.is_empty() => { println!("No tasks found."); 0 }
            Ok(tasks) => {
                for task in tasks {
                    println!("[{}] {:?} | {}", task.id, task.status, task.description);
                }
                0
            }
            Err(e) => { eprintln!("Error: {e}"); 1 }
        }
    }

    /// Soft-delete a task
    pub fn delete(
        #[arg(help = "Task ID")] id: String,
    ) -> u8 {
        let result = crate::run_async(|module| async move {
            use shaku::HasComponent;
            use not_yet_done_core::service::TaskService;
            use sea_orm::prelude::Uuid;
            let id = Uuid::parse_str(&id)
                .map_err(|_| not_yet_done_core::error::AppError::InvalidId(id))?;
            let service: &dyn TaskService = module.resolve_ref();
            service.delete_task(id).await
        });
        match result {
            Ok(()) => { println!("✓ Task deleted."); 0 }
            Err(e) => { eprintln!("Error: {e}"); 1 }
        }
    }

    /// Edit a task
    pub fn edit(
        #[arg(help = "Task ID")] id: String,
        #[arg(long, help = "New description")] description: Option<String>,
        #[arg(long, help = "Add project assignment (name or ID)")] add_project: Option<String>,
        #[arg(long, help = "Remove project assignment (name or ID)")] remove_project: Option<String>,
        #[arg(long, help = "Add tag (name, ID, global-tag:<id> or project-tag:<id>)")] add_tag: Option<String>,
        #[arg(long, help = "Remove tag (name, ID, global-tag:<id> or project-tag:<id>)")] remove_tag: Option<String>,
    ) -> u8 {
        let result = crate::run_async(|module| async move {
            use shaku::HasComponent;
            use not_yet_done_core::service::TaskService;
            use sea_orm::prelude::Uuid;
            let id = Uuid::parse_str(&id)
                .map_err(|_| not_yet_done_core::error::AppError::InvalidId(id))?;
            let service: &dyn TaskService = module.resolve_ref();
            service.edit_task(id, description, add_project, remove_project, add_tag, remove_tag).await
        });
        match result {
            Ok(task) => { println!("✓ Task updated: [{}] {}", task.id, task.description); 0 }
            Err(e)   => { eprintln!("Error: {e}"); 1 }
        }
    }
}
