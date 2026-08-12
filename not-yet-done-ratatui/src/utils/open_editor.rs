use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use tempfile::NamedTempFile;

#[derive(Debug)]
pub enum EditorError {
    Io(io::Error),
    EditorFailed(std::process::ExitStatus),
}

impl std::fmt::Display for EditorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditorError::Io(e) => write!(f, "IO error: {}", e),
            EditorError::EditorFailed(s) => write!(f, "Editor exited with status: {}", s),
        }
    }
}

impl From<io::Error> for EditorError {
    fn from(e: io::Error) -> Self {
        EditorError::Io(e)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn resolve_command(command: Option<&str>) -> String {
    command
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| std::env::var("EDITOR").ok())
        .unwrap_or_else(|| "vi".to_string())
}

fn build_command(template: &str, file: &Path, env: &HashMap<String, String>) -> Command {
    let mut cmd = Command::new("sh");

    // `{env}` expands to a sequence of shell variable assignments
    // (e.g. `PGHOST='127.0.0.1' PGPORT='40973' `). This is the only
    // way to get env vars across an RPC-style launcher like
    // `kitty @ launch` — `cmd.envs()` below sets env on the local
    // shell we spawn, but kitty's daemon spawns the actual editor
    // window with its own env, so anything we set on the local sh is
    // lost. By inlining the assignments into the script body, the env
    // travels as part of the command string itself, so even a
    // remote-spawned shell sees the vars.
    let env_prefix = render_env_prefix(env);
    let with_env = template.replace("{env}", &env_prefix);

    if with_env.contains("{file}") {
        let expanded = with_env.replace("{file}", &file.display().to_string());
        cmd.args(["-c", &expanded]);
    } else {
        let escaped = shell_escape(file);
        let script = format!("{} {}", with_env, escaped);
        cmd.args(["-c", &script]);
    }
    // Also propagate via `cmd.envs()` for the non-RPC case (e.g. inline
    // `nvim {file}` template): the spawned editor inherits the local
    // shell's env directly. Empty map = no-op.
    debug_log_env(template, file, env);
    if !env.is_empty() {
        cmd.envs(env);
    }
    cmd
}

/// Render `env` as a shell-escaped assignment prefix:
/// `KEY1="value1" KEY2="value2" ` (sorted by key, with trailing space
/// so callers can write `{env}nvim {file}` without thinking about
/// spacing). Empty map → empty string.
///
/// **Constraint:** the `{env}` token must be placed *inside* an outer
/// `'...'` single-quoted region in the user's template (the standard
/// kitty-RPC pattern: `... sh -c '{env}nvim {file}; …'`). The output
/// is double-quote-wrapped on the inner shell level (so the inner sh
/// parses each assignment correctly), and any single quotes from the
/// values are handled via POSIX close-escape-reopen so the outer
/// `'...'` stays well-formed across the substitution.
///
/// Why double-quotes inside (not single): a single-quoted inner would
/// produce `'...'` chars that immediately close the user's outer
/// `'...'` on substitution, leaving value bytes unquoted at the outer
/// shell level — where spaces split words, `$` expands, etc. The
/// double-quote form leaves outer `'...'` intact because `"` is
/// literal inside `'...'`.
///
/// Public so the script-spawn path can use the same substitution
/// semantics as the editor templates — keeps `{env}` documented in
/// one place.
pub fn render_env_prefix(env: &HashMap<String, String>) -> String {
    if env.is_empty() {
        return String::new();
    }
    let mut keys: Vec<&String> = env.keys().collect();
    keys.sort();
    let mut out = String::new();
    for k in keys {
        let v = env.get(k).expect("key listed from same map");
        // Inner sh sees `KEY="<escaped>"` — `\`, `"`, `$`, backtick
        // are the only chars special inside `"..."`.
        let inner = double_quote_escape(v);
        let wrapped = format!("\"{inner}\"");
        // Outer sh sees the wrapped value inside the user's `'...'`.
        // The only problematic char there is `'`, handled via the
        // POSIX close-escape-reopen idiom. Wrapped values never
        // contain `'` themselves — but the *original* value may have,
        // in which case it surfaces in `wrapped` and needs handling.
        let outer_escaped = outer_single_quote_escape(&wrapped);
        out.push_str(k);
        out.push('=');
        out.push_str(&outer_escaped);
        out.push(' ');
    }
    out
}

/// Escape `s` for embedding inside an inner sh `"..."` region. `\`,
/// `"`, `$`, and backtick are the four chars sh treats specially
/// inside double quotes; everything else (including `'`, whitespace,
/// `;`, `*`, `~`, …) passes through verbatim.
fn double_quote_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '"' | '$' | '`') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Escape `s` for embedding inside an outer sh `'...'` region. POSIX
/// `'...'` cannot contain a single quote at all (even backslash-escaped
/// — inside `'...'` `\` is literal), so each `'` becomes the
/// well-known `'\''` idiom: close the quote, emit a literal `'`,
/// reopen.
fn outer_single_quote_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out
}

