use std::path::{Path, PathBuf};

use globset::{Glob, GlobSetBuilder};
use ignore::WalkBuilder;

/// Per-call options for [`enumerate`].
#[derive(Debug, Clone)]
pub struct EnumerationOptions {
    /// Respect `.gitignore` / `.git/info/exclude` etc. when walking.
    pub respect_gitignore: bool,
    /// Include dot-files / dot-dirs in the walk.
    pub show_hidden: bool,
    /// Safety cap on returned entries. Prevents runaway walks on very large
    /// trees from blocking the UI thread.
    pub max_results: usize,
}

impl Default for EnumerationOptions {
    fn default() -> Self {
        Self {
            respect_gitignore: true,
            show_hidden: false,
            max_results: 5000,
        }
    }
}

/// Walk `dir` and return file paths matching any of `glob_patterns`.
///
/// `glob_patterns` is a comma-separated list of gitignore-style globs
/// (e.g. `"*.js, **/*.png"`). Empty patterns or whitespace between commas
/// are ignored. If every entry fails to parse the matcher behaves as if
/// no glob was supplied: every file in scope is returned.
///
/// The walk is bounded to the current directory unless at least one
/// pattern contains `**` or `/`, which is the cheapest way to keep
/// `*.png` from descending into deep trees uninvited.
///
/// If `dir` does not exist or is not a directory, an empty vec is
/// returned — callers can detect this via the input path themselves and
/// render an inline error.
pub fn enumerate(dir: &Path, glob_patterns: &str, opts: &EnumerationOptions) -> Vec<PathBuf> {
    if !dir.is_dir() {
        return Vec::new();
    }

    let mut builder = GlobSetBuilder::new();
    let mut any_pattern = false;
    let mut recursive = false;
    for raw in glob_patterns
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        if let Ok(g) = Glob::new(raw) {
            builder.add(g);
            any_pattern = true;
            if raw.contains("**") || raw.contains('/') {
                recursive = true;
            }
        }
    }
    let glob_set = if any_pattern {
        builder.build().ok()
    } else {
        None
    };
    let max_depth = if recursive { None } else { Some(1) };

    let mut walk = WalkBuilder::new(dir);
    walk.git_ignore(opts.respect_gitignore)
        .git_exclude(opts.respect_gitignore)
        .git_global(opts.respect_gitignore)
        .hidden(!opts.show_hidden)
        .max_depth(max_depth);

    let mut out = Vec::new();
    for result in walk.build() {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path == dir {
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let matches = match &glob_set {
            Some(gs) => {
                let rel = path.strip_prefix(dir).unwrap_or(path);
                gs.is_match(rel)
            }
            None => true,
        };
        if matches {
            out.push(path.to_path_buf());
            if out.len() >= opts.max_results {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn touch(dir: &Path, rel: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&p, b"").unwrap();
    }

    #[test]
    fn returns_files_in_current_directory_for_simple_glob() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "a.txt");
        touch(tmp.path(), "b.png");
        touch(tmp.path(), "c.txt");
        touch(tmp.path(), "nested/deep.txt");

        let opts = EnumerationOptions::default();
        let mut got = enumerate(tmp.path(), "*.txt", &opts);
        got.sort();

        assert_eq!(got.len(), 2);
        assert!(got[0].ends_with("a.txt"));
        assert!(got[1].ends_with("c.txt"));
    }

    #[test]
    fn double_star_descends_recursively() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "a.txt");
        touch(tmp.path(), "nested/deep.txt");
        touch(tmp.path(), "nested/inner/leaf.txt");

        let opts = EnumerationOptions::default();
        let mut got = enumerate(tmp.path(), "**/*.txt", &opts);
        got.sort();

        assert_eq!(got.len(), 3);
    }

    #[test]
    fn comma_separated_patterns_or() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "a.txt");
        touch(tmp.path(), "b.png");
        touch(tmp.path(), "c.rs");

        let opts = EnumerationOptions::default();
        let mut got = enumerate(tmp.path(), "*.txt, *.png", &opts);
        got.sort();

        assert_eq!(got.len(), 2);
        assert!(got[0].ends_with("a.txt"));
        assert!(got[1].ends_with("b.png"));
    }

    #[test]
    fn empty_patterns_match_all_in_current_dir_only() {
        let tmp = TempDir::new().unwrap();
        touch(tmp.path(), "a.txt");
        touch(tmp.path(), "b.png");
        touch(tmp.path(), "nested/deep.txt");

        let opts = EnumerationOptions::default();
        let got = enumerate(tmp.path(), "", &opts);

        assert_eq!(got.len(), 2);
    }

    #[test]
    fn missing_directory_yields_empty() {
        let opts = EnumerationOptions::default();
        let got = enumerate(Path::new("/does/not/exist/hopefully"), "*", &opts);
        assert!(got.is_empty());
    }

    #[test]
    fn gitignore_files_are_skipped_inside_a_git_repo() {
        let tmp = TempDir::new().unwrap();
        // ignore::WalkBuilder only honours .gitignore inside a git repo
        // (require_git default). Materialise the marker dir to opt in.
        fs::create_dir(tmp.path().join(".git")).unwrap();
        touch(tmp.path(), "keep.txt");
        touch(tmp.path(), "ignored.txt");
        fs::write(tmp.path().join(".gitignore"), b"ignored.txt\n").unwrap();

        let opts = EnumerationOptions::default();
        let got = enumerate(tmp.path(), "*.txt", &opts);
        assert_eq!(got.len(), 1);
        assert!(got[0].ends_with("keep.txt"));
    }

    #[test]
    fn max_results_caps_output() {
        let tmp = TempDir::new().unwrap();
        for i in 0..10 {
            touch(tmp.path(), &format!("f{i}.txt"));
        }

        let opts = EnumerationOptions {
            max_results: 3,
            ..EnumerationOptions::default()
        };
        let got = enumerate(tmp.path(), "*.txt", &opts);
        assert_eq!(got.len(), 3);
    }
}
