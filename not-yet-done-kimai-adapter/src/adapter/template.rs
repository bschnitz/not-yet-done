//! Edit-template rendering and parsing for the `edit` action on a
//! timesheet. Same layout as the Jira adapter's 3b format: editable
//! `key: value` lines / `---` / read-only section / `===` / body (the
//! description), plus a trailing read-only CACHE section advertising the
//! available project and activity *slugs* — single-token, whitespace-free
//! identifiers, so editor word-completion (vim `<C-n>`) actually works
//! (project/activity names contain spaces and defeat it).
//!
//! Editable fields: `project` (a combined `customer_project` slug — the
//! customer and project are coupled in one field, since a timesheet only
//! stores the project and Kimai derives its customer) and `activity` (its
//! own slug). Both accept the slug shown in the CACHE and next to the
//! field; a full name (case-insensitive) or the `#<id>` fallback form are
//! also accepted.
//! `begin` (local `YYYY-MM-DDTHH:MM[:SS]`, space separator also accepted)
//! and `duration` (`H:MM[:SS]` or plain seconds). Kimai derives duration
//! from the begin/end pair, so a duration change is materialised as
//! `end = begin + duration` in the PATCH. Running entries (no end yet)
//! ignore the duration line — only begin can move.

use std::collections::HashMap;

use chrono::NaiveDateTime;

use crate::client::{KimaiProject, KimaiTimesheet};

pub(super) const EDITABLE_MARKER: &str = "---";
pub(super) const BODY_MARKER: &str = "===";
pub(super) const CACHE_MARKER: &str =
    "#### CACHE / available projects & activities (do not edit) ####";

const ERROR_BANNER_START: &str = "# ─── ERRORS ───";
const ERROR_BANNER_END: &str = "# ──────────────";
const CONFLICT_BANNER_START: &str = "# ─── CONFLICT ───";
const CONFLICT_BANNER_END: &str = "# ─────────────────";

/// Result of [`parse_edit`]: editable values + description body.
#[derive(Debug, Default)]
pub(super) struct ParsedEdit {
    /// Lowercased key → value (inline `# comment` stripped, trimmed).
    pub(super) editable: HashMap<String, String>,
    /// Body below the `===` marker (leading/trailing blank lines trimmed,
    /// inner formatting preserved).
    pub(super) body: String,
}

/// The PATCH payload derived from a parsed buffer, plus the field names
/// that actually changed (for the confirmation message).
#[derive(Debug)]
pub(super) struct EditPlan {
    pub(super) patch: serde_json::Value,
    pub(super) changed: Vec<&'static str>,
}

/// Opaque version token for conflict detection — any upstream change to
/// an editable aspect of the record changes the token.
pub(super) fn version_token(ts: &KimaiTimesheet) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        ts.begin,
        ts.end.as_deref().unwrap_or(""),
        ts.project,
        ts.activity,
        ts.duration.unwrap_or(0),
        ts.description.as_deref().unwrap_or("")
    )
}

/// Local datetime part of a Kimai timestamp — the leading
/// `YYYY-MM-DDTHH:MM:SS` without the offset suffix. Kimai reports times in
/// the user's own timezone, so the local part is what the user edits and
/// what PATCH expects back.
fn local_part(value: &str) -> &str {
    value.get(..19).unwrap_or(value)
}

/// `H:MM:SS` display for a seconds value (negative-safe).
pub(super) fn format_duration_hms(secs: i64) -> String {
    let sign = if secs < 0 { "-" } else { "" };
    let s = secs.abs();
    format!("{sign}{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// Parse a duration value: `H:MM:SS`, `H:MM`, or plain seconds.
pub(super) fn parse_duration(value: &str) -> Result<i64, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("duration must not be empty".into());
    }
    let (sign, rest) = match value.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, value),
    };
    let parts: Vec<&str> = rest.split(':').collect();
    let nums: Option<Vec<i64>> = parts.iter().map(|p| p.trim().parse().ok()).collect();
    let secs = match (parts.len(), nums) {
        (1, Some(n)) => n[0],
        (2, Some(n)) if (0..60).contains(&n[1]) => n[0] * 3600 + n[1] * 60,
        (3, Some(n)) if (0..60).contains(&n[1]) && (0..60).contains(&n[2]) => {
            n[0] * 3600 + n[1] * 60 + n[2]
        }
        _ => {
            return Err(format!(
                "invalid duration `{value}` — expected `H:MM:SS`, `H:MM`, or seconds"
            ));
        }
    };
    Ok(sign * secs)
}