/// NYD_DEBUG=1 — mirror the env handed to the editor process (keys only,
/// no values, so we never leak the resolved password to a debug log).
/// No-op if the env var is unset. Writes to the same file as
/// `not_yet_done_content::http_log` (default `/tmp/nyd-debug.log`), but
/// we don't depend on that crate here, so the write is inlined.
fn debug_log_env(template: &str, file: &Path, env: &HashMap<String, String>) {
    if std::env::var_os("NYD_DEBUG").is_none() {
        return;
    }
    let path = std::env::var("NYD_DEBUG_LOG").unwrap_or_else(|_| "/tmp/nyd-debug.log".to_string());
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let mut keys: Vec<&String> = env.keys().collect();
    keys.sort();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let _ = writeln!(
        f,
        "ts={} DEBUG open_editor.build_command: template={:?} file={:?} env.len={} keys={:?}",
        ts,
        template,
        file.display().to_string(),
        env.len(),
        keys
    );
}

fn shell_escape(path: &Path) -> String {
    let s = path.display().to_string();
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn read_file(path: &Path) -> Result<String, EditorError> {
    let mut content = String::new();
    std::fs::File::open(path)?.read_to_string(&mut content)?;
    Ok(content)
}

/// Create a [`NamedTempFile`] with optional file extension suffix.
///
/// When `dir` is `Some`, the temp file is created in that directory
/// instead of `$TMPDIR`. This is used by the in-place edit mode (e.g.
/// for DB scripts that need to live in their real directory so LSPs
/// can discover sibling config files like `postgres-language-server.jsonc`).
/// `prefix` (e.g. `".nyd_tmp_"`) tags the file as ours so the user can
/// recognise and clean up stragglers after a crash.
fn create_tmpfile(
    suffix: Option<&str>,
    initial_content: &str,
    dir: Option<&Path>,
    prefix: Option<&str>,
) -> Result<NamedTempFile, EditorError> {
    let mut builder = tempfile::Builder::new();
    if let Some(s) = suffix {
        builder.suffix(s);
    }
    if let Some(p) = prefix {
        builder.prefix(p);
    }
    let mut tmpfile = match dir {
        Some(d) => {
            std::fs::create_dir_all(d)?;
            builder.tempfile_in(d)?
        }
        None => builder.tempfile()?,
    };
    tmpfile.write_all(initial_content.as_bytes())?;
    tmpfile.flush()?;
    Ok(tmpfile)
}

/// Resolve the file the editor will operate on.
///
/// `persistent = Some(path)` opts into **materialised** editing: the buffer
/// lives at exactly `path`, is created (parents included), seeded with
/// `initial_content`, and — crucially — is *not* wrapped in a
/// [`NamedTempFile`], so it survives after the editor closes. The returned
/// `Option<NamedTempFile>` is `None` in that case.
///
/// `persistent = None` reproduces the classic throwaway temp-file behaviour
/// via [`create_tmpfile`]; the returned guard must be kept alive for the
/// editor's lifetime.
fn resolve_edit_target(
    suffix: Option<&str>,
    initial_content: &str,
    dir: Option<&Path>,
    prefix: Option<&str>,
    persistent: Option<&Path>,
) -> Result<(PathBuf, Option<NamedTempFile>), EditorError> {
    match persistent {
        Some(path) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, initial_content.as_bytes())?;
            Ok((path.to_owned(), None))
        }
        None => {
            let tmp = create_tmpfile(suffix, initial_content, dir, prefix)?;
            let path = tmp.path().to_owned();
            Ok((path, Some(tmp)))
        }
    }
}

