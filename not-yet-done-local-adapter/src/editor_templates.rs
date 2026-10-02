//! The `TaskAdapter`'s editor-buffer format: a markdown document with a
//! `---` frontmatter (status / priority / tracking / parent / time window) and a
//! `## Description:` / `## Notes:` body. Rendering (`new_task`/`edit_task`)
//! and parsing (`parse_new_task`/`parse_edit_task`) are the two halves of
//! the [`InputSpec::Editor`](not_yet_done_content::InputSpec::Editor)
//! round-trip the adapter drives from `Node::prepare`/`Node::execute`.
//!
//! Like every other adapter's edit buffer (Jira's `.jira`, Confluence's
//! storage format), this format is **adapter-owned** — there is no shared
//! YAML schema for it, because each backend's editable shape differs. It
//! lives in this crate so the `TaskAdapter` and the (transitional) native
//! `TaskEditSession` in the TUI share one source of truth until the C1
//! cutover removes the latter.
//!
//! The structural primitives here (`split_frontmatter`, `parse_kv`,
//! `render_with_errors`) are not task-specific and are a candidate for
//! hoisting into `not-yet-done-content` as a shared frontmatter helper —
//! see the A2 follow-up note in `docs/plan-adapterize-tasks-trackings.md`.

use chrono::{DateTime, Utc};
use not_yet_done_task_core::entity::task::{Model as Task, TaskStatus};
use not_yet_done_task_core::service::check_window;
use uuid::Uuid;

use crate::datetime::{WindowEdge, fmt_window_bound, parse_window_bound};

/// Comment line above the two window keys, shared by both templates.
const WINDOW_HINT: &str = "# time window: not_before = don't start earlier, due = finish by \
(e.g. 2026-10-15, friday, friday 17:00; empty = open)";

fn window_line(key: &str, bound: Option<DateTime<Utc>>, edge: WindowEdge) -> String {
    match bound {
        Some(dt) => format!("{key}: {}", fmt_window_bound(dt, edge)),
        None => format!("{key}:"),
    }
}

/// Generate the markdown template for creating a new task.
pub fn new_task(parent_id: Option<Uuid>) -> String {
    let parent_line = parent_line(parent_id);

    format!(
        r#"---
# status: todo | in_progress | done | cancelled
status: todo
priority: 0
tracking: false
{parent_line}
# tags: comma-separated tag names
tags:
{WINDOW_HINT}
not_before:
due:
---

## Description:


## Notes:
"#
    )
}

/// The `parent:` frontmatter line for `parent_id` — commented out (blank) for a
/// top-level task, as [`new_task`] writes it.
fn parent_line(parent_id: Option<Uuid>) -> String {
    match parent_id {
        Some(id) => format!("parent: {id}"),
        None => "# parent:".to_string(),
    }
}

/// A stored template with its `parent:` set from the create's context.
///
/// The parent is the one thing a template cannot know — it is decided by
/// where the create was started — so whatever `parent:` line the template
/// has (set, blank or commented out) is replaced, the first one in place and
/// any further ones dropped. A template without one gets it before the
/// closing `---`, and one without a frontmatter gets a frontmatter holding
/// only it. Everything else is kept byte for byte.
pub fn with_parent(template: &str, parent_id: Option<Uuid>) -> String {
    let line = parent_line(parent_id);
    let is_parent = |l: &str| {
        l.trim()
            .trim_start_matches('#')
            .trim_start()
            .starts_with("parent:")
    };

    let mut out = Vec::new();
    let mut lines = template.lines();
    // Everything up to the opening `---`; anything but blank lines there
    // means there is no frontmatter.
    let mut opened = false;
    for l in &mut lines {
        out.push(l.to_string());
        if l.trim() == "---" {
            opened = true;
            break;
        }
        if !l.trim().is_empty() {
            break;
        }
    }
    if !opened {
        let body = if template.is_empty() {
            String::new()
        } else {
            format!("\n{template}")
        };
        return format!("---\n{line}\n---\n{body}");
    }
    let mut placed = false;
    let mut closed = false;
    for l in &mut lines {
        if l.trim() == "---" {
            if !placed {
                out.push(line.clone());
                placed = true;
            }
            out.push(l.to_string());
            closed = true;
            break;
        }
        if is_parent(l) {
            if !placed {
                out.push(line.clone());
                placed = true;
            }
            continue;
        }
        out.push(l.to_string());
    }
    if !closed && !placed {
        out.push(line);
    }
    out.extend(lines.map(str::to_string));
    let mut merged = out.join("\n");
    if template.ends_with('\n') {
        merged.push('\n');
    }
    merged
}

