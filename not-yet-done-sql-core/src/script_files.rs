//! On-disk layout for a SQL adapter's editable scripts.
//!
//! Two layouts live side by side under an adapter instance's data
//! directory, and neither of them cares which SQL backend is behind it:
//!
//! - `queries/<segments…>/<script>.sql` — named scripts bound to one
//!   node (a table, a view, …). The `<segments…>` come from the
//!   adapter: Postgres decomposes its node id into
//!   `<database>/<schema>/<table>`, a single-file backend might use
//!   just `<alias>/<table>`. This module only joins what it is handed.
//! - `db_scripts/<key>/<rel_path>` — a freely nestable script tree per
//!   top-level container, for utility queries that aren't tied to one
//!   node. `<key>` is an opaque directory name (Postgres passes the
//!   database name); `<rel_path>` carries the file extension, so users
//!   can mix `.sql` with `.py` and friends.
//!
//! The *buffer* format inside those files — scratch area, marker,
//! executable body, error banner — is not this module's business: it is
//! the host-facing protocol every script editor shares and lives in
//! [`not_yet_done_content::script_buffer`]. This module owns only the
//! layout, i.e. which path holds which script.

use std::path::{Path, PathBuf};

use not_yet_done_content::script_buffer::default_buffer;

/// Default script name when the editor opens a node for the first
/// time. Stored as `<segments…>/default.sql`.
pub const DEFAULT_SCRIPT_NAME: &str = "default";

// ---------------------------------------------------------------------------
// Node-scoped scripts: `queries/<segments…>/<script>.sql`
// ---------------------------------------------------------------------------

/// Directory holding all named scripts for one node.
pub fn node_scripts_dir(instance_data_dir: &Path, segments: &[String]) -> PathBuf {
    let mut path = instance_data_dir.join("queries");
    for seg in segments {
        path.push(seg);
    }
    path
}

/// Resolve the on-disk path of a named node script. `script` is the
/// bare name; the `.sql` extension is added here.
pub fn node_script_file_path(
    instance_data_dir: &Path,
    segments: &[String],
    script: &str,
) -> PathBuf {
    node_scripts_dir(instance_data_dir, segments).join(format!("{script}.sql"))
}

/// List the `.sql` script names of one node, without extension.
/// Missing directory ⇒ empty list. Sorted alphabetically. Shortcut
/// bindings live in the `query_shortcut` DB table, not on disk — the
/// TUI merges them in at listing time.
pub async fn list_node_scripts(
    instance_data_dir: &Path,
    segments: &[String],
) -> std::io::Result<Vec<String>> {
    let dir = node_scripts_dir(instance_data_dir, segments);
    let mut rd = match tokio::fs::read_dir(&dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    while let Some(entry) = rd.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("sql") {
            continue;
        }
        if let Some(s) = path.file_stem().and_then(|s| s.to_str()) {
            out.push(s.to_string());
        }
    }
    out.sort();
    Ok(out)
}

/// Read directory entries that are themselves directories. Returns
/// `Ok(None)` when `dir` doesn't exist, so callers can treat "the
/// scripts root was never created" as an empty list rather than an
/// error.
pub async fn read_subdirs(dir: &Path) -> std::io::Result<Option<Vec<String>>> {
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut names = Vec::new();
    while let Some(entry) = rd.next_entry().await? {
        if entry.file_type().await?.is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_string());
            }
        }
    }
    Ok(Some(names))
}

// ---------------------------------------------------------------------------
// Container-level scripts: `db_scripts/<key>/<rel_path>`
//
// Users can nest scripts in arbitrarily deep subdirectories. A
// `Script`'s `rel_path` carries the file extension as part of its last
// component; a `Dir`'s `rel_path` is the directory path. The empty
// rel_path (`PathBuf::new()`) refers to the container root.
// ---------------------------------------------------------------------------

/// Directory holding all container-level scripts for `key`.
pub fn db_scripts_dir(instance_data_dir: &Path, key: &str) -> PathBuf {
    instance_data_dir.join("db_scripts").join(key)
}

