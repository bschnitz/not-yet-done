//! Normalization + bidirectional slug tables.
//!
//! Slugs (`<prefix>-foo-bar`) replace raw values inside editable templates so
//! the buffer stays autocomplete-friendly. Each adapter picks its own prefix
//! per field (e.g. `ll-`/`uu-` for Jira labels/users, `ss-`/`tt-`/`uu-` for
//! Taiga statuses/tags/members) and feeds the table `(slug_source, original)`
//! pairs.
//!
//! The slug body is derived from a normalized form of `slug_source`;
//! collisions get deterministic `-2`, `-3`, … suffixes (lex sort by
//! `original`).

use std::collections::BTreeMap;

/// Normalize a string to a slug-body:
/// - lowercase
/// - German umlauts → `ae` / `oe` / `ue` / `ss`
/// - ASCII whitespace → `-`
/// - any other non-alphanumeric, non-`-` char dropped
/// - consecutive `-` collapsed; leading/trailing `-` trimmed
pub fn normalize(s: &str) -> String {
    let mut buf = String::with_capacity(s.len());
    for ch in s.chars() {
        let lower = ch.to_lowercase().next().unwrap_or(ch);
        match lower {
            'ä' => buf.push_str("ae"),
            'ö' => buf.push_str("oe"),
            'ü' => buf.push_str("ue"),
            'ß' => buf.push_str("ss"),
            c if c.is_whitespace() => buf.push('-'),
            c if c.is_ascii_alphanumeric() || c == '-' => buf.push(c),
            _ => {}
        }
    }
    let mut out = String::with_capacity(buf.len());
    let mut prev_dash = false;
    for c in buf.chars() {
        if c == '-' {
            if !prev_dash {
                out.push(c);
            }
            prev_dash = true;
        } else {
            out.push(c);
            prev_dash = false;
        }
    }
    out.trim_matches('-').to_string()
}

/// Bidirectional map between original values and rendered slugs.
pub struct SlugTable {
    slug_to_original: BTreeMap<String, String>,
    original_to_slug: BTreeMap<String, String>,
}

impl SlugTable {
    /// Build from `(slug_source, original)` pairs. `slug_source` is what gets
    /// normalized into the slug body; `original` is what comes back out of
    /// `original_for(slug)`.
    ///
    /// `prefix` is prepended to every slug (e.g. `"ll-"`, `"uu-"`). It must
    /// end with `-` for round-trip safety in editor templates.
    pub fn build<I>(items: I, prefix: &str) -> Self
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut entries: Vec<(String, String)> = items.into_iter().collect();
        entries.sort_by(|a, b| a.1.cmp(&b.1));
        entries.dedup_by(|a, b| a.1 == b.1);

        let mut groups: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        for (src, orig) in entries {
            let norm = normalize(&src);
            if norm.is_empty() {
                continue;
            }
            groups.entry(norm).or_default().push((src, orig));
        }

        let mut slug_to_original = BTreeMap::new();
        let mut original_to_slug = BTreeMap::new();
        for (norm, group) in groups {
            for (idx, (_, orig)) in group.iter().enumerate() {
                let slug = if idx == 0 {
                    format!("{prefix}{norm}")
                } else {
                    format!("{prefix}{norm}-{}", idx + 1)
                };
                slug_to_original.insert(slug.clone(), orig.clone());
                original_to_slug.insert(orig.clone(), slug);
            }
        }

        Self { slug_to_original, original_to_slug }
    }

    pub fn slug_for(&self, original: &str) -> Option<&str> {
        self.original_to_slug.get(original).map(String::as_str)
    }

    pub fn original_for(&self, slug: &str) -> Option<&str> {
        self.slug_to_original.get(slug).map(String::as_str)
    }

    /// Sorted slugs for rendering completion sections (e.g. CACHE).
    pub fn slugs(&self) -> Vec<&str> {
        self.slug_to_original.keys().map(String::as_str).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.slug_to_original.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_basics() {
        assert_eq!(normalize("Foo Bar"), "foo-bar");
        assert_eq!(normalize("Schäfer, Lina"), "schaefer-lina");
        assert_eq!(normalize("ÄÖÜß"), "aeoeuess");
        assert_eq!(normalize("a---b"), "a-b");
        assert_eq!(normalize("  spaces   "), "spaces");
        assert_eq!(normalize("weird!@#chars"), "weirdchars");
    }

    #[test]
    fn table_roundtrips() {
        let pairs = vec![
            ("bug".to_string(), "bug".to_string()),
            ("Foo Bar".to_string(), "Foo Bar".to_string()),
            ("Frontend Bug".to_string(), "Frontend Bug".to_string()),
        ];
        let t = SlugTable::build(pairs, "ll-");
        assert_eq!(t.slug_for("bug"), Some("ll-bug"));
        assert_eq!(t.slug_for("Foo Bar"), Some("ll-foo-bar"));
        assert_eq!(t.original_for("ll-bug"), Some("bug"));
        assert_eq!(t.original_for("ll-foo-bar"), Some("Foo Bar"));
    }

    #[test]
    fn collisions_get_suffix() {
        let pairs = vec![
            ("Foo Bar".to_string(), "Foo Bar".to_string()),
            ("foo bar".to_string(), "foo bar".to_string()),
        ];
        let t = SlugTable::build(pairs, "ll-");
        assert_eq!(t.slug_for("Foo Bar"), Some("ll-foo-bar"));
        assert_eq!(t.slug_for("foo bar"), Some("ll-foo-bar-2"));
    }

    #[test]
    fn slug_source_separate_from_original() {
        // User table: slug from display name, original is username.
        let pairs = vec![("Doe, Jane".to_string(), "JDOE1".to_string())];
        let t = SlugTable::build(pairs, "uu-");
        assert_eq!(t.slug_for("JDOE1"), Some("uu-doe-jane"));
        assert_eq!(t.original_for("uu-doe-jane"), Some("JDOE1"));
    }
}
