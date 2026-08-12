//! Adapter-agnostic CRUD for **view-scripts** — the small executable files
//! the TUI keeps per view level and the CLI (and, later, other frontends)
//! address programmatically.
//!
//! This crate is the single source of truth for **where** a view level's
//! scripts live on disk and the flat create/read/update/delete/rename kernel
//! over them. It deliberately knows nothing about *how* a script is executed
//! (external process, `# mode:` header, JSON payload) or *how* a chord is
//! bound to it — those stay with the embedder (the TUI). Keeping the storage
//! kernel here lets the TUI menu and the CLI adapter-action share one
//! implementation instead of each re-deriving the path scheme.
//!
//! # The path scheme (must match the historical TUI layout)
//!
//! A [`ScriptScope`] identifies one view level by its owning adapter type and
//! the view-hierarchy node-type path drilled into below the root. Scripts for
//! that level live under:
//!
//! ```text
//! <root>/<adapter>/<seg1>/<seg2>/…
//! ```
//!
//! where `<root>` defaults to `<data_dir>/not_yet_done/scripts` and each
//! segment is a view node-type with `/` and `:` replaced by `_` (so
//! `jira:issue` becomes `jira_issue`). The adapter component is joined
//! verbatim — adapter types carry no path-hostile characters. This reproduces
//! exactly the directory the TUI's `ScriptContext::scripts_dir` has always
//! written to, so existing on-disk scripts remain addressable.
//!
//! The **shortcut scope** (`script:<adapter>/<seg…>`, segments *un-escaped*)
//! mirrors the TUI's `ScriptContext::shortcut_scope`, which keys chords in the
//! `query_shortcut` store. It is exposed here so both frontends agree on the
//! key without re-deriving it.

use std::io;
use std::path::{Path, PathBuf};

mod decorator;
pub use decorator::*;

/// Filesystem-safe form of one raw view-path segment: `/` and `:` → `_`. The
/// single source of truth for how a node type becomes a directory name, shared
/// by [`ScriptScope::dir`] and the adapter-action id encoding.
pub fn escape_segment(seg: &str) -> String {
    seg.replace(['/', ':'], "_")
}

/// Identifies one view level's script directory: the owning adapter type plus
/// the view-hierarchy node-type path drilled into below the adapter root.
///
/// `segments` holds the **raw** node types (e.g. `["jira:issue"]`); escaping
/// for the filesystem happens in [`ScriptScope::dir`], while
/// [`ScriptScope::shortcut_scope`] keeps them verbatim. An empty `segments`
/// addresses the adapter's top level (`<root>/<adapter>`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScriptScope {
    /// Owning adapter type (the view's tab), e.g. `jira`, `tasks`.
    pub adapter: String,
    /// View node-type path below the adapter root, raw (un-escaped).
    pub segments: Vec<String>,
}

impl ScriptScope {
    /// Construct a scope from an adapter type and raw node-type segments.
    pub fn new(adapter: impl Into<String>, segments: Vec<String>) -> Self {
        Self {
            adapter: adapter.into(),
            segments,
        }
    }

    /// The directory holding this scope's scripts, resolved under `root`
    /// (typically [`default_root`]). Segments are filesystem-escaped; the
    /// adapter component is joined verbatim.
    pub fn dir(&self, root: &Path) -> PathBuf {
        let mut p = root.join(&self.adapter);
        for seg in &self.segments {
            p = p.join(escape_segment(seg));
        }
        p
    }

    /// This scope's segments in their filesystem-escaped form (`/`,`:` → `_`).
    pub fn escaped_segments(&self) -> Vec<String> {
        self.segments.iter().map(|s| escape_segment(s)).collect()
    }

    /// The `query_shortcut` scope key for chords bound at this level:
    /// `script:<adapter>/<segments joined by '/'>` with segments *un-escaped*,
    /// matching the TUI's shortcut scope so a chord is shared across frontends.
    pub fn shortcut_scope(&self) -> String {
        format!("script:{}/{}", self.adapter, self.segments.join("/"))
    }
}

/// One script file listed in a scope's directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptEntry {
    /// File name including extension, e.g. `daily_report.py`.
    pub name: String,
    /// Absolute path to the file.
    pub path: PathBuf,
}

