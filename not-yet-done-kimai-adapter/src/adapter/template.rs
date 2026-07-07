//! Edit-template rendering and parsing for the `edit` action on a
//! timesheet. Same layout as the Jira adapter's 3b format: editable
//! `key: value` lines / `---` / read-only section / `===` / body (the
//! description), plus a trailing read-only CACHE section advertising the
//! available `entry` *tokens* — single, whitespace-free identifiers, so
//! editor word-completion (vim `<C-n>`) actually works (project/activity
//! names contain spaces and defeat it).
//!
//! A timesheet's project, activity and customer are coupled: a timesheet
//! stores only a `(project, activity)` pair, Kimai derives the customer
//! from the project, and an activity is only bookable on the project it is
//! bound to (or on any project if it is global). So they are edited through
//! a **single** `entry` field = a `customer_project_activity` token that
//! resolves as a whole to a `(project_id, activity_id)` pair. The token
//! shown next to the field and in the CACHE is accepted; the direct
//! `#<pid>_#<aid>` escape is also accepted (and is what an entry whose
//! project/activity are unknown to the lookups renders as).
//!
//! The remaining editable fields are `begin` (local
//! `YYYY-MM-DDTHH:MM[:SS]`, space separator also accepted) and `duration`
//! (`H:MM[:SS]` or plain seconds). Kimai derives duration from the
//! begin/end pair, so a duration change is materialised as
//! `end = begin + duration` in the PATCH. Running entries (no end yet)
//! ignore the duration line — only begin can move.

use std::collections::HashMap;

use chrono::NaiveDateTime;

use crate::client::{KimaiActivity, KimaiProject, KimaiTimesheet};

pub(super) const EDITABLE_MARKER: &str = "---";
pub(super) const BODY_MARKER: &str = "===";
pub(super) const CACHE_MARKER: &str =
    "#### CACHE / available entries (do not edit) ####";

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

/// A slug, or the `id-<id>` fallback for an all-punctuation name that
/// slugifies to the empty string.
fn slug_or_id(name: &str, id: u64) -> String {
    let s = slugify(name);
    if s.is_empty() { format!("id-{id}") } else { s }
}

/// One resolvable `(project, activity)` combination: its combined token
/// (what the user types / completes), a human-readable display for the
/// inline comment, and the ids it maps back to.
struct EntryCombo {
    token: String,
    display: String,
    project: String,
    activity: String,
    project_id: u64,
    activity_id: u64,
}

/// Whether an activity can be booked on a given project. A *global*
/// activity (no `project` binding and no owning `parent_title`) fits every
/// project; otherwise it must belong to this project — by explicit
/// `project` id when the API provides one, else by matching `parent_title`
/// against the project name.
fn activity_valid_for_project(a: &KimaiActivity, pid: u64, pname: &str) -> bool {
    match a.project {
        Some(p) => p == pid,
        None => match a.parent_title.as_deref().filter(|t| !t.is_empty()) {
            Some(owner) => owner.eq_ignore_ascii_case(pname),
            None => true,
        },
    }
}

/// Build the base token and display for a `(project, activity)` pair. When
/// both entities are known the token is a slug chain
/// `[customer_]project_activity`; if either is unknown to the lookups it
/// falls back to the fully-escaped `#<pid>_#<aid>` form, which
/// [`resolve_entry`] accepts directly. Slugs never contain `_` (slugify
/// maps it to `-`), so the `_` separators are unambiguous.
fn combo_parts(
    pid: u64,
    aid: u64,
    projects: &HashMap<u64, KimaiProject>,
    activities: &HashMap<u64, KimaiActivity>,
) -> (String, String, String, String) {
    let project = projects.get(&pid);
    let activity = activities.get(&aid);
    let customer = project
        .and_then(|p| p.parent_title.as_deref())
        .filter(|c| !c.is_empty());

    let token = match (project, activity) {
        (Some(p), Some(a)) => {
            let proj = slug_or_id(&p.name, pid);
            let act = slug_or_id(&a.name, aid);
            match customer {
                Some(c) => format!("{}_{proj}_{act}", slugify(c)),
                None => format!("{proj}_{act}"),
            }
        }
        _ => format!("#{pid}_#{aid}"),
    };

    let proj_display = project
        .map(|p| p.name.clone())
        .unwrap_or_else(|| format!("#{pid}"));
    let act_display = activity
        .map(|a| a.name.clone())
        .unwrap_or_else(|| format!("#{aid}"));
    let display = match customer {
        Some(c) => format!("{c} / {proj_display} / {act_display}"),
        None => format!("{proj_display} / {act_display}"),
    };
    (token, display, proj_display, act_display)
}

