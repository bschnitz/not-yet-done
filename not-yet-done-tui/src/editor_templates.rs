use not_yet_done_core::entity::task::{Model as Task, TaskStatus};
use uuid::Uuid;

/// Generate the markdown template for creating a new task.
pub fn new_task(parent_id: Option<Uuid>) -> String {
    let parent_line = match parent_id {
        Some(id) => format!("parent: {id}"),
        None => "# parent:".to_string(),
    };

    format!(
        r#"---
# status: todo | in_progress | done | cancelled
status: todo
priority: 0
tracking: false
{parent_line}
---

## Description:


## Notes:
"#
    )
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

    format!(
        r#"---
# status: todo | in_progress | done | cancelled
status: {status_str}
priority: {}
tracking: {}
{parent_line}
---

## Description:
{}"#,
        task.priority,
        is_tracked,
        task.description,
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
}

#[derive(Debug)]
pub struct ParsedNewTask {
    pub description: String,
    pub parent_id: Option<String>,
    pub status: Option<TaskStatus>,
    pub priority: Option<i32>,
    pub tracking: bool,
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
    if content.trim() == template.trim() {
        return ParseResult::Aborted;
    }

    let (frontmatter, body) = split_frontmatter(content);

    let mut parent_id: Option<String> = None;
    let mut status: Option<TaskStatus> = None;
    let mut priority: Option<i32> = None;
    let mut tracking = false;
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
                _ => {}
            }
        }
    }

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
pub fn parse_edit_task(content: &str, template: &str, original: &Task) -> ParseResult<ParsedEditTask> {
    if content.trim() == template.trim() {
        return ParseResult::Aborted;
    }

    let (frontmatter, body) = split_frontmatter(content);

    let mut parsed_parent: Option<Option<Uuid>> = None;
    let mut parsed_status: Option<TaskStatus> = None;
    let mut parsed_priority: Option<i32> = None;
    let mut parsed_tracking: Option<bool> = None;
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
                _ => {}
            }
        }
    }

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
    if description.is_none() && status.is_none() && priority.is_none()
        && parsed_parent.is_none() && parsed_tracking.is_none()
    {
        return ParseResult::Aborted;
    }

    ParseResult::Ok(ParsedEditTask {
        description,
        parent_id: parsed_parent,
        status,
        priority,
        tracking: parsed_tracking,
    })
}

/// Re-render the editor content with error messages injected.
pub fn render_with_errors(content: &str, errors: &[FieldError]) -> String {
    let mut result = String::new();

    // Banner at top.
    result.push_str("<!-- ⚠ ERRORS — please fix and save again:\n");
    for e in errors {
        result.push_str(&format!("  - {}: {}\n", e.field, e.message));
    }
    result.push_str("-->\n");

    let field_errors: std::collections::HashMap<&str, &str> = errors
        .iter()
        .map(|e| (e.field, e.message.as_str()))
        .collect();

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
    let content = content.trim_start();

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
        assert_eq!(parse_kv("status: todo # a comment"), Some(("status", "todo")));
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

    // ----- render_with_errors -----

    #[test]
    fn render_with_errors_adds_banner() {
        let content = "---\nstatus: bogus\n---\n\n## Description:\nText\n";
        let errors = vec![FieldError { field: "status", message: "invalid".into() }];
        let rendered = render_with_errors(content, &errors);
        assert!(rendered.starts_with("<!-- ⚠ ERRORS"));
        assert!(rendered.contains("status: invalid"));
    }

    #[test]
    fn render_with_errors_inline_error() {
        let content = "---\nstatus: bogus\n---\n\n## Description:\nText\n";
        let errors = vec![FieldError { field: "status", message: "bad value".into() }];
        let rendered = render_with_errors(content, &errors);
        assert!(rendered.contains("# ⚠ bad value"));
    }

    #[test]
    fn render_with_errors_description_error() {
        let content = "---\n---\n\n## Description:\n";
        let errors = vec![FieldError { field: "description", message: "empty".into() }];
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
}
