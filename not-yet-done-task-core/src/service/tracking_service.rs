use async_trait::async_trait;
use chrono::{Datelike, NaiveDate};
use shaku::Component;
use std::sync::Arc;
use uuid::Uuid;
use chrono::TimeZone;

use crate::entity::granularity::Granularity;
use crate::entity::tracking;
use crate::error::AppError;
use crate::local_context::LocalContext;
use crate::repository::{TaskRepository, TrackingRepository};

pub enum GravityDirection {
    Start,
    End,
}

pub struct StoppedTracking {
    pub tracking: tracking::Model,
    pub task_description: String,
}

pub struct TaskSummary {
    pub task_id: Uuid,
    pub task_description: String,
    pub total_duration: chrono::Duration,
}

/// Summary for a single local calendar day.
pub struct DaySummary {
    /// The local date this summary covers.
    pub date: NaiveDate,
    /// Per-task breakdown for this day, sorted alphabetically by description.
    pub entries: Vec<TaskSummary>,
    /// Sum of all task durations for this day.
    pub day_total: chrono::Duration,
}

/// Full summary across all days in the requested range.
pub struct Summary {
    /// One entry per local calendar day that has tracked time, sorted ascending.
    pub days: Vec<DaySummary>,
    /// Grand total across all days.
    pub total: chrono::Duration,
}

/// Options for exporting trackings.
pub struct ExportOptions {
    pub ids: Vec<Uuid>,
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    pub to: Option<chrono::DateTime<chrono::Utc>>,
    pub task_id: Option<Uuid>,
    pub active_only: bool,
    pub sort_by_started_at: Option<SortDirection>,
}

#[derive(Clone, Copy)]
pub enum SortDirection {
    Asc,
    Desc,
}

/// A single exported tracking with its associated task.
pub struct ExportedTracking {
    pub tracking: tracking::Model,
    pub task: crate::entity::task::Model,
    pub duration_seconds: i64,
}

pub struct MoveOptions {
    pub allow_overlap: bool,
    pub allow_same_task_overlap: bool,
    pub allow_future: bool,
    pub gravity: Option<GravityDirection>,
    pub granularity: Option<Granularity>,
    pub offset: Option<chrono::Duration>,
}

pub struct SplitTracking {
    pub old_id: Uuid,
    pub old_started_at: chrono::DateTime<chrono::Utc>,
    pub old_ended_at: Option<chrono::DateTime<chrono::Utc>>,
    pub first_id: Uuid,
    pub first_started_at: chrono::DateTime<chrono::Utc>,
    pub first_ended_at: chrono::DateTime<chrono::Utc>,
    pub second_id: Uuid,
    pub second_started_at: chrono::DateTime<chrono::Utc>,
    pub second_ended_at: Option<chrono::DateTime<chrono::Utc>>,
    pub first_task_description: String,
    pub second_task_description: String,
}

pub struct MovedTracking {
    pub old_id: Uuid,
    pub old_started_at: chrono::DateTime<chrono::Utc>,
    pub old_ended_at: chrono::DateTime<chrono::Utc>,
    pub new_id: Uuid,
    pub new_started_at: chrono::DateTime<chrono::Utc>,
    pub new_ended_at: chrono::DateTime<chrono::Utc>,
    pub task_description: String,
}

#[async_trait]
pub trait TrackingService: shaku::Interface {
    /// Start tracking a task.
    ///
    /// If `parallel` is false (default), all other active trackings are stopped first.
    /// Returns an error if the task already has an active tracking.
    async fn start(&self, task_id: Uuid, parallel: bool) -> Result<tracking::Model, AppError>;

    /// Stop tracking for a specific task, or all active trackings if task_id is None.
    /// Returns all stopped tracking entries.
    async fn stop(&self, task_id: Option<Uuid>) -> Result<Vec<StoppedTracking>, AppError>;

    /// Return a summary of tracked time grouped by local calendar day and task.
    ///
    /// `from` and `to` carry the user's local timezone so that day boundaries
    /// are computed correctly.  Both bounds are inclusive.
    async fn summary(
        &self,
        from: LocalContext,
        to: LocalContext,
        task_id: Option<Uuid>,
    ) -> Result<Summary, AppError>;

