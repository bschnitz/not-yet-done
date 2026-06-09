//! `ll-…` (label) and `uu-…` (user) slug rendering and parsing.
//!
//! Slugs replace the raw Jira values inside the editable section and inside
//! comment bodies, so the editor buffer stays readable (no `[~jdoe]` or raw
//! label hashes). The slug tables come from the live cache plus the issue's
//! own current values.

use std::sync::Mutex;

use not_yet_done_content::slug::SlugTable;

use crate::client::{JiraIssueDetail, JiraUser};

use super::super::cache::JiraCache;
use super::template::{FieldError, Parsed3b, field_value_from_detail};

pub const LABEL_PREFIX: &str = "ll-";
pub const USER_PREFIX: &str = "uu-";

fn build_label_table(labels: &[String]) -> SlugTable {
    SlugTable::build(
        labels.iter().map(|l| (l.clone(), l.clone())),
        LABEL_PREFIX,
    )
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

/// In-buffer slug tables. Built per render/parse from the live cache plus
/// the issue's own current values (so the existing assignee / labels are
/// always representable as a slug, even if the cache is cold).
pub(super) struct SlugTables {
    pub(super) labels: SlugTable,
    pub(super) users: SlugTable,
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
        _ => resolved.to_string(),
    }
}

/// Translate `ll-…` slugs in `parsed.editable["labels"]` into the original
/// label names (comma-separated). Translate `parsed.editable["assignee"]`
/// from `uu-…` slug into the original Jira-username. Unknown slugs produce
/// `FieldError`s.
pub(super) fn resolve_slugs_inplace(
    parsed: &mut Parsed3b,
    tables: &SlugTables,
    errors: &mut Vec<FieldError>,
) {
    if let Some(raw) = parsed.editable.get("labels").cloned() {
        let mut originals: Vec<String> = Vec::new();
        for item in raw.split(',') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            if !item.starts_with(LABEL_PREFIX) {
                errors.push(FieldError {
                    message: format!(
                        "label `{item}` must start with `{}` (use a slug from the CACHE section)",
                        LABEL_PREFIX,
                    ),
                });
                continue;
            }
            match tables.labels.original_for(item) {
                Some(orig) => originals.push(orig.to_string()),
                None => errors.push(FieldError {
                    message: format!("unknown label slug `{item}`"),
                }),
            }
        }
        // Stable order so unchanged labels round-trip identically.
        originals.sort();
        originals.dedup();
        parsed.editable.insert("labels".into(), originals.join(","));
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

/// Replace `[~KEY]` mentions with `@uu-slug` for the editor display.
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

/// Reverse of `render_user_mentions`: rewrite `@uu-slug` back to `[~KEY]`.
/// Only matches at word boundaries so `email@uu-foo.com` is preserved.
/// Returns the offending slug if any `@uu-…` doesn't resolve.
pub(super) fn parse_user_mentions(text: &str, users: &SlugTable) -> std::result::Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut last = 0usize;
    let mut i = 0usize;
    while i + 4 <= bytes.len() {
        if bytes[i] == b'@' && &bytes[i + 1..i + 4] == b"uu-" {
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

fn is_slug_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-'
}