/// The default on-disk root for view-scripts: `<data_dir>/not_yet_done/scripts`.
/// Falls back to `./not_yet_done/scripts` when no platform data dir is known
/// (mirrors the TUI's historical fallback of the current directory).
pub fn default_root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("not_yet_done")
        .join("scripts")
}

/// Flat create/read/update/delete/rename over a scope's script files, rooted at
/// a configurable base directory (injectable for tests).
///
/// "Flat" means it treats each scope directory as a bucket of files; it does
/// not recurse into subdirectories (the TUI menu lists files only, never
/// nested directories). Hierarchical backends (e.g. Postgres db-scripts) will
/// be layered on later behind the adapter surface, not here.
#[derive(Debug, Clone)]
pub struct ScriptRepo {
    root: PathBuf,
}

impl Default for ScriptRepo {
    fn default() -> Self {
        Self::new(default_root())
    }
}

impl ScriptRepo {
    /// A repo rooted at `root`. Use [`ScriptRepo::default`] for the real
    /// `<data_dir>/not_yet_done/scripts` location.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The configured root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory for `scope` under this repo's root.
    pub fn dir(&self, scope: &ScriptScope) -> PathBuf {
        scope.dir(&self.root)
    }

    /// Absolute path of a named script within `scope` (no existence check).
    pub fn path(&self, scope: &ScriptScope, name: &str) -> PathBuf {
        self.dir(scope).join(name)
    }

    /// List the scripts in `scope`, files only, sorted case-insensitively by
    /// name. A missing directory yields an empty list rather than an error —
    /// an un-populated level is a normal state, not a failure.
    pub fn list(&self, scope: &ScriptScope) -> io::Result<Vec<ScriptEntry>> {
        let dir = self.dir(scope);
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut entries: Vec<ScriptEntry> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
            .map(|e| ScriptEntry {
                name: e.file_name().to_string_lossy().to_string(),
                path: e.path(),
            })
            .collect();
        entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(entries)
    }

    /// Read a script's contents.
    pub fn read(&self, scope: &ScriptScope, name: &str) -> io::Result<String> {
        std::fs::read_to_string(self.path(scope, name))
    }

    /// Whether a named script exists in `scope`.
    pub fn exists(&self, scope: &ScriptScope, name: &str) -> bool {
        self.path(scope, name).is_file()
    }

    /// Write (create or overwrite) a script's contents, creating the scope
    /// directory if needed. This is the update path — the caller decides
    /// whether an overwrite is intended (see [`ScriptRepo::create`] for the
    /// no-clobber variant).
    pub fn write(&self, scope: &ScriptScope, name: &str, content: &str) -> io::Result<PathBuf> {
        let dir = self.dir(scope);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(name);
        std::fs::write(&path, content)?;
        Ok(path)
    }

    /// Create a new script from `template`, refusing to clobber an existing
    /// file (returns [`io::ErrorKind::AlreadyExists`]). `name` is normalised
    /// via [`normalize_name`] so a bare `foo` becomes `foo.py`, matching the
    /// TUI's create-new default suffix. Returns the created file's path.
    pub fn create(&self, scope: &ScriptScope, name: &str, template: &str) -> io::Result<PathBuf> {
        let name = normalize_name(name);
        if self.exists(scope, &name) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("script '{name}' already exists"),
            ));
        }
        self.write(scope, &name, template)
    }

    /// Delete a script.
    pub fn delete(&self, scope: &ScriptScope, name: &str) -> io::Result<()> {
        std::fs::remove_file(self.path(scope, name))
    }

    /// Rename a script within the same scope, refusing to clobber an existing
    /// target (returns [`io::ErrorKind::AlreadyExists`]).
    pub fn rename(&self, scope: &ScriptScope, from: &str, to: &str) -> io::Result<()> {
        if self.exists(scope, to) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("script '{to}' already exists"),
            ));
        }
        std::fs::rename(self.path(scope, from), self.path(scope, to))
    }
}