    async fn move_tracking(
        &self,
        entry_id: Uuid,
        new_start: LocalContext,
        options: MoveOptions,
    ) -> Result<MovedTracking, AppError>;

    /// Export trackings with their associated tasks.
    async fn export_trackings(
        &self,
        options: ExportOptions,
    ) -> Result<Vec<ExportedTracking>, AppError>;

    /// Restore a soft-deleted tracking by un-deleting it and hard-deleting
    /// all its successors (trackings that have it as predecessor, recursively).
    async fn restore_tracking(&self, entry_id: Uuid) -> Result<tracking::Model, AppError>;

    /// Split a tracking at a given time point into two new trackings.
    ///
    /// The original tracking is soft-deleted. Both new trackings reference
    /// the original as predecessor. If the original is active (no ended_at),
    /// the first part becomes completed and the second part stays active.
    /// An optional `second_task_id` assigns the second part to a different task.
    async fn split_tracking(
        &self,
        entry_id: Uuid,
        at: LocalContext,
        second_task_id: Option<Uuid>,
    ) -> Result<SplitTracking, AppError>;
}

#[derive(Component)]
#[shaku(interface = TrackingService)]
pub struct TrackingServiceImpl {
    #[shaku(inject)]
    tracking_repository: Arc<dyn TrackingRepository>,
    #[shaku(inject)]
    task_repository: Arc<dyn TaskRepository>,
}

#[async_trait]
impl TrackingService for TrackingServiceImpl {
    async fn start(
        &self,
        task_id: Uuid,
        parallel: bool,
    ) -> Result<tracking::Model, AppError> {
        // Guard: task must exist
        self.task_repository.find_by_id(task_id).await?;

        // Guard: task must not already have an active tracking
        if self.tracking_repository.find_active_for_task(task_id).await?.is_some() {
            return Err(AppError::TrackingAlreadyActive(task_id));
        }

        let now = chrono::Utc::now();

        if !parallel {
            let active = self.tracking_repository.find_all_active().await?;
            for t in active {
                if t.task_id != task_id {
                    self.tracking_repository.stop(t.id, now).await?;
                }
            }
        }

        self.tracking_repository.insert(task_id, now, None).await
    }

    async fn stop(&self, task_id: Option<Uuid>) -> Result<Vec<StoppedTracking>, AppError> {
        let now = chrono::Utc::now();

        let to_stop = match task_id {
            Some(id) => {
                let t = self.tracking_repository
                    .find_active_for_task(id)
                    .await?
                    .ok_or(AppError::NoActiveTracking(id))?;
                vec![t]
            }
            None => self.tracking_repository.find_all_active().await?,
        };

        if to_stop.is_empty() {
            return Err(AppError::NoActiveTrackingAny);
        }

        let mut result = Vec::new();
        for t in to_stop {
            let task = self.task_repository.find_by_id(t.task_id).await?;
            let stopped = self.tracking_repository.stop(t.id, now).await?;
            result.push(StoppedTracking {
                tracking: stopped,
                task_description: task.description,
            });
        }
        Ok(result)
    }