/// All bookable `(project, activity)` combinations, plus the current pair
/// (`ensure`) even if the lookups don't know it or the activity is not
/// otherwise valid for the project — so an unchanged buffer always
/// round-trips. Colliding base tokens are disambiguated by appending
/// `-<pid>-<aid>`.
fn entry_combos(
    projects: &HashMap<u64, KimaiProject>,
    activities: &HashMap<u64, KimaiActivity>,
    ensure: Option<(u64, u64)>,
) -> Vec<EntryCombo> {
    let mut pairs: Vec<(u64, u64)> = Vec::new();
    for (&pid, p) in projects {
        for (&aid, a) in activities {
            if activity_valid_for_project(a, pid, &p.name) {
                pairs.push((pid, aid));
            }
        }
    }
    if let Some(pair) = ensure
        && !pairs.contains(&pair)
    {
        pairs.push(pair);
    }

    let raw: Vec<(String, String, String, String, u64, u64)> = pairs
        .into_iter()
        .map(|(pid, aid)| {
            let (token, display, project, activity) =
                combo_parts(pid, aid, projects, activities);
            (token, display, project, activity, pid, aid)
        })
        .collect();

    let mut counts: HashMap<String, usize> = HashMap::new();
    for (token, ..) in &raw {
        *counts.entry(token.clone()).or_default() += 1;
    }

    raw.into_iter()
        .map(|(token, display, project, activity, pid, aid)| {
            let token = if counts[&token] > 1 {
                format!("{token}-{pid}-{aid}")
            } else {
                token
            };
            EntryCombo {
                token,
                display,
                project,
                activity,
                project_id: pid,
                activity_id: aid,
            }
        })
        .collect()
}

/// One bookable entry surfaced to consumers outside the editor: the slug
/// `token`, its readable `label` ("Customer / Project / Activity"), and the
/// project / activity clear names broken out separately (needed e.g. to group
/// a report by "Project - Activity").
pub(super) struct EntrySlug {
    pub token: String,
    pub label: String,
    pub project: String,
    pub activity: String,
}

/// The complete set of bookable project+activity entries, sorted by token for
/// a stable order. This is the same completion set edit mode offers, surfaced
/// for consumers outside the editor (e.g. the adapter's
/// `list_values("entry_combos")` → CLI). Keeps [`EntryCombo`] private.
pub(super) fn entry_slug_options(
    projects: &HashMap<u64, KimaiProject>,
    activities: &HashMap<u64, KimaiActivity>,
) -> Vec<EntrySlug> {
    let mut options: Vec<EntrySlug> = entry_combos(projects, activities, None)
        .into_iter()
        .map(|c| EntrySlug {
            token: c.token,
            label: c.display,
            project: c.project,
            activity: c.activity,
        })
        .collect();
    options.sort_by(|a, b| a.token.cmp(&b.token));
    options
}

/// Resolve an `entry` value back to a `(project_id, activity_id)` pair.
/// Accepts the direct `#<pid>_#<aid>` escape and, otherwise, an exact
/// (case-insensitive) match against the combined tokens. On failure lists
/// the available tokens.
fn resolve_entry(
    value: &str,
    projects: &HashMap<u64, KimaiProject>,
    activities: &HashMap<u64, KimaiActivity>,
    ensure: Option<(u64, u64)>,
) -> Result<(u64, u64), String> {
    let value = value.trim();
    if let Some((p, a)) = value.split_once('_')
        && let (Some(pid), Some(aid)) = (
            p.trim().strip_prefix('#').and_then(|v| v.parse::<u64>().ok()),
            a.trim().strip_prefix('#').and_then(|v| v.parse::<u64>().ok()),
        )
    {
        return Ok((pid, aid));
    }

    let combos = entry_combos(projects, activities, ensure);
    let lowered = value.to_lowercase();
    if let Some(c) = combos.iter().find(|c| c.token == lowered) {
        return Ok((c.project_id, c.activity_id));
    }
    let mut available: Vec<&str> = combos.iter().map(|c| c.token.as_str()).collect();
    available.sort_unstable();
    Err(format!(
        "unknown entry `{value}` (available: {})",
        available.join(", ")
    ))
}