fn pause_tui() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    Ok(())
}

fn resume_tui() -> io::Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Open an editor **inline** — ratatui pauses, the editor runs in the
/// same terminal, and ratatui resumes afterwards.
///
/// `suffix` sets the temp file extension (e.g. `Some(".md")`) for syntax
/// highlighting in the editor.
pub fn open_editor_inline(
    command: Option<&str>,
    initial_content: &str,
    suffix: Option<&str>,
) -> Result<String, EditorError> {
    open_editor_inline_in(
        command,
        initial_content,
        suffix,
        None,
        None,
        &HashMap::new(),
        None,
    )
}

/// Variant of [`open_editor_inline`] that places the temp file in a
/// caller-supplied directory with a caller-supplied filename prefix,
/// and propagates `env` to the spawned editor.
///
/// `dir = None`, `prefix = None`, and an empty `env` reproduce the
/// default behavior. `env` is intended for adapter-owned credentials
/// (e.g. libpq `PG*` vars) so an editor-spawned LSP sees the same
/// backend the TUI is connected to.
pub fn open_editor_inline_in(
    command: Option<&str>,
    initial_content: &str,
    suffix: Option<&str>,
    dir: Option<&Path>,
    prefix: Option<&str>,
    env: &HashMap<String, String>,
    persistent: Option<&Path>,
) -> Result<String, EditorError> {
    // `_tmpfile` guard is kept alive until the function returns (temp mode);
    // in persistent mode it is `None` and the file lives on at `path`.
    let (path, _tmpfile) = resolve_edit_target(suffix, initial_content, dir, prefix, persistent)?;

    pause_tui()?;

    let template = resolve_command(command);
    let status = build_command(&template, &path, env).status()?;

    let restore_result = resume_tui();
    restore_result?;

    if !status.success() {
        return Err(EditorError::EditorFailed(status));
    }

    read_file(&path)
}

/// Launch an editor command that returns immediately (e.g. `kitty @ launch`)
/// while keeping a handle to poll for completion.
///
/// Ratatui is briefly paused so the command has clean terminal access,
/// then immediately resumed.
pub fn open_editor_launch(
    command: Option<&str>,
    initial_content: &str,
    suffix: Option<&str>,
) -> Result<DetachedEditor, EditorError> {
    open_editor_launch_in(
        command,
        initial_content,
        suffix,
        None,
        None,
        &HashMap::new(),
        None,
    )
}

pub fn open_editor_launch_in(
    command: Option<&str>,
    initial_content: &str,
    suffix: Option<&str>,
    dir: Option<&Path>,
    prefix: Option<&str>,
    env: &HashMap<String, String>,
    persistent: Option<&Path>,
) -> Result<DetachedEditor, EditorError> {
    let (path, tmpfile) = resolve_edit_target(suffix, initial_content, dir, prefix, persistent)?;
    let persistent_path = persistent.map(Path::to_owned);
    let done_path = done_path_for(&path);

    pause_tui()?;

    let template = resolve_command(command);
    let status = build_command(&template, &path, env).status();

    let restore_result = resume_tui();
    restore_result?;

    let status = status?;
    if !status.success() {
        return Err(EditorError::EditorFailed(status));
    }

    let mtime = std::fs::metadata(&path)
        .ok()
        .and_then(|m| m.modified().ok());
    Ok(DetachedEditor {
        file_path: path,
        done_path,
        last_mtime: mtime,
        _tmpfile: tmpfile,
        persistent_path,
    })
}

