//! Turning `sources:` glob patterns into the adapter's root children.
//!
//! Two problems live here.
//!
//! **Expanding the patterns.** [`globset`] matches paths, it does not walk
//! the filesystem, so we bring our own walker: split each pattern at the
//! first component containing a metacharacter, walk downwards from the
//! literal prefix with `tokio::fs`, and test every candidate against the
//! compiled matcher. Walking async matters — a pattern can point at a
//! network mount, and a synchronous `read_dir` loop would block the whole
//! runtime while it stalls.
//!
//! **Naming the files.** A node id has to be built from path *segments*,
//! but a source is identified by an absolute file path — which is neither
//! a single segment nor short. Ids also have to be stable across restarts
//! because they end up in on-disk script paths and in the `query_shortcut`
//! table. So each source gets `<sanitized stem>-<hash of its full path>`;
//! see [`source_key`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use globset::GlobBuilder;

/// One database file the adapter exposes as a root child.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceEntry {
    /// Stable id segment, see [`source_key`].
    pub key: String,
    /// What the user sees: the file name.
    pub label: String,
    /// Absolute path to the database file.
    pub path: PathBuf,
}

/// How deep a `**` pattern is allowed to descend. A bound is needed
/// because `**` is unbounded by definition and a stray
/// `~/**/*.db` should not turn into a full home-directory crawl.
const MAX_RECURSIVE_DEPTH: usize = 16;

/// Characters that make a path component a glob rather than a literal.
fn has_meta(component: &str) -> bool {
    component.contains(['*', '?', '[', '{'])
}

/// Expand a leading `~` / `~/…` to the user's home directory. Anything
/// else is returned unchanged — in particular `~user/…` is *not*
/// expanded, since resolving other users' homes needs passwd lookups and
/// would silently do the wrong thing when it failed.
pub fn expand_home(pattern: &str) -> String {
    let Some(rest) = pattern.strip_prefix('~') else {
        return pattern.to_string();
    };
    if !(rest.is_empty() || rest.starts_with('/')) {
        return pattern.to_string();
    }
    match dirs::home_dir() {
        Some(home) => format!("{}{rest}", home.display()),
        None => pattern.to_string(),
    }
}

/// Split a pattern into its literal directory prefix and the glob tail.
/// `/srv/data/*.db` → (`/srv/data`, `Some("*.db")`);
/// `/srv/data/x.db` → (`/srv/data/x.db`, `None`).
fn split_pattern(pattern: &str) -> (PathBuf, Option<String>) {
    let mut prefix = PathBuf::new();
    let mut tail: Vec<&str> = Vec::new();
    for component in pattern.split('/') {
        if tail.is_empty() && !has_meta(component) {
            if component.is_empty() && prefix.as_os_str().is_empty() {
                // Leading `/` of an absolute pattern.
                prefix.push("/");
            } else if !component.is_empty() {
                prefix.push(component);
            }
        } else {
            tail.push(component);
        }
    }
    if tail.is_empty() {
        (prefix, None)
    } else {
        (prefix, Some(tail.join("/")))
    }
}

/// FNV-1a, 32 bit. Hand-written on purpose: the key it feeds is
/// persisted, and `std::hash::DefaultHasher` explicitly does not
/// guarantee the same output across Rust releases.
fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Filesystem-safe, single-segment form of a file stem.
fn sanitize_stem(stem: &str) -> String {
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() {
        "db".to_string()
    } else {
        trimmed.chars().take(40).collect()
    }
}

/// Stable id segment for one database file: the readable stem plus a hash
/// of the full path.
///
/// The hash is what makes it correct rather than merely pretty. Two
/// sources can share a file name (`app/data.db` and `backup/data.db`),
/// and the stem alone would collide — silently merging two databases into
/// one tree node and one script directory. The hash is over the whole
/// path, so a file keeps its key as long as it keeps its location.
pub fn source_key(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("db");
    let hash = fnv1a32(path.as_os_str().as_encoded_bytes());
    format!("{}-{hash:08x}", sanitize_stem(stem))
}

/// Expand every pattern and return one entry per distinct file, sorted by
/// label (then key, so same-named files keep a deterministic order).
///
/// Patterns that match nothing — or name a directory that does not exist
/// — contribute nothing rather than failing: a config listing several
/// machines' data directories should still work on the machine where only
/// one of them is mounted.
pub async fn resolve_sources(patterns: &[String]) -> Vec<SourceEntry> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut entries: Vec<SourceEntry> = Vec::new();
    for pattern in patterns {
        for path in expand_pattern(&expand_home(pattern)).await {
            if !seen.insert(path.clone()) {
                continue;
            }
            let label = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("database")
                .to_string();
            entries.push(SourceEntry {
                key: source_key(&path),
                label,
                path,
            });
        }
    }
    entries.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.key.cmp(&b.key)));
    entries
}

