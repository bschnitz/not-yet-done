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
    pseudo_email, pseudo_filename, pseudo_issue_key, pseudo_person, pseudo_username,
};
use not_yet_done_content::{Anonymizer, StandardAnonymizer};

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
            // Free text (issue summary; the generic `label`, which is the
            // summary for issues and a person/filename for sub-nodes — all safe
            // as free text).
            "summary" | "label" => self.std.scrub_value(key, value),
            // Structural / addressing — verbatim.
            "type" | "status" | "priority" | "updated" | "created" | "size"
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
        let who = a.scrub_value("assignee", "Max Mustermann");
        assert!(!who.contains("Mustermann"));
        // Display name and username derive consistently.
        assert_eq!(
            a.scrub_value("username", "Max Mustermann"),
            a.scrub_value("display_name", "Max Mustermann").to_lowercase().replace(' ', ".")
        );
    }

    #[test]
    fn structural_fields_pass_through() {
        let a = JiraAnonymizer::default();
        for (k, v) in [("status", "In Progress"), ("type", "Bug"), ("updated", "2026-06-22T10:00:00Z")] {
            assert_eq!(a.scrub_value(k, v), v);
        }
    }

    #[test]
    fn empty_assignee_stays_empty() {
        let a = JiraAnonymizer::default();
        assert_eq!(a.scrub_value("assignee", ""), "");
    }
}
