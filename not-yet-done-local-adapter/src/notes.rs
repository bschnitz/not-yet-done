//! Task notes: per-task .md files in a hierarchical directory structure.
//!
//! Layout:
//!   <notes_dir>/
//!     <id>_<sanitized_description>.md
//!     <id>_<sanitized_description>/
//!       <child_id>_<child_desc>.md
//!       ...

use std::path::PathBuf;
use uuid::Uuid;
use not_yet_done_task_core::entity::task::Model as Task;

/// Base directory for task notes.
pub fn notes_dir() -> PathBuf {
    dirs::data_local_dir()
        .expect("Could not determine data directory")
        .join("not_yet_done")
        .join("notes")
}

/// Sanitize a task description for use in a filename.
/// Uses slug::slugify for Unicode transliteration, max 50 chars.
fn sanitize(desc: &str) -> String {
    let s = slug::slugify(desc);
    if s.len() > 50 {
        // Truncate at a hyphen boundary if possible.
        let truncated = &s[..50];
        if let Some(last_hyphen) = truncated.rfind('-') {
            truncated[..last_hyphen].to_string()
        } else {
            truncated.to_string()
        }
    } else {
        s
    }
}

/// Stem for a task: `{short_id}_{sanitized_description}`.
fn task_stem(id: Uuid, description: &str) -> String {
    let short_id = id.to_string()[..8].to_string();
    let desc = sanitize(description);
    if desc.is_empty() {
        short_id
    } else {
        format!("{}_{}", short_id, desc)
    }
}

/// Compute the notes directory for a task's children, based on parent chain.
/// Returns the directory where this task's notes file and children folder live.
pub fn task_notes_parent_dir(task: &Task, all_tasks: &[Task]) -> PathBuf {
    let mut chain = Vec::new();
    let mut current = task.parent_id;
    while let Some(pid) = current {
        if let Some(parent) = all_tasks.iter().find(|t| t.id == pid) {
            chain.push(task_stem(parent.id, &parent.description));
            current = parent.parent_id;
        } else {
            break;
        }
    }
    chain.reverse();
    let mut dir = notes_dir();
    for segment in chain {
        dir = dir.join(segment);
    }
    dir
}

/// Full path to the notes .md file for a task.
pub fn notes_path(task: &Task, all_tasks: &[Task]) -> PathBuf {
    let dir = task_notes_parent_dir(task, all_tasks);
    dir.join(format!("{}.md", task_stem(task.id, &task.description)))
}

/// Find an existing notes file for a task by its ID prefix, regardless of description.
/// Returns the path if found.
pub fn find_notes_file(task: &Task, all_tasks: &[Task]) -> Option<PathBuf> {
    let dir = task_notes_parent_dir(task, all_tasks);
    let prefix = format!("{}_", &task.id.to_string()[..8]);
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&prefix) && name.ends_with(".md") {
                return Some(entry.path());
            }
        }
    }
    None
}

/// Find an existing notes subdirectory for a task by its ID prefix.
pub fn find_notes_dir(task: &Task, all_tasks: &[Task]) -> Option<PathBuf> {
    let dir = task_notes_parent_dir(task, all_tasks);
    let prefix = format!("{}_", &task.id.to_string()[..8]);
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&prefix) && entry.path().is_dir() {
                return Some(entry.path());
            }
        }
    }
    None
}

/// Read notes content for a task. Returns empty string if no file exists.
pub fn read_notes(task: &Task, all_tasks: &[Task]) -> String {
    find_notes_file(task, all_tasks)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default()
}

/// Delete the notes file for a task (when content was cleared).
pub fn delete_notes(task: &Task, all_tasks: &[Task]) {
    if let Some(path) = find_notes_file(task, all_tasks) {
        let _ = std::fs::remove_file(&path);
    }
}

/// Write notes content for a task. Creates directories as needed.
/// If content is empty, does nothing (doesn't create empty files).
pub fn write_notes(task: &Task, all_tasks: &[Task], content: &str) {
    if content.trim().is_empty() {
        return;
    }
    let path = notes_path(task, all_tasks);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, content);
}