/// Generate the markdown template for editing an existing task (without notes).
pub fn edit_task(task: &Task, is_tracked: bool) -> String {
    edit_task_inner(task, is_tracked)
}

pub fn edit_task_with_notes(task: &Task, is_tracked: bool, notes: &str) -> String {
    let base = edit_task_inner(task, is_tracked);
    if notes.is_empty() {
        format!("{}\n\n## Notes:\n", base)
    } else {
        format!("{}\n\n## Notes:\n{}", base, notes)
    }
}

fn edit_task_inner(task: &Task, is_tracked: bool) -> String {
    let status_str = match task.status {
        TaskStatus::Todo => "todo",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Done => "done",
        TaskStatus::Cancelled => "cancelled",
    };
    let parent_line = match task.parent_id {
        Some(id) => format!("parent: {id}"),
        None => "# parent:".to_string(),
    };
    let not_before_line = window_line("not_before", task.not_before, WindowEdge::Start);
    let due_line = window_line("due", task.due, WindowEdge::End);

    format!(
        r#"---
# status: todo | in_progress | done | cancelled
status: {status_str}
priority: {}
tracking: {}
{parent_line}
{WINDOW_HINT}
{not_before_line}
{due_line}
---

## Description:
{}"#,
        task.priority, is_tracked, task.description,
    )
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ParsedEditTask {
    pub description: Option<String>,
    pub parent_id: Option<Option<Uuid>>,
    pub status: Option<TaskStatus>,
    pub priority: Option<i32>,
    pub tracking: Option<bool>,
    /// The new time window `(not_before, due)` as a whole, `None` when
    /// neither bound changed.
    pub window: Option<(Option<DateTime<Utc>>, Option<DateTime<Utc>>)>,
}

#[derive(Debug)]
pub struct ParsedNewTask {
    pub description: String,
    pub parent_id: Option<String>,
    pub status: Option<TaskStatus>,
    pub priority: Option<i32>,
    pub tracking: bool,
    pub not_before: Option<DateTime<Utc>>,
    pub due: Option<DateTime<Utc>>,
    /// Tag names from `tags:`, in the order written, empty when none.
    pub tags: Vec<String>,
}

#[derive(Debug)]
pub struct FieldError {
    pub field: &'static str,
    pub message: String,
}

#[derive(Debug)]
pub enum ParseResult<T> {
    Aborted,
    Ok(T),
    Errors {
        errors: Vec<FieldError>,
        original_content: String,
    },
}

pub fn parse_new_task(content: &str, template: &str) -> ParseResult<ParsedNewTask> {
    if unchanged(content, template) {
        return ParseResult::Aborted;
    }

    let (frontmatter, body) = split_frontmatter(content);

    let mut parent_id: Option<String> = None;
    let mut status: Option<TaskStatus> = None;
    let mut priority: Option<i32> = None;
    let mut tracking = false;
    let mut not_before = None;
    let mut due = None;
    let mut tags = Vec::new();
    let mut errors = Vec::new();

    for line in frontmatter.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some((key, value)) = parse_kv(line) {
            match key {
                "tracking" => {
                    tracking = value.trim() == "true";
                }
                "parent" => {
                    let v = value.trim();
                    if !v.is_empty() {
                        if Uuid::parse_str(v).is_err() {
                            errors.push(FieldError {
                                field: "parent",
                                message: format!("Invalid UUID: {v}"),
                            });
                        } else {
                            parent_id = Some(v.to_string());
                        }
                    }
                }
                "status" => {
                    let v = value.trim();
                    if !v.is_empty() {
                        match v {
                            "todo" => status = Some(TaskStatus::Todo),
                            "in_progress" => status = Some(TaskStatus::InProgress),
                            "done" => status = Some(TaskStatus::Done),
                            "cancelled" => status = Some(TaskStatus::Cancelled),
                            _ => {
                                errors.push(FieldError {
                                    field: "status",
                                    message: format!(
                                        "\"{v}\" is not valid. Choose: todo | in_progress | done | cancelled"
                                    ),
                                });
                            }
                        }
                    }
                }
                "priority" => {
                    let v = value.trim();
                    if !v.is_empty() {
                        match v.parse::<i32>() {
                            Ok(p) => priority = Some(p),
                            Err(_) => {
                                errors.push(FieldError {
                                    field: "priority",
                                    message: format!("\"{v}\" is not a valid integer"),
                                });
                            }
                        }
                    }
                }
                "not_before" => {
                    not_before = parse_bound_field("not_before", value, WindowEdge::Start, &mut errors)
                }
                "due" => due = parse_bound_field("due", value, WindowEdge::End, &mut errors),
                "tags" => {
                    tags = value
                        .split(',')
                        .map(str::trim)
                        .filter(|t| !t.is_empty())
                        .map(str::to_string)
                        .collect()
                }
                _ => {}
            }
        }
    }
    check_window_fields(not_before, due, &mut errors);
    errors.extend(misplaced_frontmatter(content));

    let description = parse_description(&body);

    if description.is_empty() {
        errors.push(FieldError {
            field: "description",
            message: "Description must not be empty".to_string(),
        });
    }

    if errors.is_empty() {
        ParseResult::Ok(ParsedNewTask {
            description,
            parent_id,
            status,
            priority,
            tracking,
            not_before,
            due,
            tags,
        })
    } else {
        ParseResult::Errors {
            errors,
            original_content: content.to_string(),
        }
    }
}

