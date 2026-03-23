use async_trait::async_trait;
use shaku::Component;
use std::sync::Arc;
use uuid::Uuid;

use crate::entity::tracking;
use crate::error::AppError;
use crate::repository::{TaskRepository, TrackingRepository};
use crate::entity::granularity::{Granularity};

pub enum GravityDirection { Start, End }

pub struct StoppedTracking {
    pub tracking: tracking::Model,
    pub task_description: String,
}

pub struct TaskSummary {
    pub task_id: Uuid,
    pub task_description: String,
    pub total_duration: chrono::Duration,
}

pub struct Summary {
    pub entries: Vec<TaskSummary>,
    pub total: chrono::Duration,
}

pub struct MoveOptions {
    pub allow_overlap: bool,
    pub allow_future: bool,
    pub gravity: Option<GravityDirection>,
    pub granularity: Option<Granularity>,
    pub offset: Option<chrono::Duration>,
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

    async fn summary(
        &self,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
        task_id: Option<Uuid>,
    ) -> Result<Summary, AppError>;

    async fn move_tracking(
        &self,
        entry_id: Uuid,
        new_start: chrono::DateTime<chrono::Utc>,
        options: MoveOptions,
    ) -> Result<MovedTracking, AppError>;
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
        // Guard: task must not already have an active tracking
        if let Some(_) = self.tracking_repository.find_active_for_task(task_id).await? {
            return Err(AppError::TrackingAlreadyActive(task_id));
        }

        let now = chrono::Utc::now();

        if !parallel {
            // Stop all other active trackings
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
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
        task_id: Option<Uuid>,
    ) -> Result<Summary, AppError> {
        let now = chrono::Utc::now();
        let trackings = self.tracking_repository.find_in_range(from, to, task_id).await?;

        // Gruppieren nach task_id, Dauer clampen auf [from, to]
        let mut by_task: std::collections::HashMap<Uuid, chrono::Duration> =
        std::collections::HashMap::new();

        for t in &trackings {
            let start = t.started_at.max(from);
            let end = t.ended_at.unwrap_or(now).min(to);
            let duration = end - start;
            if duration > chrono::Duration::zero() {
                *by_task.entry(t.task_id).or_insert(chrono::Duration::zero()) += duration;
            }
        }

        // Task-Beschreibungen holen und Einträge aufbauen
        let mut entries: Vec<TaskSummary> = Vec::new();
        for (tid, duration) in &by_task {
            let task = self.task_repository.find_by_id(*tid).await?;
            entries.push(TaskSummary {
                task_id: *tid,
                task_description: task.description,
                total_duration: *duration,
            });
        }

        // Alphabetisch nach Beschreibung sortieren für stabile Ausgabe
        entries.sort_by(|a, b| a.task_description.cmp(&b.task_description));

        let total = entries.iter().fold(chrono::Duration::zero(), |acc, e| acc + e.total_duration);

        Ok(Summary { entries, total })
    }

    async fn move_tracking(
        &self,
        entry_id: Uuid,
        new_start: chrono::DateTime<chrono::Utc>,
        options: MoveOptions,
    ) -> Result<MovedTracking, AppError> {
        let now = chrono::Utc::now();

        // 1. Tracking laden und validieren
        let tracking = self.tracking_repository
            .find_by_id(entry_id)
            .await?
            .ok_or(AppError::TrackingNotFound(entry_id))?;

        if tracking.ended_at.is_none() {
            return Err(AppError::TrackingStillActive(entry_id));
        }

        let duration = tracking.ended_at.unwrap() - tracking.started_at;

        // 2. Startzeitpunkt berechnen
        let proposed_start = self.resolve_start(new_start, duration, &options)?;

        let candidate_start = match &options.gravity {
            Some(gravity) => {
                self.find_free_slot(
                    proposed_start, duration, entry_id,
                    tracking.task_id, gravity
                ).await?
            }
            None => {
                // Ohne gravity: einmal prüfen, kein iteratives Suchen
                let overlapping = self.tracking_repository
                    .find_overlapping(proposed_start, proposed_start + duration, entry_id)
                .await?;
                if overlapping.iter().any(|t| t.task_id == tracking.task_id) {
                    return Err(AppError::OverlapSameTask);
                }
                if !overlapping.is_empty() && !options.allow_overlap {
                    return Err(AppError::OverlapOtherTask);
                }
                proposed_start
            }
        };

        // 3. Zukunfts-Check
        if !options.allow_future && candidate_start > now {
            return Err(AppError::TrackingInFuture);
        }

        // 4. Task-Beschreibung holen
        let task = self.task_repository.find_by_id(tracking.task_id).await?;

        // 5. Immutability-Pattern: altes löschen, neues anlegen
        let candidate_end = candidate_start + duration;
        let old_id = tracking.id;
        let old_started_at = tracking.started_at;
        let old_ended_at = tracking.ended_at.unwrap();

        // Altes Tracking soft-deleten (ended_at bleibt wie es ist)
        self.tracking_repository.soft_delete_keeping_times(old_id).await?;

        // Neues Tracking anlegen
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
}

impl TrackingServiceImpl {
    fn resolve_start(
        &self,
        new_start: chrono::DateTime<chrono::Utc>,
        duration: chrono::Duration,
        options: &MoveOptions,
    ) -> Result<chrono::DateTime<chrono::Utc>, AppError> {
        // Ohne gravity: direkt new_start + offset
        let Some(ref gravity) = options.gravity else {
            return Ok(new_start + options.offset.unwrap_or(chrono::Duration::zero()));
        };

        let granularity = options.granularity.as_ref()
            .unwrap_or(&Granularity::Day);

        let offset = options.offset.unwrap_or(chrono::Duration::zero());

        match gravity {
            GravityDirection::Start => {
                let snapped = granularity.snap_start(new_start);
                Ok(snapped + offset)
            }
            GravityDirection::End => {
                let snapped = granularity.snap_end(new_start);
                Ok(snapped + offset - duration)
                // snap_end gibt den spätesten Endzeitpunkt,
                // start = end - duration
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

                    // Hinter das letzte überlappende Tracking springen
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

                    // Vor das erste überlappende Tracking springen
                    candidate_end = overlapping.iter()
                        .map(|t| t.started_at)
                        .min()
                        .ok_or(AppError::NoFreeSlot)?;
                }
            }
        }
    }
}
