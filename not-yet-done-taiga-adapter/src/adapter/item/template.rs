//! 3b template render/parse/validate for Taiga items.
//!
//! Layout:
//! ```text
//! subject: …
//! status: ss-…
//! assignee: uu-… (or empty)
//! tags: tt-foo, tt-bar
//! ---
//! ref: 42
//! type: task
//! modified: 2026-…
//! ===
//!
//! <description body>
//!
//! # === COMPLETIONS ===
//! # statuses: ss-new, ss-in-progress, …
//! # users: uu-alice, uu-bob, …
//! # tags: tt-frontend, tt-bug, …
//! ```

use std::collections::{HashMap, HashSet};

use super::ItemDetail;
use super::slugs::{
    STATUS_PREFIX, TAG_PREFIX, TaigaSlugTables, USER_PREFIX,
};

pub(super) const EDITABLE_MARKER: &str = "---";
pub(super) const BODY_MARKER: &str = "===";
pub(super) const CACHE_MARKER: &str = "# === COMPLETIONS ===";
pub(super) const ERROR_BANNER_START: &str = "# ─── ERRORS ───────────────────────────────";
pub(super) const ERROR_BANNER_END: &str = "# ─── /ERRORS ──────────────────────────────";
pub(super) const FOREIGN_BANNER_START: &str = "# ─── UPSTREAM CHANGED ─────────────────────";
pub(super) const FOREIGN_BANNER_END: &str = "# ─── /UPSTREAM CHANGED ────────────────────";

#[derive(Debug, Clone)]
pub(super) struct FieldError {
    pub(super) message: String,
}

#[derive(Debug, Default)]
pub(super) struct Parsed3b {
    pub(super) editable: HashMap<String, String>,
    pub(super) body: String,
}

/// Diff against current upstream — what `execute_edit_full` writes.
#[derive(Debug, Default)]
pub(super) struct ChangeSet {
    pub(super) metadata_changes: Vec<(String, String)>,
    pub(super) body: Option<String>,
}

pub(super) fn edit_full_fields() -> Vec<String> {
    vec![
        "subject".into(),
        "status".into(),
        "assignee".into(),
        "tags".into(),
    ]
}

/// Render the editable value of one field (slug-aware).
fn editable_value_with_slugs(
    detail: &ItemDetail,
    key: &str,
    tables: &TaigaSlugTables,
) -> String {
    match key {
        "subject" => detail.subject.clone(),
        "status" => tables
            .statuses
            .slug_for(&detail.status)
            .map(String::from)
            .unwrap_or_else(|| detail.status.clone()),
        "assignee" => detail
            .assignee_usernames
            .iter()
            .map(|u| {
                tables
                    .users
                    .slug_for(u)
                    .map(String::from)
                    .unwrap_or_else(|| u.clone())
            })
            .collect::<Vec<_>>()
            .join(", "),
        "tags" => detail
            .tags
            .iter()
            .filter_map(|t| tables.tags.slug_for(t).map(String::from))
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    }
}

