//! Taiga realism anonymizer.
//!
//! The content layer already applies a safe
//! [`StandardAnonymizer`](not_yet_done_content::StandardAnonymizer) when
//! `NYD_ANON` is set, so Taiga data is never leaked without this. This override
//! only buys realism: a ref stays ref-shaped (`demoproject#12` → `acme#12`),
//! assignees/authors stay person names, filenames keep their extension.
//! Deterministic and keyed per [`not_yet_done_content::anonymize`].

use not_yet_done_content::anonymize::{pseudo_filename, pseudo_person, pseudo_ref};
use not_yet_done_content::{Anonymizer, StandardAnonymizer};

#[derive(Debug, Clone, Copy, Default)]
pub struct TaigaAnonymizer {
    std: StandardAnonymizer,
}

impl TaigaAnonymizer {
    /// Comma-separated person list (`assignee` carries multiple display names).
    fn people_csv(&self, value: &str) -> String {
        if value.is_empty() {
            return String::new();
        }
        value
            .split(',')
            .map(|tok| pseudo_person(tok.trim()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl Anonymizer for TaigaAnonymizer {
    fn scrub_value(&self, key: &str, value: &str) -> String {
        match key {
            // `slug#123` / `#123` — format-preserving.
            "ref" => pseudo_ref(value),
            // People (assignee may be a comma-separated list; actor/author single).
            "assignee" => self.people_csv(value),
            "author" | "actor" => pseudo_person(value),
            "filename" => pseudo_filename(value),
            // Free text (subject/body/description/tags, the project *name* on a
            // notification, and the generic `label` = subject/body — all safe
            // as neutral words).
            "subject" | "body" | "description" | "tags" | "project" | "label" => {
                self.std.scrub_value(key, value)
            }
            // Structural / addressing — verbatim.
            "type" | "status" | "modified" | "created" | "attachments" | "size"
            | "version" | "event" | "read" => value.to_string(),
            _ => self.std.scrub_value(key, value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_is_format_preserving_and_assignees_are_people() {
        let a = TaigaAnonymizer::default();
        let r = a.scrub_value("ref", "demoproject#12");
        assert!(r.ends_with("#12") && !r.contains("demoproject"));
        assert_eq!(a.scrub_value("ref", "#7"), "#7", "bare ref kept");
        let who = a.scrub_value("assignee", "John Doe, Jane Roe");
        assert!(!who.contains("Doe") && !who.contains("Roe"));
        assert_eq!(who.split(',').count(), 2, "two assignees preserved");
    }

    #[test]
    fn structural_fields_pass_through() {
        let a = TaigaAnonymizer::default();
        for (k, v) in [("status", "New"), ("type", "userstory"), ("version", "4")] {
            assert_eq!(a.scrub_value(k, v), v);
        }
    }
}
