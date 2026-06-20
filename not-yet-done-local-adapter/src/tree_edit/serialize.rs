//! Serialize a task subtree into a markdown checkbox list.

use std::collections::HashSet;

use not_yet_done_task_core::entity::task::{Model as Task, TaskStatus};
use uuid::Uuid;

/// Serialize the subtree rooted at `root` into markdown.
///
/// Format per line:
/// ```text
/// - [ ] Description  (p=0  id=1d88c39d)
///   - [x] Child task  (p=5  id=a3f2b1c4)
/// ```
#[allow(dead_code)]
pub fn serialize(root: &Task, subtree: &[Task]) -> String {
    serialize_with_indent(root, subtree, 4, &HashSet::new())
}

pub fn serialize_with_indent(
    root: &Task,
    subtree: &[Task],
    indent_size: usize,
    tracked_ids: &HashSet<Uuid>,
) -> String {
    let mut out = String::new();
    write_node(&mut out, root, subtree, 0, indent_size, tracked_ids);
    out
}

fn write_node(
    out: &mut String,
    task: &Task,
    all: &[Task],
    depth: usize,
    indent_size: usize,
    tracked_ids: &HashSet<Uuid>,
) {
    let indent = " ".repeat(depth * indent_size);
    let marker = if task.deleted { 'D' } else { status_marker(&task.status) };
    let short_id = short_id(task.id);
    let flags = if tracked_ids.contains(&task.id) { "-t " } else { "" };

    out.push_str(&format!(
        "{indent}- [{marker}] {flags}{}  (p={}  id={short_id})\n",
        task.description, task.priority,
    ));

    let mut children: Vec<&Task> = all.iter()
        .filter(|t| t.parent_id == Some(task.id) && t.id != task.id)
        .collect();
    // Non-deleted first, then deleted. Within each group, sort by description.
    children.sort_by(|a, b| a.deleted.cmp(&b.deleted).then(a.description.cmp(&b.description)));

    for child in children {
        write_node(out, child, all, depth + 1, indent_size, tracked_ids);
    }
}

fn status_marker(status: &TaskStatus) -> char {
    match status {
        TaskStatus::Todo => ' ',
        TaskStatus::InProgress => '~',
        TaskStatus::Done => 'x',
        TaskStatus::Cancelled => '-',
    }
}

/// First 8 hex chars of a UUID — unique enough within a single subtree.
pub fn short_id(id: Uuid) -> String {
    id.to_string()[..8].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn task(id_prefix: &str, desc: &str, parent_prefix: Option<&str>, status: TaskStatus, priority: i32) -> Task {
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

    #[test]
    fn serialize_flat() {
        let root = task("a1b2c3d4", "Root", None, TaskStatus::Todo, 0);
        let out = serialize(&root, &[root.clone()]);
        assert!(out.starts_with("- [ ] Root  (p=0  id="));
    }

    #[test]
    fn serialize_with_children() {
        let root = task("a1b2c3d4", "Root", None, TaskStatus::Todo, 0);
        let child = task("e5f6a7b8", "Child", Some("a1b2c3d4"), TaskStatus::Done, 3);
        let subtree = vec![root.clone(), child];
        let out = serialize(&root, &subtree);
        assert!(out.contains("- [ ] Root"));
        assert!(out.contains("  - [x] Child  (p=3"));
    }
}