fn readonly_value(detail: &ItemDetail, key: &str) -> String {
    match key {
        "ref" => match &detail.project_slug {
            Some(slug) if !slug.is_empty() => format!("{slug}#{}", detail.r#ref),
            _ => format!("#{}", detail.r#ref),
        },
        "type" => detail.item_type.as_str().to_string(),
        "modified" => detail.modified.clone().unwrap_or_default(),
        _ => String::new(),
    }
}

pub(super) fn render_cache_section(tables: &TaigaSlugTables) -> String {
    if tables.statuses.is_empty() && tables.users.is_empty() && tables.tags.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push('\n');
    out.push_str(CACHE_MARKER);
    out.push('\n');
    if !tables.statuses.is_empty() {
        out.push_str("# statuses: ");
        out.push_str(&tables.statuses.slugs().join(", "));
        out.push('\n');
    }
    if !tables.users.is_empty() {
        out.push_str("# users: ");
        out.push_str(&tables.users.slugs().join(", "));
        out.push('\n');
    }
    if !tables.tags.is_empty() {
        out.push_str("# tags: ");
        out.push_str(&tables.tags.slugs().join(", "));
        out.push('\n');
    }
    out
}

/// Build a 3b buffer.
pub(super) fn render_3b(
    editable_fields: &[String],
    detail: &ItemDetail,
    tables: &TaigaSlugTables,
    editable_overrides: Option<&HashMap<String, String>>,
    body_override: Option<&str>,
    append_cache: bool,
) -> String {
    let mut out = String::new();
    for key in editable_fields {
        let value = editable_overrides
            .and_then(|o| o.get(key).cloned())
            .unwrap_or_else(|| editable_value_with_slugs(detail, key, tables));
        out.push_str(&format!("{key}: {value}\n"));
    }
    out.push_str(EDITABLE_MARKER);
    out.push('\n');
    let editable_set: HashSet<&str> = editable_fields.iter().map(String::as_str).collect();
    for key in ["ref", "type", "modified"] {
        if editable_set.contains(key) {
            continue;
        }
        out.push_str(&format!("{key}: {}\n", readonly_value(detail, key)));
    }
    out.push_str(BODY_MARKER);
    out.push_str("\n\n");
    let body = body_override.unwrap_or(detail.description.as_str());
    out.push_str(body);
    if append_cache {
        out.push_str(&render_cache_section(tables));
    }
    out
}

/// Re-render after Reopen, preserving user header edits.
pub(super) fn render_3b_from_parsed(
    parsed: &Parsed3b,
    editable_fields: &[String],
    detail: &ItemDetail,
    tables: &TaigaSlugTables,
) -> String {
    let mut overrides: HashMap<String, String> = HashMap::new();
    for key in editable_fields {
        if let Some(v) = parsed.editable.get(key) {
            overrides.insert(key.clone(), resolved_to_slug(key, v, tables));
        }
    }
    render_3b(
        editable_fields,
        detail,
        tables,
        Some(&overrides),
        Some(&parsed.body),
        true,
    )
}

/// Reverse of `editable_value_with_slugs` — for re-rendering already-resolved
/// values back into slug form.
fn resolved_to_slug(key: &str, resolved: &str, tables: &TaigaSlugTables) -> String {
    match key {
        "status" => tables
            .statuses
            .slug_for(resolved)
            .map(String::from)
            .unwrap_or_else(|| resolved.to_string()),
        "assignee" => {
            if resolved.is_empty() {
                String::new()
            } else {
                tables
                    .users
                    .slug_for(resolved)
                    .map(String::from)
                    .unwrap_or_else(|| resolved.to_string())
            }
        }
        "tags" => resolved
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|orig| {
                tables
                    .tags
                    .slug_for(orig)
                    .map(String::from)
                    .unwrap_or_else(|| orig.to_string())
            })
            .collect::<Vec<_>>()
            .join(", "),
        _ => resolved.to_string(),
    }
}

pub(super) fn strip_banner(text: &str) -> &str {
    for (start, end) in [
        (ERROR_BANNER_START, ERROR_BANNER_END),
        (FOREIGN_BANNER_START, FOREIGN_BANNER_END),
    ] {
        if let Some(rest) = text.strip_prefix(start) {
            let after_start = rest.strip_prefix('\n').unwrap_or(rest);
            let needle = format!("\n{end}");
            if let Some(pos) = after_start.find(&needle) {
                let after_end = &after_start[pos + needle.len()..];
                return after_end.strip_prefix('\n').unwrap_or(after_end);
            }
            return after_start;
        }
    }
    text
}

pub(super) fn strip_cache_section(text: &str) -> &str {
    if let Some(pos) = text.find(CACHE_MARKER) {
        text[..pos].trim_end_matches(|c: char| c.is_whitespace())
    } else {
        text
    }
}

pub(super) fn parse_3b(text: &str) -> std::result::Result<Parsed3b, Vec<FieldError>> {
    let text = strip_cache_section(text);
    let text = strip_banner(text);

    #[derive(PartialEq)]
    enum Section { Editable, Readonly, Body }
    let mut section = Section::Editable;
    let mut editable: HashMap<String, String> = HashMap::new();
    let mut body_lines: Vec<&str> = Vec::new();
    let mut errors: Vec<FieldError> = Vec::new();
    let mut saw_editable_marker = false;
    let mut saw_body_marker = false;

    for (idx, line) in text.lines().enumerate() {
        let lineno = idx + 1;
        let trimmed = line.trim_end();
        match section {
            Section::Editable => {
                if trimmed == EDITABLE_MARKER {
                    section = Section::Readonly;
                    saw_editable_marker = true;
                    continue;
                }
                if trimmed == BODY_MARKER {
                    errors.push(FieldError {
                        message: format!("line {lineno}: `===` before `---` marker"),
                    });
                    section = Section::Body;
                    saw_body_marker = true;
                    continue;
                }
                let raw = line.trim_start();
                if raw.is_empty() || raw.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = raw.split_once(':') {
                    let key = key.trim().to_string();
                    if key.is_empty() {
                        errors.push(FieldError {
                            message: format!("line {lineno}: empty key"),
                        });
                        continue;
                    }
                    let value = value.split('#').next().unwrap_or("").trim().to_string();
                    editable.insert(key, value);
                } else {
                    errors.push(FieldError {
                        message: format!("line {lineno}: expected `key: value`"),
                    });
                }
            }
            Section::Readonly => {
                if trimmed == BODY_MARKER {
                    section = Section::Body;
                    saw_body_marker = true;
                    continue;
                }
                if trimmed == EDITABLE_MARKER {
                    errors.push(FieldError {
                        message: format!("line {lineno}: duplicate `---` marker"),
                    });
                }
            }
            Section::Body => body_lines.push(line),
        }
    }

    if !saw_editable_marker {
        errors.push(FieldError {
            message: "missing `---` marker between editable and read-only sections".into(),
        });
    }
    if !saw_body_marker {
        errors.push(FieldError {
            message: "missing `===` marker before body".into(),
        });
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    while body_lines.first().is_some_and(|l| l.trim().is_empty()) {
        body_lines.remove(0);
    }
    while body_lines.last().is_some_and(|l| l.trim().is_empty()) {
        body_lines.pop();
    }
    Ok(Parsed3b { editable, body: body_lines.join("\n") })
}

/// Field-level checks (run after `parse_3b`).
pub(super) fn validate_3b(
    parsed: &Parsed3b,
    editable_fields: &[String],
) -> Vec<FieldError> {
    let mut errors = Vec::new();
    let allowed: HashSet<&str> = editable_fields.iter().map(String::as_str).collect();
    for key in parsed.editable.keys() {
        if !allowed.contains(key.as_str()) {
            errors.push(FieldError {
                message: format!(
                    "unknown editable field `{key}` (allowed: {})",
                    editable_fields.join(", "),
                ),
            });
        }
    }
    if editable_fields.iter().any(|f| f == "subject") {
        match parsed.editable.get("subject") {
            Some(v) if v.trim().is_empty() => errors.push(FieldError {
                message: "subject must not be empty".into(),
            }),
            None => errors.push(FieldError {
                message: "subject is required".into(),
            }),
            _ => {}
        }
    }
    errors
}

/// Translate `ss-…` / `uu-…` / `tt-…` slugs in the parsed editable section
/// into their canonical wire-form values. Unknown slugs produce errors.
pub(super) fn resolve_slugs_inplace(
    parsed: &mut Parsed3b,
    tables: &TaigaSlugTables,
    errors: &mut Vec<FieldError>,
) {
    if let Some(raw) = parsed.editable.get("status").cloned() {
        let raw = raw.trim();
        if raw.is_empty() {
            errors.push(FieldError {
                message: "status must not be empty".into(),
            });
        } else if !raw.starts_with(STATUS_PREFIX) {
            errors.push(FieldError {
                message: format!(
                    "status `{raw}` must be a `{STATUS_PREFIX}` slug from the COMPLETIONS section"
                ),
            });
        } else {
            match tables.statuses.original_for(raw) {
                Some(orig) => {
                    parsed.editable.insert("status".into(), orig.to_string());
                }
                None => errors.push(FieldError {
                    message: format!("unknown status slug `{raw}`"),
                }),
            }
        }
    }

    // assignee: comma-separated list of `uu-` slugs (empty = no assignees).
    // Resolved to canonical usernames, kept comma-joined for downstream
    // diff/execute. Duplicates collapse; order from the buffer is preserved
    // (lets users keep their preferred ordering).
    if let Some(raw) = parsed.editable.get("assignee").cloned() {
        let mut originals: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for item in raw.split(',') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            if !item.starts_with(USER_PREFIX) {
                errors.push(FieldError {
                    message: format!(
                        "assignee `{item}` must be a `{USER_PREFIX}` slug from COMPLETIONS (or empty)"
                    ),
                });
                continue;
            }
            match tables.users.original_for(item) {
                Some(orig) => {
                    let orig = orig.to_string();
                    if seen.insert(orig.clone()) {
                        originals.push(orig);
                    }
                }
                None => errors.push(FieldError {
                    message: format!("unknown user slug `{item}`"),
                }),
            }
        }
        parsed.editable.insert("assignee".into(), originals.join(","));
    }

    if let Some(raw) = parsed.editable.get("tags").cloned() {
        let mut originals: Vec<String> = Vec::new();
        for item in raw.split(',') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            if !item.starts_with(TAG_PREFIX) {
                errors.push(FieldError {
                    message: format!(
                        "tag `{item}` must start with `{TAG_PREFIX}` (use a slug from COMPLETIONS)"
                    ),
                });
                continue;
            }
            match tables.tags.original_for(item) {
                Some(orig) => originals.push(orig.to_string()),
                None => errors.push(FieldError {
                    message: format!("unknown tag slug `{item}`"),
                }),
            }
        }
        originals.sort();
        originals.dedup();
        parsed.editable.insert("tags".into(), originals.join(","));
    }
}