/// Parse a begin value: local datetime, `T` or space separator, seconds
/// optional.
pub(super) fn parse_begin(value: &str) -> Result<NaiveDateTime, String> {
    let value = value.trim();
    for fmt in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(value, fmt) {
            return Ok(dt);
        }
    }
    Err(format!(
        "invalid begin `{value}` — expected `YYYY-MM-DDTHH:MM[:SS]`"
    ))
}

/// Turn a display name into a single-token slug: lowercase, every run of
/// non-alphanumeric characters collapsed to one `-`, leading/trailing `-`
/// trimmed. Whitespace-free by construction, so editor word-completion can
/// pick it up. An all-punctuation name slugifies to the empty string — the
/// caller falls back to the `#<id>` form.
fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_alphanumeric() {
            slug.extend(ch.to_lowercase());
            prev_dash = false;
        } else if !prev_dash && !slug.is_empty() {
            slug.push('-');
            prev_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

/// Append `-<id>` to every base token that collides with another, so each
/// resulting token maps back to exactly one id.
fn dedup_tokens(base: HashMap<u64, String>) -> HashMap<u64, String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for t in base.values() {
        *counts.entry(t.as_str()).or_default() += 1;
    }
    base.iter()
        .map(|(id, t)| {
            let token = if counts[t.as_str()] > 1 {
                format!("{t}-{id}")
            } else {
                t.clone()
            };
            (*id, token)
        })
        .collect()
}

/// Build an id → slug map for a set of id → name entries. Slugs are unique
/// (colliding names get their numeric id appended); an empty slug falls back
/// to `id-<id>`. Used for activities.
fn build_slugs(names: &HashMap<u64, String>) -> HashMap<u64, String> {
    let base = names
        .iter()
        .map(|(id, name)| {
            let s = slugify(name);
            (*id, if s.is_empty() { format!("id-{id}") } else { s })
        })
        .collect();
    dedup_tokens(base)
}

/// Build an id → combined-token map for projects: `customer_project` slug,
/// or the bare project slug when the project has no customer. Tokens are
/// unique (collisions get the numeric id appended). This is the coupled
/// customer+project identifier shown and entered in the edit buffer.
fn project_tokens(projects: &HashMap<u64, KimaiProject>) -> HashMap<u64, String> {
    let base = projects
        .iter()
        .map(|(id, p)| {
            let proj = slugify(&p.name);
            let proj = if proj.is_empty() { format!("id-{id}") } else { proj };
            let token = match p.parent_title.as_deref().map(slugify).filter(|c| !c.is_empty()) {
                Some(customer) => format!("{customer}_{proj}"),
                None => proj,
            };
            (*id, token)
        })
        .collect();
    dedup_tokens(base)
}

/// Resolve an activity value back to its id. Tries, in order: the `#<id>`
/// fallback form (verbatim), the slug shown in the buffer/CACHE, and finally
/// a full name (case-insensitive) for backward compatibility and hand-typed
/// entries. On failure lists the available slugs.
fn resolve_name(
    value: &str,
    names: &HashMap<u64, String>,
    what: &str,
) -> Result<u64, String> {
    let value = value.trim();
    if let Some(id) = value.strip_prefix('#').and_then(|v| v.parse().ok()) {
        return Ok(id);
    }
    let lowered = value.to_lowercase();
    let slugs = build_slugs(names);
    if let Some((id, _)) = slugs.iter().find(|(_, s)| **s == lowered) {
        return Ok(*id);
    }
    if let Some((id, _)) = names.iter().find(|(_, n)| n.to_lowercase() == lowered) {
        return Ok(*id);
    }
    let mut available: Vec<&str> = slugs.values().map(String::as_str).collect();
    available.sort_unstable();
    Err(format!(
        "unknown {what} `{value}` (available: {})",
        available.join(", ")
    ))
}