    async fn summary(
        &self,
        from: LocalContext,
        to: LocalContext,
        task_id: Option<Uuid>,
    ) -> Result<Summary, AppError> {
        let now = chrono::Utc::now();
        // Use the timezone from `from` for all day-boundary calculations.
        // `to` is expected to carry the same timezone (same user session).
        let tz = from.timezone;

        let trackings = self.tracking_repository
            .find_in_range(from.utc, to.utc, task_id)
            .await?;

        // Group durations by (local_date, task_id), clamping each tracking to
        // the requested [from, to] window.
        let mut by_day_task: std::collections::BTreeMap<
            (NaiveDate, Uuid),
            chrono::Duration,
        > = std::collections::BTreeMap::new();

        for t in &trackings {
            let start_utc = t.started_at.max(from.utc);
            let end_utc = t.ended_at.unwrap_or(now).min(to.utc);
            if end_utc <= start_utc {
                continue;
            }

            // Walk through each local day this tracking spans and accumulate
            // only the portion that falls within that day.
            let mut cursor = start_utc;
            while cursor < end_utc {
                let local_cursor = cursor.with_timezone(&tz);
                let local_date = local_cursor.date_naive();

                // End of the current local day in UTC
                let day_end_local = tz
                    .with_ymd_and_hms(
                        local_date.year(),
                        local_date.month(),
                        local_date.day(),
                        23, 59, 59,
                    )
                    .unwrap();
                let day_end_utc = day_end_local.to_utc() + chrono::Duration::seconds(1);

                let slice_end = end_utc.min(day_end_utc);
                let duration = slice_end - cursor;

                if duration > chrono::Duration::zero() {
                    *by_day_task
                        .entry((local_date, t.task_id))
                        .or_insert(chrono::Duration::zero()) += duration;
                }

                cursor = day_end_utc;
            }
        }

        // Build the per-day summaries.
        // BTreeMap iteration is already sorted by (date, task_id).
        let mut days_map: std::collections::BTreeMap<NaiveDate, Vec<(Uuid, chrono::Duration)>> =
            std::collections::BTreeMap::new();
        for ((date, tid), duration) in &by_day_task {
            days_map.entry(*date).or_default().push((*tid, *duration));
        }

        let mut total = chrono::Duration::zero();
        let mut days: Vec<DaySummary> = Vec::new();

        for (date, task_durations) in days_map {
            // Resolve task descriptions and build entries
            let mut entries: Vec<TaskSummary> = Vec::new();
            for (tid, duration) in task_durations {
                let task = self.task_repository.find_by_id(tid).await?;
                entries.push(TaskSummary {
                    task_id: tid,
                    task_description: task.description,
                    total_duration: duration,
                });
            }
            entries.sort_by(|a, b| a.task_description.cmp(&b.task_description));

            let day_total = entries
                .iter()
                .fold(chrono::Duration::zero(), |acc, e| acc + e.total_duration);
            total += day_total;

            days.push(DaySummary { date, entries, day_total });
        }

        Ok(Summary { days, total })
    }

    async fn move_tracking(
        &self,
        entry_id: Uuid,
        new_start: LocalContext,
        options: MoveOptions,
    ) -> Result<MovedTracking, AppError> {
        let now = chrono::Utc::now();

        let tracking = self.tracking_repository
            .find_by_id(entry_id)
            .await?
            .ok_or(AppError::TrackingNotFound(entry_id))?;

        if tracking.ended_at.is_none() {
            return Err(AppError::TrackingStillActive(entry_id));
        }

        let duration = tracking.ended_at.unwrap() - tracking.started_at;

        let proposed_start = self.resolve_start(new_start, duration, &options)?;

        let candidate_start = match &options.gravity {
            Some(gravity) => {
                self.find_free_slot(
                    proposed_start, duration, entry_id, tracking.task_id, gravity,
                ).await?
            }
            None => {
                let overlapping = self.tracking_repository
                    .find_overlapping(proposed_start, proposed_start + duration, entry_id)
                    .await?;
                if !options.allow_same_task_overlap
                    && overlapping.iter().any(|t| t.task_id == tracking.task_id)
                {
                    return Err(AppError::OverlapSameTask);
                }
                if !overlapping.is_empty() && !options.allow_overlap {
                    return Err(AppError::OverlapOtherTask);
                }
                proposed_start
            }
        };

        if !options.allow_future && candidate_start > now {
            return Err(AppError::TrackingInFuture);
        }

        let task = self.task_repository.find_by_id(tracking.task_id).await?;

        let candidate_end = candidate_start + duration;
        let old_id = tracking.id;
        let old_started_at = tracking.started_at;
        let old_ended_at = tracking.ended_at.unwrap();

        self.tracking_repository.soft_delete_keeping_times(old_id).await?;

        let new_tracking = self.tracking_repository
            .insert_with_end(tracking.task_id, candidate_start, candidate_end, Some(old_id))
            .await?;

        Ok(MovedTracking {
            old_id,
            old_started_at,
            old_ended_at,
            new_id: new_tracking.id,
            new_started_at: new_tracking.started_at,
            new_ended_at: new_tracking.ended_at.unwrap(),
            task_description: task.description,
        })
    }

