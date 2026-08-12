//! Normalization + bidirectional slug tables.
//!
//! Slugs (`<prefix>_foo_bar`) replace raw values inside editable templates so
//! the buffer stays autocomplete-friendly. Each adapter picks its own prefix
//! per field (e.g. `ll_`/`uu_` for Jira labels/users, `ss_`/`tt_`/`uu_` for
//! Taiga statuses/tags/members) and feeds the table `(slug_source, original)`
//! pairs.
//!
//! The separator is `_` (not `-`) so a slug stays a single word-token in the
//! editor — `-` would split it at Markdown/editor word boundaries.
//!
//! The slug body is derived from a normalized form of `slug_source`;
//! collisions get deterministic `_2`, `_3`, … suffixes (lex sort by
//! `original`).

use std::collections::BTreeMap;

/// Normalize a string to a slug-body:
/// - lowercase
/// - German umlauts → `ae` / `oe` / `ue` / `ss`
/// - ASCII whitespace and `-` → `_` (so a hyphenated source keeps its word
///   boundary as an underscore instead of dropping the separator)
/// - any other non-alphanumeric, non-`_` char dropped
/// - consecutive `_` collapsed; leading/trailing `_` trimmed
pub fn normalize(s: &str) -> String {
    let mut buf = String::with_capacity(s.len());
    for ch in s.chars() {
        let lower = ch.to_lowercase().next().unwrap_or(ch);
        match lower {
            'ä' => buf.push_str("ae"),
            'ö' => buf.push_str("oe"),
            'ü' => buf.push_str("ue"),
            'ß' => buf.push_str("ss"),
            c if c.is_whitespace() || c == '-' => buf.push('_'),
            c if c.is_ascii_alphanumeric() || c == '_' => buf.push(c),
            _ => {}
        }
    }
    let mut out = String::with_capacity(buf.len());
    let mut prev_sep = false;
    for c in buf.chars() {
        if c == '_' {
            if !prev_sep {
                out.push(c);
            }
            prev_sep = true;
        } else {
            out.push(c);
            prev_sep = false;
        }
    }
    out.trim_matches('_').to_string()
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
    /// `prefix` is prepended to every slug (e.g. `"ll_"`, `"uu_"`). It must
    /// end with `_` for round-trip safety in editor templates.
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
                    format!("{prefix}{norm}_{}", idx + 1)
                };
                slug_to_original.insert(slug.clone(), orig.clone());
                original_to_slug.insert(orig.clone(), slug);
            }
        }

        Self {
            slug_to_original,
            original_to_slug,
        }
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
        assert_eq!(normalize("Foo Bar"), "foo_bar");
        assert_eq!(normalize("Schäfer, Lina"), "schaefer_lina");
        assert_eq!(normalize("ÄÖÜß"), "aeoeuess");
        assert_eq!(normalize("a___b"), "a_b");
        assert_eq!(normalize("a   b"), "a_b");
        assert_eq!(normalize("  spaces   "), "spaces");
        assert_eq!(normalize("weird!@#chars"), "weirdchars");
        // `-` maps to the `_` separator (word boundary preserved).
        assert_eq!(normalize("foo-bar"), "foo_bar");
        assert_eq!(normalize("a---b"), "a_b");
        assert_eq!(normalize("in-progress bug"), "in_progress_bug");
    }

    #[test]
    fn table_roundtrips() {
        let pairs = vec![
            ("bug".to_string(), "bug".to_string()),
            ("Foo Bar".to_string(), "Foo Bar".to_string()),
            ("Frontend Bug".to_string(), "Frontend Bug".to_string()),
        ];
        let t = SlugTable::build(pairs, "ll_");
        assert_eq!(t.slug_for("bug"), Some("ll_bug"));
        assert_eq!(t.slug_for("Foo Bar"), Some("ll_foo_bar"));
        assert_eq!(t.original_for("ll_bug"), Some("bug"));
        assert_eq!(t.original_for("ll_foo_bar"), Some("Foo Bar"));
    }

    #[test]
    fn collisions_get_suffix() {
        let pairs = vec![
            ("Foo Bar".to_string(), "Foo Bar".to_string()),
            ("foo bar".to_string(), "foo bar".to_string()),
        ];
        let t = SlugTable::build(pairs, "ll_");
        assert_eq!(t.slug_for("Foo Bar"), Some("ll_foo_bar"));
        assert_eq!(t.slug_for("foo bar"), Some("ll_foo_bar_2"));
    }

    #[test]
    fn slug_source_separate_from_original() {
        // User table: slug from display name, original is username.
        let pairs = vec![("Doe, Jane".to_string(), "JDOE1".to_string())];
        let t = SlugTable::build(pairs, "uu_");
        assert_eq!(t.slug_for("JDOE1"), Some("uu_doe_jane"));
        assert_eq!(t.original_for("uu_doe_jane"), Some("JDOE1"));
    }
}