/// Resolve the on-disk path of a named container-level script. `script`
/// must include its extension (callers either pass through what the
/// listing returned or default-append `.sql` themselves).
pub fn db_script_file_path(instance_data_dir: &Path, key: &str, script: &str) -> PathBuf {
    db_scripts_dir(instance_data_dir, key).join(script)
}

/// One on-disk container-level script file. `script` includes the file
/// extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbScriptEntry {
    pub database: String,
    pub script: String,
}

/// Walk `db_scripts/` and return every regular file at the first level
/// below each container key. The `script` field carries the full
/// filename including extension; an empty extension is fine.
pub async fn list_all_db_scripts(instance_data_dir: &Path) -> std::io::Result<Vec<DbScriptEntry>> {
    let root = instance_data_dir.join("db_scripts");
    let mut out = Vec::new();
    let keys = match read_subdirs(&root).await? {
        Some(v) => v,
        None => return Ok(out),
    };
    for key in keys {
        let key_dir = root.join(&key);
        let mut rd = match tokio::fs::read_dir(&key_dir).await {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        while let Some(entry) = rd.next_entry().await? {
            if !entry
                .file_type()
                .await
                .map(|t| t.is_file())
                .unwrap_or(false)
            {
                continue;
            }
            let script = match entry.file_name().into_string() {
                Ok(s) => s,
                Err(_) => continue,
            };
            out.push(DbScriptEntry {
                database: key.clone(),
                script,
            });
        }
    }
    out.sort_by(|a, b| (&a.database, &a.script).cmp(&(&b.database, &b.script)));
    Ok(out)
}

/// List every regular file in a single container's flat scripts dir.
/// Each entry carries its full filename including extension. Missing
/// directory ⇒ empty list. Sorted alphabetically.
pub async fn list_db_scripts_in_database(
    instance_data_dir: &Path,
    key: &str,
) -> std::io::Result<Vec<String>> {
    let dir = db_scripts_dir(instance_data_dir, key);
    let mut rd = match tokio::fs::read_dir(&dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    while let Some(entry) = rd.next_entry().await? {
        if !entry
            .file_type()
            .await
            .map(|t| t.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        if let Ok(s) = entry.file_name().into_string() {
            out.push(s);
        }
    }
    out.sort();
    Ok(out)
}

/// Read a container-level script. Missing file ⇒ default template via
/// [`default_db_script_file`] (extension-dependent).
pub async fn read_db_script(
    instance_data_dir: &Path,
    key: &str,
    script: &str,
) -> std::io::Result<String> {
    let path = db_script_file_path(instance_data_dir, key, script);
    match tokio::fs::read_to_string(&path).await {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(default_db_script_file(key, script))
        }
        Err(e) => Err(e),
    }
}

/// Persist a container-level script. Creates the `db_scripts/<key>/`
/// parent directory on first save.
pub async fn write_db_script(
    instance_data_dir: &Path,
    key: &str,
    script: &str,
    content: &str,
) -> std::io::Result<()> {
    let path = db_script_file_path(instance_data_dir, key, script);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&path, content).await
}

/// Remove a container-level script. Missing file is silently ignored so
/// the caller can use this as an idempotent delete.
pub async fn delete_db_script(
    instance_data_dir: &Path,
    key: &str,
    script: &str,
) -> std::io::Result<()> {
    let path = db_script_file_path(instance_data_dir, key, script);
    match tokio::fs::remove_file(&path).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Default file contents shown the first time the editor opens a
/// container-level script. For SQL-flavored scripts: scratch hint,
/// marker, and a placeholder SELECT (the format the per-`:w` executor
/// expects). For any other extension the file starts empty — non-SQL
/// scripts have no established template and forcing one would just
/// confuse the user.
pub fn default_db_script_file(key: &str, script: &str) -> String {
    let _ = key;
    if is_sql_extension(script) {
        default_buffer("SELECT 1;\n")
    } else {
        String::new()
    }
}

/// True if the filename's extension is a SQL flavor (`sql`, `psql`,
/// `pgsql`, `sqlite`). Used by both the template selector above and the
/// edit session to decide whether to apply SQL-specific behavior
/// (template, `.sql` suffix for the temp file, …).
pub fn is_sql_extension(script: &str) -> bool {
    matches!(
        Path::new(script).extension().and_then(|s| s.to_str()),
        Some("sql") | Some("psql") | Some("pgsql") | Some("sqlite")
    )
}

/// One entry inside the recursive script tree for a single container.
///
/// `rel_path` is the path relative to `db_scripts/<key>/`. A flat root
/// script addressed as `audit.sql` has `rel_path = "audit.sql"`; a
/// nested script at `db_scripts/<key>/util/audit.sql` has
/// `rel_path = "util/audit.sql"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbScriptTreeEntry {
    Dir { rel_path: PathBuf },
    Script { rel_path: PathBuf },
}

impl DbScriptTreeEntry {
    pub fn rel_path(&self) -> &Path {
        match self {
            Self::Dir { rel_path } | Self::Script { rel_path } => rel_path,
        }
    }

    /// Last path component as a string. Used by the TUI as a row label.
    pub fn name(&self) -> &str {
        self.rel_path()
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
    }

    pub fn is_dir(&self) -> bool {
        matches!(self, Self::Dir { .. })
    }
}

/// Resolve a `Dir`-relative path to the on-disk directory. The empty
/// rel_path returns the container root itself.
pub fn db_script_dir_path(instance_data_dir: &Path, key: &str, rel_path: &Path) -> PathBuf {
    db_scripts_dir(instance_data_dir, key).join(rel_path)
}

/// Resolve a `Script`-relative path to the on-disk file. `rel_path`
/// carries the extension as part of its final component — the storage
/// layer does not invent a default extension.
pub fn db_script_path(instance_data_dir: &Path, key: &str, rel_path: &Path) -> PathBuf {
    db_scripts_dir(instance_data_dir, key).join(rel_path)
}

/// One level of the script tree under `db_scripts/<key>/<dir_rel_path>/`.
/// Returns `Dir` entries for subdirectories and `Script` entries for any
/// regular file (the extension is part of the file name, e.g.
/// `audit.sql`, `migrate.py`). Sorted: dirs first, then scripts, each
/// group alphabetically by name — so the tree view groups folders above
/// files, matching the convention most file managers use.
///
/// Missing directory ⇒ empty list, so a freshly added container with no
/// scripts yet doesn't surface a synthetic error.
pub async fn list_db_script_entries(
    instance_data_dir: &Path,
    key: &str,
    dir_rel_path: &Path,
) -> std::io::Result<Vec<DbScriptTreeEntry>> {
    let dir = db_script_dir_path(instance_data_dir, key, dir_rel_path);
    let mut rd = match tokio::fs::read_dir(&dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut scripts: Vec<PathBuf> = Vec::new();
    while let Some(entry) = rd.next_entry().await? {
        let ft = entry.file_type().await?;
        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        if ft.is_dir() {
            dirs.push(dir_rel_path.join(&name));
        } else if ft.is_file() {
            scripts.push(dir_rel_path.join(&name));
        }
    }
    dirs.sort();
    scripts.sort();
    let mut out: Vec<DbScriptTreeEntry> = dirs
        .into_iter()
        .map(|rel_path| DbScriptTreeEntry::Dir { rel_path })
        .collect();
    out.extend(
        scripts
            .into_iter()
            .map(|rel_path| DbScriptTreeEntry::Script { rel_path }),
    );
    Ok(out)
}

/// Walk the full script tree for `key` and return every dir and script
/// as a flat list, sorted by rel_path, so callers building a tree view
/// get a deterministic order. Missing root ⇒ empty list.
pub async fn walk_db_script_entries(
    instance_data_dir: &Path,
    key: &str,
) -> std::io::Result<Vec<DbScriptTreeEntry>> {
    let mut out: Vec<DbScriptTreeEntry> = Vec::new();
    let mut stack: Vec<PathBuf> = vec![PathBuf::new()];
    while let Some(dir_rel) = stack.pop() {
        let entries = list_db_script_entries(instance_data_dir, key, &dir_rel).await?;
        for e in entries {
            if let DbScriptTreeEntry::Dir { rel_path } = &e {
                stack.push(rel_path.clone());
            }
            out.push(e);
        }
    }
    out.sort_by(|a, b| a.rel_path().cmp(b.rel_path()));
    Ok(out)
}

/// Create an empty directory at `db_scripts/<key>/<rel_path>/` (and any
/// missing parents). Errors if a regular file already sits at any
/// segment of the path (it would shadow the dir on the next listing).
/// Idempotent when the dir already exists.
pub async fn create_db_script_dir(
    instance_data_dir: &Path,
    key: &str,
    rel_path: &Path,
) -> std::io::Result<()> {
    if rel_path.as_os_str().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "rel_path must not be empty",
        ));
    }
    let abs = db_script_dir_path(instance_data_dir, key, rel_path);
    // `create_dir_all` is happy if the dir already exists, but fails on
    // a file collision with `NotADirectory` / `AlreadyExists` — surface
    // that as-is.
    tokio::fs::create_dir_all(&abs).await
}