/// Parse the editor file content for an edit operation.
/// Returns only the fields that changed compared to the original task.
pub fn parse_edit_task(
    content: &str,
    template: &str,
    original: &Task,
) -> ParseResult<ParsedEditTask> {
    if unchanged(content, template) {
        return ParseResult::Aborted;
    }

    let (frontmatter, body) = split_frontmatter(content);

    let mut parsed_parent: Option<Option<Uuid>> = None;
    let mut parsed_status: Option<TaskStatus> = None;
    let mut parsed_priority: Option<i32> = None;
    let mut parsed_tracking: Option<bool> = None;
    // Outer `None`: the key is absent (or its text is unchanged) — keep the
    // stored bound. A buffer without the lines, as a script may hand one
    // over, must not wipe a window it never mentioned.
    let mut parsed_not_before: Option<Option<DateTime<Utc>>> = None;
    let mut parsed_due: Option<Option<DateTime<Utc>>> = None;
    let mut errors = Vec::new();

    for line in frontmatter.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some((key, value)) = parse_kv(line) {
            match key {
                "tracking" => {
                    let v = value.trim() == "true";
                    parsed_tracking = Some(v);
                }
                "parent" => {
                    let v = value.trim();
                    if v.is_empty() {
                        if original.parent_id.is_some() {
                            parsed_parent = Some(None);
                        }
                    } else {
                        match Uuid::parse_str(v) {
                            Ok(id) => {
                                if original.parent_id != Some(id) {
                                    parsed_parent = Some(Some(id));
                                }
                            }
                            Err(_) => {
                                errors.push(FieldError {
                                    field: "parent",
                                    message: format!("Invalid UUID: {v}"),
                                });
                            }
                        }
                    }
                }
                "status" => {
                    let v = value.trim();
                    if !v.is_empty() {
                        match v {
                            "todo" => parsed_status = Some(TaskStatus::Todo),
                            "in_progress" => parsed_status = Some(TaskStatus::InProgress),
                            "done" => parsed_status = Some(TaskStatus::Done),
                            "cancelled" => parsed_status = Some(TaskStatus::Cancelled),
                            _ => {
                                errors.push(FieldError {
                                    field: "status",
                                    message: format!(
                                        "\"{v}\" is not valid. Choose: todo | in_progress | done | cancelled"
                                    ),
                                });
                            }
                        }
                    }
                }
                "priority" => {
                    let v = value.trim();
                    if !v.is_empty() {
                        match v.parse::<i32>() {
                            Ok(p) => parsed_priority = Some(p),
                            Err(_) => {
                                errors.push(FieldError {
                                    field: "priority",
                                    message: format!("\"{v}\" is not a valid integer"),
                                });
                            }
                        }
                    }
                }
                "not_before" => {
                    parsed_not_before = edited_bound(
                        "not_before",
                        value,
                        original.not_before,
                        WindowEdge::Start,
                        &mut errors,
                    )
                }
                "due" => {
                    parsed_due =
                        edited_bound("due", value, original.due, WindowEdge::End, &mut errors)
                }
                _ => {}
            }
        }
    }
    let window = (parsed_not_before.is_some() || parsed_due.is_some()).then(|| {
        (
            parsed_not_before.unwrap_or(original.not_before),
            parsed_due.unwrap_or(original.due),
        )
    });
    if let Some((not_before, due)) = window {
        check_window_fields(not_before, due, &mut errors);
    }
    let window = window.filter(|w| *w != (original.not_before, original.due));
    errors.extend(misplaced_frontmatter(content));

    let description_text = parse_description(&body);

    if description_text.is_empty() {
        errors.push(FieldError {
            field: "description",
            message: "Description must not be empty".to_string(),
        });
    }

    if !errors.is_empty() {
        return ParseResult::Errors {
            errors,
            original_content: content.to_string(),
        };
    }

    // Only return changed fields.
    let description = if description_text != original.description {
        Some(description_text)
    } else {
        None
    };
    let status = parsed_status.filter(|s| *s != original.status);
    let priority = parsed_priority.filter(|p| *p != original.priority);

    // If nothing changed at all, treat as abort.
    if description.is_none()
        && status.is_none()
        && priority.is_none()
        && parsed_parent.is_none()
        && parsed_tracking.is_none()
        && window.is_none()
    {
        return ParseResult::Aborted;
    }

    ParseResult::Ok(ParsedEditTask {
        description,
        parent_id: parsed_parent,
        status,
        priority,
        tracking: parsed_tracking,
        window,
    })
}

