//! 3b template rendering, parsing, validation, and diff against current
//! upstream state. The format is "editable section / `---` / read-only
//! section / `===` / body" with optional trailing CACHE section.

use std::collections::{HashMap, HashSet};

use not_yet_done_content::*;

use crate::client::JiraIssueDetail;

use super::JiraIssueNode;
use super::markers::{
    BODY_MARKER, CACHE_MARKER, CONFLICT_BANNER_END, CONFLICT_BANNER_START, CONFLICT_MARK_MIDDLE,
    EDITABLE_MARKER, ERROR_BANNER_END, ERROR_BANNER_START, FOREIGN_BANNER_END,
    FOREIGN_BANNER_START,
};
use super::slugs::{SlugTables, build_slug_tables, editable_value_with_slugs, resolved_to_slug};
use super::wiki_md::normalize_ws;

/// Field-level error produced by [`JiraIssueNode::parse_3b`] /
/// [`JiraIssueNode::validate_3b`]. Rendered as a `# • <message>` bullet
/// inside the error banner.
#[derive(Debug, Clone)]
pub(super) struct FieldError {
    pub(super) message: String,
}

/// Result of [`JiraIssueNode::parse_3b`]: extracted editable values + body.
#[derive(Debug, Default)]
pub(super) struct Parsed3b {
    /// Lowercased key → value (already inline-comment-stripped, trimmed).
    pub(super) editable: HashMap<String, String>,
    /// Body text below the `===` marker (leading/trailing blank lines
    /// trimmed but inner formatting preserved).
    pub(super) body: String,
}

/// Diff between a parsed buffer and the current upstream state. Replaces
/// the trait-level `EditorOutput` removed in the action-unification refactor.
#[derive(Debug, Default)]
pub(super) struct ChangeSet {
    pub(super) metadata_changes: Vec<(String, String)>,
    pub(super) content: Option<Vec<u8>>,
    /// Resolved target status *name* when the editable `status` field changed.
    /// Kept apart from `metadata_changes` because a status change is not a
    /// plain field PUT — it routes to a workflow transition
    /// ([`JiraIssueNode::apply_status_transition`]).
    pub(super) status_change: Option<String>,
}

/// Editable fields for the `edit_full` action on a Jira issue. Hard-coded
/// rather than YAML-driven — the adapter owns the action's shape. `status`
/// looks like an ordinary field on the editor surface but is resolved to a
/// workflow transition on save (see [`ChangeSet::status_change`]).
pub(super) fn edit_full_fields() -> Vec<String> {
    vec![
        "summary".into(),
        "status".into(),
        "labels".into(),
        "assignee".into(),
    ]
}

/// Return the metadata value for `key` directly from `JiraIssueDetail`,
/// for keys we know about. Empty string for unknown keys.
pub(super) fn field_value_from_detail(d: &JiraIssueDetail, key: &str) -> String {
    match key {
        "summary" => d.summary.clone(),
        "assignee" => d.assignee.clone(),
        "creator" => d.creator.clone(),
        "fix_versions" => d.fix_versions.clone(),
        "reporter" => d.reporter.clone(),
        "priority" => d.priority.clone(),
        "status" => d.status.clone(),
        "type" => d.issue_type.clone(),
        "key" | "number" => d.key.clone(),
        _ => String::new(),
    }
}

/// Strip a previously-rendered error or conflict banner from the top of a
/// buffer. Idempotent on banner-less input.
pub(super) fn strip_banner(text: &str) -> &str {
    for (start, end) in [
        (ERROR_BANNER_START, ERROR_BANNER_END),
        (CONFLICT_BANNER_START, CONFLICT_BANNER_END),
        (FOREIGN_BANNER_START, FOREIGN_BANNER_END),
    ] {
        if let Some(rest) = text.strip_prefix(start) {
            // Skip the start marker line; find the end marker line.
            let after_start = rest.strip_prefix('\n').unwrap_or(rest);
            // Find the end marker on its own line.
            let needle = format!("\n{end}");
            if let Some(pos) = after_start.find(&needle) {
                let after_end = &after_start[pos + needle.len()..];
                return after_end.strip_prefix('\n').unwrap_or(after_end);
            }
            // Banner started but no end — strip just the start line and bail.
            return after_start;
        }
    }
    text
}