/// Move notes file and directory when a task's parent changes.
/// `old_tasks` reflects the hierarchy before the change,
/// `new_tasks` reflects the hierarchy after the change.
pub fn move_notes(task: &Task, old_tasks: &[Task], new_tasks: &[Task]) {
    let old_dir = task_notes_parent_dir(task, old_tasks);
    let new_dir = task_notes_parent_dir(task, new_tasks);
    if old_dir == new_dir {
        return;
    }

    let stem = task_stem(task.id, &task.description);

    // Move .md file.
    let old_file = old_dir.join(format!("{stem}.md"));
    if old_file.exists() {
        let _ = std::fs::create_dir_all(&new_dir);
        let new_file = new_dir.join(format!("{stem}.md"));
        let _ = std::fs::rename(&old_file, &new_file);
    }

    // Move subdirectory (children notes).
    let old_subdir = old_dir.join(&stem);
    if old_subdir.exists() && old_subdir.is_dir() {
        let _ = std::fs::create_dir_all(&new_dir);
        let new_subdir = new_dir.join(&stem);
        let _ = std::fs::rename(&old_subdir, &new_subdir);
    }
}

/// Rename notes file and directory when a task's description changes.
pub fn rename_notes(task: &Task, old_description: &str, new_description: &str, all_tasks: &[Task]) {
    let dir = task_notes_parent_dir(task, all_tasks);
    let old_stem = task_stem(task.id, old_description);
    let new_stem = task_stem(task.id, new_description);
    if old_stem == new_stem {
        return;
    }

    // Rename .md file.
    let old_file = dir.join(format!("{}.md", old_stem));
    let new_file = dir.join(format!("{}.md", new_stem));
    if old_file.exists() {
        let _ = std::fs::rename(&old_file, &new_file);
    }

    // Rename subdirectory.
    let old_dir = dir.join(&old_stem);
    let new_dir = dir.join(&new_stem);
    if old_dir.exists() && old_dir.is_dir() {
        let _ = std::fs::rename(&old_dir, &new_dir);
    }
}

/// Mark a task's notes as deleted by renaming with _deleted_at_ suffix.
pub fn mark_notes_deleted(task: &Task, all_tasks: &[Task]) {
    let now = chrono::Local::now().format("%Y-%m-%dT%H-%M-%S").to_string();

    if let Some(path) = find_notes_file(task, all_tasks) {
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let new_name = format!("{}_deleted_at_{}.md", stem, now);
        let new_path = path.with_file_name(new_name);
        let _ = std::fs::rename(&path, &new_path);
    }

    if let Some(dir_path) = find_notes_dir(task, all_tasks) {
        let name = dir_path.file_name().unwrap_or_default().to_string_lossy();
        let new_name = format!("{}_deleted_at_{}", name, now);
        let new_path = dir_path.with_file_name(new_name);
        let _ = std::fs::rename(&dir_path, &new_path);
    }
}

/// Undo deletion: remove _deleted_at_ suffix from notes file and directory.
pub fn unmark_notes_deleted(task: &Task, all_tasks: &[Task]) {
    let dir = task_notes_parent_dir(task, all_tasks);
    let prefix = format!("{}_", &task.id.to_string()[..8]);

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with(&prefix) { continue; }
            if let Some(del_pos) = name.find("_deleted_at_") {
                let base = &name[..del_pos];
                if name.ends_with(".md") {
                    let new_name = format!("{}.md", base);
                    let _ = std::fs::rename(entry.path(), dir.join(new_name));
                } else if entry.path().is_dir() {
                    let _ = std::fs::rename(entry.path(), dir.join(base));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_basic() {
        assert_eq!(sanitize("Hello World"), "hello-world");
        assert_eq!(sanitize("hello/world.txt"), "hello-world-txt");
    }

    #[test]
    fn sanitize_unicode() {
        assert_eq!(sanitize("Ärger mit Über-Größe"), "arger-mit-uber-grosse");
        assert_eq!(sanitize("#9 — MIG-ART"), "9-mig-art");
    }

    #[test]
    fn sanitize_max_length() {
        let long = "a ".repeat(60);
        assert!(sanitize(&long).len() <= 50);
    }

    #[test]
    fn task_stem_basic() {
        let id = Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap();
        assert_eq!(task_stem(id, "Build API"), "12345678_build-api");
    }
}