/// Spawn an editor as a **detached** process — ratatui is NOT paused.
/// Stdio is redirected to null.
pub fn open_editor_detached(
    command: Option<&str>,
    initial_content: &str,
    suffix: Option<&str>,
) -> Result<DetachedEditor, EditorError> {
    open_editor_detached_in(
        command,
        initial_content,
        suffix,
        None,
        None,
        &HashMap::new(),
        None,
    )
}

pub fn open_editor_detached_in(
    command: Option<&str>,
    initial_content: &str,
    suffix: Option<&str>,
    dir: Option<&Path>,
    prefix: Option<&str>,
    env: &HashMap<String, String>,
    persistent: Option<&Path>,
) -> Result<DetachedEditor, EditorError> {
    let (path, tmpfile) = resolve_edit_target(suffix, initial_content, dir, prefix, persistent)?;
    let persistent_path = persistent.map(Path::to_owned);
    let done_path = done_path_for(&path);

    let template = resolve_command(command);
    let _child = build_command(&template, &path, env)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()?;

    let mtime = std::fs::metadata(&path)
        .ok()
        .and_then(|m| m.modified().ok());
    Ok(DetachedEditor {
        file_path: path,
        done_path,
        last_mtime: mtime,
        _tmpfile: tmpfile,
        persistent_path,
    })
}

fn done_path_for(path: &Path) -> PathBuf {
    let mut done = path.as_os_str().to_owned();
    done.push(".done");
    PathBuf::from(done)
}

/// Handle to a launched editor process running in a separate window/split.
pub struct DetachedEditor {
    file_path: PathBuf,
    done_path: PathBuf,
    /// Last known mtime — used to detect saves.
    last_mtime: Option<std::time::SystemTime>,
    /// Temp-file guard: `Some` in throwaway mode (deleted on drop), `None`
    /// in persistent mode (the file at [`Self::file_path`] must survive).
    _tmpfile: Option<NamedTempFile>,
    /// When set, this editor is materialised: the buffer must persist at
    /// this path after the editor closes. Because the detached editor
    /// template typically `mv`s the file to `{file}.done` on exit, cleanup
    /// restores the final content back to this path instead of deleting it.
    persistent_path: Option<PathBuf>,
}

impl DetachedEditor {
    /// Has the editor signalled completion via `{file}.done`?
    pub fn is_done(&self) -> bool {
        self.done_path.exists()
    }

    /// Read the final content from `{file}.done`.
    pub fn read_content(&self) -> Result<String, EditorError> {
        read_file(&self.done_path)
    }

    /// Clean up after the editor closed and its content was read.
    ///
    /// Throwaway mode: delete the `{file}.done` marker (the temp file itself
    /// is removed when [`Self::_tmpfile`] drops).
    ///
    /// Persistent mode: the editor template `mv`d the real file to
    /// `{file}.done`, so the persisted path no longer exists. Restore it by
    /// renaming `.done` back to the real path (which also removes the marker),
    /// leaving the final saved content at [`Self::persistent_path`]. If the
    /// rename fails (e.g. the template `cp`'d instead of `mv`d, so the marker
    /// and the real file coexist), fall back to removing the marker.
    pub fn cleanup(&self) {
        match &self.persistent_path {
            Some(dst) => {
                if std::fs::rename(&self.done_path, dst).is_err() {
                    let _ = std::fs::remove_file(&self.done_path);
                }
            }
            None => {
                let _ = std::fs::remove_file(&self.done_path);
            }
        }
    }