/// Strip the trailing CACHE section before parsing. The marker line and
/// everything after it are dropped. Idempotent on input without the marker.
pub(super) fn strip_cache_section(text: &str) -> &str {
    if let Some(pos) = text.find(CACHE_MARKER) {
        // Trim trailing whitespace / blank lines before the marker so the
        // body parse doesn't pick up a stray newline.
        text[..pos].trim_end_matches(|c: char| c.is_whitespace())
    } else {
        text
    }
}

/// Strip `# `-prefixed comment lines from a template buffer (e.g. the
/// header of the `create_comment` template).
pub(super) fn strip_template_comments(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Render the trailing CACHE section. Empty string when both tables are
/// empty (no available slugs to advertise).
pub(super) fn render_cache_section(tables: &SlugTables) -> String {
    if tables.labels.is_empty() && tables.users.is_empty() && tables.statuses.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push('\n');
    out.push_str(CACHE_MARKER);
    out.push('\n');
    if !tables.labels.is_empty() {
        out.push_str("# labels: ");
        out.push_str(&tables.labels.slugs().join(", "));
        out.push('\n');
    }
    if !tables.users.is_empty() {
        out.push_str("# users: ");
        out.push_str(&tables.users.slugs().join(", "));
        out.push('\n');
    }
    if !tables.statuses.is_empty() {
        out.push_str("# statuses: ");
        out.push_str(&tables.statuses.slugs().join(", "));
        out.push('\n');
    }
    out
}

/// Append the read-only metadata block that sits below the `---` marker:
/// every standard field that the caller did *not* place in the editable
/// section above it. Single source for both render paths, so the two can't
/// drift apart on which fields a ticket buffer shows.
fn push_readonly_section(out: &mut String, detail: &JiraIssueDetail, editable_set: &HashSet<&str>) {
    for (label, value) in [
        ("number", detail.key.as_str()),
        ("type", detail.issue_type.as_str()),
        ("status", detail.status.as_str()),
        ("priority", detail.priority.as_str()),
        ("assignee", detail.assignee.as_str()),
        ("creator", detail.creator.as_str()),
        ("fix_versions", detail.fix_versions.as_str()),
    ] {
        // `number` doesn't exist as an editable key so always show it;
        // others suppress when the caller put them above the marker.
        let editable_key = if label == "number" { "key" } else { label };
        if editable_set.contains(editable_key) || editable_set.contains(label) {
            continue;
        }
        out.push_str(&format!("{label}: {value}\n"));
    }
}

/// Build a 3b buffer from a `Parsed3b` + the read-only fields of `detail`.
/// Used when re-emitting after a Reopen so user header edits survive.
/// `parsed.editable` carries already-resolved values (`labels` = original
/// names, `assignee` = Jira-username); `tables` translates them back into
/// `ll-…` / `uu-…` slugs for the rendered buffer.
pub(super) fn render_3b_from_parsed(
    parsed: &Parsed3b,
    editable_fields: &[String],
    detail: &JiraIssueDetail,
    tables: &SlugTables,
) -> String {
    let mut out = String::new();
    for key in editable_fields {
        let value = match parsed.editable.get(key) {
            Some(v) => resolved_to_slug(key, v, tables),
            None => editable_value_with_slugs(detail, key, tables),
        };
        out.push_str(&format!("{key}: {value}\n"));
    }
    out.push_str(EDITABLE_MARKER);
    out.push('\n');
    let editable_set: HashSet<&str> = editable_fields.iter().map(String::as_str).collect();
    push_readonly_section(&mut out, detail, &editable_set);
    out.push_str(BODY_MARKER);
    out.push_str("\n\n");
    out.push_str(&parsed.body);
    out
}

/// Translate a `metadata_changes` slice (parsed-editable form) into a
/// Jira `fields` JSON map. `labels` is comma-separated; `assignee` carries
/// the resolved Jira-username (empty = unassign).
pub(super) fn metadata_changes_to_fields(
    changes: &[(String, String)],
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let mut fields = serde_json::Map::new();
    for (key, value) in changes {
        match key.as_str() {
            "summary" => {
                fields.insert("summary".into(), serde_json::Value::String(value.clone()));
            }
            "labels" => {
                let arr: Vec<serde_json::Value> = value
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| serde_json::Value::String(s.to_string()))
                    .collect();
                fields.insert("labels".into(), serde_json::Value::Array(arr));
            }
            "assignee" => {
                let v = if value.is_empty() {
                    serde_json::json!({ "name": null })
                } else {
                    serde_json::json!({ "name": value })
                };
                fields.insert("assignee".into(), v);
            }
            other => {
                return Err(ContentError::NotSupported(format!(
                    "Updating field '{other}' is not supported"
                )));
            }
        }
    }
    Ok(fields)
}

