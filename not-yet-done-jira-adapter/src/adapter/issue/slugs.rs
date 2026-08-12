//! `ll_…` (label) and `uu_…` (user) slug rendering and parsing.
//!
//! Slugs replace the raw Jira values inside the editable section and inside
//! comment bodies, so the editor buffer stays readable (no `[~jdoe]` or raw
//! label hashes). The slug tables come from the live cache plus the issue's
//! own current values.

use std::sync::Mutex;

use not_yet_done_content::slug::{SlugTable, normalize};

use crate::client::{JiraClient, JiraIssueDetail, JiraUser};

use super::super::cache::{JiraCache, persist_labels};
use super::template::{FieldError, Parsed3b, field_value_from_detail};

pub const LABEL_PREFIX: &str = "ll_";
pub const USER_PREFIX: &str = "uu_";
/// Status slug prefix (`ss_…`). Unlike labels/users, a status is not a plain
/// field write — the resolved target routes to a workflow *transition* — but
/// on the editor surface it behaves exactly like the other slug fields:
/// rendered as a slug, listed in the CACHE legend, resolved on save.
pub const STATUS_PREFIX: &str = "ss_";

fn build_label_table(labels: &[String]) -> SlugTable {
    SlugTable::build(labels.iter().map(|l| (l.clone(), l.clone())), LABEL_PREFIX)
}

/// User table — slug normalized from `display_name`, original is `name`
/// (the Jira-username, same value as inside `[~name]` mentions).
fn build_user_table(users: &[JiraUser]) -> SlugTable {
    SlugTable::build(
        users
            .iter()
            .filter(|u| !u.name.is_empty())
            .map(|u| (u.display_name.clone(), u.name.clone())),
        USER_PREFIX,
    )
}

/// Status table — slug source *and* original are the status name. Unlike
/// labels/users there is no separate wire id: a status change routes to a
/// workflow *transition* resolved by name (see `apply_status_transition`).
/// `current` is always included so the issue's own status is representable
/// even before any transition was observed; `reachable` are the terminal
/// status names of every known transition path (direct or indirect).
pub(super) fn build_status_table<'a>(
    reachable: impl IntoIterator<Item = &'a str>,
    current: &str,
) -> SlugTable {
    let mut names: Vec<String> = Vec::new();
    let mut push = |n: &str| {
        if !n.is_empty() && !names.iter().any(|x| x == n) {
            names.push(n.to_string());
        }
    };
    push(current);
    for n in reachable {
        push(n);
    }
    SlugTable::build(names.iter().map(|n| (n.clone(), n.clone())), STATUS_PREFIX)
}

/// In-buffer slug tables. Built per render/parse from the live cache plus
/// the issue's own current values (so the existing assignee / labels are
/// always representable as a slug, even if the cache is cold). The
/// `statuses` table is seeded empty by [`build_slug_tables`] and filled in
/// by the async [`JiraIssueNode::slug_tables`], which needs the live
/// transitions + workflow-edge cache to know the reachable statuses.
pub(super) struct SlugTables {
    pub(super) labels: SlugTable,
    pub(super) users: SlugTable,
    pub(super) statuses: SlugTable,
}

pub(super) fn build_slug_tables(cache: &Mutex<JiraCache>) -> SlugTables {
    let (all_labels, all_users) = {
        let c = cache.lock().unwrap();
        (c.labels_snapshot(), c.users_snapshot())
    };
    // Every fetched issue mirrors its assignee / reporter / creator into
    // the cache via `fetch_issue`, and every comment its author via
    // `fetch_comments`, so the cache snapshot is authoritative for
    // whatever this template needs.
    SlugTables {
        labels: build_label_table(&all_labels),
        users: build_user_table(&all_users),
        statuses: build_status_table(std::iter::empty(), ""),
    }
}

/// Slug-aware companion to `field_value_from_detail` — used in the editable
/// section so labels/assignee render as `ll-…` / `uu-…` slugs rather than
/// raw Jira values. Falls back to the raw value when no slug exists.
pub(super) fn editable_value_with_slugs(
    detail: &JiraIssueDetail,
    key: &str,
    tables: &SlugTables,
) -> String {
    match key {
        "labels" => detail
            .labels
            .iter()
            .filter_map(|l| tables.labels.slug_for(l).map(String::from))
            .collect::<Vec<_>>()
            .join(", "),
        "assignee" => {
            if detail.assignee_key.is_empty() {
                String::new()
            } else {
                tables
                    .users
                    .slug_for(&detail.assignee_key)
                    .map(String::from)
                    .unwrap_or_else(|| detail.assignee_key.clone())
            }
        }
        "status" => tables
            .statuses
            .slug_for(&detail.status)
            .map(String::from)
            .unwrap_or_else(|| detail.status.clone()),
        _ => field_value_from_detail(detail, key),
    }
}

