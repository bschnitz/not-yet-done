use tusks::tusks;

#[tusks()]
#[command(about = "Manage time tracking")]
pub mod cli {
    pub use crate::cli as parent_;

    /// Start tracking time for a task.
    ///
    /// By default all other active trackings are stopped before starting the new one.
    /// Use --parallel to keep existing trackings running. Note that each task can only
    /// have one active tracking at a time — starting a task that is already being tracked
    /// will return an error regardless of --parallel.
    pub fn start(
        #[arg(help = "Task ID to start tracking")] task_id: String,
        #[arg(
            long,
            help = "Keep other tasks' active trackings running instead of stopping them"
        )]
        parallel: bool,
    ) -> u8 {
        let result = crate::run_async(|module| async move {
            use not_yet_done_task_core::service::TrackingService;
            use sea_orm::prelude::Uuid;
            use shaku::HasComponent;
            let task_id = Uuid::parse_str(&task_id)
                .map_err(|_| not_yet_done_task_core::error::AppError::InvalidId(task_id))?;
            let service: &dyn TrackingService = module.resolve_ref();
            service.start(task_id, parallel).await
        });
        match result {
            Ok(tracking) => {
                println!(
                    "✓ Tracking started: [{}] started at {}",
                    tracking.id,
                    tracking.started_at
                        .with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M:%S")
                );
                0
            }
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        }
    }

    /// Stop tracking. Stops the active tracking for a specific task, or all active
    /// trackings if no task ID is given.
    pub fn stop(
        #[arg(long, help = "Task ID to stop tracking for (stops all active trackings if omitted)")]
        task_id: Option<String>,
    ) -> u8 {
        let result = crate::run_async(|module| async move {
            use not_yet_done_task_core::service::TrackingService;
            use sea_orm::prelude::Uuid;
            use shaku::HasComponent;

            let task_id = match task_id {
                Some(id) => Some(
                    Uuid::parse_str(&id)
                        .map_err(|_| not_yet_done_task_core::error::AppError::InvalidId(id))?,
                ),
                None => None,
            };

            let service: &dyn TrackingService = module.resolve_ref();
            service.stop(task_id).await
        });

        match result {
            Ok(stopped) => {
                for s in &stopped {
                    println!(
                        "✓ Tracking stopped: [{}] {} | {} → {}",
                        s.tracking.id,
                        s.task_description,
                        s.tracking.started_at
                            .with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M"),
                        s.tracking.ended_at
                            .unwrap()
                            .with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M"),
                    );
                }
                0
            }
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        }
    }

    /// Show a summary of tracked time grouped by day and task.
    /// Defaults to today if no date range is given.
    ///
    /// Examples:
    ///   nyd track summary
    ///   nyd track summary --from 2026-03-01 --to 2026-03-22
    ///   nyd track summary --from 2026-03-01
    pub fn summary(
        #[arg(
            long,
            help = "Start date/time (e.g. '2026-03-01', 'yesterday', 'last monday'), defaults to today"
        )]
        from: Option<crate::datetime::LocalDateTime>,
        #[arg(
            long,
            help = "End date/time (e.g. '2026-03-22', 'today'), defaults to today"
        )]
        to: Option<crate::datetime::LocalDateTime>,
        #[arg(long, help = "Filter by task ID")]
        task_id: Option<String>,
    ) -> u8 {
        use chrono::{Local, TimeZone};

        let now = Local::now();
        let tz = *now.offset();

        let from_ctx = from.map(|d| d.into()).unwrap_or_else(|| {
            let utc = Local
                .from_local_datetime(
                    &now.date_naive().and_hms_opt(0, 0, 0).unwrap(),
                )
                .single()
                .unwrap()
                .to_utc();
            not_yet_done_task_core::local_context::LocalContext::new(utc, tz)
        });

        let to_ctx = to.map(|d| d.into()).unwrap_or_else(|| {
            let utc = Local
                .from_local_datetime(
                    &now.date_naive().and_hms_opt(23, 59, 59).unwrap(),
                )
                .single()
                .unwrap()
                .to_utc();
            not_yet_done_task_core::local_context::LocalContext::new(utc, tz)
        });

        if from_ctx.utc > to_ctx.utc {
            eprintln!("Error: --from must not be after --to");
            return 1;
        }

        let result = crate::run_async(|module| async move {
            use not_yet_done_task_core::service::TrackingService;
            use sea_orm::prelude::Uuid;
            use shaku::HasComponent;

            let task_id = match task_id {
                Some(id) => Some(
                    Uuid::parse_str(&id)
                        .map_err(|_| not_yet_done_task_core::error::AppError::InvalidId(id))?,
                ),
                None => None,
            };

            let service: &dyn TrackingService = module.resolve_ref();
            service.summary(from_ctx, to_ctx, task_id).await
        });

        match result {
            Ok(summary) => {
                if summary.days.is_empty() {
                    println!("No tracked time found for the given range.");
                    return 0;
                }

                println!(
                    "From {} to {}\n",
                    from_ctx.to_local().format("%Y-%m-%d"),
                    to_ctx.to_local().format("%Y-%m-%d"),
                );

                // Determine column width from all task descriptions across all days
                let max_desc_len = summary.days.iter()
                    .flat_map(|d| d.entries.iter())
                    .map(|e| e.task_description.len())
                    .max()
                    .unwrap_or(0)
                    .max(5); // at least "Total"

                let sep_width = max_desc_len + 50;

                for day in &summary.days {
                    println!("{}", day.date.format("%Y-%m-%d"));

                    for entry in &day.entries {
                        println!(
                            "  [{task_id}] {desc:<width$}  {dur}",
                            task_id = entry.task_id,
                            desc = entry.task_description,
                            width = max_desc_len,
                            dur = format_duration(entry.total_duration),
                        );
                    }

                    println!(
                        "  {label:<width$}  {dur}",
                        label = "Day total",
                        width = max_desc_len + 38, // UUID + brackets + spaces
                        dur = format_duration(day.day_total),
                    );
                    println!();
                }

                println!("{}", "─".repeat(sep_width));
                println!(
                    "  {label:<width$}  {dur}",
                    label = "Total",
                    width = max_desc_len + 38,
                    dur = format_duration(summary.total),
                );
                0
            }
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        }
    }

    fn format_duration(d: chrono::Duration) -> String {
        let total_secs = d.num_seconds().max(0);
        let h = total_secs / 3600;
        let m = (total_secs % 3600) / 60;
        let s = total_secs % 60;
        format!("{h}:{m:02}:{s:02}")
    }

    /// Restore a soft-deleted tracking by un-deleting it and hard-deleting
    /// all its successors.
    ///
    /// Examples:
    ///   nyd track restore <id>
    pub fn restore(
        #[arg(help = "Tracking entry ID to restore")]
        entry_id: String,
    ) -> u8 {
        use sea_orm::prelude::Uuid;

        let entry_id = match Uuid::parse_str(&entry_id) {
            Ok(id) => id,
            Err(_) => {
                eprintln!("Error: Invalid tracking ID '{}'", entry_id);
                return 1;
            }
        };

        let result = crate::run_async(|module| async move {
            use not_yet_done_task_core::service::TrackingService;
            use shaku::HasComponent;
            let service: &dyn TrackingService = module.resolve_ref();
            service.restore_tracking(entry_id).await
        });

        match result {
            Ok(tracking) => {
                println!(
                    "✓ Tracking restored: [{}] {} → {}",
                    tracking.id,
                    tracking.started_at
                        .with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M"),
                    tracking.ended_at
                        .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| "running".to_string()),
                );
                0
            }
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        }
    }

    /// Split a tracking entry into two at a given time point.
    ///
    /// The original tracking is soft-deleted. Two new trackings are created,
    /// both referencing the original as predecessor. If the original is still
    /// active (running), the first part becomes completed and the second stays active.
    ///
    /// Examples:
    ///   nyd track split <id> "10:30"
    ///   nyd track split <id> "yesterday 14:00" --task <other-task-id>
    pub fn split(
        #[arg(help = "Tracking entry ID to split")]
        entry_id: String,
        #[arg(help = "Time point to split at (e.g. '10:30', 'yesterday 14:00')")]
        at: crate::datetime::LocalDateTime,
        #[arg(long, help = "Assign the second part to a different task")]
        task: Option<String>,
    ) -> u8 {
        use sea_orm::prelude::Uuid;

        let entry_id = match Uuid::parse_str(&entry_id) {
            Ok(id) => id,
            Err(_) => {
                eprintln!("Error: Invalid tracking ID '{}'", entry_id);
                return 1;
            }
        };

        let second_task_id = match task {
            Some(ref id) => match Uuid::parse_str(id) {
                Ok(uid) => Some(uid),
                Err(_) => {
                    eprintln!("Error: Invalid task ID '{}'", id);
                    return 1;
                }
            },
            None => None,
        };

        let at_ctx: not_yet_done_task_core::local_context::LocalContext = at.into();

        let result = crate::run_async(|module| async move {
            use not_yet_done_task_core::service::TrackingService;
            use shaku::HasComponent;
            let service: &dyn TrackingService = module.resolve_ref();
            service.split_tracking(entry_id, at_ctx, second_task_id).await
        });

        match result {
            Ok(split) => {
                use chrono::Local;
                let fmt = |dt: chrono::DateTime<chrono::Utc>| {
                    dt.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string()
                };
                let fmt_opt = |dt: Option<chrono::DateTime<chrono::Utc>>| {
                    dt.map(|d| fmt(d)).unwrap_or_else(|| "running".to_string())
                };
                println!("✓ Tracking split:");
                println!(
                    "  Old:    [{}] {} → {}",
                    split.old_id,
                    fmt(split.old_started_at),
                    fmt_opt(split.old_ended_at),
                );
                println!(
                    "  First:  [{}] {} → {}  ({})",
                    split.first_id,
                    fmt(split.first_started_at),
                    fmt(split.first_ended_at),
                    split.first_task_description,
                );
                println!(
                    "  Second: [{}] {} → {}  ({})",
                    split.second_id,
                    fmt(split.second_started_at),
                    fmt_opt(split.second_ended_at),
                    split.second_task_description,
                );
                0
            }
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        }
    }

    /// Move a completed tracking entry to a new start time.
    ///
    /// Examples:
    ///   nyd track move <id> "yesterday 9am"
    ///   nyd track move <id> "2026-03-22" --gravity end
    ///   nyd track move <id> "today" --gravity start --offset +1h
    ///   nyd track move <id> "2026-03-20" --allow-overlap --allow-future
    pub fn r#move(
        #[arg(help = "Tracking entry ID to move")]
        entry_id: String,
        #[arg(help = "New start time (e.g. 'yesterday 9am', '2026-03-22', 'today 14:00')")]
        start: crate::datetime::LocalDateTime,
        #[arg(long, help = "Allow overlap with other tasks' trackings")]
        allow_overlap: bool,
        #[arg(long, help = "Allow overlap with same task's trackings")]
        allow_same_task_overlap: bool,
        #[arg(long, help = "Allow moving the tracking into the future")]
        allow_future: bool,
        #[arg(
            long,
            value_parser = ["start", "end"],
            help = "Snap to boundary and find next free slot ('start' = forward, 'end' = backward)"
        )]
        gravity: Option<String>,
        #[arg(long, help = "Offset to apply after gravity (e.g. +1h, -30min, +2days)")]
        offset: Option<crate::offset::LocalOffset>,
        #[arg(long, help = "Output result as JSON (for scripting)")]
        json: bool,
    ) -> u8 {
        use not_yet_done_task_core::entity::granularity::Granularity;
        use not_yet_done_task_core::service::{GravityDirection, MoveOptions};
        use sea_orm::prelude::Uuid;

        let entry_id = match Uuid::parse_str(&entry_id) {
            Ok(id) => id,
            Err(_) => {
                eprintln!("Error: Invalid tracking ID '{}'", entry_id);
                return 1;
            }
        };

        let gravity_dir = match gravity.as_deref() {
            Some("start") => Some(GravityDirection::Start),
            Some("end") => Some(GravityDirection::End),
            _ => None,
        };

        let granularity = gravity_dir.as_ref().map(|_| {
            Granularity::from_original(&start.original)
        });

        let options = MoveOptions {
            allow_overlap,
            allow_same_task_overlap,
            allow_future,
            gravity: gravity_dir,
            granularity,
            offset: offset.map(|o| o.duration),
        };

        let start_ctx: not_yet_done_task_core::local_context::LocalContext = start.into();

        let result = crate::run_async(|module| async move {
            use not_yet_done_task_core::service::TrackingService;
            use shaku::HasComponent;
            let service: &dyn TrackingService = module.resolve_ref();
            service.move_tracking(entry_id, start_ctx, options).await
        });

        match result {
            Ok(moved) => {
                if json {
                    use chrono::SecondsFormat;
                    let fmt = |dt: chrono::DateTime<chrono::Utc>| {
                        dt.to_rfc3339_opts(SecondsFormat::Nanos, true)
                    };
                    println!(
                        "{{\"old_id\":\"{}\",\"new_id\":\"{}\",\"new_started_at\":\"{}\",\"new_ended_at\":\"{}\"}}",
                        moved.old_id, moved.new_id,
                        fmt(moved.new_started_at), fmt(moved.new_ended_at),
                    );
                } else {
                    use chrono::Local;
                    println!("✓ Tracking moved:");
                    println!("  Task:  {}", moved.task_description);
                    println!(
                        "  Old:   [{}] {} → {}",
                        moved.old_id,
                        moved.old_started_at
                            .with_timezone(&Local)
                            .format("%Y-%m-%d %H:%M"),
                        moved.old_ended_at
                            .with_timezone(&Local)
                            .format("%Y-%m-%d %H:%M"),
                    );
                    println!(
                        "  New:   [{}] {} → {}",
                        moved.new_id,
                        moved.new_started_at
                            .with_timezone(&Local)
                            .format("%Y-%m-%d %H:%M"),
                        moved.new_ended_at
                            .with_timezone(&Local)
                            .format("%Y-%m-%d %H:%M"),
                    );
                }
                0
            }
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        }
    }

    /// Export trackings as JSON with their associated tasks.
    ///
    /// When IDs are given, exports those specific trackings.
    /// Otherwise exports all trackings, optionally filtered by date range,
    /// task, or active status.
    ///
    /// Examples:
    ///   nyd track export <id1> <id2>
    ///   nyd track export --from "last monday" --to "today"
    ///   nyd track export --task-id <uuid> --sort-by-started-at asc
    ///   nyd track export --active-only --pretty
    pub fn export(
        #[arg(help = "Tracking IDs to export (exports all if omitted)")]
        ids: Vec<String>,
        #[arg(long, help = "Start date/time filter (e.g. '2026-03-01', 'last monday')")]
        from: Option<crate::datetime::LocalDateTime>,
        #[arg(long, help = "End date/time filter (e.g. '2026-03-22', 'today')")]
        to: Option<crate::datetime::LocalDateTime>,
        #[arg(long, help = "Filter by task ID")]
        task_id: Option<String>,
        #[arg(long, help = "Only export active (running) trackings")]
        active_only: bool,
        #[arg(long, value_parser = ["asc", "desc"], help = "Sort by started_at (asc or desc)")]
        sort_by_started_at: Option<String>,
        #[arg(long, help = "Pretty-print the JSON output")]
        pretty: bool,
    ) -> u8 {
        use not_yet_done_task_core::service::{ExportOptions, SortDirection};
        use sea_orm::prelude::Uuid;
        use serde::Serialize;

        // Parse IDs.
        let parsed_ids: Vec<Uuid> = match ids.iter()
            .map(|s| Uuid::parse_str(s).map_err(|_| s.clone()))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(ids) => ids,
            Err(bad) => {
                eprintln!("Error: Invalid tracking ID '{bad}'");
                return 1;
            }
        };

        let parsed_task_id = match task_id {
            Some(ref id) => match Uuid::parse_str(id) {
                Ok(uid) => Some(uid),
                Err(_) => {
                    eprintln!("Error: Invalid task ID '{id}'");
                    return 1;
                }
            },
            None => None,
        };

        let sort_dir = match sort_by_started_at.as_deref() {
            Some("asc") => Some(SortDirection::Asc),
            Some("desc") => Some(SortDirection::Desc),
            _ => None,
        };

        let options = ExportOptions {
            ids: parsed_ids,
            from: from.map(|d| d.utc),
            to: to.map(|d| d.utc),
            task_id: parsed_task_id,
            active_only,
            sort_by_started_at: sort_dir,
        };

        let result = crate::run_async(|module| async move {
            use not_yet_done_task_core::service::TrackingService;
            use shaku::HasComponent;
            let service: &dyn TrackingService = module.resolve_ref();
            service.export_trackings(options).await
        });

        match result {
            Ok(entries) => {
                #[derive(Serialize)]
                struct TrackingJson {
                    id: String,
                    task_id: String,
                    predecessor_id: Option<String>,
                    started_at: String,
                    ended_at: Option<String>,
                    duration_seconds: i64,
                    deleted: bool,
                }

                #[derive(Serialize)]
                struct TaskJson {
                    id: String,
                    description: String,
                    status: String,
                    priority: i32,
                    parent_id: Option<String>,
                    deleted: bool,
                    deleted_at: Option<String>,
                    created_at: String,
                    updated_at: String,
                    last_tracked_at: Option<String>,
                }

                #[derive(Serialize)]
                struct ExportEntry {
                    tracking: TrackingJson,
                    task: TaskJson,
                }

                use chrono::SecondsFormat;
                let fmt = |dt: chrono::DateTime<chrono::Utc>| {
                    dt.to_rfc3339_opts(SecondsFormat::Nanos, true)
                };

                let export: Vec<ExportEntry> = entries.into_iter().map(|e| {
                    ExportEntry {
                        tracking: TrackingJson {
                            id: e.tracking.id.to_string(),
                            task_id: e.tracking.task_id.to_string(),
                            predecessor_id: e.tracking.predecessor_id.map(|id| id.to_string()),
                            started_at: fmt(e.tracking.started_at),
                            ended_at: e.tracking.ended_at.map(fmt),
                            duration_seconds: e.duration_seconds,
                            deleted: e.tracking.deleted,
                        },
                        task: TaskJson {
                            id: e.task.id.to_string(),
                            description: e.task.description,
                            status: format!("{:?}", e.task.status).to_lowercase(),
                            priority: e.task.priority,
                            parent_id: e.task.parent_id.map(|id| id.to_string()),
                            deleted: e.task.deleted,
                            deleted_at: e.task.deleted_at.map(fmt),
                            created_at: fmt(e.task.created_at),
                            updated_at: fmt(e.task.updated_at),
                            last_tracked_at: e.task.last_tracked_at.map(fmt),
                        },
                    }
                }).collect();

                let json_str = if pretty {
                    serde_json::to_string_pretty(&export)
                } else {
                    serde_json::to_string(&export)
                };

                match json_str {
                    Ok(s) => { println!("{s}"); 0 }
                    Err(e) => { eprintln!("Error serializing JSON: {e}"); 1 }
                }
            }
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        }
    }
}
