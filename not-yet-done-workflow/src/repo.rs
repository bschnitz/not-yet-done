//! Flat create/read/update/delete/rename over a directory of workflow `.md`
//! files, plus a [`WorkflowRepo::load`] convenience that reads and parses one.
//!
//! Mirrors the view-scripts kernel (`not-yet-done-scripts`): the directory is
//! the single source of truth, this is the storage layer over it, and it knows
//! nothing about *how* a workflow runs. Unlike scripts there is no per-view
//! scope — a workflow adapter instance owns one flat directory of definitions.

use std::io;
use std::path::{Path, PathBuf};

use crate::model::WorkflowDef;
use crate::parse::parse_workflow;

/// The default on-disk root for workflow definitions:
/// `<data_dir>/not_yet_done/workflows`. Falls back to `./not_yet_done/workflows`
/// when no platform data dir is known.
pub fn default_root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("not_yet_done")
        .join("workflows")
}

/// One workflow file in the repo directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowEntry {
    /// The workflow id — the file stem (without the `.md` extension).
    pub name: String,
    /// Absolute path to the `.md` file.
    pub path: PathBuf,
}

/// Flat CRUD over a root directory of `<name>.md` workflow files.
#[derive(Debug, Clone)]
pub struct WorkflowRepo {
    root: PathBuf,
}

impl Default for WorkflowRepo {
    fn default() -> Self {
        Self::new(default_root())
    }
}

impl WorkflowRepo {
    /// A repo rooted at `root`. Use [`WorkflowRepo::default`] for the real
    /// `<data_dir>/not_yet_done/workflows` location.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The configured root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Absolute path of a workflow file by name (no existence check). `name` is
    /// normalised so a bare `release` maps to `release.md`.
    pub fn path(&self, name: &str) -> PathBuf {
        self.root.join(normalize_name(name))
    }

    /// List the workflow files, `.md` only, sorted case-insensitively by name.
    /// A missing directory yields an empty list — an un-populated instance is a
    /// normal state, not a failure.
    pub fn list(&self) -> io::Result<Vec<WorkflowEntry>> {
        let rd = match std::fs::read_dir(&self.root) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut entries: Vec<WorkflowEntry> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
            .filter(|e| {
                Path::new(&e.file_name())
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("md"))
            })
            .map(|e| WorkflowEntry {
                name: file_stem(&e.file_name().to_string_lossy()),
                path: e.path(),
            })
            .collect();
        entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(entries)
    }

    /// Whether a named workflow exists.
    pub fn exists(&self, name: &str) -> bool {
        self.path(name).is_file()
    }

    /// Read a workflow's raw markdown.
    pub fn read(&self, name: &str) -> io::Result<String> {
        std::fs::read_to_string(self.path(name))
    }

    /// Read and parse a workflow into its [`WorkflowDef`]. The id passed to the
    /// parser is the file stem, so a workflow's `name` is stable regardless of
    /// its frontmatter.
    pub fn load(&self, name: &str) -> io::Result<WorkflowDef> {
        let raw = self.read(name)?;
        Ok(parse_workflow(&file_stem(name), &raw))
    }

    /// Write (create or overwrite) a workflow's raw markdown, creating the root
    /// directory if needed. Returns the file path.
    pub fn write(&self, name: &str, content: &str) -> io::Result<PathBuf> {
        std::fs::create_dir_all(&self.root)?;
        let path = self.path(name);
        std::fs::write(&path, content)?;
        Ok(path)
    }

    /// Create a new workflow from `template`, refusing to clobber an existing
    /// file ([`io::ErrorKind::AlreadyExists`]). Returns the created path.
    pub fn create(&self, name: &str, template: &str) -> io::Result<PathBuf> {
        if self.exists(name) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("workflow '{}' already exists", file_stem(name)),
            ));
        }
        self.write(name, template)
    }

    /// Delete a workflow file.
    pub fn delete(&self, name: &str) -> io::Result<()> {
        std::fs::remove_file(self.path(name))
    }

    /// Rename a workflow, refusing to clobber an existing target
    /// ([`io::ErrorKind::AlreadyExists`]).
    pub fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        if self.exists(to) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("workflow '{}' already exists", file_stem(to)),
            ));
        }
        std::fs::rename(self.path(from), self.path(to))
    }
}

/// The file stem of a workflow name: drop a trailing `.md` (case-insensitive).
/// A name with any other extension is kept verbatim (its stem is itself).
fn file_stem(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if ext.eq_ignore_ascii_case("md") && !stem.is_empty() => stem.to_string(),
        _ => name.to_string(),
    }
}

/// Normalise a user-typed workflow name to a file name: append `.md` unless it
/// already ends in `.md` (case-insensitive).
pub fn normalize_name(name: &str) -> String {
    if Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
    {
        name.to_string()
    } else {
        format!("{name}.md")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, WorkflowRepo) {
        let t = tempfile::tempdir().unwrap();
        let repo = WorkflowRepo::new(t.path());
        (t, repo)
    }

    #[test]
    fn normalize_and_stem_round_trip() {
        assert_eq!(normalize_name("release"), "release.md");
        assert_eq!(normalize_name("release.md"), "release.md");
        assert_eq!(normalize_name("release.MD"), "release.MD");
        assert_eq!(file_stem("release.md"), "release");
        assert_eq!(file_stem("release"), "release");
    }

    #[test]
    fn list_missing_dir_is_empty() {
        let (_t, repo) = repo();
        assert!(repo.list().unwrap().is_empty());
    }

    #[test]
    fn create_list_read_load_and_reject_clobber() {
        let (_t, repo) = repo();
        let path = repo
            .create("release", "---\ntitle: Rel\n---\n## Build\ndo\n")
            .unwrap();
        assert!(path.ends_with("release.md"));

        let listed = repo.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "release");

        // A non-.md sibling is ignored by the listing (written directly, since
        // `write` would normalise the name to `.md`).
        std::fs::write(repo.root().join("notes.txt"), "ignore me").unwrap();
        assert_eq!(repo.list().unwrap().len(), 1);

        let def = repo.load("release").unwrap();
        assert_eq!(def.name, "release");
        assert_eq!(def.title, "Rel");
        assert_eq!(def.steps.len(), 1);

        let err = repo.create("release", "x").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn rename_and_delete() {
        let (_t, repo) = repo();
        repo.create("a", "## s\n").unwrap();
        repo.rename("a", "b").unwrap();
        assert!(!repo.exists("a"));
        assert!(repo.exists("b"));

        repo.create("c", "## s\n").unwrap();
        let err = repo.rename("c", "b").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);

        repo.delete("b").unwrap();
        assert!(!repo.exists("b"));
    }

    #[test]
    fn list_sorts_case_insensitively() {
        let (_t, repo) = repo();
        repo.write("Beta.md", "").unwrap();
        repo.write("alpha.md", "").unwrap();
        let names: Vec<String> = repo.list().unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["alpha", "Beta"]);
    }
}