/// Parse one window bound's value; empty = no bound. A bad value is recorded
/// against its key and reads as "no bound" — the buffer reopens anyway.
fn parse_bound_field(
    key: &'static str,
    value: &str,
    edge: WindowEdge,
    errors: &mut Vec<FieldError>,
) -> Option<DateTime<Utc>> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    match parse_window_bound(v, edge) {
        Ok(dt) => Some(dt),
        Err(message) => {
            errors.push(FieldError {
                field: key,
                message,
            });
            None
        }
    }
}

/// Like [`parse_bound_field`], but `None` when the text still reads exactly as
/// the stored bound renders. Comparing the text rather than the instant keeps
/// a bound stored with seconds (`in 2 hours`) from counting as edited just
/// because its minute-precision rendering parses back without them.
fn edited_bound(
    key: &'static str,
    value: &str,
    stored: Option<DateTime<Utc>>,
    edge: WindowEdge,
    errors: &mut Vec<FieldError>,
) -> Option<Option<DateTime<Utc>>> {
    let unchanged = match stored {
        Some(dt) => value.trim() == fmt_window_bound(dt, edge),
        None => value.trim().is_empty(),
    };
    if unchanged {
        return None;
    }
    Some(parse_bound_field(key, value, edge, errors))
}

/// The service refuses a window that ends before it starts; saying so at the
/// `due:` line lets the user fix it in place instead of losing the buffer.
fn check_window_fields(
    not_before: Option<DateTime<Utc>>,
    due: Option<DateTime<Utc>>,
    errors: &mut Vec<FieldError>,
) {
    if check_window(not_before, due).is_err() {
        errors.push(FieldError {
            field: "due",
            message: "due must lie after not_before".to_string(),
        });
    }
}

/// First line of the banner [`render_with_errors`] puts above a buffer it
/// sends back.
const ERROR_BANNER: &str = "<!-- ⚠ ERRORS — please fix and save again:";

/// Saved without a change, i.e. abandoned. Only a buffer the adapter rendered
/// fresh can be abandoned that way: one reopened with an error banner was
/// already saved once, and saving it again unchanged means "try again" — the
/// cause may have been fixed outside the buffer (a tag created meanwhile), and
/// treating it as abandoned would drop everything typed.
fn unchanged(content: &str, template: &str) -> bool {
    content.trim() == template.trim() && !template.trim_start().starts_with(ERROR_BANNER)
}

/// Re-render the editor content with error messages injected.
pub fn render_with_errors(content: &str, errors: &[FieldError]) -> String {
    let mut result = String::new();

    // Banner at top.
    result.push_str(ERROR_BANNER);
    result.push('\n');
    for e in errors {
        result.push_str(&format!("  - {}: {}\n", e.field, e.message));
    }
    result.push_str("-->\n");

    let field_errors: std::collections::HashMap<&str, &str> = errors
        .iter()
        .map(|e| (e.field, e.message.as_str()))
        .collect();

    let rest = skip_leading_comments(content);
    if !rest.starts_with("---") {
        // No block in front to annotate: inventing an empty one would leave
        // the misplaced block behind it, and the next save fails the same way.
        result.push_str(rest);
        return result;
    }
    let (frontmatter, body) = split_frontmatter(content);

    result.push_str("---\n");
    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if let Some((key, _)) = parse_kv(trimmed) {
            result.push_str(line);
            result.push('\n');
            if let Some(msg) = field_errors.get(key) {
                result.push_str(&format!("# ⚠ {msg}\n"));
            }
        } else {
            // Keep comments / blank lines, but strip old error comments.
            if !trimmed.starts_with("# ⚠") {
                result.push_str(line);
                result.push('\n');
            }
        }
    }
    result.push_str("---\n");

    if let Some(msg) = field_errors.get("description") {
        result.push_str(&format!("\n<!-- ⚠ {msg} -->\n"));
    }

    result.push_str(&body);

    result
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn split_frontmatter(content: &str) -> (String, String) {
    let content = skip_leading_comments(content);

    let Some(rest) = content.strip_prefix("---") else {
        return (String::new(), content.to_string());
    };
    let rest = rest.strip_prefix('\n').unwrap_or(rest);

    // Closing --- at start (empty frontmatter).
    if let Some(stripped) = rest.strip_prefix("---") {
        let after = stripped.strip_prefix('\n').unwrap_or(stripped);
        return (String::new(), after.to_string());
    }

    if let Some(pos) = rest.find("\n---") {
        let fm = &rest[..pos];
        let after = &rest[pos + 4..];
        let after = after.strip_prefix('\n').unwrap_or(after);
        (fm.to_string(), after.to_string())
    } else {
        (rest.to_string(), String::new())
    }
}