    async fn export_trackings(
        &self,
        options: ExportOptions,
    ) -> Result<Vec<ExportedTracking>, AppError> {
        let now = chrono::Utc::now();

        // Collect trackings based on provided options.
        let mut trackings: Vec<tracking::Model> = if !options.ids.is_empty() {
            // Fetch by explicit IDs.
            let mut result = Vec::new();
            for id in &options.ids {
                if let Some(t) = self.tracking_repository.find_by_id(*id).await? {
                    result.push(t);
                }
            }
            result
        } else if options.from.is_some() || options.to.is_some() {
            // Date range query.
            let from = options.from.unwrap_or(chrono::DateTime::UNIX_EPOCH);
            let to = options.to.unwrap_or(now);
            self.tracking_repository.find_in_range(from, to, options.task_id).await?
        } else if let Some(task_id) = options.task_id {
            // All trackings for a specific task.
            self.tracking_repository.find_in_range(
                chrono::DateTime::UNIX_EPOCH, now, Some(task_id),
            ).await?
        } else if options.active_only {
            self.tracking_repository.find_all_active().await?
        } else {
            self.tracking_repository.find_all().await?
        };

        // Filter: active only.
        if options.active_only && !options.ids.is_empty() {
            trackings.retain(|t| t.ended_at.is_none());
        }

        // Filter: task_id (when fetched by IDs).
        if let Some(task_id) = options.task_id {
            if !options.ids.is_empty() {
                trackings.retain(|t| t.task_id == task_id);
            }
        }

        // Sort.
        if let Some(dir) = options.sort_by_started_at {
            match dir {
                SortDirection::Asc => trackings.sort_by_key(|t| t.started_at),
                SortDirection::Desc => {
                    trackings.sort_by_key(|t| t.started_at);
                    trackings.reverse();
                }
            }
        }

        // Build export entries with tasks.
        let mut result = Vec::with_capacity(trackings.len());
        // Cache tasks to avoid repeated lookups.
        let mut task_cache: std::collections::HashMap<Uuid, crate::entity::task::Model> =
            std::collections::HashMap::new();

        for t in trackings {
            let task = if let Some(cached) = task_cache.get(&t.task_id) {
                cached.clone()
            } else {
                let task = self.task_repository.find_by_id(t.task_id).await?;
                task_cache.insert(t.task_id, task.clone());
                task
            };

            let duration_seconds = (t.ended_at.unwrap_or(now) - t.started_at).num_seconds();

            result.push(ExportedTracking {
                tracking: t,
                task,
                duration_seconds,
            });
        }

        Ok(result)
    }

    async fn restore_tracking(&self, entry_id: Uuid) -> Result<tracking::Model, AppError> {
        let tracking = self.tracking_repository
            .find_by_id(entry_id)
            .await?
            .ok_or(AppError::TrackingNotFound(entry_id))?;

        if !tracking.deleted {
            return Err(AppError::TrackingNotDeleted(entry_id));
        }

        // Recursively hard-delete all successors.
        self.hard_delete_successors(entry_id).await?;

        // Un-delete the original.
        self.tracking_repository.undelete(entry_id).await?;

        self.tracking_repository
            .find_by_id(entry_id)
            .await?
            .ok_or(AppError::TrackingNotFound(entry_id))
    }