/// Render the edit buffer for one timesheet.
pub(super) fn render_edit_template(
    ts: &KimaiTimesheet,
    projects: &HashMap<u64, KimaiProject>,
    activities: &HashMap<u64, KimaiActivity>,
) -> String {
    let combos = entry_combos(projects, activities, Some((ts.project, ts.activity)));

    // Single coupled `entry` field = customer_project_activity token. The
    // current pair is guaranteed present via `ensure`; the readable
    // `Customer / Project / Activity` rides along as an inline `# comment`
    // (stripped on parse) so the buffer stays legible.
    let entry = combos
        .iter()
        .find(|c| c.project_id == ts.project && c.activity_id == ts.activity)
        .map(|c| format!("{}  # {}", c.token, c.display))
        .unwrap_or_else(|| format!("#{}_#{}", ts.project, ts.activity));
    let running = ts.end.is_none();

    let mut out = String::new();
    out.push_str(&format!("entry: {entry}\n"));
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

    // Advertise the combined tokens (single, whitespace-free) so editor
    // word-completion works.
    let mut entry_tokens: Vec<&str> = combos.iter().map(|c| c.token.as_str()).collect();
    entry_tokens.sort_unstable();
    entry_tokens.dedup();
    if !entry_tokens.is_empty() {
        out.push('\n');
        out.push_str(CACHE_MARKER);
        out.push('\n');
        out.push_str(&format!("# entries: {}\n", entry_tokens.join(", ")));
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
    activities: &HashMap<u64, KimaiActivity>,
) -> Result<Option<EditPlan>, Vec<String>> {
    const ALLOWED: [&str; 3] = ["entry", "begin", "duration"];
    let mut errors: Vec<String> = Vec::new();

    for key in parsed.editable.keys() {
        if !ALLOWED.contains(&key.as_str()) {
            errors.push(format!(
                "unknown editable field `{key}` (allowed: {})",
                ALLOWED.join(", ")
            ));
        }
    }

    let entry = parsed.editable.get("entry").and_then(|v| {
        match resolve_entry(v, projects, activities, Some((current.project, current.activity))) {
            Ok(pair) => Some(pair),
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

    if let Some((project_id, activity_id)) = entry {
        if project_id != current.project {
            patch.insert("project".into(), project_id.into());
            changed.push("project");
        }
        if activity_id != current.activity {
            patch.insert("activity".into(), activity_id.into());
            changed.push("activity");
        }
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

/// Build the POST body for a new timesheet from the create form's fields.
/// Resolves the `entry` token to a `(project, activity)` id pair (same tokens
/// the edit buffer and `entry_combos` use, plus the `#<pid>_#<aid>` escape),
/// parses the local `begin` and the `duration`, and materialises
/// `end = begin + duration` (Kimai derives duration from the begin/end pair,
/// exactly as the edit path does). All field problems are collected like
/// [`build_edit_plan`]; `description` may be empty.
pub(super) fn build_create_body(
    entry: &str,
    begin: &str,
    duration: &str,
    description: &str,
    projects: &HashMap<u64, KimaiProject>,
    activities: &HashMap<u64, KimaiActivity>,
) -> Result<serde_json::Value, Vec<String>> {
    let mut errors: Vec<String> = Vec::new();

    let pair = match resolve_entry(entry, projects, activities, None) {
        Ok(pair) => Some(pair),
        Err(e) => {
            errors.push(e);
            None
        }
    };
    let begin_dt = parse_begin(begin).map_err(|e| errors.push(e)).ok();
    let secs = parse_duration(duration).map_err(|e| errors.push(e)).ok();
    if let Some(s) = secs
        && s <= 0
    {
        errors.push(format!("duration must be positive, got `{duration}`"));
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let (project_id, activity_id) = pair.expect("no error → pair resolved");
    let begin_dt = begin_dt.expect("no error → begin parsed");
    let end_dt = begin_dt + chrono::Duration::seconds(secs.expect("no error → duration parsed"));

    let mut body = serde_json::Map::new();
    body.insert(
        "begin".into(),
        begin_dt.format("%Y-%m-%dT%H:%M:%S").to_string().into(),
    );
    body.insert(
        "end".into(),
        end_dt.format("%Y-%m-%dT%H:%M:%S").to_string().into(),
    );
    body.insert("project".into(), project_id.into());
    body.insert("activity".into(), activity_id.into());
    body.insert("description".into(), description.trim().into());
    Ok(serde_json::Value::Object(body))
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

    fn lookups() -> (HashMap<u64, KimaiProject>, HashMap<u64, KimaiActivity>) {
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
        // Activity 3 is global (bookable on every project); activity 4 is
        // bound to project 7 only.
        let activities = HashMap::from([
            (
                3,
                KimaiActivity {
                    id: 3,
                    name: "Development".to_string(),
                    project: None,
                    parent_title: None,
                },
            ),
            (
                4,
                KimaiActivity {
                    id: 4,
                    name: "Meeting".to_string(),
                    project: Some(7),
                    parent_title: None,
                },
            ),
        ]);
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
        assert!(template.contains(
            "entry: acme-corp_website-relaunch_development  # Acme Corp / Website Relaunch / Development"
        ));
        assert!(template.contains("begin: 2030-01-15T09:00:00"));
        assert!(template.contains("duration: 1:30:00"));
        // Project, activity and customer are folded into the single coupled
        // `entry` token above — no separate fields survive.
        assert!(!template.contains("customer:"));
        assert!(!template.contains("project:"));
        assert!(!template.contains("activity:"));
        assert!(template.contains(CACHE_MARKER));
        // Global activity 3 appears under both projects; bound activity 4
        // only under its owning project 7.
        assert!(template.contains(
            "# entries: acme-corp_website-relaunch_development, acme-corp_website-relaunch_meeting, internal_development"
        ));

        let parsed = parse_edit(&template).unwrap();
        assert_eq!(parsed.body, "Refactor login form\nsecond line");
        let plan = build_edit_plan(&parsed, &ts, &projects, &activities).unwrap();
        assert!(plan.is_none(), "unchanged buffer must produce no patch");
    }

    #[test]
    fn entry_slug_options_lists_every_bookable_combo_sorted() {
        let (projects, activities) = lookups();
        let options = entry_slug_options(&projects, &activities);

        // Global activity 3 pairs with both projects; bound activity 4 only
        // with its owning project 7. Sorted by token.
        let tokens: Vec<&str> = options.iter().map(|e| e.token.as_str()).collect();
        assert_eq!(
            tokens,
            vec![
                "acme-corp_website-relaunch_development",
                "acme-corp_website-relaunch_meeting",
                "internal_development",
            ]
        );

        // The label is the human-readable "Customer / Project / Activity";
        // project and activity clear names are also broken out separately.
        let dev = options
            .iter()
            .find(|e| e.token == "acme-corp_website-relaunch_development")
            .unwrap();
        assert_eq!(dev.label, "Acme Corp / Website Relaunch / Development");
        assert_eq!(dev.project, "Website Relaunch");
        assert_eq!(dev.activity, "Development");
        // A customer-less project drops the leading segment.
        let internal = options
            .iter()
            .find(|e| e.token == "internal_development")
            .unwrap();
        assert_eq!(internal.label, "Internal / Development");
        assert_eq!(internal.project, "Internal");
        assert_eq!(internal.activity, "Development");
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
    fn global_activity_appears_under_every_project_bound_activity_only_its_own() {
        let (projects, activities) = lookups();
        let combos = entry_combos(&projects, &activities, None);
        let tokens: Vec<&str> = combos.iter().map(|c| c.token.as_str()).collect();
        // Global activity 3 under both projects.
        assert!(tokens.contains(&"acme-corp_website-relaunch_development"));
        assert!(tokens.contains(&"internal_development"));
        // Bound activity 4 only under its owning project 7.
        assert!(tokens.contains(&"acme-corp_website-relaunch_meeting"));
        assert!(!tokens.contains(&"internal_meeting"));
        assert_eq!(combos.len(), 3);
    }

    #[test]
    fn resolve_entry_accepts_token_and_escape() {
        let (projects, activities) = lookups();
        assert_eq!(
            resolve_entry("acme-corp_website-relaunch_development", &projects, &activities, None)
                .unwrap(),
            (7, 3)
        );
        // Case-insensitive on the token.
        assert_eq!(
            resolve_entry("ACME-CORP_WEBSITE-RELAUNCH_MEETING", &projects, &activities, None)
                .unwrap(),
            (7, 4)
        );
        // Bare token for the customer-less project.
        assert_eq!(
            resolve_entry("internal_development", &projects, &activities, None).unwrap(),
            (8, 3)
        );
        // Direct `#<pid>_#<aid>` escape, resolvable even without lookups.
        assert_eq!(
            resolve_entry("#42_#99", &projects, &activities, None).unwrap(),
            (42, 99)
        );
        let err = resolve_entry("nope", &projects, &activities, None).unwrap_err();
        assert!(err.contains("unknown entry `nope`"));
        assert!(err.contains("acme-corp_website-relaunch_development"));
    }

    #[test]
    fn colliding_entry_tokens_get_disambiguated() {
        // Same project name under the same customer → identical base token
        // for a shared global activity; the id pair disambiguates.
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
                    parent_title: Some("Acme Corp".to_string()),
                },
            ),
        ]);
        let activities = HashMap::from([(
            5,
            KimaiActivity {
                id: 5,
                name: "Dev".to_string(),
                project: None,
                parent_title: None,
            },
        )]);
        assert_eq!(
            resolve_entry("acme-corp_support_dev-1-5", &projects, &activities, None).unwrap(),
            (1, 5)
        );
        assert_eq!(
            resolve_entry("acme-corp_support_dev-2-5", &projects, &activities, None).unwrap(),
            (2, 5)
        );
    }

    #[test]
    fn entry_edit_patches_changed_ids_only() {
        let (projects, activities) = lookups();
        let ts = sample_timesheet();
        let template = render_edit_template(&ts, &projects, &activities);
        // Switch project only (activity 3 is global, stays valid).
        let edited = template.replace(
            "entry: acme-corp_website-relaunch_development",
            "entry: internal_development",
        );
        let parsed = parse_edit(&edited).unwrap();
        let plan = build_edit_plan(&parsed, &ts, &projects, &activities)
            .unwrap()
            .expect("changed");
        assert_eq!(plan.patch["project"], 8);
        assert!(plan.patch.get("activity").is_none());
        assert_eq!(plan.changed, vec!["project"]);

        // Switch activity only (still project 7).
        let edited = template.replace(
            "entry: acme-corp_website-relaunch_development",
            "entry: acme-corp_website-relaunch_meeting",
        );
        let parsed = parse_edit(&edited).unwrap();
        let plan = build_edit_plan(&parsed, &ts, &projects, &activities)
            .unwrap()
            .expect("changed");
        assert_eq!(plan.patch["activity"], 4);
        assert!(plan.patch.get("project").is_none());
        assert_eq!(plan.changed, vec!["activity"]);
    }

    #[test]
    fn hash_id_fallback_round_trips() {
        // Both entities missing from the lookups → entry renders as the
        // `#<pid>_#<aid>` escape; the unchanged buffer must resolve back to
        // the current pair and produce no patch.
        let projects = HashMap::new();
        let activities = HashMap::new();
        let ts = sample_timesheet();
        let template = render_edit_template(&ts, &projects, &activities);
        assert!(template.contains("entry: #7_#3"));
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
    fn build_create_body_resolves_entry_and_derives_end() {
        let (projects, activities) = lookups();
        let body = build_create_body(
            "acme-corp_website-relaunch_development",
            "2030-01-15T09:00",
            "1:30:00",
            "  Kickoff  ",
            &projects,
            &activities,
        )
        .unwrap();
        assert_eq!(body["project"], 7);
        assert_eq!(body["activity"], 3);
        assert_eq!(body["begin"], "2030-01-15T09:00:00");
        assert_eq!(body["end"], "2030-01-15T10:30:00");
        assert_eq!(body["description"], "Kickoff");
    }

    #[test]
    fn build_create_body_accepts_hash_escape_and_seconds_and_empty_description() {
        let body = build_create_body(
            "#42_#99",
            "2030-01-15 08:00:00",
            "5400",
            "",
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(body["project"], 42);
        assert_eq!(body["activity"], 99);
        assert_eq!(body["begin"], "2030-01-15T08:00:00");
        assert_eq!(body["end"], "2030-01-15T09:30:00");
        assert_eq!(body["description"], "");
    }

    #[test]
    fn build_create_body_collects_field_errors() {
        let (projects, activities) = lookups();
        let errors = build_create_body(
            "nope", "not-a-date", "0", "x", &projects, &activities,
        )
        .unwrap_err();
        // Unknown entry, unparseable begin, and non-positive duration all reported.
        assert!(errors.iter().any(|e| e.contains("unknown entry `nope`")));
        assert!(errors.iter().any(|e| e.contains("invalid begin")));
        assert!(errors.iter().any(|e| e.contains("duration must be positive")));
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