/// Delete an empty directory at `db_scripts/<key>/<rel_path>/`. Refuses
/// to delete a non-empty directory: we use `Other` with the message
/// `"not empty (N entries)"` so the TUI can show it verbatim. Missing
/// dir ⇒ Ok (idempotent).
pub async fn delete_db_script_dir(
    instance_data_dir: &Path,
    key: &str,
    rel_path: &Path,
) -> std::io::Result<()> {
    if rel_path.as_os_str().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to delete container script root",
        ));
    }
    let abs = db_script_dir_path(instance_data_dir, key, rel_path);
    let mut rd = match tokio::fs::read_dir(&abs).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let mut count = 0usize;
    while let Some(_e) = rd.next_entry().await? {
        count += 1;
    }
    if count > 0 {
        return Err(std::io::Error::other(format!(
            "not empty ({count} entries)"
        )));
    }
    tokio::fs::remove_dir(&abs).await
}

/// Move (or rename) a dir or script entry inside the same container.
/// The function figures out whether the source is a dir or a file by
/// probing the filesystem.
///
/// - Cross-device renames are not supported (the instance dir is one
///   mount; bubbling `std::io::Error` is fine — the TUI surfaces it).
/// - Name collision in target: hard error (no overwrite).
/// - Missing source: hard error.
/// - `mkdir -p` for the target's parent, so moving into a fresh subtree
///   works without a separate prepare step.
pub async fn move_db_script_entry(
    instance_data_dir: &Path,
    key: &str,
    src_rel: &Path,
    dst_rel: &Path,
) -> std::io::Result<()> {
    if src_rel.as_os_str().is_empty() || dst_rel.as_os_str().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "rel_paths must not be empty",
        ));
    }
    let src_dir = db_script_dir_path(instance_data_dir, key, src_rel);
    let src_file = db_script_path(instance_data_dir, key, src_rel);
    let (src_abs, dst_abs) = if tokio::fs::metadata(&src_dir)
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        (src_dir, db_script_dir_path(instance_data_dir, key, dst_rel))
    } else if tokio::fs::metadata(&src_file)
        .await
        .map(|m| m.is_file())
        .unwrap_or(false)
    {
        (src_file, db_script_path(instance_data_dir, key, dst_rel))
    } else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("source not found: {}", src_rel.display()),
        ));
    };
    if tokio::fs::metadata(&dst_abs).await.is_ok() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("target already exists: {}", dst_rel.display()),
        ));
    }
    if let Some(parent) = dst_abs.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::rename(&src_abs, &dst_abs).await
}

