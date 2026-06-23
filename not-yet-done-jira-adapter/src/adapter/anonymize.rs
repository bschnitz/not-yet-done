//! Jira realism anonymizer.
//!
//! The content layer already wraps every adapter in a safe
//! [`StandardAnonymizer`](not_yet_done_content::StandardAnonymizer) when
//! `NYD_ANON` is set, so Jira data is *never* leaked even without this. This
//! override only buys **realism** for a screenshot: an issue key stays
//! key-shaped (`DEMO-4711` → `ACME-4711`), an assignee stays a person name,
//! a filename keeps its extension — instead of every value becoming neutral
//! pool words. It is keyed deterministically (see [`not_yet_done_content::anonymize`]).

use not_yet_done_content::anonymize::{
    pseudo_email, pseudo_filename, pseudo_issue_key, pseudo_person, pseudo_username, stable_hash,
};
use not_yet_done_content::{Anonymizer, StandardAnonymizer};

/// Plausible, fully generic workflow statuses. We map the *real* status onto one
/// of these instead of passing it through verbatim, because a Jira workflow can
/// be customised with status names that embed a customer/project term (e.g.
/// "Waiting for ACME") — which a verbatim status would leak into a screenshot.
/// Mapping is deterministic (hash-keyed) so the same real status always shows
/// the same placeholder, keeping group-by-status views coherent.
const STATUS_POOL: &[&str] = &[
    "To Do",
    "In Progress",
    "In Review",
    "Blocked",
    "Done",
    "Backlog",
];

/// Map a real status to a stable [`STATUS_POOL`] entry. Empty / letterless
/// values pass through (nothing to leak).
fn pseudo_status(real: &str) -> String {
    if !real.chars().any(|c| c.is_ascii_alphabetic()) {
        return real.to_string();
    }
    let idx = (stable_hash(real) % STATUS_POOL.len() as u64) as usize;
    STATUS_POOL[idx].to_string()
}

#[derive(Debug, Clone, Copy, Default)]
pub struct JiraAnonymizer {
    std: StandardAnonymizer,
}

impl Anonymizer for JiraAnonymizer {
    fn scrub_value(&self, key: &str, value: &str) -> String {
        match key {
            // Issue keys (the issue's own key, and the parent-issue key carried
            // on comments/attachments) — format-preserving.
            "key" | "issue" => pseudo_issue_key(value),
            // People.
            "assignee" | "author" | "display_name" => pseudo_person(value),
            "username" => pseudo_username(value),
            "email" => pseudo_email(value),
            // Files.
            "filename" => pseudo_filename(value),
            // Status: mapped to a generic pool — a customised workflow status can
            // embed a customer/project term, so verbatim would leak.
            "status" => pseudo_status(value),
            // Free text (issue summary; the generic `label`, which is the
            // summary for issues and a person/filename for sub-nodes — all safe
            // as free text).
            "summary" | "label" => self.std.scrub_value(key, value),
            // Structural / addressing — verbatim. (`type`/`priority` are standard
            // enums — "Bug", "Story", "High" — not customer-identifying.)
            "type" | "priority" | "updated" | "created" | "size"
            | "mime_type" | "attachments" => value.to_string(),
            // Unknown future column → safe fallback.
            _ => self.std.scrub_value(key, value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_key_is_format_preserving_assignee_is_a_person() {
        let a = JiraAnonymizer::default();
        let k = a.scrub_value("key", "DEMO-4711");
        assert!(k.ends_with("-4711") && !k.contains("DEMO"));
        let who = a.scrub_value("assignee", "John Doe");
        assert!(!who.contains("Doe"));
        // Display name and username derive consistently.
        assert_eq!(
            a.scrub_value("username", "John Doe"),
            a.scrub_value("display_name", "John Doe").to_lowercase().replace(' ', ".")
        );
    }

    #[test]
    fn structural_fields_pass_through() {
        let a = JiraAnonymizer::default();
        for (k, v) in [("type", "Bug"), ("priority", "High"), ("updated", "2026-06-22T10:00:00Z")] {
            assert_eq!(a.scrub_value(k, v), v);
        }
    }

    #[test]
    fn status_is_mapped_to_a_generic_pool_deterministically() {
        let a = JiraAnonymizer::default();
        // A customised status that embeds a customer term must not survive.
        let s = a.scrub_value("status", "Waiting for ACME");
        assert!(!s.contains("ACME"), "real status term leaked: {s}");
        assert!(STATUS_POOL.contains(&s.as_str()), "must land in the pool: {s}");
        // Deterministic: same input → same placeholder.
        assert_eq!(s, a.scrub_value("status", "Waiting for ACME"));
        // Letterless passes through.
        assert_eq!(a.scrub_value("status", ""), "");
    }

    #[test]
    fn empty_assignee_stays_empty() {
        let a = JiraAnonymizer::default();
        assert_eq!(a.scrub_value("assignee", ""), "");
    }
}
