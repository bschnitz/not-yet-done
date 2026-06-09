use tusks::tusks;

#[tusks()]
#[command(about = "Manage projects")]
pub mod cli {
    pub use crate::cli as parent_;

    /// Create a new project
    pub fn add(
        #[arg(help = "Project name")] name: String,
        #[arg(long, help = "Optional project description")] description: Option<String>,
    ) -> u8 {
        let result = crate::run_async(|module| async move {
            use shaku::HasComponent;
            use not_yet_done_core::service::ProjectService;
            let service: &dyn ProjectService = module.resolve_ref();
            service.add_project(name, description).await
        });
        match result {
            Ok(p) => { println!("✓ Project created: [{}] {}", p.id, p.name); 0 }
            Err(e) => { eprintln!("Error: {e}"); 1 }
        }
    }

    /// List all projects
    pub fn list() -> u8 {
        let result = crate::run_async(|module| async move {
            use shaku::HasComponent;
            use not_yet_done_core::service::ProjectService;
            let service: &dyn ProjectService = module.resolve_ref();
            service.list_projects().await
        });
        match result {
            Ok(projects) if projects.is_empty() => { println!("No projects found."); 0 }
            Ok(projects) => {
                for p in projects {
                    let desc = p.description.as_deref().unwrap_or("—");
                    println!("[{}] {} | {}", p.id, p.name, desc);
                }
                0
            }
            Err(e) => { eprintln!("Error: {e}"); 1 }
        }
    }

    /// Edit a project
    pub fn edit(
        #[arg(help = "Project ID")] id: String,
        #[arg(long, help = "New name")] name: Option<String>,
        #[arg(long, help = "New description")] description: Option<String>,
    ) -> u8 {
        let result = crate::run_async(|module| async move {
            use shaku::HasComponent;
            use not_yet_done_core::service::ProjectService;
            use sea_orm::prelude::Uuid;
            let id = Uuid::parse_str(&id)
                .map_err(|_| not_yet_done_core::error::AppError::InvalidId(id))?;
            let service: &dyn ProjectService = module.resolve_ref();
            service.edit_project(id, name, description).await
        });
        match result {
            Ok(p) => { println!("✓ Project updated: [{}] {}", p.id, p.name); 0 }
            Err(e) => { eprintln!("Error: {e}"); 1 }
        }
    }

    /// Delete a project
    pub fn delete(
        #[arg(help = "Project ID")] id: String,
        #[arg(long, help = "Also soft-delete all tasks assigned to this project")] cascade: bool,
    ) -> u8 {
        let result = crate::run_async(|module| async move {
            use shaku::HasComponent;
            use not_yet_done_core::service::ProjectService;
            use sea_orm::prelude::Uuid;
            let id = Uuid::parse_str(&id)
                .map_err(|_| not_yet_done_core::error::AppError::InvalidId(id))?;
            let service: &dyn ProjectService = module.resolve_ref();
            service.delete_project(id, cascade).await
        });
        match result {
            Ok(()) => { println!("✓ Project deleted."); 0 }
            Err(e) => { eprintln!("Error: {e}"); 1 }
        }
    }
}