    /// Check if the file has been modified since the last check (i.e. `:w`).
    /// Returns `true` once per save, then resets.
    pub fn has_changed(&mut self) -> bool {
        let Ok(meta) = std::fs::metadata(&self.file_path) else {
            return false;
        };
        let Ok(mtime) = meta.modified() else {
            return false;
        };
        if self.last_mtime == Some(mtime) {
            return false;
        }
        self.last_mtime = Some(mtime);
        true
    }

    /// Read the current (live) content of the file being edited.
    pub fn read_live_content(&self) -> Result<String, EditorError> {
        read_file(&self.file_path)
    }

    /// Overwrite the file with new content (e.g. with error annotations).
    pub fn write_file(&self, content: &str) -> Result<(), EditorError> {
        use std::io::Write;
        let mut f = std::fs::File::create(&self.file_path)?;
        f.write_all(content.as_bytes())?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Legacy compat
// ---------------------------------------------------------------------------

pub fn open_editor(editor: Option<&str>, initial_content: &str) -> Result<String, EditorError> {
    open_editor_inline(editor, initial_content, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dq_escape_plain() {
        assert_eq!(double_quote_escape("foo"), "foo");
        assert_eq!(double_quote_escape(""), "");
    }

    #[test]
    fn dq_escape_specials() {
        assert_eq!(double_quote_escape("a\\b"), "a\\\\b");
        assert_eq!(double_quote_escape("a\"b"), "a\\\"b");
        assert_eq!(double_quote_escape("a$b"), "a\\$b");
        assert_eq!(double_quote_escape("a`b"), "a\\`b");
        // single quote and space pass through verbatim — outer
        // `'...'` is what we worry about, not the inner `"..."`.
        assert_eq!(double_quote_escape("a'b c;d"), "a'b c;d");
    }

    #[test]
    fn outer_sq_escape_plain() {
        assert_eq!(outer_single_quote_escape("foo"), "foo");
    }

    #[test]
    fn outer_sq_escape_single_quote() {
        assert_eq!(outer_single_quote_escape("a'b"), "a'\\''b");
    }

    #[test]
    fn env_prefix_empty() {
        let env: HashMap<String, String> = HashMap::new();
        assert_eq!(render_env_prefix(&env), "");
    }

    #[test]
    fn env_prefix_sorted_with_double_quotes() {
        let mut env = HashMap::new();
        env.insert("PGPORT".into(), "5432".into());
        env.insert("PGHOST".into(), "h".into());
        assert_eq!(render_env_prefix(&env), "PGHOST=\"h\" PGPORT=\"5432\" ");
    }

    #[test]
    fn env_prefix_password_with_dollar() {
        // The bug we shipped 632b191 with: outer sh expanded `$$` to
        // the PID before the value ever reached the inner sh's
        // assignment. Double-quote wrapping + `\$` escape ensures
        // inner sh sees a literal `$` after its own `"..."` parsing.
        let mut env = HashMap::new();
        env.insert("PGPASSWORD".into(), "pa$$word".into());
        assert_eq!(render_env_prefix(&env), "PGPASSWORD=\"pa\\$\\$word\" ");
    }

    #[test]
    fn env_prefix_password_with_single_quote() {
        // The other 632b191 bug: a literal `'` in the value closed
        // the user template's outer `'...'`, letting subsequent text
        // get re-parsed at the wrong shell level. Close-escape-reopen
        // around the wrapped value preserves the outer quoting.
        let mut env = HashMap::new();
        env.insert("PGPASSWORD".into(), "it's".into());
        assert_eq!(render_env_prefix(&env), "PGPASSWORD=\"it'\\''s\" ");
    }

    #[test]
    fn env_prefix_password_with_space() {
        let mut env = HashMap::new();
        env.insert("PGPASSWORD".into(), "with space".into());
        assert_eq!(render_env_prefix(&env), "PGPASSWORD=\"with space\" ");
    }
}