/// Resolve a project value back to its id. The buffer/CACHE form is the
/// combined `customer_project` slug; a bare project slug (projects with no
/// customer), a full project name (case-insensitive) and the `#<id>`
/// fallback are also accepted. On failure lists the available combined
/// tokens.
fn resolve_project(
    value: &str,
    projects: &HashMap<u64, KimaiProject>,
) -> Result<u64, String> {
    let value = value.trim();
    if let Some(id) = value.strip_prefix('#').and_then(|v| v.parse().ok()) {
        return Ok(id);
    }
    let lowered = value.to_lowercase();
    let tokens = project_tokens(projects);
    if let Some((id, _)) = tokens.iter().find(|(_, t)| **t == lowered) {
        return Ok(*id);
    }
    if let Some((id, _)) = projects.iter().find(|(_, p)| p.name.to_lowercase() == lowered) {
        return Ok(*id);
    }
    let mut available: Vec<&str> = tokens.values().map(String::as_str).collect();
    available.sort_unstable();
    Err(format!(
        "unknown project `{value}` (available: {})",
        available.join(", ")
    ))
}

/// Render the edit buffer for one timesheet.
pub(super) fn render_edit_template(
    ts: &KimaiTimesheet,
    projects: &HashMap<u64, KimaiProject>,
    activities: &HashMap<u64, String>,
) -> String {
    let project_token_map = project_tokens(projects);
    let activity_slugs = build_slugs(activities);

    // Project value = the combined `customer_project` slug; activity = its
    // own slug. The readable name(s) ride along as an inline `# comment`
    // (stripped on parse) so the buffer stays legible. Entities missing from
    // the lookup fall back to `#<id>`.
    let project = match (project_token_map.get(&ts.project), projects.get(&ts.project)) {
        (Some(token), Some(p)) => {
            let display = match p.parent_title.as_deref().filter(|c| !c.is_empty()) {
                Some(customer) => format!("{customer} / {}", p.name),
                None => p.name.clone(),
            };
            format!("{token}  # {display}")
        }
        _ => format!("#{}", ts.project),
    };
    let activity = match (activity_slugs.get(&ts.activity), activities.get(&ts.activity)) {
        (Some(slug), Some(name)) => format!("{slug}  # {name}"),
        _ => format!("#{}", ts.activity),
    };
    let running = ts.end.is_none();

    let mut out = String::new();
    out.push_str(&format!("project: {project}\n"));
    out.push_str(&format!("activity: {activity}\n"));
    out.push_str(&format!("begin: {}\n", local_part(&ts.begin)));
    if running {
        out.push_str("# running entry — duration is ignored, only begin can move\n");
    }
    out.push_str(&format!(
        "duration: {}\n",
        format_duration_hms(ts.duration.unwrap_or(0))
    ));
    out.push_str(EDITABLE_MARKER);
    out.push('\n');
    out.push_str(&format!("id: {}\n", ts.id));
    out.push_str(&format!(
        "end: {}\n",
        ts.end.as_deref().map(local_part).unwrap_or("(running)")
    ));
    if !ts.tags.is_empty() {
        out.push_str(&format!("tags: {}\n", ts.tags.join(", ")));
    }
    out.push_str(BODY_MARKER);
    out.push_str("\n\n");
    out.push_str(ts.description.as_deref().unwrap_or(""));
    out.push('\n');

    // Advertise the tokens (single, whitespace-free) so editor
    // word-completion works — projects as `customer_project`.
    let mut project_token_list: Vec<&str> =
        project_token_map.values().map(String::as_str).collect();
    project_token_list.sort_unstable();
    let mut activity_slug_list: Vec<&str> =
        activity_slugs.values().map(String::as_str).collect();
    activity_slug_list.sort_unstable();
    if !project_token_list.is_empty() || !activity_slug_list.is_empty() {
        out.push('\n');
        out.push_str(CACHE_MARKER);
        out.push('\n');
        if !project_token_list.is_empty() {
            out.push_str(&format!("# projects: {}\n", project_token_list.join(", ")));
        }
        if !activity_slug_list.is_empty() {
            out.push_str(&format!("# activities: {}\n", activity_slug_list.join(", ")));
        }
    }
    out
}