/// Reverse of `resolve_slugs_inplace` — for re-rendering a previously
/// resolved value back into its slug form. Falls back to the resolved
/// value when there's no matching slug (shouldn't normally happen since
/// the table is built off the same cache).
pub(super) fn resolved_to_slug(key: &str, resolved: &str, tables: &SlugTables) -> String {
    match key {
        "labels" => resolved
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|orig| {
                tables
                    .labels
                    .slug_for(orig)
                    .map(String::from)
                    .unwrap_or_else(|| orig.to_string())
            })
            .collect::<Vec<_>>()
            .join(", "),
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
        "status" => {
            if resolved.is_empty() {
                String::new()
            } else {
                tables
                    .statuses
                    .slug_for(resolved)
                    .map(String::from)
                    .unwrap_or_else(|| resolved.to_string())
            }
        }
        _ => resolved.to_string(),
    }
}

/// Resolve one edited label token to the real Jira label name. The `ll_`
/// prefix is *optional* and matching is case-insensitive: the token is
/// normalized (same normalization the slug table uses) and looked up in the
/// label table. A hit yields the canonical label; a miss falls back to the
/// token verbatim (minus any `ll_` prefix), because Jira accepts free-form
/// labels — and [`canonicalize_labels_via_jira`] has already had a chance to
/// upgrade an unknown token to a real label's canonical casing.
fn resolve_label_item(item: &str, tables: &SlugTables) -> String {
    let body = item.strip_prefix(LABEL_PREFIX).unwrap_or(item);
    let norm = normalize(body);
    if norm.is_empty() {
        return body.to_string();
    }
    tables
        .labels
        .original_for(&format!("{LABEL_PREFIX}{norm}"))
        .map(String::from)
        .unwrap_or_else(|| body.to_string())
}

/// Resolve one edited status token to a canonical status *name*. Like labels,
/// the `ss_` prefix is optional and matching is case-insensitive: the token is
/// normalized and looked up in the status table. A hit yields the canonical
/// status name; a miss falls back to the token verbatim (minus any `ss_`
/// prefix). Status is never a hard error here — the save path
/// ([`JiraIssueNode::apply_status_transition`]) validates that a workflow path
/// to the resolved status actually exists.
fn resolve_status_item(item: &str, tables: &SlugTables) -> String {
    let body = item.strip_prefix(STATUS_PREFIX).unwrap_or(item);
    let norm = normalize(body);
    if norm.is_empty() {
        return body.to_string();
    }
    tables
        .statuses
        .original_for(&format!("{STATUS_PREFIX}{norm}"))
        .map(String::from)
        .unwrap_or_else(|| body.to_string())
}

/// Ask Jira's label-suggest endpoint for the canonical casing of any label the
/// local cache doesn't already know, rewriting `parsed.editable["labels"]` in
/// place. Run this *before* [`resolve_slugs_inplace`] at the real save sites:
/// it is what turns an edited `ll_xy7` (or bare `xy7`) into the existing `XY7`
/// label instead of silently creating a lowercase duplicate.
///
/// Labels already in the cache are left as typed (no network call —
/// `resolve_slugs_inplace` maps them). A token Jira has no suggestion for is
/// also kept as typed, so the save still goes through and Jira creates it.
pub(super) async fn canonicalize_labels_via_jira(
    client: &JiraClient,
    cache: &Mutex<JiraCache>,
    tables: &SlugTables,
    parsed: &mut Parsed3b,
) {
    let Some(raw) = parsed.editable.get("labels").cloned() else {
        return;
    };
    let mut out: Vec<String> = Vec::new();
    for item in raw.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let body = item.strip_prefix(LABEL_PREFIX).unwrap_or(item);
        let norm = normalize(body);
        // Empty after normalization, or already known locally → leave as typed.
        if norm.is_empty()
            || tables
                .labels
                .original_for(&format!("{LABEL_PREFIX}{norm}"))
                .is_some()
        {
            out.push(item.to_string());
            continue;
        }
        // Unknown locally: query Jira. Prefer an exact-case hit, else the first
        // case-insensitive match; if nothing matches, keep the typed body.
        match client.suggest_labels(body).await {
            Ok(suggestions) => {
                let canon = suggestions
                    .iter()
                    .find(|s| s.as_str() == body)
                    .or_else(|| suggestions.iter().find(|s| normalize(s) == norm));
                match canon {
                    Some(canon) => {
                        persist_labels(cache, vec![canon.clone()]).await;
                        out.push(canon.clone());
                    }
                    None => out.push(body.to_string()),
                }
            }
            Err(_) => out.push(body.to_string()),
        }
    }
    parsed.editable.insert("labels".into(), out.join(","));
}