impl JiraIssueNode {
    /// Render a 3b-format buffer.
    ///
    /// `detail` is the source for read-only field values (and the body, when
    /// no override is provided). `editable_overrides` lets callers seed the
    /// editable section from an alternate source (e.g. user edits during a
    /// conflict re-render). `body_override` does the same for the body.
    pub(super) fn render_3b(
        &self,
        editable_fields: &[String],
        detail: &JiraIssueDetail,
        editable_overrides: Option<&HashMap<String, String>>,
        body_override: Option<&str>,
        tables: &SlugTables,
    ) -> String {
        self.render_3b_full(
            editable_fields,
            detail,
            editable_overrides,
            body_override,
            true,
            tables,
        )
    }

    /// Variant that lets the caller suppress the trailing CACHE section,
    /// so `render_with_comments` can place it after the comment list.
    ///
    /// `tables` is threaded in (rather than built here) because the status
    /// slug table needs the async live-transitions + workflow-edge lookup,
    /// which a sync render method can't perform. Sync callers pass
    /// [`build_slug_tables`] (empty status table → status renders as its plain
    /// name and round-trips unchanged); async callers pass
    /// [`JiraIssueNode::slug_tables`].
    pub(super) fn render_3b_full(
        &self,
        editable_fields: &[String],
        detail: &JiraIssueDetail,
        editable_overrides: Option<&HashMap<String, String>>,
        body_override: Option<&str>,
        append_cache: bool,
        tables: &SlugTables,
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
        push_readonly_section(&mut out, detail, &editable_set);

        out.push_str(BODY_MARKER);
        out.push_str("\n\n");

        let body = body_override.unwrap_or(detail.description.as_str());
        out.push_str(body);

        if append_cache {
            out.push_str(&render_cache_section(&tables));
        }

        out
    }

    /// Strict 3b parser. Returns either the structured content or a list of
    /// structural errors (missing markers, malformed lines). Field-level
    /// validation (required fields, allowed keys) lives in `validate_3b`.
    pub(super) fn parse_3b(&self, text: &str) -> std::result::Result<Parsed3b, Vec<FieldError>> {
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
        let mut errors: Vec<FieldError> = Vec::new();
        let mut saw_editable_marker = false;
        let mut saw_body_marker = false;

        for (idx, line) in text.lines().enumerate() {
            let lineno = idx + 1;
            let trimmed = line.trim_end();

            // Reject unresolved git-style conflict markers in any section.
            if trimmed == CONFLICT_MARK_MIDDLE
                || trimmed.starts_with("<<<<<<<")
                || trimmed.starts_with(">>>>>>>")
            {
                errors.push(FieldError {
                    message: format!(
                        "line {lineno}: unresolved conflict marker — keep one side and remove the markers"
                    ),
                });
                continue;
            }

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
                        // Strip inline `# comment` from the value.
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
                    // Ignore everything else — section is read-only.
                }
                Section::Body => {
                    body_lines.push(line);
                }
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

        // Trim leading / trailing blank lines from the body.
        while body_lines.first().is_some_and(|l| l.trim().is_empty()) {
            body_lines.remove(0);
        }
        while body_lines.last().is_some_and(|l| l.trim().is_empty()) {
            body_lines.pop();
        }
        let body = body_lines.join("\n");

        Ok(Parsed3b { editable, body })
    }

    /// Field-level checks on a parsed buffer. Reports unknown editable keys
    /// and missing-required values. Run after `parse_3b` succeeds.
    pub(super) fn validate_3b(
        &self,
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

        if editable_fields.iter().any(|f| f == "summary") {
            match parsed.editable.get("summary") {
                Some(v) if v.trim().is_empty() => errors.push(FieldError {
                    message: "summary must not be empty".into(),
                }),
                None => errors.push(FieldError {
                    message: "summary is required".into(),
                }),
                _ => {}
            }
        }

        errors
    }