/// Strip the trailing CACHE section before parsing. Idempotent.
fn strip_cache_section(text: &str) -> &str {
    match text.find(CACHE_MARKER) {
        Some(pos) => text[..pos].trim_end_matches(|c: char| c.is_whitespace()),
        None => text,
    }
}

/// Strip a previously-rendered error/conflict banner from the top of a
/// buffer so reopens don't stack banners. Idempotent on banner-less input.
fn strip_banner(text: &str) -> &str {
    for (start, end) in [
        (ERROR_BANNER_START, ERROR_BANNER_END),
        (CONFLICT_BANNER_START, CONFLICT_BANNER_END),
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

/// Structural parse of an edit buffer. Field-level validation happens in
/// [`build_edit_plan`].
pub(super) fn parse_edit(text: &str) -> Result<ParsedEdit, Vec<String>> {
    let text = strip_cache_section(text);
    let text = strip_banner(text);

    #[derive(PartialEq)]
    enum Section {
        Editable,
        Readonly,
        Body,
    }
    let mut section = Section::Editable;
    let mut editable: HashMap<String, String> = HashMap::new();
    let mut body_lines: Vec<&str> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
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
                    errors.push(format!("line {lineno}: `===` before `---` marker"));
                    section = Section::Body;
                    saw_body_marker = true;
                    continue;
                }
                let raw = line.trim_start();
                if raw.is_empty() || raw.starts_with('#') {
                    continue;
                }
                match raw.split_once(':') {
                    Some((key, value)) if !key.trim().is_empty() => {
                        // Strip inline ` # comment` — but only when the `#`
                        // follows the value across whitespace, so the
                        // `#<id>` fallback value form survives.
                        let value = value.trim();
                        let value = match value.find(" #") {
                            Some(pos) => value[..pos].trim_end(),
                            None => value,
                        };
                        editable.insert(key.trim().to_lowercase(), value.to_string());
                    }
                    _ => errors.push(format!("line {lineno}: expected `key: value`")),
                }
            }
            Section::Readonly => {
                if trimmed == BODY_MARKER {
                    section = Section::Body;
                    saw_body_marker = true;
                }
                // Everything else is read-only — ignored.
            }
            Section::Body => body_lines.push(line),
        }
    }

    if !saw_editable_marker {
        errors.push("missing `---` marker between editable and read-only sections".into());
    }
    if !saw_body_marker {
        errors.push("missing `===` marker before the description body".into());
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
    Ok(ParsedEdit {
        editable,
        body: body_lines.join("\n"),
    })
}

