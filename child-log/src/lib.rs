//! A child process's stderr, in a file instead of on the terminal.
//!
//! A long-running child says things while it runs — a browser its Chromium
//! warnings, a helper its progress. There are three places to put that, and
//! two of them are wrong:
//!
//! - **Inherit the parent's stderr.** Free, and it works right up until the
//!   parent owns the screen. A TUI in the alternate screen buffer gets the
//!   child's lines written across its own, and nothing it draws afterwards
//!   repairs them.
//! - **A pipe.** Correct only while somebody reads it. A parent that is busy
//!   waiting for the child to do its actual job is not reading, and a pipe
//!   nobody drains blocks the child once the buffer fills — so the cure stops
//!   the thing it was meant to observe.
//! - **A file.** Nobody has to drain it, `tail -f` reads it while the child
//!   runs, it survives the child, and the path can be named in the error the
//!   parent reports. That is what this crate hands out.
//!
//! The one cost of a file is that it grows: a child's stderr is append-only
//! and nobody trims it. So the file is set aside before the child starts if
//! it has grown past [`ChildLog::roll_at`], and exactly one generation is
//! kept, as `<name>.log.1` — the interesting window is always the last two
//! runs, and a chain of ten is just a disk filling up more slowly.
//!
//! ```no_run
//! use child_log::ChildLog;
//!
//! let log = ChildLog::at("/tmp/demo/browser.log");
//! let child = std::process::Command::new("some-browser")
//!     .stdin(std::process::Stdio::null())
//!     .stderr(log.stdio())
//!     .spawn()?;
//! # let _ = child;
//! eprintln!("it says what it is doing in {}", log.path().display());
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! Every step is best-effort, because a log is not the job: a file that
//! cannot be rolled over is appended to anyway, and one that cannot be opened
//! at all becomes [`Stdio::null()`] rather than a reason to refuse to start
//! the child.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// How large a log may grow before a starting child sets it aside, unless
/// [`ChildLog::roll_at`] says otherwise. Large enough to hold a noisy run
/// whole, small enough that two of them are not a problem.
pub const DEFAULT_ROLL_AT: u64 = 8 * 1024 * 1024;

/// A file to point a child's stderr (or stdout) at.
///
/// Holding the path rather than an open file is deliberate: the path is what
/// the parent needs for the error message it may have to write, it stays
/// valid across several starts of the same child, and the file itself is
/// opened — and rolled over — at the moment a child is actually started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildLog {
    path: PathBuf,
    roll_at: u64,
}

impl ChildLog {
    /// The log at `path`. Its directory is created when the file is opened,
    /// not now, so building one is free and cannot fail.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            roll_at: DEFAULT_ROLL_AT,
        }
    }

    /// The log of a child called `name`, in `dir`: `<dir>/<name>.log`, with
    /// the name reduced to what a file name is happy with.
    ///
    /// Here rather than at every call site, because the name a parent knows
    /// its child by is usually one a person typed — into a config file, on a
    /// command line — and is therefore free to contain a slash.
    pub fn named(dir: impl AsRef<Path>, name: &str) -> Self {
        Self::at(dir.as_ref().join(format!("{}.log", file_safe(name))))
    }

    /// Set the file aside at a different size than [`DEFAULT_ROLL_AT`].
    #[must_use]
    pub fn roll_at(mut self, bytes: u64) -> Self {
        self.roll_at = bytes;
        self
    }

    /// Where it is — for the error message that has to tell somebody where to
    /// look.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Where the kept previous generation is, once there has been one.
    pub fn previous(&self) -> PathBuf {
        self.path.with_extension("log.1")
    }

    /// Roll the file over if it has grown too large, then open it for
    /// appending, creating its directory if need be.
    ///
    /// Use this when a child that cannot be observed is worse than no child;
    /// [`stdio`](Self::stdio) is the usual way, and swallows the failure.
    pub fn open(&self) -> std::io::Result<File> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        self.roll_over();
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
    }

    /// The same file as a child's `stderr` — or [`Stdio::null()`] if it
    /// cannot be opened, because a log that fails is not a start that fails.
    pub fn stdio(&self) -> Stdio {
        self.open()
            .map(Stdio::from)
            .unwrap_or_else(|_| Stdio::null())
    }

    /// Set the file aside if it has outgrown [`roll_at`](Self::roll_at), so
    /// the child about to start appends to an empty one. The kept generation
    /// is overwritten, never chained.
    fn roll_over(&self) {
        let too_big = std::fs::metadata(&self.path).is_ok_and(|m| m.len() > self.roll_at);
        if too_big {
            let _ = std::fs::rename(&self.path, self.previous());
        }
    }
}

/// `name`, reduced to what a file name is happy with: everything that is not
/// a letter, a digit, a dash or an underscore becomes a dash, and a run of
/// them collapses into one. An empty result becomes `child`, so there is
/// always a file to open.
fn file_safe(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "child".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn a_small_log_is_left_alone_and_a_large_one_is_set_aside() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let log = ChildLog::at(dir.path().join("child.log")).roll_at(16);
        let kept = dir.path().join("child.log.1");

        std::fs::write(log.path(), b"short").expect("write");
        log.open().expect("opened");
        assert_eq!(std::fs::read(log.path()).expect("still there"), b"short");
        assert!(!kept.exists(), "nothing to keep yet");

        std::fs::write(log.path(), vec![b'x'; 17]).expect("write");
        log.open().expect("opened");
        assert_eq!(std::fs::metadata(&kept).expect("kept").len(), 17);
        assert_eq!(
            std::fs::metadata(log.path()).expect("a fresh one").len(),
            0,
            "the child appends to an empty file"
        );
    }

    #[test]
    fn the_kept_generation_is_overwritten_rather_than_chained() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let log = ChildLog::at(dir.path().join("child.log")).roll_at(4);

        std::fs::write(log.path(), vec![b'x'; 5]).expect("write");
        log.open().expect("opened");
        std::fs::write(log.path(), vec![b'y'; 6]).expect("write");
        log.open().expect("opened");

        assert_eq!(std::fs::metadata(log.previous()).expect("kept").len(), 6);
        assert!(!dir.path().join("child.log.1.1").exists());
    }

    #[test]
    fn opening_makes_the_directory_and_appends_to_what_is_there() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let log = ChildLog::at(dir.path().join("logs").join("child.log"));

        write!(log.open().expect("first"), "one ").expect("wrote");
        write!(log.open().expect("second"), "two").expect("wrote");

        assert_eq!(
            std::fs::read_to_string(log.path()).expect("read"),
            "one two"
        );
    }

    #[test]
    fn a_childs_name_becomes_a_file_name() {
        let dir = Path::new("/var/log/nyd");
        assert_eq!(
            ChildLog::named(dir, "jira-login").path(),
            Path::new("/var/log/nyd/jira-login.log")
        );
        assert_eq!(
            ChildLog::named(dir, "my browser/2").path(),
            Path::new("/var/log/nyd/my-browser-2.log"),
            "a name a person typed cannot reach outside the directory"
        );
        assert_eq!(
            ChildLog::named(dir, "../..").path(),
            Path::new("/var/log/nyd/child.log")
        );
    }

    #[test]
    fn a_log_that_cannot_be_opened_is_not_a_reason_to_refuse_a_child() {
        let dir = tempfile::tempdir().expect("a temp dir");
        // A directory where the file should be: opening it can only fail.
        let path = dir.path().join("child.log");
        std::fs::create_dir(&path).expect("in the way");
        let log = ChildLog::at(&path);

        assert!(log.open().is_err());
        let _: Stdio = log.stdio();
    }
}