/// Translate `parsed.editable["labels"]` tokens into canonical Jira label
/// names (comma-separated) via [`resolve_label_item`] — `ll_` optional,
/// case-insensitive, never an error. Translate `parsed.editable["assignee"]`
/// from a `uu-…` slug into the original Jira-username; an unknown user slug
/// *is* a `FieldError` (unlike labels, an assignee can't be created free-form).
pub(super) fn resolve_slugs_inplace(
    parsed: &mut Parsed3b,
    tables: &SlugTables,
    errors: &mut Vec<FieldError>,
) {
    if let Some(raw) = parsed.editable.get("labels").cloned() {
        let mut originals: Vec<String> = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|item| resolve_label_item(item, tables))
            .collect();
        // Stable order so unchanged labels round-trip identically.
        originals.sort();
        originals.dedup();
        parsed.editable.insert("labels".into(), originals.join(","));
    }

    // Status: prefix-optional, case-insensitive, resolvable → canonical
    // status name, else verbatim. An emptied status field means "no change"
    // (there is no such thing as an empty status), so drop the key entirely.
    // Never a `FieldError` — the transition lookup on save is the validator.
    if let Some(raw) = parsed.editable.get("status").cloned() {
        let raw = raw.trim();
        if raw.is_empty() {
            parsed.editable.remove("status");
        } else {
            parsed
                .editable
                .insert("status".into(), resolve_status_item(raw, tables));
        }
    }

    if let Some(raw) = parsed.editable.get("assignee").cloned() {
        let raw = raw.trim();
        if raw.is_empty() {
            parsed.editable.insert("assignee".into(), String::new());
        } else if !raw.starts_with(USER_PREFIX) {
            errors.push(FieldError {
                message: format!(
                    "assignee `{raw}` must be a `{}` slug from the CACHE section (or empty)",
                    USER_PREFIX,
                ),
            });
        } else {
            match tables.users.original_for(raw) {
                Some(orig) => {
                    parsed.editable.insert("assignee".into(), orig.to_string());
                }
                None => errors.push(FieldError {
                    message: format!("unknown user slug `{raw}`"),
                }),
            }
        }
    }
}

/// Replace `[~KEY]` mentions with `@uu_slug` for the editor display.
/// Unknown KEYs (no slug in the user table) are kept verbatim — they
/// round-trip back unchanged by `parse_user_mentions`.
pub(super) fn render_user_mentions(text: &str, users: &SlugTable) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest.find("[~") {
        out.push_str(&rest[..idx]);
        let after = &rest[idx + 2..];
        if let Some(end) = after.find(']') {
            let key = &after[..end];
            if let Some(slug) = users.slug_for(key) {
                out.push('@');
                out.push_str(slug);
            } else {
                // No slug — keep the raw mention.
                out.push_str(&rest[idx..idx + 2 + end + 1]);
            }
            rest = &after[end + 1..];
        } else {
            out.push_str(&rest[idx..]);
            return out;
        }
    }
    out.push_str(rest);
    out
}

/// Reverse of `render_user_mentions`: rewrite `@uu_slug` back to `[~KEY]`.
/// Only matches at word boundaries so `email@uu_foo.com` is preserved.
/// Returns the offending slug if any `@uu_…` doesn't resolve.
pub(super) fn parse_user_mentions(
    text: &str,
    users: &SlugTable,
) -> std::result::Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut last = 0usize;
    let mut i = 0usize;
    while i + 4 <= bytes.len() {
        if bytes[i] == b'@' && &bytes[i + 1..i + 4] == b"uu_" {
            let prev_ok = i == 0 || !is_ascii_word_byte(bytes[i - 1]);
            if prev_ok {
                let mut end = i + 4;
                while end < bytes.len() && is_slug_byte(bytes[end]) {
                    end += 1;
                }
                if end > i + 4 {
                    let slug = &text[i + 1..end];
                    match users.original_for(slug) {
                        Some(key) => {
                            out.push_str(&text[last..i]);
                            out.push_str("[~");
                            out.push_str(key);
                            out.push(']');
                            last = end;
                            i = end;
                            continue;
                        }
                        None => return Err(slug.to_string()),
                    }
                }
            }
        }
        i += 1;
    }
    out.push_str(&text[last..]);
    Ok(out)
}