/// Validate a parsed buffer against the current upstream record and build
/// the PATCH payload from the changed fields only. `Ok(None)` = nothing
/// changed.
pub(super) fn build_edit_plan(
    parsed: &ParsedEdit,
    current: &KimaiTimesheet,
    projects: &HashMap<u64, KimaiProject>,
    activities: &HashMap<u64, String>,
) -> Result<Option<EditPlan>, Vec<String>> {
    const ALLOWED: [&str; 4] = ["project", "activity", "begin", "duration"];
    let mut errors: Vec<String> = Vec::new();

    for key in parsed.editable.keys() {
        if !ALLOWED.contains(&key.as_str()) {
            errors.push(format!(
                "unknown editable field `{key}` (allowed: {})",
                ALLOWED.join(", ")
            ));
        }
    }

    let project_id = parsed.editable.get("project").and_then(|v| {
        match resolve_project(v, projects) {
            Ok(id) => Some(id),
            Err(e) => {
                errors.push(e);
                None
            }
        }
    });
    let activity_id = parsed.editable.get("activity").and_then(|v| {
        match resolve_name(v, activities, "activity") {
            Ok(id) => Some(id),
            Err(e) => {
                errors.push(e);
                None
            }
        }
    });
    let begin = parsed.editable.get("begin").and_then(|v| {
        parse_begin(v).map_err(|e| errors.push(e)).ok()
    });
    let duration = parsed.editable.get("duration").and_then(|v| {
        parse_duration(v).map_err(|e| errors.push(e)).ok()
    });
    if !errors.is_empty() {
        return Err(errors);
    }

    let running = current.end.is_none();
    let current_begin = local_part(&current.begin);
    let current_duration = current.duration.unwrap_or(0);

    let mut patch = serde_json::Map::new();
    let mut changed: Vec<&'static str> = Vec::new();

    if let Some(id) = project_id
        && id != current.project
    {
        patch.insert("project".into(), id.into());
        changed.push("project");
    }
    if let Some(id) = activity_id
        && id != current.activity
    {
        patch.insert("activity".into(), id.into());
        changed.push("activity");
    }

    let begin_changed = begin
        .is_some_and(|b| b.format("%Y-%m-%dT%H:%M:%S").to_string() != current_begin);
    let duration_changed = !running && duration.is_some_and(|d| d != current_duration);
    if begin_changed || duration_changed {
        // Kimai derives duration from the begin/end pair, so both a begin
        // move and a duration edit are expressed through begin + end.
        let new_begin = begin
            .or_else(|| parse_begin(current_begin).ok())
            .expect("current begin parses");
        patch.insert(
            "begin".into(),
            new_begin.format("%Y-%m-%dT%H:%M:%S").to_string().into(),
        );
        if begin_changed {
            changed.push("begin");
        }
        if !running {
            let new_duration = duration.unwrap_or(current_duration);
            let end = new_begin + chrono::Duration::seconds(new_duration);
            patch.insert(
                "end".into(),
                end.format("%Y-%m-%dT%H:%M:%S").to_string().into(),
            );
            if duration_changed {
                changed.push("duration");
            }
        }
    }

    let current_description = current.description.as_deref().unwrap_or("").trim();
    if parsed.body.trim() != current_description {
        patch.insert("description".into(), parsed.body.trim().into());
        changed.push("description");
    }

    if patch.is_empty() {
        return Ok(None);
    }
    Ok(Some(EditPlan {
        patch: serde_json::Value::Object(patch),
        changed,
    }))
}

/// Prepend an error banner above the user's buffer for a Reopen. A
/// pre-existing banner is stripped first so reopens don't stack.
pub(super) fn render_with_errors(text: &str, errors: &[String]) -> String {
    let stripped = strip_banner(text);
    let mut out = String::new();
    out.push_str(ERROR_BANNER_START);
    out.push('\n');
    for e in errors {
        out.push_str(&format!("# • {e}\n"));
    }
    out.push_str(ERROR_BANNER_END);
    out.push('\n');
    out.push_str(stripped);
    out
}

