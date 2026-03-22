use tusks::tusks;

#[tusks()]
#[command(about = "Manage tags")]
pub mod cli {
    pub use crate::cli as parent_;

    /// Create a new tag (global or project-specific with --project)
    pub fn add(
        #[arg(help = "Tag name")] name: String,
        #[arg(long, help = "Tag color as hex string (e.g. #FF5733)")] color: Option<String>,
        #[arg(long, help = "Create as project-specific tag (name or ID)")] project: Option<String>,
    ) -> u8 {
        let result: Result<String, not_yet_done_core::error::AppError> =
            crate::run_async(|module| async move {
                use shaku::HasComponent;
                use not_yet_done_core::service::TagService;
                let service: &dyn TagService = module.resolve_ref();
                if let Some(proj) = project {
                    let tag = service.add_project_tag(name, color, proj).await?;
                    Ok(format!("✓ Project tag created: [project-tag:{}] {}", tag.id, tag.name))
                } else {
                    let tag = service.add_global(name, color).await?;
                    Ok(format!("✓ Global tag created: [global-tag:{}] {}", tag.id, tag.name))
                }
            });
        match result {
            Ok(msg) => { println!("{msg}"); 0 }
            Err(e)  => { eprintln!("Error: {e}"); 1 }
        }
    }

    /// List tags
    pub fn list(
        #[arg(long, help = "Show only tags for this project (name or ID)")] project: Option<String>,
        #[arg(long, help = "Show only global tags")] global: bool,
    ) -> u8 {
        let result: Result<Vec<String>, not_yet_done_core::error::AppError> =
            crate::run_async(|module| async move {
                use shaku::HasComponent;
                use not_yet_done_core::service::{TagItem, TagService};
                let service: &dyn TagService = module.resolve_ref();

                if let Some(proj) = project {
                    let tags = service.list_by_project(proj).await?;
                    Ok(tags.into_iter()
                        .map(|t| format!("[project-tag:{}] {}{}",
                            t.id, t.name,
                            t.color.as_deref().map(|c| format!(" ({})", c)).unwrap_or_default()))
                        .collect())
                } else if global {
                    let tags = service.list_global().await?;
                    Ok(tags.into_iter()
                        .map(|t| format!("[global-tag:{}] {}{}",
                            t.id, t.name,
                            t.color.as_deref().map(|c| format!(" ({})", c)).unwrap_or_default()))
                        .collect())
                } else {
                    let items = service.list_all().await?;
                    Ok(items.into_iter().map(|item| match item {
                        TagItem::Global(t) => format!("[global-tag:{}] {}{}",
                            t.id, t.name,
                            t.color.as_deref().map(|c| format!(" ({})", c)).unwrap_or_default()),
                        TagItem::Project { tag: t, project_name } =>
                            format!("[project-tag:{}] {} (project: {}){}",
                                t.id, t.name, project_name,
                                t.color.as_deref().map(|c| format!(" ({})", c)).unwrap_or_default()),
                    }).collect())
                }
            });
        match result {
            Ok(lines) if lines.is_empty() => { println!("No tags found."); 0 }
            Ok(lines) => { lines.iter().for_each(|l| println!("{l}")); 0 }
            Err(e)    => { eprintln!("Error: {e}"); 1 }
        }
    }

    /// Edit a tag
    ///
    /// Use the full tag ID including prefix:
    ///   global-tag:<uuid>   for global tags
    ///   project-tag:<uuid>  for project-specific tags
    pub fn edit(
        #[arg(help = "Tag ID (global-tag:<uuid> or project-tag:<uuid>)")] id: String,
        #[arg(long, help = "New tag name")] name: Option<String>,
        #[arg(long, help = "New color as hex string (e.g. #FF5733)")] color: Option<String>,
    ) -> u8 {
        let result: Result<String, not_yet_done_core::error::AppError> =
            crate::run_async(|module| async move {
                use shaku::HasComponent;
                use not_yet_done_core::service::{TagItem, TagService};
                let service: &dyn TagService = module.resolve_ref();
                let item = service.edit(id, name, color).await?;
                Ok(match item {
                    TagItem::Global(t) =>
                        format!("✓ Tag updated: [global-tag:{}] {}", t.id, t.name),
                    TagItem::Project { tag: t, .. } =>
                        format!("✓ Tag updated: [project-tag:{}] {}", t.id, t.name),
                })
            });
        match result {
            Ok(msg) => { println!("{msg}"); 0 }
            Err(e)  => { eprintln!("Error: {e}"); 1 }
        }
    }

    /// Delete a tag
    ///
    /// Use the full tag ID including prefix:
    ///   global-tag:<uuid>   for global tags
    ///   project-tag:<uuid>  for project-specific tags
    pub fn delete(
        #[arg(help = "Tag ID (global-tag:<uuid> or project-tag:<uuid>)")] id: String,
    ) -> u8 {
        let result = crate::run_async(|module| async move {
            use shaku::HasComponent;
            use not_yet_done_core::service::TagService;
            let service: &dyn TagService = module.resolve_ref();
            service.delete(id).await
        });
        match result {
            Ok(()) => { println!("✓ Tag deleted."); 0 }
            Err(e) => { eprintln!("Error: {e}"); 1 }
        }
    }
}