/// A `---` block that is not at the top: [`split_frontmatter`] reads only a
/// leading one, so every key in it (tags, parent …) would be dropped without
/// a word. Only a block that reads like front matter counts — key lines,
/// comments and blank lines up to a closing `---`, at least one key — so a
/// `---` rule in a description stays text.
fn misplaced_frontmatter(content: &str) -> Option<FieldError> {
    let rest = skip_leading_comments(content);
    if rest.starts_with("---") {
        return None;
    }
    let lines: Vec<&str> = rest.lines().collect();
    let found = lines.iter().enumerate().any(|(i, line)| {
        if line.trim_end() != "---" {
            return false;
        }
        let inner = &lines[i + 1..];
        let Some(end) = inner.iter().position(|l| l.trim_end() == "---") else {
            return false;
        };
        let block = &inner[..end];
        block.iter().any(|l| parse_kv(l.trim()).is_some())
            && block.iter().all(|l| {
                let l = l.trim();
                l.is_empty() || l.starts_with('#') || parse_kv(l).is_some()
            })
    });
    found.then(|| FieldError {
        field: "frontmatter",
        message: "the `---` block must be the first thing in the buffer \
                  (only comments may precede it); its keys were not read"
            .to_string(),
    })
}

/// `content` without leading whitespace and `<!-- … -->` blocks — the error
/// banner [`render_with_errors`] puts in front of the frontmatter. Without
/// this a reopened buffer has no frontmatter at all, and its next save reads
/// as "nothing changed".
fn skip_leading_comments(content: &str) -> &str {
    let mut rest = content.trim_start();
    while let Some(inner) = rest.strip_prefix("<!--") {
        let Some(end) = inner.find("-->") else {
            break;
        };
        rest = inner[end + 3..].trim_start();
    }
    rest
}

fn parse_kv(line: &str) -> Option<(&str, &str)> {
    let colon = line.find(':')?;
    let key = line[..colon].trim();
    if key.is_empty() || key.starts_with('#') {
        return None;
    }
    let value = line[colon + 1..].trim();
    let value = if let Some(hash) = value.find(" #") {
        value[..hash].trim()
    } else {
        value
    };
    Some((key, value))
}

fn parse_description(body: &str) -> String {
    let mut lines = body.lines();
    let mut desc_lines = Vec::new();
    let mut found_heading = false;

    for line in &mut lines {
        let trimmed = line.trim();
        if !found_heading {
            if let Some(after) = trimmed.strip_prefix("## Description:") {
                let after = after.trim();
                if !after.is_empty() {
                    desc_lines.push(after.to_string());
                }
                found_heading = true;
            }
            continue;
        }
        // Stop at ## Notes: heading.
        if trimmed.starts_with("## Notes:") {
            break;
        }
        desc_lines.push(line.to_string());
    }

    if !found_heading {
        // No ## Description: heading — take everything up to ## Notes:
        let raw = body.split("## Notes:").next().unwrap_or(body);
        return raw.trim().to_string();
    }

    while desc_lines.first().is_some_and(|l| l.trim().is_empty()) {
        desc_lines.remove(0);
    }
    while desc_lines.last().is_some_and(|l| l.trim().is_empty()) {
        desc_lines.pop();
    }

    desc_lines.join("\n")
}