/// Prepend a conflict banner (record changed upstream while editing). The
/// user's edits stay in the buffer; saving again diffs against the fresh
/// upstream state via the renewed version token.
pub(super) fn render_with_conflict(text: &str) -> String {
    let stripped = strip_banner(text);
    let mut out = String::new();
    out.push_str(CONFLICT_BANNER_START);
    out.push('\n');
    out.push_str("# • this timesheet changed on the server while you were editing\n");
    out.push_str("# • your buffer is unchanged — review and save again to apply\n");
    out.push_str(CONFLICT_BANNER_END);
    out.push('\n');
    out.push_str(stripped);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_timesheet() -> KimaiTimesheet {
        KimaiTimesheet {
            id: 4711,
            project: 7,
            activity: 3,
            begin: "2030-01-15T09:00:00+0100".into(),
            end: Some("2030-01-15T10:30:00+0100".into()),
            duration: Some(5400),
            description: Some("Refactor login form\nsecond line".into()),
            tags: vec!["backend".into()],
        }
    }

    fn lookups() -> (HashMap<u64, KimaiProject>, HashMap<u64, String>) {
        let projects = HashMap::from([
            (
                7,
                KimaiProject {
                    id: 7,
                    name: "Website Relaunch".to_string(),
                    parent_title: Some("Acme Corp".to_string()),
                },
            ),
            (
                8,
                KimaiProject {
                    id: 8,
                    name: "Internal".to_string(),
                    parent_title: None,
                },
            ),
        ]);
        let activities =
            HashMap::from([(3, "Development".to_string()), (4, "Meeting".to_string())]);
        (projects, activities)
    }

    #[test]
    fn duration_formats_and_parses() {
        assert_eq!(format_duration_hms(5400), "1:30:00");
        assert_eq!(format_duration_hms(0), "0:00:00");
        assert_eq!(format_duration_hms(3661), "1:01:01");

        assert_eq!(parse_duration("1:30:00").unwrap(), 5400);
        assert_eq!(parse_duration("1:30").unwrap(), 5400);
        assert_eq!(parse_duration("5400").unwrap(), 5400);
        assert!(parse_duration("1:75").is_err());
        assert!(parse_duration("abc").is_err());
    }

    #[test]
    fn begin_parses_variants() {
        for v in [
            "2030-01-15T09:00:00",
            "2030-01-15T09:00",
            "2030-01-15 09:00:00",
            "2030-01-15 09:00",
        ] {
            assert!(parse_begin(v).is_ok(), "should parse: {v}");
        }
        assert!(parse_begin("15.01.2030 09:00").is_err());
    }

    #[test]
    fn template_round_trips_unchanged() {
        let (projects, activities) = lookups();
        let ts = sample_timesheet();
        let template = render_edit_template(&ts, &projects, &activities);
        assert!(template.contains("project: acme-corp_website-relaunch  # Acme Corp / Website Relaunch"));
        assert!(template.contains("activity: development  # Development"));
        assert!(template.contains("begin: 2030-01-15T09:00:00"));
        assert!(template.contains("duration: 1:30:00"));
        // Customer is no longer a standalone read-only line — it is folded
        // into the coupled project token above.
        assert!(!template.contains("customer:"));
        assert!(template.contains(CACHE_MARKER));
        assert!(template.contains("# projects: acme-corp_website-relaunch, internal"));
        assert!(template.contains("# activities: development, meeting"));

        let parsed = parse_edit(&template).unwrap();
        assert_eq!(parsed.body, "Refactor login form\nsecond line");
        let plan = build_edit_plan(&parsed, &ts, &projects, &activities).unwrap();
        assert!(plan.is_none(), "unchanged buffer must produce no patch");
    }

    #[test]
    fn duration_edit_recomputes_end() {
        let (projects, activities) = lookups();
        let ts = sample_timesheet();
        let template = render_edit_template(&ts, &projects, &activities);
        let edited = template.replace("duration: 1:30:00", "duration: 2:00:00");

        let parsed = parse_edit(&edited).unwrap();
        let plan = build_edit_plan(&parsed, &ts, &projects, &activities)
            .unwrap()
            .expect("changed");
        assert_eq!(plan.patch["begin"], "2030-01-15T09:00:00");
        assert_eq!(plan.patch["end"], "2030-01-15T11:00:00");
        assert_eq!(plan.changed, vec!["duration"]);
        assert!(plan.patch.get("project").is_none());
    }

    #[test]
    fn slugify_produces_single_tokens() {
        assert_eq!(slugify("Website Relaunch"), "website-relaunch");
        assert_eq!(slugify("  Multiple   spaces / slashes  "), "multiple-spaces-slashes");
        assert_eq!(slugify("Bug-Fixing (urgent!)"), "bug-fixing-urgent");
        assert_eq!(slugify("!!!"), "");
    }

    #[test]
    fn build_slugs_disambiguates_collisions() {
        let names = HashMap::from([
            (1, "Website Relaunch".to_string()),
            (2, "Website  Relaunch!".to_string()),
            (3, "Internal".to_string()),
        ]);
        let slugs = build_slugs(&names);
        // Both collide on `website-relaunch` → id suffix disambiguates.
        assert_eq!(slugs[&1], "website-relaunch-1");
        assert_eq!(slugs[&2], "website-relaunch-2");
        // The unique one stays clean.
        assert_eq!(slugs[&3], "internal");
    }

    #[test]
    fn resolve_name_accepts_slug_name_and_hash_id() {
        // Activity resolution: slug / full name (any case) / `#<id>`.
        let (_, activities) = lookups();
        assert_eq!(resolve_name("development", &activities, "activity").unwrap(), 3);
        assert_eq!(resolve_name("Development", &activities, "activity").unwrap(), 3);
        assert_eq!(resolve_name("MEETING", &activities, "activity").unwrap(), 4);
        assert_eq!(resolve_name("#42", &activities, "activity").unwrap(), 42);
        let err = resolve_name("nope", &activities, "activity").unwrap_err();
        assert!(err.contains("development, meeting"));
    }

    #[test]
    fn resolve_project_accepts_combined_name_and_hash_id() {
        let (projects, _) = lookups();
        // Combined `customer_project` token.
        assert_eq!(resolve_project("acme-corp_website-relaunch", &projects).unwrap(), 7);
        // Bare token for the customer-less project.
        assert_eq!(resolve_project("internal", &projects).unwrap(), 8);
        // Full project name (any case), backward compat.
        assert_eq!(resolve_project("Website Relaunch", &projects).unwrap(), 7);
        assert_eq!(resolve_project("WEBSITE RELAUNCH", &projects).unwrap(), 7);
        // `#<id>` fallback.
        assert_eq!(resolve_project("#42", &projects).unwrap(), 42);
        let err = resolve_project("nope", &projects).unwrap_err();
        assert!(err.contains("acme-corp_website-relaunch, internal"));
    }

    #[test]
    fn same_project_name_under_two_customers_gets_distinct_tokens() {
        let projects = HashMap::from([
            (
                1,
                KimaiProject {
                    id: 1,
                    name: "Support".to_string(),
                    parent_title: Some("Acme Corp".to_string()),
                },
            ),
            (
                2,
                KimaiProject {
                    id: 2,
                    name: "Support".to_string(),
                    parent_title: Some("Globex".to_string()),
                },
            ),
        ]);
        assert_eq!(resolve_project("acme-corp_support", &projects).unwrap(), 1);
        assert_eq!(resolve_project("globex_support", &projects).unwrap(), 2);
    }

    #[test]
    fn project_and_activity_resolve_by_slug() {
        let (projects, activities) = lookups();
        let ts = sample_timesheet();
        let template = render_edit_template(&ts, &projects, &activities);
        let edited = template
            .replace("project: acme-corp_website-relaunch", "project: internal")
            .replace("activity: development", "activity: meeting");

        let parsed = parse_edit(&edited).unwrap();
        let plan = build_edit_plan(&parsed, &ts, &projects, &activities)
            .unwrap()
            .expect("changed");
        assert_eq!(plan.patch["project"], 8);
        assert_eq!(plan.patch["activity"], 4);
    }

    #[test]
    fn unknown_project_lists_available_tokens() {
        let (projects, activities) = lookups();
        let ts = sample_timesheet();
        let template = render_edit_template(&ts, &projects, &activities);
        let edited = template.replace("project: acme-corp_website-relaunch", "project: Nope");

        let parsed = parse_edit(&edited).unwrap();
        let errors = build_edit_plan(&parsed, &ts, &projects, &activities).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("unknown project `Nope`"));
        assert!(errors[0].contains("acme-corp_website-relaunch, internal"));
    }

    #[test]
    fn hash_id_fallback_round_trips() {
        let (_, activities) = lookups();
        let ts = sample_timesheet();
        // Project 7 missing from the lookup → rendered as `#7`; saving the
        // unchanged buffer must not fail resolution or produce a patch.
        let projects = HashMap::new();
        let template = render_edit_template(&ts, &projects, &activities);
        assert!(template.contains("project: #7"));
        let parsed = parse_edit(&template).unwrap();
        let plan = build_edit_plan(&parsed, &ts, &projects, &activities).unwrap();
        assert!(plan.is_none());
    }

    #[test]
    fn description_edit_patches_description_only() {
        let (projects, activities) = lookups();
        let ts = sample_timesheet();
        let template = render_edit_template(&ts, &projects, &activities);
        let edited = template.replace(
            "Refactor login form\nsecond line",
            "Rewrote the login flow\nwith extra detail",
        );

        let parsed = parse_edit(&edited).unwrap();
        let plan = build_edit_plan(&parsed, &ts, &projects, &activities)
            .unwrap()
            .expect("changed");
        assert_eq!(
            plan.patch["description"],
            "Rewrote the login flow\nwith extra detail"
        );
        assert_eq!(plan.changed, vec!["description"]);
        assert!(plan.patch.get("begin").is_none());
    }

    #[test]
    fn running_entry_ignores_duration() {
        let (projects, activities) = lookups();
        let ts = KimaiTimesheet {
            end: None,
            duration: Some(0),
            ..sample_timesheet()
        };
        let template = render_edit_template(&ts, &projects, &activities);
        assert!(template.contains("# running entry"));
        assert!(template.contains("end: (running)"));

        let edited = template.replace("duration: 0:00:00", "duration: 3:00:00");
        let parsed = parse_edit(&edited).unwrap();
        let plan = build_edit_plan(&parsed, &ts, &projects, &activities).unwrap();
        assert!(plan.is_none(), "duration edits on a running entry are ignored");
    }

    #[test]
    fn begin_edit_on_running_entry_sends_begin_without_end() {
        let (projects, activities) = lookups();
        let ts = KimaiTimesheet {
            end: None,
            duration: Some(0),
            ..sample_timesheet()
        };
        let template = render_edit_template(&ts, &projects, &activities);
        let edited =
            template.replace("begin: 2030-01-15T09:00:00", "begin: 2030-01-15T08:30:00");
        let parsed = parse_edit(&edited).unwrap();
        let plan = build_edit_plan(&parsed, &ts, &projects, &activities)
            .unwrap()
            .expect("changed");
        assert_eq!(plan.patch["begin"], "2030-01-15T08:30:00");
        assert!(plan.patch.get("end").is_none());
        assert_eq!(plan.changed, vec!["begin"]);
    }

    #[test]
    fn parse_errors_report_missing_markers() {
        let errors = parse_edit("project: X\nno markers here").unwrap_err();
        assert!(errors.iter().any(|e| e.contains("---")));
        assert!(errors.iter().any(|e| e.contains("===")));
    }

    #[test]
    fn error_banner_strips_before_reparse() {
        let (projects, activities) = lookups();
        let ts = sample_timesheet();
        let template = render_edit_template(&ts, &projects, &activities);
        let banned = render_with_errors(&template, &["something".into()]);
        let rebanned = render_with_errors(&banned, &["else".into()]);
        assert_eq!(rebanned.matches(ERROR_BANNER_START).count(), 1);
        let parsed = parse_edit(&rebanned).unwrap();
        assert_eq!(parsed.body, "Refactor login form\nsecond line");
    }

    #[test]
    fn version_token_changes_with_fields() {
        let ts = sample_timesheet();
        let mut other = sample_timesheet();
        other.duration = Some(9000);
        assert_ne!(version_token(&ts), version_token(&other));
        assert_eq!(version_token(&ts), version_token(&sample_timesheet()));
    }
}
