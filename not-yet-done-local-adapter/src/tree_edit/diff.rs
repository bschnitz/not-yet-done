//! Diff engine: compare parsed tree against original tasks and apply
//! changes via the TaskService.

use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

use not_yet_done_task_core::entity::task::Model as Task;
use not_yet_done_task_core::repository::TrackingRepository;
use not_yet_done_task_core::service::TaskService;

use super::parse::{self, ParsedItem};
use super::serialize::short_id;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Parse the editor content, diff against the originals, and apply all
/// changes sequentially (so new items get UUIDs before their children).
pub async fn apply_changes(
    content: &str,
    original_tasks: &[Task],
    root_id: Uuid,
    task_service: &Arc<dyn TaskService>,
    tracking_repo: &Arc<dyn TrackingRepository>,
    tracked_ids: &HashSet<Uuid>,
    allow_parallel: bool,
) -> Result<String, String> {
    let items = parse::parse(content).map_err(|e| e.to_string())?;

    if items.is_empty() {
        return Err("Empty tree — nothing to apply".into());
    }

    // Validate: if !allow_parallel, at most one item may have -t flag.
    let tracked_items: Vec<&ParsedItem> = items.iter().filter(|i| i.has_flag('t')).collect();
    if !allow_parallel && tracked_items.len() > 1 {
        let names: Vec<&str> = tracked_items
            .iter()
            .map(|i| i.description.as_str())
            .collect();
        return Err(format!(
            "Only one task can be tracked at a time (allow_parallel=false). Tracked: {}",
            names.join(", "),
        ));
    }

    let root_task = original_tasks
        .iter()
        .find(|t| t.id == root_id)
        .ok_or("Root task not found in originals")?;
    let root_parent = root_task.parent_id;

    // id_at_depth[d] = UUID of the last item processed at depth d.
    let mut id_at_depth: Vec<Option<Uuid>> = vec![None; 32];
    let mut seen_ids: Vec<Uuid> = Vec::new();

    let mut created = 0usize;
    let mut updated = 0usize;
    let mut deleted = 0usize;

    for item in &items {
        let depth = item.depth;
        let parent_id = if depth == 0 {
            root_parent
        } else {
            id_at_depth.get(depth - 1).copied().flatten()
        };

        if let Some(ref sid) = item.short_id {
            let full_id = resolve_short_id(sid, original_tasks)?;
            let original = original_tasks
                .iter()
                .find(|t| t.id == full_id)
                .ok_or_else(|| format!("Task {sid} not found"))?;

            seen_ids.push(full_id);
            let parent_changed = parent_id != original.parent_id;
            updated += apply_updates(task_service, full_id, item, original, parent_id).await?;

            // Move notes when parent changes.
            if parent_changed {
                let mut updated_task = original.clone();
                updated_task.parent_id = parent_id;
                crate::notes::move_notes(
                    &updated_task,
                    original_tasks,
                    &build_updated_tasks(original_tasks, full_id, parent_id),
                );
            }

            ensure_depth(&mut id_at_depth, depth);
            id_at_depth[depth] = Some(full_id);
        } else {
            // New item — create immediately so children can reference its UUID.
            let new_task = task_service
                .add_task(
                    item.description.clone(),
                    None,
                    parent_id.map(|id| id.to_string()),
                    None,
                    Some(item.status.clone()),
                    Some(item.priority.unwrap_or(0)),
                )
                .await
                .map_err(|e| format!("Failed to create '{}': {e}", item.description))?;

            ensure_depth(&mut id_at_depth, depth);
            id_at_depth[depth] = Some(new_task.id);
            created += 1;
        }
    }

    // Soft-delete items in the original that are missing from the editor.
    for task in original_tasks {
        if !task.deleted && !seen_ids.contains(&task.id) {
            task_service
                .delete_task(task.id)
                .await
                .map_err(|e| format!("Failed to delete task: {e}"))?;
            deleted += 1;
        }
    }

    // Handle tracking changes.
    let mut tracking_started = 0usize;
    let mut tracking_stopped = 0usize;

    for item in &items {
        if let Some(ref sid) = item.short_id {
            if let Ok(full_id) = resolve_short_id(sid, original_tasks) {
                let was_tracked = tracked_ids.contains(&full_id);
                let wants_tracked = item.has_flag('t');

                if wants_tracked && !was_tracked {
                    // If !allow_parallel, stop all other active trackings first.
                    if !allow_parallel {
                        let active = tracking_repo.find_all_active().await.unwrap_or_default();
                        for t in active {
                            if t.task_id != full_id {
                                let _ = tracking_repo.stop(t.id, chrono::Utc::now()).await;
                            }
                        }
                    }
                    tracking_repo
                        .insert(full_id, chrono::Utc::now(), None)
                        .await
                        .map_err(|e| format!("Failed to start tracking: {e}"))?;
                    tracking_started += 1;
                } else if !wants_tracked && was_tracked {
                    let active = tracking_repo
                        .find_active_for_task(full_id)
                        .await
                        .map_err(|e| format!("Failed to find active tracking: {e}"))?;
                    if let Some(t) = active {
                        tracking_repo
                            .stop(t.id, chrono::Utc::now())
                            .await
                            .map_err(|e| format!("Failed to stop tracking: {e}"))?;
                    }
                    tracking_stopped += 1;
                }
            }
        }
    }

    if created == 0
        && updated == 0
        && deleted == 0
        && tracking_started == 0
        && tracking_stopped == 0
    {
        return Ok("No changes".into());
    }

    let mut parts = Vec::new();
    if created > 0 {
        parts.push(format!("{created} created"));
    }
    if updated > 0 {
        parts.push(format!("{updated} updated"));
    }
    if deleted > 0 {
        parts.push(format!("{deleted} deleted"));
    }
    if tracking_started > 0 {
        parts.push(format!("{tracking_started} tracking started"));
    }
    if tracking_stopped > 0 {
        parts.push(format!("{tracking_stopped} tracking stopped"));
    }
    Ok(parts.join(", "))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Apply field-level updates for an existing task. Returns the number of
/// service calls made.
async fn apply_updates(
    service: &Arc<dyn TaskService>,
    id: Uuid,
    item: &ParsedItem,
    original: &Task,
    parent_id: Option<Uuid>,
) -> Result<usize, String> {
    let desc_changed = item.description != original.description;
    let status_changed = item.status != original.status;
    let priority_changed = item.priority.is_some_and(|p| p != original.priority);
    let parent_changed = parent_id != original.parent_id;
    let deleted_changed = item.deleted != original.deleted;

    if !desc_changed && !status_changed && !priority_changed && !parent_changed && !deleted_changed
    {
        return Ok(0);
    }

    service
        .update_task(
            id,
            if desc_changed {
                Some(item.description.clone())
            } else {
                None
            },
            if status_changed {
                Some(item.status.clone())
            } else {
                None
            },
            if priority_changed {
                item.priority
            } else {
                None
            },
            if parent_changed {
                Some(parent_id)
            } else {
                None
            },
            if deleted_changed {
                Some(item.deleted)
            } else {
                None
            },
        )
        .await
        .map_err(|e| format!("Failed to update task: {e}"))?;

    Ok(1)
}

fn resolve_short_id(short: &str, tasks: &[Task]) -> Result<Uuid, String> {
    let matches: Vec<&Task> = tasks.iter().filter(|t| short_id(t.id) == short).collect();
    match matches.len() {
        0 => Err(format!("Unknown id={short}")),
        1 => Ok(matches[0].id),
        _ => Err(format!(
            "Ambiguous id={short} (matches {} tasks)",
            matches.len()
        )),
    }
}

/// Build a snapshot of tasks with one task's parent_id updated.
/// Used to compute the new notes path after a reparent.
fn build_updated_tasks(
    original_tasks: &[Task],
    task_id: Uuid,
    new_parent: Option<Uuid>,
) -> Vec<Task> {
    original_tasks
        .iter()
        .map(|t| {
            if t.id == task_id {
                let mut updated = t.clone();
                updated.parent_id = new_parent;
                updated
            } else {
                t.clone()
            }
        })
        .collect()
}

fn ensure_depth(id_at_depth: &mut Vec<Option<Uuid>>, depth: usize) {
    while id_at_depth.len() <= depth + 1 {
        id_at_depth.push(None);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use not_yet_done_task_core::entity::task::TaskStatus;

    fn task(
        id_prefix: &str,
        desc: &str,
        parent_prefix: Option<&str>,
        status: TaskStatus,
        priority: i32,
    ) -> Task {
        Task {
            id: make_uuid(id_prefix),
            description: desc.to_string(),
            status,
            deleted: false,
            deleted_at: None,
            priority,
            parent_id: parent_prefix.map(make_uuid),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_tracked_at: None,
            path: None,
        }
    }

    fn make_uuid(prefix: &str) -> Uuid {
        let hex: String = prefix.chars().cycle().take(32).collect();
        Uuid::parse_str(&hex).unwrap()
    }

    // Note: apply_changes is async and requires a TaskService, so we only
    // test parsing + structure here. Integration tests with a real DB would
    // cover the full flow.

    #[test]
    fn parse_round_trip_no_changes() {
        let root = task("a1b2c3d4", "Root", None, TaskStatus::Todo, 0);
        let tasks = vec![root.clone()];
        let content = crate::tree_edit::serialize(&root, &tasks);
        let items = parse::parse(&content).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].description, "Root");
        assert_eq!(items[0].status, TaskStatus::Todo);
    }

    #[test]
    fn parse_detects_new_items() {
        let root = task("a1b2c3d4", "Root", None, TaskStatus::Todo, 0);
        let sid = short_id(root.id);
        let content =
            format!("- [ ] Root  (p=0  id={sid})\n  - [ ] New child\n    - [x] New grandchild\n");
        let items = parse::parse(&content).unwrap();
        assert_eq!(items.len(), 3);
        assert!(items[1].short_id.is_none()); // new
        assert!(items[2].short_id.is_none()); // new
        assert_eq!(items[1].depth, 1);
        assert_eq!(items[2].depth, 2);
    }

    #[test]
    fn parse_nested_new_items_have_correct_depth() {
        let root = task("a1b2c3d4", "Root", None, TaskStatus::Todo, 0);
        let sid = short_id(root.id);
        let content = format!(
            "- [ ] Root  (p=0  id={sid})\n  - [ ] New parent\n    - [ ] New child\n      - [x] New grandchild\n"
        );
        let items = parse::parse(&content).unwrap();
        assert_eq!(items.len(), 4);
        // Root at depth 0.
        assert_eq!(items[0].depth, 0);
        assert!(items[0].short_id.is_some());
        // New parent at depth 1 — will be created as child of root.
        assert_eq!(items[1].depth, 1);
        assert!(items[1].short_id.is_none());
        assert_eq!(items[1].description, "New parent");
        // New child at depth 2 — will be created as child of "New parent".
        assert_eq!(items[2].depth, 2);
        assert!(items[2].short_id.is_none());
        assert_eq!(items[2].description, "New child");
        // New grandchild at depth 3.
        assert_eq!(items[3].depth, 3);
        assert_eq!(items[3].status, TaskStatus::Done);
    }

    #[test]
    fn parse_detects_status_change() {
        let root = task("a1b2c3d4", "Root", None, TaskStatus::Todo, 0);
        let sid = short_id(root.id);
        let content = format!("- [x] Root  (p=0  id={sid})\n");
        let items = parse::parse(&content).unwrap();
        assert_eq!(items[0].status, TaskStatus::Done);
    }
}
