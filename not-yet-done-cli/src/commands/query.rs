use tusks::tusks;

#[tusks()]
#[command(about = "Run filter queries against tasks or trackings")]
pub mod cli {
    pub use crate::cli as parent_;

    /// Execute a filter query from a YAML file.
    ///
    /// Reads the query from a file (--file) or stdin, resolves natural-language
    /// dates, and executes the filter against the database.
    ///
    /// Examples:
    ///   nyd query --entity tracking --file filter.yaml
    ///   echo 'query: [deleted, =, false]' | nyd query --entity task
    ///   nyd query --entity tracking --file filter.yaml --debug
    pub fn run(
        #[arg(long, short, help = "Entity to query: 'task' or 'tracking'")]
        entity: String,
        #[arg(long, short, help = "Path to YAML filter file (reads stdin if omitted)")]
        file: Option<String>,
        #[arg(long, help = "Show resolved filter expression before executing")]
        debug: bool,
    ) -> u8 {
        let content = match file {
            Some(path) => match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error reading {path}: {e}");
                    return 1;
                }
            },
            None => {
                use std::io::Read;
                let mut buf = String::new();
                if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
                    eprintln!("Error reading stdin: {e}");
                    return 1;
                }
                buf
            }
        };

        let parsed = match not_yet_done_task_core::filter::query_filter::parse(&content) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Filter parse error: {e}");
                return 1;
            }
        };

        if debug {
            match not_yet_done_task_core::filter::query_filter::resolve_and_dump(&content) {
                Ok(resolved) => eprintln!("--- Resolved filter ---\n{resolved}---"),
                Err(e) => eprintln!("--- Could not dump resolved filter: {e} ---"),
            }
            eprintln!("--- FilterExpr ---\n{:#?}\n---", parsed.expr);
        }

        match entity.as_str() {
            "task" | "tasks" => super::run_task_query(parsed.expr, parsed.options, debug),
            "tracking" | "trackings" => super::run_tracking_query(parsed.expr, debug),
            other => {
                eprintln!("Unknown entity '{other}'. Use 'task' or 'tracking'.");
                1
            }
        }
    }
}

fn run_task_query(
    expr: not_yet_done_task_core::filter::FilterExpr,
    options: not_yet_done_task_core::filter::query_filter::QueryOptions,
    debug: bool,
) -> u8 {
    let result = crate::run_async(|module| async move {
        use not_yet_done_task_core::service::TaskService;
        use shaku::HasComponent;
        let service: &dyn TaskService = module.resolve_ref();
        service.list_filtered_with_options(&expr, &options).await
    });
    match result {
        Ok(tasks) => {
            if debug {
                eprintln!("Found {} tasks", tasks.len());
            }
            let json: Vec<serde_json::Value> = tasks
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "id": t.id.to_string(),
                        "description": t.description,
                        "status": format!("{:?}", t.status).to_lowercase(),
                        "priority": t.priority,
                        "parent_id": t.parent_id.map(|p| p.to_string()),
                        "deleted": t.deleted,
                        "created_at": t.created_at.to_rfc3339(),
                        "updated_at": t.updated_at.to_rfc3339(),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&json).unwrap());
            0
        }
        Err(e) => {
            eprintln!("Query error: {e}");
            1
        }
    }
}

fn run_tracking_query(expr: not_yet_done_task_core::filter::FilterExpr, debug: bool) -> u8 {
    let result = crate::run_async(|module| async move {
        use not_yet_done_task_core::repository::TrackingRepository;
        use shaku::HasComponent;
        let repo: &dyn TrackingRepository = module.resolve_ref();
        repo.find_filtered(&expr).await
    });
    match result {
        Ok(trackings) => {
            if debug {
                eprintln!("Found {} trackings", trackings.len());
            }
            let json: Vec<serde_json::Value> = trackings
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "id": t.id.to_string(),
                        "task_id": t.task_id.to_string(),
                        "started_at": t.started_at.to_rfc3339(),
                        "ended_at": t.ended_at.map(|e| e.to_rfc3339()),
                        "deleted": t.deleted,
                        "created_at": t.created_at.to_rfc3339(),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&json).unwrap());
            0
        }
        Err(e) => {
            eprintln!("Query error: {e}");
            1
        }
    }
}