/// Every existing file matching one already home-expanded pattern.
async fn expand_pattern(pattern: &str) -> Vec<PathBuf> {
    let (prefix, tail) = split_pattern(pattern);
    let Some(tail) = tail else {
        // A literal path is its own match — as long as it exists.
        return match tokio::fs::metadata(&prefix).await {
            Ok(meta) if meta.is_file() => vec![prefix],
            _ => Vec::new(),
        };
    };
    let Ok(glob) = GlobBuilder::new(pattern)
        // Without this `*` would happily match across `/`, so
        // `/srv/*.db` would also find `/srv/a/b.db`.
        .literal_separator(true)
        .build()
    else {
        return Vec::new();
    };
    let matcher = glob.compile_matcher();
    let max_depth = if tail.contains("**") {
        MAX_RECURSIVE_DEPTH
    } else {
        tail.split('/').count()
    };

    let mut found = Vec::new();
    let mut queue: Vec<(PathBuf, usize)> = vec![(prefix, 0)];
    while let Some((dir, depth)) = queue.pop() {
        if depth >= max_depth {
            continue;
        }
        let Ok(mut read_dir) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let path = entry.path();
            // `file_type` does not follow symlinks, so a symlinked
            // directory is never descended into — that keeps the walk
            // free of cycles. Symlinks *to files* still match below.
            match entry.file_type().await {
                Ok(ft) if ft.is_dir() => queue.push((path, depth + 1)),
                Ok(_) => {
                    if matcher.is_match(&path) {
                        found.push(path);
                    }
                }
                Err(_) => {}
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_pattern_separates_literal_prefix_from_glob_tail() {
        assert_eq!(
            split_pattern("/srv/data/*.db"),
            (PathBuf::from("/srv/data"), Some("*.db".to_string()))
        );
        assert_eq!(
            split_pattern("/srv/data/**/*.sqlite"),
            (PathBuf::from("/srv/data"), Some("**/*.sqlite".to_string()))
        );
        assert_eq!(
            split_pattern("/srv/data/metrics.db"),
            (PathBuf::from("/srv/data/metrics.db"), None)
        );
    }

    #[test]
    fn source_key_is_stable_readable_and_collision_free() {
        let a = source_key(Path::new("/srv/app/data.db"));
        let b = source_key(Path::new("/srv/backup/data.db"));
        assert!(a.starts_with("data-"), "{a}");
        assert_ne!(a, b, "same file name in different dirs must differ");
        assert_eq!(
            a,
            source_key(Path::new("/srv/app/data.db")),
            "must be stable"
        );
        assert!(!a.contains('/'), "key has to be a single path segment: {a}");
    }

    #[test]
    fn source_key_sanitizes_awkward_names() {
        let key = source_key(Path::new("/srv/my data (v2).sqlite"));
        // Spaces and parens collapse to `_`; trailing `_` is trimmed so
        // the readable part never ends in filler.
        assert!(key.starts_with("my_data__v2-"), "{key}");
    }

    #[test]
    fn fnv1a32_matches_the_reference_vectors() {
        // Reference values from the FNV spec — guards against an
        // accidental change to the constants, which would silently
        // rewrite every persisted key.
        assert_eq!(fnv1a32(b""), 0x811c_9dc5);
        assert_eq!(fnv1a32(b"a"), 0xe40c_292c);
        assert_eq!(fnv1a32(b"foobar"), 0xbf9c_f968);
    }

    #[test]
    fn expand_home_only_touches_a_leading_tilde_segment() {
        assert_eq!(expand_home("/srv/x.db"), "/srv/x.db");
        assert_eq!(expand_home("~other/x.db"), "~other/x.db");
        if let Some(home) = dirs::home_dir() {
            assert_eq!(
                expand_home("~/notes/x.db"),
                format!("{}/notes/x.db", home.display())
            );
        }
    }

    async fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.expect("mkdir");
        }
        tokio::fs::write(path, b"").await.expect("write");
    }

    #[tokio::test]
    async fn resolve_sources_expands_globs_and_skips_non_matches() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        touch(&root.join("alpha.db")).await;
        touch(&root.join("beta.db")).await;
        touch(&root.join("notes.txt")).await;
        touch(&root.join("nested/gamma.db")).await;

        let pattern = format!("{}/*.db", root.display());
        let found = resolve_sources(&[pattern]).await;
        let labels: Vec<&str> = found.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, vec!["alpha.db", "beta.db"]);
    }

    #[tokio::test]
    async fn resolve_sources_descends_for_a_recursive_pattern() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        touch(&root.join("top.db")).await;
        touch(&root.join("a/mid.db")).await;
        touch(&root.join("a/b/deep.db")).await;

        let found = resolve_sources(&[format!("{}/**/*.db", root.display())]).await;
        let labels: Vec<&str> = found.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, vec!["deep.db", "mid.db", "top.db"]);
    }

    #[tokio::test]
    async fn resolve_sources_dedupes_overlapping_patterns() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("only.db");
        touch(&file).await;

        let found = resolve_sources(&[
            format!("{}/*.db", tmp.path().display()),
            file.display().to_string(),
        ])
        .await;
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[tokio::test]
    async fn a_pattern_matching_nothing_is_not_an_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let found = resolve_sources(&[
            format!("{}/does-not-exist/*.db", tmp.path().display()),
            "/nonexistent-root-for-tests/*.db".to_string(),
        ])
        .await;
        assert!(found.is_empty(), "{found:?}");
    }

    #[tokio::test]
    async fn a_literal_path_matches_only_itself() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("one.db");
        touch(&file).await;
        touch(&tmp.path().join("two.db")).await;

        let found = resolve_sources(&[file.display().to_string()]).await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, file);
    }

    #[tokio::test]
    async fn a_single_star_does_not_cross_a_directory_boundary() {
        let tmp = tempfile::tempdir().expect("tempdir");
        touch(&tmp.path().join("flat.db")).await;
        touch(&tmp.path().join("sub/nested.db")).await;

        let found = resolve_sources(&[format!("{}/*.db", tmp.path().display())]).await;
        let labels: Vec<&str> = found.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, vec!["flat.db"]);
    }
}