/// Rename a dir or script entry in place: keeps the same parent, only
/// the last segment changes. Thin wrapper around
/// [`move_db_script_entry`]; rejects names containing path separators,
/// which would silently turn the rename into a move.
pub async fn rename_db_script_entry(
    instance_data_dir: &Path,
    key: &str,
    rel_path: &Path,
    new_name: &str,
) -> std::io::Result<()> {
    if new_name.is_empty() || new_name.contains('/') || new_name.contains('\\') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "new_name must be a single path segment",
        ));
    }
    let parent = rel_path.parent().unwrap_or_else(|| Path::new(""));
    let dst_rel = parent.join(new_name);
    move_db_script_entry(instance_data_dir, key, rel_path, &dst_rel).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::script_buffer::QUERY_MARKER;

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp dir")
    }

    fn segs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn script_rel(p: &str) -> PathBuf {
        PathBuf::from(p)
    }

    #[test]
    fn node_script_path_joins_every_segment_under_queries() {
        let base = Path::new("/tmp/nyd/instance");
        let p = node_script_file_path(base, &segs(&["mydb", "public", "users"]), "default");
        assert_eq!(
            p,
            Path::new("/tmp/nyd/instance/queries/mydb/public/users/default.sql")
        );
    }

    #[test]
    fn node_script_path_works_for_a_shallower_layout() {
        // A single-file backend has no schema level; the layout follows
        // whatever segments the adapter derives from its node id.
        let base = Path::new("/tmp/nyd/instance");
        let p = node_script_file_path(base, &segs(&["main", "users"]), "active_only");
        assert_eq!(
            p,
            Path::new("/tmp/nyd/instance/queries/main/users/active_only.sql")
        );
    }

    #[test]
    fn node_script_path_with_no_segments_stays_in_queries_root() {
        let base = Path::new("/tmp/nyd/instance");
        let p = node_script_file_path(base, &[], DEFAULT_SCRIPT_NAME);
        assert_eq!(p, Path::new("/tmp/nyd/instance/queries/default.sql"));
    }

    #[tokio::test]
    async fn list_node_scripts_returns_empty_when_dir_missing() {
        let dir = tmpdir();
        let v = list_node_scripts(dir.path(), &segs(&["db", "public", "users"]))
            .await
            .unwrap();
        assert!(v.is_empty());
    }

    #[tokio::test]
    async fn list_node_scripts_returns_sql_files_only_sorted() {
        let dir = tmpdir();
        let s = segs(&["db", "public", "users"]);
        let td = node_scripts_dir(dir.path(), &s);
        tokio::fs::create_dir_all(&td).await.unwrap();
        tokio::fs::write(td.join("zeta.sql"), "select 1")
            .await
            .unwrap();
        tokio::fs::write(td.join("alpha.sql"), "select 2")
            .await
            .unwrap();
        tokio::fs::write(td.join("notes.txt"), "ignore me")
            .await
            .unwrap();
        let v = list_node_scripts(dir.path(), &s).await.unwrap();
        assert_eq!(v, vec!["alpha".to_string(), "zeta".to_string()]);
    }

    #[test]
    fn is_sql_extension_accepts_known_sql_flavors() {
        assert!(is_sql_extension("foo.sql"));
        assert!(is_sql_extension("foo.psql"));
        assert!(is_sql_extension("foo.pgsql"));
        assert!(is_sql_extension("foo.sqlite"));
        assert!(is_sql_extension("nested/path/audit.sql"));
        assert!(!is_sql_extension("foo.py"));
        assert!(!is_sql_extension("foo.md"));
        assert!(!is_sql_extension("noext"));
        assert!(!is_sql_extension(""));
    }

    #[test]
    fn default_db_script_file_template_only_for_sql_flavors() {
        assert!(default_db_script_file("mydb", "audit.sql").contains(QUERY_MARKER));
        assert!(default_db_script_file("mydb", "audit.psql").contains(QUERY_MARKER));
        assert!(default_db_script_file("mydb", "audit.pgsql").contains(QUERY_MARKER));
        assert!(default_db_script_file("mydb", "audit.py").is_empty());
        assert!(default_db_script_file("mydb", "notes.md").is_empty());
    }

    #[tokio::test]
    async fn db_script_path_is_under_db_scripts_subdir() {
        let base = Path::new("/tmp/nyd/instance");
        // Caller passes the full filename (extension included). The helper
        // does not hard-code `.sql`.
        let p = db_script_file_path(base, "mydb", "cross_schema.sql");
        assert_eq!(
            p,
            Path::new("/tmp/nyd/instance/db_scripts/mydb/cross_schema.sql")
        );
        // Non-SQL extension just lands as-is.
        let py = db_script_file_path(base, "mydb", "migrate.py");
        assert_eq!(
            py,
            Path::new("/tmp/nyd/instance/db_scripts/mydb/migrate.py")
        );
    }

    #[tokio::test]
    async fn list_db_scripts_returns_empty_when_dir_missing() {
        let dir = tmpdir();
        let v = list_db_scripts_in_database(dir.path(), "mydb")
            .await
            .unwrap();
        assert!(v.is_empty());
        let all = list_all_db_scripts(dir.path()).await.unwrap();
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn db_scripts_roundtrip_read_write_delete() {
        let dir = tmpdir();
        write_db_script(dir.path(), "mydb", "cross_schema.sql", "SELECT 42;")
            .await
            .unwrap();
        let got = read_db_script(dir.path(), "mydb", "cross_schema.sql")
            .await
            .unwrap();
        assert_eq!(got, "SELECT 42;");

        // Missing `.sql` file ⇒ default SQL template (marker + placeholder).
        let dflt = read_db_script(dir.path(), "mydb", "no_such.sql")
            .await
            .unwrap();
        assert!(dflt.contains(QUERY_MARKER));

        // Missing non-SQL file ⇒ empty body: the editor opens with no
        // SQL boilerplate (a Python/markdown script has no marker model).
        let py = read_db_script(dir.path(), "mydb", "no_such.py")
            .await
            .unwrap();
        assert!(py.is_empty());

        delete_db_script(dir.path(), "mydb", "cross_schema.sql")
            .await
            .unwrap();
        let after = read_db_script(dir.path(), "mydb", "cross_schema.sql")
            .await
            .unwrap();
        assert!(
            after.contains(QUERY_MARKER),
            "delete then re-read returns default"
        );

        // Idempotent delete.
        delete_db_script(dir.path(), "mydb", "cross_schema.sql")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn list_all_db_scripts_walks_container_dirs() {
        let dir = tmpdir();
        write_db_script(dir.path(), "alpha", "two.sql", "x")
            .await
            .unwrap();
        write_db_script(dir.path(), "alpha", "one.sql", "x")
            .await
            .unwrap();
        // Mixed extension: surfaces with its real filename so the user
        // sees what kind of file each script is.
        write_db_script(dir.path(), "alpha", "helper.py", "x")
            .await
            .unwrap();
        write_db_script(dir.path(), "beta", "only.sql", "x")
            .await
            .unwrap();

        let v = list_all_db_scripts(dir.path()).await.unwrap();
        let pairs: Vec<(&str, &str)> = v
            .iter()
            .map(|e| (e.database.as_str(), e.script.as_str()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("alpha", "helper.py"),
                ("alpha", "one.sql"),
                ("alpha", "two.sql"),
                ("beta", "only.sql"),
            ]
        );

        let single = list_db_scripts_in_database(dir.path(), "alpha")
            .await
            .unwrap();
        assert_eq!(
            single,
            vec![
                "helper.py".to_string(),
                "one.sql".to_string(),
                "two.sql".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn db_script_path_helpers_compose_with_rel_path() {
        let base = Path::new("/tmp/nyd/instance");
        // Root-flat script — extension is part of rel_path.
        assert_eq!(
            db_script_path(base, "mydb", &script_rel("audit.sql")),
            Path::new("/tmp/nyd/instance/db_scripts/mydb/audit.sql")
        );
        // Nested script keeps its extension verbatim.
        assert_eq!(
            db_script_path(base, "mydb", &script_rel("util/audit.sql")),
            Path::new("/tmp/nyd/instance/db_scripts/mydb/util/audit.sql")
        );
        // Custom extensions land as-is.
        assert_eq!(
            db_script_path(base, "mydb", &script_rel("util/migrate.py")),
            Path::new("/tmp/nyd/instance/db_scripts/mydb/util/migrate.py")
        );
        // Nested dir.
        assert_eq!(
            db_script_dir_path(base, "mydb", &script_rel("util/inner")),
            Path::new("/tmp/nyd/instance/db_scripts/mydb/util/inner")
        );
        // Empty rel_path == container root.
        assert_eq!(
            db_script_dir_path(base, "mydb", Path::new("")),
            Path::new("/tmp/nyd/instance/db_scripts/mydb")
        );
    }

    #[tokio::test]
    async fn list_db_script_entries_empty_when_missing() {
        let dir = tmpdir();
        let v = list_db_script_entries(dir.path(), "mydb", Path::new(""))
            .await
            .unwrap();
        assert!(v.is_empty());
    }

    #[tokio::test]
    async fn list_db_script_entries_groups_dirs_then_scripts_sorted() {
        let dir = tmpdir();
        let db_root = db_scripts_dir(dir.path(), "mydb");
        tokio::fs::create_dir_all(db_root.join("util"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(db_root.join("audit"))
            .await
            .unwrap();
        tokio::fs::write(db_root.join("zeta.sql"), "x")
            .await
            .unwrap();
        tokio::fs::write(db_root.join("alpha.sql"), "x")
            .await
            .unwrap();
        // Non-SQL files are first-class entries: the extension stays in
        // the rel_path so the TUI can render the right icon/label.
        tokio::fs::write(db_root.join("notes.md"), "ignore me not")
            .await
            .unwrap();

        let v = list_db_script_entries(dir.path(), "mydb", Path::new(""))
            .await
            .unwrap();
        assert_eq!(
            v,
            vec![
                DbScriptTreeEntry::Dir {
                    rel_path: script_rel("audit")
                },
                DbScriptTreeEntry::Dir {
                    rel_path: script_rel("util")
                },
                DbScriptTreeEntry::Script {
                    rel_path: script_rel("alpha.sql")
                },
                DbScriptTreeEntry::Script {
                    rel_path: script_rel("notes.md")
                },
                DbScriptTreeEntry::Script {
                    rel_path: script_rel("zeta.sql")
                },
            ]
        );
    }

    #[tokio::test]
    async fn walk_db_script_entries_returns_full_tree_sorted_by_rel_path() {
        let dir = tmpdir();
        let db_root = db_scripts_dir(dir.path(), "mydb");
        tokio::fs::create_dir_all(db_root.join("util/inner"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(db_root.join("audit"))
            .await
            .unwrap();
        tokio::fs::write(db_root.join("util/helper.sql"), "x")
            .await
            .unwrap();
        tokio::fs::write(db_root.join("util/inner/deep.sql"), "x")
            .await
            .unwrap();
        tokio::fs::write(db_root.join("audit/main.sql"), "x")
            .await
            .unwrap();
        tokio::fs::write(db_root.join("root.sql"), "x")
            .await
            .unwrap();

        let v = walk_db_script_entries(dir.path(), "mydb").await.unwrap();
        // Sorted by rel_path: dirs and scripts interleave. A Script's
        // rel_path keeps its extension; Dirs have none.
        assert_eq!(
            v,
            vec![
                DbScriptTreeEntry::Dir {
                    rel_path: script_rel("audit")
                },
                DbScriptTreeEntry::Script {
                    rel_path: script_rel("audit/main.sql")
                },
                DbScriptTreeEntry::Script {
                    rel_path: script_rel("root.sql")
                },
                DbScriptTreeEntry::Dir {
                    rel_path: script_rel("util")
                },
                DbScriptTreeEntry::Script {
                    rel_path: script_rel("util/helper.sql")
                },
                DbScriptTreeEntry::Dir {
                    rel_path: script_rel("util/inner")
                },
                DbScriptTreeEntry::Script {
                    rel_path: script_rel("util/inner/deep.sql")
                },
            ]
        );
    }

    #[tokio::test]
    async fn create_db_script_dir_creates_nested_parents() {
        let dir = tmpdir();
        create_db_script_dir(dir.path(), "mydb", &script_rel("a/b/c"))
            .await
            .unwrap();
        let abs = db_script_dir_path(dir.path(), "mydb", &script_rel("a/b/c"));
        assert!(tokio::fs::metadata(&abs).await.unwrap().is_dir());
        // Idempotent.
        create_db_script_dir(dir.path(), "mydb", &script_rel("a/b/c"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_db_script_dir_rejects_empty_rel_path() {
        let dir = tmpdir();
        let err = create_db_script_dir(dir.path(), "mydb", Path::new(""))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn delete_db_script_dir_empty_succeeds_and_nonempty_errors() {
        let dir = tmpdir();
        create_db_script_dir(dir.path(), "mydb", &script_rel("empty"))
            .await
            .unwrap();
        delete_db_script_dir(dir.path(), "mydb", &script_rel("empty"))
            .await
            .unwrap();

        // Non-empty.
        create_db_script_dir(dir.path(), "mydb", &script_rel("full"))
            .await
            .unwrap();
        let p = db_script_path(dir.path(), "mydb", &script_rel("full/inside"));
        tokio::fs::write(&p, "x").await.unwrap();
        let err = delete_db_script_dir(dir.path(), "mydb", &script_rel("full"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not empty"), "got: {err}");
    }

    #[tokio::test]
    async fn delete_db_script_dir_missing_is_ok() {
        let dir = tmpdir();
        delete_db_script_dir(dir.path(), "mydb", &script_rel("gone"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn delete_db_script_dir_rejects_empty_rel_path() {
        let dir = tmpdir();
        let err = delete_db_script_dir(dir.path(), "mydb", Path::new(""))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn move_db_script_entry_moves_file_and_creates_parent() {
        let dir = tmpdir();
        write_db_script(dir.path(), "mydb", "foo", "SELECT 1;")
            .await
            .unwrap();
        move_db_script_entry(
            dir.path(),
            "mydb",
            &script_rel("foo"),
            &script_rel("util/foo_renamed"),
        )
        .await
        .unwrap();
        // Source gone.
        let src = db_script_path(dir.path(), "mydb", &script_rel("foo"));
        assert!(!src.exists());
        // Target exists with original content.
        let dst = db_script_path(dir.path(), "mydb", &script_rel("util/foo_renamed"));
        let content = tokio::fs::read_to_string(&dst).await.unwrap();
        assert_eq!(content, "SELECT 1;");
    }

    #[tokio::test]
    async fn move_db_script_entry_moves_directory() {
        let dir = tmpdir();
        create_db_script_dir(dir.path(), "mydb", &script_rel("src_dir"))
            .await
            .unwrap();
        let p = db_script_path(dir.path(), "mydb", &script_rel("src_dir/inner"));
        tokio::fs::write(&p, "x").await.unwrap();

        move_db_script_entry(
            dir.path(),
            "mydb",
            &script_rel("src_dir"),
            &script_rel("nested/dst_dir"),
        )
        .await
        .unwrap();

        let dst_inside = db_script_path(dir.path(), "mydb", &script_rel("nested/dst_dir/inner"));
        assert!(dst_inside.exists());
    }

    #[tokio::test]
    async fn move_db_script_entry_rejects_collision() {
        let dir = tmpdir();
        write_db_script(dir.path(), "mydb", "a", "x").await.unwrap();
        write_db_script(dir.path(), "mydb", "b", "y").await.unwrap();
        let err = move_db_script_entry(dir.path(), "mydb", &script_rel("a"), &script_rel("b"))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    }

    #[tokio::test]
    async fn move_db_script_entry_missing_source_errors() {
        let dir = tmpdir();
        let err = move_db_script_entry(dir.path(), "mydb", &script_rel("nope"), &script_rel("ok"))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[tokio::test]
    async fn rename_db_script_entry_keeps_parent_changes_last_segment() {
        let dir = tmpdir();
        create_db_script_dir(dir.path(), "mydb", &script_rel("util"))
            .await
            .unwrap();
        let p = db_script_path(dir.path(), "mydb", &script_rel("util/old"));
        tokio::fs::write(&p, "x").await.unwrap();

        rename_db_script_entry(dir.path(), "mydb", &script_rel("util/old"), "new")
            .await
            .unwrap();
        let renamed = db_script_path(dir.path(), "mydb", &script_rel("util/new"));
        assert!(renamed.exists());
    }

    #[tokio::test]
    async fn rename_db_script_entry_rejects_separators_in_new_name() {
        let dir = tmpdir();
        write_db_script(dir.path(), "mydb", "foo", "x")
            .await
            .unwrap();
        let err = rename_db_script_entry(dir.path(), "mydb", &script_rel("foo"), "a/b")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn rename_db_script_entry_rejects_empty_new_name() {
        let dir = tmpdir();
        write_db_script(dir.path(), "mydb", "foo", "x")
            .await
            .unwrap();
        let err = rename_db_script_entry(dir.path(), "mydb", &script_rel("foo"), "")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }
}