/// Extract notes content from the editor body (everything after `## Notes:`).
pub fn parse_notes(body: &str) -> String {
    let (_, full_body) = split_frontmatter(body);
    if let Some(pos) = full_body.find("## Notes:") {
        let after = &full_body[pos + "## Notes:".len()..];
        let after = after.strip_prefix('\n').unwrap_or(after);
        let trimmed = after.trim();
        trimmed.to_string()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_task() {
        let content = "---\nstatus: todo\npriority: 0\n---\n\n## Description:\nDo the thing\n";
        let template = "---\nstatus: todo\npriority: 0\n# parent:\n---\n\n## Description:\n";
        match parse_new_task(content, template) {
            ParseResult::Ok(task) => {
                assert_eq!(task.description, "Do the thing");
                assert!(task.parent_id.is_none());
                assert_eq!(task.status, Some(TaskStatus::Todo));
                assert_eq!(task.priority, Some(0));
            }
            other => panic!("Expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn parse_description_on_heading_line() {
        let content = "---\n---\n\n## Description: First line\nSecond line\n";
        let template = "---\nstatus: todo\n---\n\n## Description:\n";
        match parse_new_task(content, template) {
            ParseResult::Ok(task) => {
                assert_eq!(task.description, "First line\nSecond line");
            }
            other => panic!("Expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn parse_with_parent() {
        let id = Uuid::new_v4();
        let content = format!("---\nparent: {id}\n---\n\n## Description:\nSub task\n");
        let template = "---\n---\n\n## Description:\n";
        match parse_new_task(&content, template) {
            ParseResult::Ok(task) => {
                assert_eq!(task.parent_id, Some(id.to_string()));
                assert_eq!(task.description, "Sub task");
            }
            other => panic!("Expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn abort_on_no_changes() {
        let template = "---\n# status: todo\npriority: 0\n# parent:\n---\n\n## Description:\n";
        assert!(matches!(
            parse_new_task(template, template),
            ParseResult::Aborted
        ));
    }

    #[test]
    fn error_on_empty_description() {
        let content = "---\n---\n\n## Description:\n";
        let template = "---\n# parent:\n---\n\n## Description:\n";
        match parse_new_task(content, template) {
            ParseResult::Errors { errors, .. } => {
                assert!(errors.iter().any(|e| e.field == "description"));
            }
            other => panic!("Expected Errors, got {other:?}"),
        }
    }

    #[test]
    fn error_on_invalid_parent_uuid() {
        let content = "---\nparent: not-a-uuid\n---\n\n## Description:\nSomething\n";
        let template = "---\n---\n\n## Description:\n";
        match parse_new_task(content, template) {
            ParseResult::Errors { errors, .. } => {
                assert!(errors.iter().any(|e| e.field == "parent"));
            }
            other => panic!("Expected Errors, got {other:?}"),
        }
    }

    #[test]
    fn error_on_invalid_status() {
        let content = "---\nstatus: bogus\n---\n\n## Description:\nSomething\n";
        let template = "---\n---\n\n## Description:\n";
        match parse_new_task(content, template) {
            ParseResult::Errors { errors, .. } => {
                assert!(errors.iter().any(|e| e.field == "status"));
            }
            other => panic!("Expected Errors, got {other:?}"),
        }
    }

    #[test]
    fn error_on_invalid_priority() {
        let content = "---\npriority: abc\n---\n\n## Description:\nSomething\n";
        let template = "---\n---\n\n## Description:\n";
        match parse_new_task(content, template) {
            ParseResult::Errors { errors, .. } => {
                assert!(errors.iter().any(|e| e.field == "priority"));
            }
            other => panic!("Expected Errors, got {other:?}"),
        }
    }

    // ----- split_frontmatter -----

    #[test]
    fn split_frontmatter_normal() {
        let (fm, body) = split_frontmatter("---\nstatus: todo\n---\nbody text");
        assert_eq!(fm, "status: todo");
        assert_eq!(body, "body text");
    }

    #[test]
    fn split_frontmatter_empty() {
        let (fm, body) = split_frontmatter("---\n---\nbody");
        assert_eq!(fm, "");
        assert_eq!(body, "body");
    }

    #[test]
    fn split_frontmatter_no_frontmatter() {
        let (fm, body) = split_frontmatter("just plain text");
        assert_eq!(fm, "");
        assert_eq!(body, "just plain text");
    }

    #[test]
    fn split_frontmatter_missing_closing() {
        let (fm, body) = split_frontmatter("---\nstatus: todo\nno closing");
        assert_eq!(fm, "status: todo\nno closing");
        assert_eq!(body, "");
    }

    // ----- parse_kv -----

    #[test]
    fn parse_kv_normal() {
        assert_eq!(parse_kv("status: todo"), Some(("status", "todo")));
    }

    #[test]
    fn parse_kv_with_inline_comment() {
        assert_eq!(
            parse_kv("status: todo # a comment"),
            Some(("status", "todo"))
        );
    }

    #[test]
    fn parse_kv_comment_line() {
        assert_eq!(parse_kv("# this is a comment"), None);
    }

    #[test]
    fn parse_kv_empty_value() {
        assert_eq!(parse_kv("name:"), Some(("name", "")));
    }

    // ----- parse_edit_task -----

    #[test]
    fn edit_task_no_changes() {
        let task = make_task("Hello", TaskStatus::Todo, 0, None);
        let template = edit_task(&task, false);
        match parse_edit_task(&template, &template, &task) {
            ParseResult::Aborted => {}
            other => panic!("Expected Aborted, got {other:?}"),
        }
    }

    #[test]
    fn edit_task_description_changed() {
        let task = make_task("Old", TaskStatus::Todo, 0, None);
        let template = edit_task(&task, false);
        let content = template.replace("Old", "New");
        match parse_edit_task(&content, &template, &task) {
            ParseResult::Ok(parsed) => {
                assert_eq!(parsed.description, Some("New".to_string()));
                assert!(parsed.status.is_none());
                assert!(parsed.priority.is_none());
            }
            other => panic!("Expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn edit_task_status_changed() {
        let task = make_task("Task", TaskStatus::Todo, 0, None);
        let template = edit_task(&task, false);
        let content = template.replace("status: todo", "status: done");
        match parse_edit_task(&content, &template, &task) {
            ParseResult::Ok(parsed) => {
                assert_eq!(parsed.status, Some(TaskStatus::Done));
            }
            other => panic!("Expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn edit_task_priority_changed() {
        let task = make_task("Task", TaskStatus::Todo, 0, None);
        let template = edit_task(&task, false);
        let content = template.replace("priority: 0", "priority: 7");
        match parse_edit_task(&content, &template, &task) {
            ParseResult::Ok(parsed) => {
                assert_eq!(parsed.priority, Some(7));
            }
            other => panic!("Expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn edit_task_saves_after_an_error_reopen() {
        let task = make_task("Task", TaskStatus::Todo, 0, None);
        let template = edit_task(&task, false);
        let broken = template.replace("status: todo", "status: bogus");
        let ParseResult::Errors {
            errors,
            original_content,
        } = parse_edit_task(&broken, &template, &task)
        else {
            panic!("expected the bogus status to be refused");
        };
        // The reopened buffer — banner on top — is the baseline of the retry.
        let reopened = render_with_errors(&original_content, &errors);
        let fixed = reopened.replace("status: bogus", "status: done");
        match parse_edit_task(&fixed, &reopened, &task) {
            ParseResult::Ok(parsed) => assert_eq!(parsed.status, Some(TaskStatus::Done)),
            other => panic!("Expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn split_frontmatter_skips_a_leading_comment() {
        let (fm, body) = split_frontmatter("<!-- note\n  - x -->\n---\nstatus: todo\n---\nbody");
        assert_eq!(fm, "status: todo");
        assert_eq!(body, "body");
    }

    #[test]
    fn a_late_frontmatter_block_is_an_error() {
        let content = "A title\n---\ntags: todo\n# parent:\n---\n\n## Description:\nText\n";
        let template = new_task(None);
        let ParseResult::Errors { errors, .. } = parse_new_task(content, &template) else {
            panic!("expected errors");
        };
        assert!(
            errors.iter().any(|e| e.field == "frontmatter"),
            "{errors:?}"
        );
        // Reopened, the buffer keeps the block where it was instead of
        // gaining an empty one in front of it.
        let reopened = render_with_errors(content, &errors);
        assert!(reopened.contains("A title\n---\ntags: todo"), "{reopened}");
        assert_eq!(reopened.matches("\n---\n").count(), 2, "{reopened}");
    }

    #[test]
    fn a_rule_in_a_description_is_not_a_frontmatter_block() {
        let content = "---\n---\n\n## Description:\nAbove\n---\nBelow\n---\n";
        assert!(misplaced_frontmatter(content).is_none());
        let no_block = "## Description:\nAbove\n---\nnote: see below\n---\n";
        // A key-only stretch between rules in a buffer without front matter
        // does read as a misplaced block — that buffer has no other place
        // its keys could be.
        assert!(misplaced_frontmatter(no_block).is_some());
        assert!(misplaced_frontmatter("## Description:\nAbove\n---\nBelow\n").is_none());
    }

    // ----- render_with_errors -----

    /// A buffer reopened with errors and saved again unchanged is a retry,
    /// not an abandoned create: the cause may have been fixed outside it.
    #[test]
    fn unchanged_resave_of_an_error_buffer_is_parsed() {
        let content = "---\ntags: todo\n---\n\n## Description:\nText\n";
        let errors = vec![FieldError {
            field: "tags",
            message: "Unknown tag: todo".into(),
        }];
        let reopened = render_with_errors(content, &errors);
        match parse_new_task(&reopened, &reopened) {
            ParseResult::Ok(task) => {
                assert_eq!(task.tags, vec!["todo".to_string()]);
                assert_eq!(task.description, "Text");
            }
            other => panic!("expected Ok, got {other:?}"),
        }
        // A fresh buffer saved unchanged is still abandoned.
        let fresh = new_task(None);
        assert!(matches!(
            parse_new_task(&fresh, &fresh),
            ParseResult::Aborted
        ));
    }

    #[test]
    fn render_with_errors_adds_banner() {
        let content = "---\nstatus: bogus\n---\n\n## Description:\nText\n";
        let errors = vec![FieldError {
            field: "status",
            message: "invalid".into(),
        }];
        let rendered = render_with_errors(content, &errors);
        assert!(rendered.starts_with("<!-- ⚠ ERRORS"));
        assert!(rendered.contains("status: invalid"));
    }

    #[test]
    fn render_with_errors_inline_error() {
        let content = "---\nstatus: bogus\n---\n\n## Description:\nText\n";
        let errors = vec![FieldError {
            field: "status",
            message: "bad value".into(),
        }];
        let rendered = render_with_errors(content, &errors);
        assert!(rendered.contains("# ⚠ bad value"));
    }

    #[test]
    fn render_with_errors_description_error() {
        let content = "---\n---\n\n## Description:\n";
        let errors = vec![FieldError {
            field: "description",
            message: "empty".into(),
        }];
        let rendered = render_with_errors(content, &errors);
        assert!(rendered.contains("<!-- ⚠ empty -->"));
    }

    // ----- helper -----

    fn make_task(desc: &str, status: TaskStatus, priority: i32, parent: Option<Uuid>) -> Task {
        Task {
            id: Uuid::new_v4(),
            description: desc.to_string(),
            status,
            deleted: false,
            deleted_at: None,
            priority,
            parent_id: parent,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            last_tracked_at: None,
            not_before: None,
            due: None,
            recur: None,
            series_id: None,
            path: None,
        }
    }

    #[test]
    fn parse_in_progress_status() {
        let content = "---\nstatus: in_progress\npriority: 5\n---\n\n## Description:\nDo it\n";
        let template = "---\n---\n\n## Description:\n";
        match parse_new_task(content, template) {
            ParseResult::Ok(task) => {
                assert_eq!(task.status, Some(TaskStatus::InProgress));
                assert_eq!(task.priority, Some(5));
            }
            other => panic!("Expected Ok, got {other:?}"),
        }
    }

    // ----- stored templates -----

    const PARENT: &str = "6f1c2a1e-0000-4000-8000-000000000001";

    fn parent() -> Option<Uuid> {
        Some(Uuid::parse_str(PARENT).unwrap())
    }

    #[test]
    fn with_parent_replaces_a_commented_parent_line_in_place() {
        let template = new_task(None);
        let merged = with_parent(&template, parent());
        assert_eq!(merged, new_task(parent()));
    }

    #[test]
    fn with_parent_overrides_a_parent_the_template_wrote() {
        let template = "---\nparent: 00000000-0000-4000-8000-00000000dead\nstatus: todo\nparent:\n---\n\n## Description:\n";
        let merged = with_parent(template, parent());
        assert_eq!(
            merged,
            format!("---\nparent: {PARENT}\nstatus: todo\n---\n\n## Description:\n")
        );
    }

    #[test]
    fn with_parent_inserts_a_missing_line_before_the_closing_fence() {
        let merged = with_parent("---\ntags: todo\n---\nbody", None);
        assert_eq!(merged, "---\ntags: todo\n# parent:\n---\nbody");
    }

    #[test]
    fn with_parent_gives_a_bare_template_a_frontmatter() {
        let merged = with_parent("## Description:\n", parent());
        assert_eq!(merged, format!("---\nparent: {PARENT}\n---\n\n## Description:\n"));
    }

    #[test]
    fn a_template_saved_unchanged_creates_nothing() {
        let template = with_parent("---\ntags: todo\n---\n\n## Description:\n", parent());
        assert!(matches!(
            parse_new_task(&template, &template),
            ParseResult::Aborted
        ));
    }

    #[test]
    fn parse_new_task_reads_tags_as_a_comma_list() {
        let template = new_task(None);
        let content = template
            .replace("tags:", "tags: todo, work ,, home")
            .replace("## Description:\n", "## Description:\nBuy milk\n");
        match parse_new_task(&content, &template) {
            ParseResult::Ok(task) => assert_eq!(task.tags, ["todo", "work", "home"]),
            other => panic!("Expected Ok, got {other:?}"),
        }
    }
}
