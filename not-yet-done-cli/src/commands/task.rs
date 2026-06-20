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
            use not_yet_done_task_core::service::TaskService;
            let service: &dyn TaskService = module.resolve_ref();
            service.add_task(description, project, parent, tag, None, None).await
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
            use not_yet_done_task_core::service::TaskService;
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
            use not_yet_done_task_core::service::TaskService;
            use sea_orm::prelude::Uuid;
            let id = Uuid::parse_str(&id)
                .map_err(|_| not_yet_done_task_core::error::AppError::InvalidId(id))?;
            let service: &dyn TaskService = module.resolve_ref();
            service.delete_task(id).await
        });
        match result {
            Ok(()) => { println!("✓ Task deleted."); 0 }
            Err(e) => { eprintln!("Error: {e}"); 1 }
        }
    }

    /// Export a task subtree as nested JSON.
    ///
    /// Returns the complete tree below the given root node. Use
    /// --last-tracked-since to prune leaves that haven't been tracked
    /// since a given date.
    ///
    /// Examples:
    ///   nyd task tree <id>
    ///   nyd task tree <id> --last-tracked-since "2026-04-01"
    pub fn tree(
        #[arg(help = "Root task ID (or description prefix)")] root: String,
        #[arg(long, help = "Only include leaves tracked at or after this date")]
        last_tracked_since: Option<crate::datetime::LocalDateTime>,
        #[arg(long, help = "Pretty-print the JSON output")]
        pretty: bool,
    ) -> u8 {
        use not_yet_done_task_core::service::TaskService;
        use sea_orm::prelude::Uuid;
        use serde::Serialize;
        use shaku::HasComponent;
        use std::collections::{HashMap, HashSet};

        let since = last_tracked_since.map(|d| d.utc);

        let root_id = match Uuid::parse_str(&root) {
            Ok(id) => id,
            Err(_) => {
                // Try to find by description prefix.
                let result = crate::run_async(|module| async move {
                    let service: &dyn TaskService = module.resolve_ref();
                    service.list_tasks(None).await
                });
                match result {
                    Ok(tasks) => {
                        let lower = root.to_lowercase();
                        let matches: Vec<_> = tasks.iter()
                            .filter(|t| t.description.to_lowercase().starts_with(&lower))
                            .collect();
                        if matches.len() == 1 {
                            matches[0].id
                        } else if matches.is_empty() {
                            eprintln!("Error: No task found matching '{root}'");
                            return 1;
                        } else {
                            eprintln!("Error: Ambiguous — {} tasks match '{root}':", matches.len());
                            for m in &matches {
                                eprintln!("  [{}] {}", m.id, m.description);
                            }
                            return 1;
                        }
                    }
                    Err(e) => { eprintln!("Error: {e}"); return 1; }
                }
            }
        };

        let result = crate::run_async(|module| async move {
            let service: &dyn TaskService = module.resolve_ref();
            service.get_subtree(root_id, since).await
        });

        match result {
            Ok(tasks) => {
                #[derive(Serialize)]
                struct TreeNode {
                    id: String,
                    description: String,
                    last_tracked_at: Option<String>,
                    children: Vec<TreeNode>,
                }

                // Build a nested tree from the flat list.
                let id_set: HashSet<Uuid> = tasks.iter().map(|t| t.id).collect();
                let mut children_map: HashMap<Option<Uuid>, Vec<&not_yet_done_task_core::entity::task::Model>> = HashMap::new();
                for t in &tasks {
                    // Group by parent, but if parent is not in our set, treat as top-level.
                    let key = if t.parent_id.map(|p| id_set.contains(&p)).unwrap_or(false) {
                        t.parent_id
                    } else {
                        None
                    };
                    children_map.entry(key).or_default().push(t);
                }

                fn build_tree(
                    parent: Option<Uuid>,
                    children_map: &HashMap<Option<Uuid>, Vec<&not_yet_done_task_core::entity::task::Model>>,
                ) -> Vec<TreeNode> {
                    let Some(children) = children_map.get(&parent) else {
                        return vec![];
                    };
                    children.iter().map(|t| {
                        use chrono::SecondsFormat;
                        TreeNode {
                            id: t.id.to_string(),
                            description: t.description.clone(),
                            last_tracked_at: t.last_tracked_at.map(|dt|
                                dt.to_rfc3339_opts(SecondsFormat::Nanos, true)
                            ),
                            children: build_tree(Some(t.id), children_map),
                        }
                    }).collect()
                }

                let roots = build_tree(None, &children_map);

                let json_str = if pretty {
                    serde_json::to_string_pretty(&roots)
                } else {
                    serde_json::to_string(&roots)
                };

                match json_str {
                    Ok(s) => { println!("{s}"); 0 }
                    Err(e) => { eprintln!("Error serializing JSON: {e}"); 1 }
                }
            }
            Err(e) => { eprintln!("Error: {e}"); 1 }
        }
    }

    /// Locate a task by `/-rooted path. Path segments are matched
    /// against task `description` — substring by default, regex when
    /// prefixed with `re:`. Mirrors the TUI's `:focus-task` semantics.
    ///
    /// On a unique hit prints `{"id":"...", "description":"...",
    /// "parent_id":"...|null"}` to stdout, exit 0.
    /// On no-match / ambiguous / bad-regex prints the reason to stderr,
    /// exit ≠ 0. No `--create-if-missing`: chain with `task add` if you
    /// want create-on-miss.
    ///
    /// Examples:
    ///   nyd task show --path /Work/Clients/Acme/Tickets
    ///   nyd task show -i --path '/work/clients/acme/tickets/re:\b42\b'
    pub fn show(
        #[arg(long, help = "/-rooted path of segment matchers (re: prefix opts in to regex)")]
        path: String,
        #[arg(short = 'i', long, help = "Case-insensitive matching")]
        case_insensitive: bool,
    ) -> u8 {
        use not_yet_done_task_core::service::TaskService;
        use not_yet_done_task_core::task_path::{walk_task_path, WalkOutcome};
        use serde::Serialize;
        use shaku::HasComponent;

        let result = crate::run_async(|module| async move {
            let service: &dyn TaskService = module.resolve_ref();
            service.list_tasks(None).await
        });
        let tasks = match result {
            Ok(t) => t,
            Err(e) => { eprintln!("Error: {e}"); return 1; }
        };

        match walk_task_path(&tasks, &path, case_insensitive) {
            WalkOutcome::Found(id) => {
                let task = tasks.iter().find(|t| t.id == id).expect("walker returned known id");
                #[derive(Serialize)]
                struct Out<'a> {
                    id: String,
                    description: &'a str,
                    parent_id: Option<String>,
                }
                let out = Out {
                    id: task.id.to_string(),
                    description: &task.description,
                    parent_id: task.parent_id.map(|p| p.to_string()),
                };
                match serde_json::to_string(&out) {
                    Ok(s) => { println!("{s}"); 0 }
                    Err(e) => { eprintln!("Error serializing JSON: {e}"); 1 }
                }
            }
            WalkOutcome::MissingLeadingSlash => {
                eprintln!("Error: path must start with '/' (got {path:?})");
                2
            }
            WalkOutcome::EmptyPath => {
                eprintln!("Error: path is empty");
                2
            }
            WalkOutcome::BadRegex { seg, msg, .. } => {
                eprintln!("Error: bad segment {seg:?} — {msg}");
                3
            }
            WalkOutcome::NotFound { depth, seg, .. } => {
                // Re-split path to render a scope hint matching the TUI.
                let segments: Vec<&str> = path
                    .strip_prefix('/')
                    .unwrap_or("")
                    .split('/')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                let scope = if depth == 0 {
                    "root level".to_string()
                } else {
                    format!("under {:?}", segments[depth - 1])
                };
                eprintln!("Error: no task matching {seg:?} at {scope}");
                4
            }
            WalkOutcome::Ambiguous { seg, candidates, .. } => {
                eprintln!(
                    "Error: {seg:?} is ambiguous ({} candidates):",
                    candidates.len()
                );
                for id in candidates.iter().take(10) {
                    if let Some(t) = tasks.iter().find(|t| t.id == *id) {
                        eprintln!("  [{}] {}", t.id, t.description);
                    }
                }
                if candidates.len() > 10 {
                    eprintln!("  … (+{} more)", candidates.len() - 10);
                }
                5
            }
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
            use not_yet_done_task_core::service::TaskService;
            use sea_orm::prelude::Uuid;
            let id = Uuid::parse_str(&id)
                .map_err(|_| not_yet_done_task_core::error::AppError::InvalidId(id))?;
            let service: &dyn TaskService = module.resolve_ref();
            service.edit_task(id, description, add_project, remove_project, add_tag, remove_tag).await
        });
        match result {
            Ok(task) => { println!("✓ Task updated: [{}] {}", task.id, task.description); 0 }
            Err(e)   => { eprintln!("Error: {e}"); 1 }
        }
    }
}