    /// Diff parsed editable values + body against the supplied detail
    /// snapshot (typically `self.detail().await?` or a freshly-fetched one
    /// during conflict merge). Returns only the changes.
    pub(super) fn diff_against_current(
        &self,
        parsed: &Parsed3b,
        detail: &JiraIssueDetail,
    ) -> ChangeSet {
        let mut metadata_changes = Vec::new();
        let mut status_change = None;
        for (key, new_value) in &parsed.editable {
            // Status is not a plain field write: a change routes to a workflow
            // transition, so it never joins `metadata_changes` (which feeds
            // `metadata_changes_to_fields` — that rejects `status`).
            if key == "status" {
                if field_value_from_detail(detail, "status") != *new_value {
                    status_change = Some(new_value.clone());
                }
                continue;
            }
            let unchanged = match key.as_str() {
                "labels" => {
                    let mut current: Vec<&str> = detail.labels.iter().map(String::as_str).collect();
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
                "assignee" => detail.assignee_key == *new_value,
                _ => field_value_from_detail(detail, key) == *new_value,
            };
            if !unchanged {
                metadata_changes.push((key.clone(), new_value.clone()));
            }
        }

        // Whitespace-tolerant body compare: Jira sometimes round-trips a
        // description with different blank-line spacing (e.g. UI saves
        // re-format wiki markup with extra blank lines, REST returns it
        // single-spaced). On top of that, the `edit_markdown` flow feeds a
        // body that has been through `wiki→md→wiki`, which the round-trip
        // guard only guarantees stable *modulo `normalize_ws`* (per-line
        // trim + blank-line drop) — a weaker `normalize_blank_lines` compare
        // would flag such an untouched body as changed, churn a needless PUT,
        // and (since a just-added comment already bumped the version) turn it
        // into a spurious upstream conflict. Compare with the same
        // normalization the guard uses so an untouched body is never a change.
        let body = parsed.body.trim();
        let content = if normalize_ws(body) != normalize_ws(detail.description.trim()) {
            Some(body.as_bytes().to_vec())
        } else {
            None
        };

        ChangeSet {
            metadata_changes,
            content,
            status_change,
        }
    }

    /// If the user emptied a required editable field (caught by
    /// `validate_3b`), restore that field from the buffer they opened
    /// with. Keeps every other edit (other fields, body) intact. Returns
    /// the user buffer unchanged when nothing needs restoring.
    pub(super) fn restore_blanked_editable(
        &self,
        user_text: &str,
        original_text: Option<&str>,
        editable_fields: &[String],
        detail: &JiraIssueDetail,
    ) -> String {
        let Some(original) = original_text else {
            return user_text.to_string();
        };
        let Ok(mut user_parsed) = self.parse_3b(user_text) else {
            return user_text.to_string();
        };
        let Ok(original_parsed) = self.parse_3b(original) else {
            return user_text.to_string();
        };

        let mut changed = false;
        for key in editable_fields {
            // Only restore *required* fields — for optional ones (labels,
            // assignee) an empty value is a legitimate clear and must be kept.
            if key != "summary" {
                continue;
            }
            let needs_restore = user_parsed
                .editable
                .get(key)
                .map(|v| v.trim().is_empty())
                .unwrap_or(true);
            if !needs_restore {
                continue;
            }
            let Some(orig_val) = original_parsed.editable.get(key) else {
                continue;
            };
            if orig_val.trim().is_empty() {
                continue;
            }
            user_parsed.editable.insert(key.clone(), orig_val.clone());
            changed = true;
        }

        if !changed {
            return user_text.to_string();
        }
        let tables = build_slug_tables(&self.cache);
        render_3b_from_parsed(&user_parsed, editable_fields, detail, &tables)
    }

    /// Prepend an `# ─── ERRORS ───` banner above the editable section.
    /// Any pre-existing banner is stripped first to avoid stacking on
    /// repeated reopens.
    pub(super) fn render_with_errors(&self, original_text: &str, errors: &[FieldError]) -> String {
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
}