    async fn split_tracking(
        &self,
        entry_id: Uuid,
        at: LocalContext,
        second_task_id: Option<Uuid>,
    ) -> Result<SplitTracking, AppError> {
        let tracking = self.tracking_repository
            .find_by_id(entry_id)
            .await?
            .ok_or(AppError::TrackingNotFound(entry_id))?;

        let split_at = at.utc;
        let is_active = tracking.ended_at.is_none();
        let effective_end = tracking.ended_at.unwrap_or(chrono::Utc::now());

        // Validate split point is strictly between start and end.
        if split_at <= tracking.started_at || split_at >= effective_end {
            return Err(AppError::SplitPointOutOfRange);
        }

        let second_task = second_task_id.unwrap_or(tracking.task_id);
        // Validate second task exists if different.
        if second_task != tracking.task_id {
            self.task_repository.find_by_id(second_task).await?;
        }

        let first_task_desc = self.task_repository.find_by_id(tracking.task_id).await?.description;
        let second_task_desc = if second_task == tracking.task_id {
            first_task_desc.clone()
        } else {
            self.task_repository.find_by_id(second_task).await?.description
        };

        // Soft-delete original.
        self.tracking_repository.soft_delete_keeping_times(entry_id).await?;

        // Create first part: started_at → split_at (always completed).
        let first = self.tracking_repository
            .insert_with_end(tracking.task_id, tracking.started_at, split_at, Some(entry_id))
            .await?;

        // Create second part: split_at → ended_at (active if original was active).
        let second = if is_active {
            self.tracking_repository
                .insert(second_task, split_at, Some(entry_id))
                .await?
        } else {
            self.tracking_repository
                .insert_with_end(second_task, split_at, tracking.ended_at.unwrap(), Some(entry_id))
                .await?
        };

        Ok(SplitTracking {
            old_id: entry_id,
            old_started_at: tracking.started_at,
            old_ended_at: tracking.ended_at,
            first_id: first.id,
            first_started_at: first.started_at,
            first_ended_at: split_at,
            second_id: second.id,
            second_started_at: split_at,
            second_ended_at: second.ended_at,
            first_task_description: first_task_desc,
            second_task_description: second_task_desc,
        })
    }
}

impl TrackingServiceImpl {
    async fn hard_delete_successors(&self, root_id: Uuid) -> Result<(), AppError> {
        // Iterative BFS to avoid async recursion boxing.
        let mut queue = vec![root_id];
        let mut to_delete = Vec::new();
        while let Some(id) = queue.pop() {
            let successors = self.tracking_repository.find_by_predecessor(id).await?;
            for s in successors {
                queue.push(s.id);
                to_delete.push(s.id);
            }
        }
        // Delete in reverse (leaves first).
        for id in to_delete.into_iter().rev() {
            self.tracking_repository.hard_delete(id).await?;
        }
        Ok(())
    }

    fn resolve_start(
        &self,
        new_start: LocalContext,
        duration: chrono::Duration,
        options: &MoveOptions,
    ) -> Result<chrono::DateTime<chrono::Utc>, AppError> {
        let offset = options.offset.unwrap_or(chrono::Duration::zero());

        let Some(ref gravity) = options.gravity else {
            return Ok(new_start.utc + offset);
        };

        let granularity = options.granularity.as_ref().unwrap_or(&Granularity::Day);

        match gravity {
            GravityDirection::Start => {
                let snapped = granularity.snap_start(new_start.utc, new_start.timezone);
                Ok(snapped + offset)
            }
            GravityDirection::End => {
                let snapped = granularity.snap_end(new_start.utc, new_start.timezone);
                Ok(snapped + offset - duration)
            }
        }
    }

    async fn find_free_slot(
        &self,
        proposed_start: chrono::DateTime<chrono::Utc>,
        duration: chrono::Duration,
        exclude_id: Uuid,
        task_id: Uuid,
        gravity: &GravityDirection,
    ) -> Result<chrono::DateTime<chrono::Utc>, AppError> {
        match gravity {
            GravityDirection::Start => {
                let mut candidate = proposed_start;
                loop {
                    let overlapping = self.tracking_repository
                        .find_overlapping(candidate, candidate + duration, exclude_id)
                        .await?;
                    if overlapping.is_empty() {
                        return Ok(candidate);
                    }
                    if overlapping.iter().any(|t| t.task_id == task_id) {
                        return Err(AppError::OverlapSameTask);
                    }
                    candidate = overlapping.iter()
                        .filter_map(|t| t.ended_at)
                        .max()
                        .ok_or(AppError::NoFreeSlot)?;
                }
            }
            GravityDirection::End => {
                let mut candidate_end = proposed_start + duration;
                loop {
                    let overlapping = self.tracking_repository
                        .find_overlapping(candidate_end - duration, candidate_end, exclude_id)
                        .await?;
                    if overlapping.is_empty() {
                        return Ok(candidate_end - duration);
                    }
                    if overlapping.iter().any(|t| t.task_id == task_id) {
                        return Err(AppError::OverlapSameTask);
                    }
                    candidate_end = overlapping.iter()
                        .map(|t| t.started_at)
                        .min()
                        .ok_or(AppError::NoFreeSlot)?;
                }
            }
        }
    }
}
