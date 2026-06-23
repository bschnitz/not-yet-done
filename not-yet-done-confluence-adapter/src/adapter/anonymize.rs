//! Confluence realism anonymizer.
//!
//! The content layer already applies a safe
//! [`StandardAnonymizer`](not_yet_done_content::StandardAnonymizer) when
//! `NYD_ANON` is set, so Confluence data is never leaked without this. This
//! override only buys realism: a space key stays code-shaped (`DEMO` → `ACME`),
//! authors stay person names, filenames keep their extension; page titles and
//! comment bodies become neutral words. Deterministic and keyed per
//! [`not_yet_done_content::anonymize`].

use not_yet_done_content::anonymize::{pseudo_filename, pseudo_person, pseudo_project_code};
use not_yet_done_content::{Anonymizer, StandardAnonymizer};

#[derive(Debug, Clone, Copy, Default)]
pub struct ConfluenceAnonymizer {
    std: StandardAnonymizer,
}

impl Anonymizer for ConfluenceAnonymizer {
    fn scrub_value(&self, key: &str, value: &str) -> String {
        match key {
            // Space keys (a space's own `key`, the `space` column on CQL hits,
            // and the `space_key` carried on tree-search hits) — code-shaped.
            "key" | "space" | "space_key" => pseudo_project_code(value),
            "author" => pseudo_person(value),
            "filename" => pseudo_filename(value),
            // Free text (space name, page title, comment body, generic label).
            "name" | "title" | "body" | "label" => self.std.scrub_value(key, value),
            // Structural / addressing — verbatim. `id` / `page` are internal
            // numeric ids (addressing); `type` a content kind.
            "id" | "page" | "type" | "modified" | "created" | "size" | "mime_type" => {
                value.to_string()
            }
            _ => self.std.scrub_value(key, value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_key_is_code_shaped_author_is_a_person() {
        let a = ConfluenceAnonymizer::default();
        let k = a.scrub_value("key", "DEMO");
        assert!(!k.contains("DEMO") && k.chars().all(|c| c.is_ascii_uppercase()));
        // The same space key maps the same wherever it surfaces.
        assert_eq!(a.scrub_value("space", "DEMO"), k);
        assert_eq!(a.scrub_value("space_key", "DEMO"), k);
        assert!(!a.scrub_value("author", "John Doe").contains("Doe"));
    }

    #[test]
    fn structural_and_id_fields_pass_through() {
        let a = ConfluenceAnonymizer::default();
        for (k, v) in [("id", "123456"), ("page", "98765"), ("type", "page")] {
            assert_eq!(a.scrub_value(k, v), v);
        }
    }
}
