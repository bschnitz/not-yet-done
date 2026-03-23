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
            use not_yet_done_core::service::TrackingService;
            use sea_orm::prelude::Uuid;
            use shaku::HasComponent;
            let task_id = Uuid::parse_str(&task_id)
                .map_err(|_| not_yet_done_core::error::AppError::InvalidId(task_id))?;
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
            use not_yet_done_core::service::TrackingService;
            use sea_orm::prelude::Uuid;
            use shaku::HasComponent;

            let task_id = match task_id {
                Some(id) => Some(
                    Uuid::parse_str(&id)
                        .map_err(|_| not_yet_done_core::error::AppError::InvalidId(id))?,
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
            not_yet_done_core::local_context::LocalContext::new(utc, tz)
        });

        let to_ctx = to.map(|d| d.into()).unwrap_or_else(|| {
            let utc = Local
                .from_local_datetime(
                    &now.date_naive().and_hms_opt(23, 59, 59).unwrap(),
                )
                .single()
                .unwrap()
                .to_utc();
            not_yet_done_core::local_context::LocalContext::new(utc, tz)
        });

        if from_ctx.utc > to_ctx.utc {
            eprintln!("Error: --from must not be after --to");
            return 1;
        }

        let result = crate::run_async(|module| async move {
            use not_yet_done_core::service::TrackingService;
            use sea_orm::prelude::Uuid;
            use shaku::HasComponent;

            let task_id = match task_id {
                Some(id) => Some(
                    Uuid::parse_str(&id)
                        .map_err(|_| not_yet_done_core::error::AppError::InvalidId(id))?,
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
    ) -> u8 {
        use not_yet_done_core::entity::granularity::Granularity;
        use not_yet_done_core::service::{GravityDirection, MoveOptions};
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
            allow_future,
            gravity: gravity_dir,
            granularity,
            offset: offset.map(|o| o.duration),
        };

        let start_ctx: not_yet_done_core::local_context::LocalContext = start.into();

        let result = crate::run_async(|module| async move {
            use not_yet_done_core::service::TrackingService;
            use shaku::HasComponent;
            let service: &dyn TrackingService = module.resolve_ref();
            service.move_tracking(entry_id, start_ctx, options).await
        });

        match result {
            Ok(moved) => {
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
                0
            }
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        }
    }
}