fn is_ascii_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables_with(labels: &[&str]) -> SlugTables {
        let owned: Vec<String> = labels.iter().map(|s| s.to_string()).collect();
        SlugTables {
            labels: build_label_table(&owned),
            users: build_user_table(&[]),
            statuses: build_status_table(std::iter::empty(), ""),
        }
    }

    fn tables_with_statuses(reachable: &[&str], current: &str) -> SlugTables {
        SlugTables {
            labels: build_label_table(&[]),
            users: build_user_table(&[]),
            statuses: build_status_table(reachable.iter().copied(), current),
        }
    }

    /// Run the status field through `resolve_slugs_inplace` and return the
    /// rewritten value (or `None` if the key was dropped), asserting status
    /// never produces a `FieldError`.
    fn resolve_status(input: &str, tables: &SlugTables) -> Option<String> {
        let mut parsed = Parsed3b::default();
        parsed.editable.insert("status".into(), input.to_string());
        let mut errors = Vec::new();
        resolve_slugs_inplace(&mut parsed, tables, &mut errors);
        assert!(errors.is_empty(), "status must never error, got {errors:?}");
        parsed.editable.get("status").cloned()
    }

    #[test]
    fn build_status_table_includes_current_and_reachable() {
        let t = build_status_table(["In Progress", "Done"], "Open");
        assert_eq!(t.slug_for("Open"), Some("ss_open"));
        assert_eq!(t.slug_for("In Progress"), Some("ss_in_progress"));
        assert_eq!(t.slug_for("Done"), Some("ss_done"));
        assert_eq!(t.original_for("ss_in_progress"), Some("In Progress"));
    }

    #[test]
    fn build_status_table_dedupes_current_against_reachable() {
        // A reachable status equal to the current one must not double-insert.
        let t = build_status_table(["Open", "Done"], "Open");
        assert_eq!(t.slugs(), vec!["ss_done", "ss_open"]);
    }

    #[test]
    fn status_prefix_optional_and_case_insensitive() {
        let tables = tables_with_statuses(&["In Progress"], "Open");
        for typed in [
            "ss_in_progress",
            "in progress",
            "In Progress",
            "ss_In_Progress",
        ] {
            assert_eq!(
                resolve_status(typed, &tables).as_deref(),
                Some("In Progress"),
                "input {typed:?}",
            );
        }
    }

    #[test]
    fn unknown_status_kept_verbatim_without_error() {
        // Not a known reachable status → passes through (minus any `ss_`
        // prefix), no error. The save-time transition lookup rejects it.
        let tables = tables_with_statuses(&["In Progress"], "Open");
        assert_eq!(
            resolve_status("ss_nirvana", &tables).as_deref(),
            Some("nirvana")
        );
    }

    #[test]
    fn emptied_status_drops_the_key() {
        let tables = tables_with_statuses(&["In Progress"], "Open");
        assert_eq!(resolve_status("   ", &tables), None);
    }

    /// Run the labels field through `resolve_slugs_inplace` and return the
    /// rewritten value, asserting labels never produce a `FieldError`.
    fn resolve_labels(input: &str, tables: &SlugTables) -> String {
        let mut parsed = Parsed3b::default();
        parsed.editable.insert("labels".into(), input.to_string());
        let mut errors = Vec::new();
        resolve_slugs_inplace(&mut parsed, tables, &mut errors);
        assert!(errors.is_empty(), "labels must never error, got {errors:?}");
        parsed.editable.get("labels").cloned().unwrap_or_default()
    }

    #[test]
    fn label_prefix_optional_and_case_insensitive() {
        let tables = tables_with(&["XY7"]);
        // A cached label `XY7` (slug `ll_xy7`) is found regardless of the
        // `ll_` prefix or the casing the user typed.
        for typed in ["ll_xy7", "xy7", "XY7", "ll_Xy7", "ll_XY7"] {
            assert_eq!(resolve_labels(typed, &tables), "XY7", "input {typed:?}");
        }
    }

    #[test]
    fn unknown_label_kept_verbatim_without_error() {
        let tables = tables_with(&["XY7"]);
        // Not in the cache → passes through (minus any `ll_` prefix), no error.
        // (At a real save site `canonicalize_labels_via_jira` would first try
        // to upgrade it to a real label's canonical casing.)
        assert_eq!(resolve_labels("ll_newthing", &tables), "newthing");
        assert_eq!(resolve_labels("Another", &tables), "Another");
    }

    #[test]
    fn labels_dedup_and_sort_stably() {
        let tables = tables_with(&["XY7", "bug"]);
        assert_eq!(resolve_labels("ll_bug, xy7, XY7", &tables), "XY7,bug");
    }

    #[test]
    fn unknown_assignee_slug_still_errors() {
        // Labels went permissive; assignees did NOT — an unknown user slug is
        // still a hard error (you can't invent a Jira user).
        let tables = tables_with(&[]);
        let mut parsed = Parsed3b::default();
        parsed
            .editable
            .insert("assignee".into(), "uu_nobody".into());
        let mut errors = Vec::new();
        resolve_slugs_inplace(&mut parsed, &tables, &mut errors);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("unknown user slug"));
    }
}

fn is_slug_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