pub(super) fn render_with_errors(original_text: &str, errors: &[FieldError]) -> String {
    let stripped = strip_banner(original_text);
    let mut out = String::new();
    out.push_str(ERROR_BANNER_START);
    out.push('\n');
    for e in errors {
        out.push_str(&format!("# • {}\n", e.message));
    }
    out.push_str(ERROR_BANNER_END);
    out.push('\n');
    out.push_str(stripped);
    out
}

/// Diff parsed editable against current detail. `assignee` is compared by
/// canonical username; `tags` compares as sorted set; everything else by
/// equality on the resolved value.
pub(super) fn diff_against_current(
    parsed: &Parsed3b,
    detail: &ItemDetail,
) -> ChangeSet {
    let mut metadata_changes = Vec::new();
    for (key, new_value) in &parsed.editable {
        let unchanged = match key.as_str() {
            "subject" => detail.subject == *new_value,
            "status" => detail.status == *new_value,
            "assignee" => {
                let mut current: Vec<&str> = detail
                    .assignee_usernames
                    .iter()
                    .map(String::as_str)
                    .collect();
                current.sort();
                let mut new_list: Vec<&str> = new_value
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect();
                new_list.sort();
                new_list.dedup();
                current == new_list
            }
            "tags" => {
                let mut current: Vec<&str> = detail.tags.iter().map(String::as_str).collect();
                current.sort();
                let mut new_list: Vec<&str> = new_value
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect();
                new_list.sort();
                new_list.dedup();
                current == new_list
            }
            _ => true,
        };
        if !unchanged {
            metadata_changes.push((key.clone(), new_value.clone()));
        }
    }
    let body = parsed.body.trim();
    let body_change = if body != detail.description.trim() {
        Some(body.to_string())
    } else {
        None
    };
    ChangeSet { metadata_changes, body: body_change }
}