/// Normalise a user-typed script name: if it carries no extension, append the
/// default `.py` suffix. Mirrors the TUI create-new behaviour so a bare name
/// yields the same file regardless of frontend.
pub fn normalize_name(name: &str) -> String {
    if name.contains('.') {
        name.to_string()
    } else {
        format!("{name}.py")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn dir_escapes_segments_but_not_adapter() {
        let scope = ScriptScope::new("jira", vec!["jira:issue".into(), "jira:comment".into()]);
        let dir = scope.dir(Path::new("/root"));
        assert_eq!(dir, PathBuf::from("/root/jira/jira_issue/jira_comment"));
    }

    #[test]
    fn dir_with_no_segments_is_adapter_root() {
        let scope = ScriptScope::new("tasks", vec![]);
        assert_eq!(scope.dir(Path::new("/root")), PathBuf::from("/root/tasks"));
    }

    #[test]
    fn shortcut_scope_keeps_segments_unescaped() {
        let scope = ScriptScope::new("jira", vec!["jira:issue".into()]);
        assert_eq!(scope.shortcut_scope(), "script:jira/jira:issue");
    }

    #[test]
    fn shortcut_scope_with_no_segments_has_trailing_slash() {
        // Matches the TUI: `format!("script:{tab}/{}", view_path.join("/"))`
        // with an empty view_path yields a trailing slash.
        let scope = ScriptScope::new("tasks", vec![]);
        assert_eq!(scope.shortcut_scope(), "script:tasks/");
    }

    #[test]
    fn list_missing_dir_is_empty() {
        let t = tmp();
        let repo = ScriptRepo::new(t.path());
        let scope = ScriptScope::new("jira", vec!["jira:issue".into()]);
        assert!(repo.list(&scope).unwrap().is_empty());
    }

    #[test]
    fn create_then_list_read_and_reject_clobber() {
        let t = tmp();
        let repo = ScriptRepo::new(t.path());
        let scope = ScriptScope::new("jira", vec!["jira:issue".into()]);

        // bare name gains the .py suffix
        let path = repo.create(&scope, "report", "print('hi')").unwrap();
        assert!(path.ends_with("jira/jira_issue/report.py"));

        let listed = repo.list(&scope).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "report.py");

        assert_eq!(repo.read(&scope, "report.py").unwrap(), "print('hi')");

        // second create refuses to clobber
        let err = repo.create(&scope, "report", "x").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn explicit_extension_is_preserved() {
        let t = tmp();
        let repo = ScriptRepo::new(t.path());
        let scope = ScriptScope::new("tasks", vec![]);
        repo.create(&scope, "run.sh", "#!/bin/sh\n").unwrap();
        assert!(repo.exists(&scope, "run.sh"));
        assert!(!repo.exists(&scope, "run.sh.py"));
    }

    #[test]
    fn write_overwrites_and_creates_dirs() {
        let t = tmp();
        let repo = ScriptRepo::new(t.path());
        let scope = ScriptScope::new("jira", vec!["jira:issue".into()]);
        repo.write(&scope, "a.py", "one").unwrap();
        repo.write(&scope, "a.py", "two").unwrap();
        assert_eq!(repo.read(&scope, "a.py").unwrap(), "two");
    }

    #[test]
    fn delete_removes_file() {
        let t = tmp();
        let repo = ScriptRepo::new(t.path());
        let scope = ScriptScope::new("tasks", vec![]);
        repo.create(&scope, "a", "x").unwrap();
        assert!(repo.exists(&scope, "a.py"));
        repo.delete(&scope, "a.py").unwrap();
        assert!(!repo.exists(&scope, "a.py"));
    }

    #[test]
    fn rename_moves_and_rejects_clobber() {
        let t = tmp();
        let repo = ScriptRepo::new(t.path());
        let scope = ScriptScope::new("tasks", vec![]);
        repo.create(&scope, "a", "x").unwrap();
        repo.rename(&scope, "a.py", "b.py").unwrap();
        assert!(!repo.exists(&scope, "a.py"));
        assert_eq!(repo.read(&scope, "b.py").unwrap(), "x");

        repo.create(&scope, "c", "y").unwrap();
        let err = repo.rename(&scope, "c.py", "b.py").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn list_sorts_case_insensitively_files_only() {
        let t = tmp();
        let repo = ScriptRepo::new(t.path());
        let scope = ScriptScope::new("tasks", vec![]);
        repo.write(&scope, "Beta.py", "").unwrap();
        repo.write(&scope, "alpha.py", "").unwrap();
        // a nested directory must not appear in the flat listing
        std::fs::create_dir_all(repo.dir(&scope).join("subdir")).unwrap();
        let names: Vec<String> = repo
            .list(&scope)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, vec!["alpha.py", "Beta.py"]);
    }
}
