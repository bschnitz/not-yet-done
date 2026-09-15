//! Which unit file wins — and what it hides.
//!
//! `ListUnitFiles` reports one file per unit name: the winner. It says nothing
//! about the loser, and the loser is the interesting half of the question one
//! actually has after a package update — *my* `~/.config` copy is in force and
//! the distribution's newer file underneath it is not being read at all. That
//! is a silent failure mode: the unit works, it is simply not the unit the
//! package ships.
//!
//! So this module walks the search path itself and records every file it finds
//! under each name. Two files with the same name mean one is shadowing the
//! other, and the `shadows` column names the file that lost.
//!
//! The search path comes from `systemd-analyze unit-paths` rather than a
//! constant: it is eighteen directories on a normal login, several of them
//! under `/run/user/<uid>`, and two of them are generator output that exists
//! only for this boot. Hard-coding that list would mean maintaining systemd's
//! lookup order in a second place, and getting it wrong silently.
//!
//! Which scope is asked matters as much as asking: `--user` and `--system`
//! return two disjoint sets of directories, and running the user question
//! against system units does not produce a wrong shadow — it produces *no*
//! shadow, ever, which is the failure mode this module exists to catch.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::config::Manager;

/// Every unit file in the search path, grouped by name, in priority order.
#[derive(Default)]
pub struct SearchPath {
    by_name: HashMap<String, Vec<PathBuf>>,
}

impl SearchPath {
    /// Walk this manager's unit search path.
    ///
    /// Rebuilt per listing rather than cached: it is eighteen `readdir` calls
    /// and the answer changes whenever the user writes a unit — which, in this
    /// tab, they do.
    pub async fn load_for(manager: Manager) -> Self {
        let mut by_name: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for dir in unit_paths(manager).await {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                // Drop-in directories (`foo.service.d`) are not unit files, and
                // `.wants`/`.requires` hold the enablement symlinks rather than
                // units.
                if entry.path().is_dir() {
                    continue;
                }
                by_name.entry(name).or_default().push(entry.path());
            }
        }
        Self { by_name }
    }

    /// The highest-priority file this one hides, if any.
    ///
    /// `winner` is the path `ListUnitFiles` reported, and everything after it
    /// in the list is shadowed. Reporting only the first is deliberate: a
    /// column shows one path, and the one that matters is the file that *would*
    /// be in force if this one went away.
    pub fn shadowed(&self, name: &str, winner: &str) -> String {
        let Some(paths) = self.by_name.get(name) else {
            return String::new();
        };
        // A winner that is not in our list at all — a generated unit in a
        // directory we could not read — reports nothing rather than guessing
        // that the first file we did find is the one it hides.
        let Some(at) = paths.iter().position(|p| p.as_os_str() == winner) else {
            return String::new();
        };
        paths
            .get(at + 1)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// The search path, highest priority first, as systemd itself reports it.
///
/// A failure here costs the `shadows` column and nothing else: no search path
/// means no file is known to shadow another, which reads as "nothing to report"
/// rather than as a wrong answer.
async fn unit_paths(manager: Manager) -> Vec<PathBuf> {
    let scope = match manager {
        Manager::User => "--user",
        Manager::System => "--system",
    };
    let out = tokio::process::Command::new("systemd-analyze")
        .args([scope, "unit-paths"])
        .output()
        .await;
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_with(files: &[(&str, &str)]) -> (tempfile::TempDir, SearchPath) {
        let dir = tempfile::tempdir().unwrap();
        let mut by_name: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for (sub, name) in files {
            let d = dir.path().join(sub);
            std::fs::create_dir_all(&d).unwrap();
            let p = d.join(name);
            std::fs::write(&p, "[Unit]\n").unwrap();
            by_name.entry((*name).into()).or_default().push(p);
        }
        (dir, SearchPath { by_name })
    }

    #[test]
    fn a_single_file_shadows_nothing() {
        let (_d, sp) = path_with(&[("usr", "foo.service")]);
        let winner = _d.path().join("usr/foo.service");
        assert_eq!(sp.shadowed("foo.service", winner.to_str().unwrap()), "");
    }

    #[test]
    fn the_winner_names_the_file_underneath_it() {
        let (_d, sp) = path_with(&[("home", "foo.service"), ("usr", "foo.service")]);
        let winner = _d.path().join("home/foo.service");
        let hidden = _d.path().join("usr/foo.service");
        assert_eq!(
            sp.shadowed("foo.service", winner.to_str().unwrap()),
            hidden.to_string_lossy()
        );
    }

    #[test]
    fn only_what_comes_after_the_winner_counts_as_shadowed() {
        // The lowest-priority file is in force (the ones above it are for a
        // different unit name in reality; here it stands for a winner that is
        // last). Nothing is below it, so nothing is shadowed.
        let (_d, sp) = path_with(&[("home", "foo.service"), ("usr", "foo.service")]);
        let winner = _d.path().join("usr/foo.service");
        assert_eq!(sp.shadowed("foo.service", winner.to_str().unwrap()), "");
    }

    #[test]
    fn a_winner_we_never_saw_reports_nothing_rather_than_guessing() {
        let (_d, sp) = path_with(&[("usr", "foo.service")]);
        assert_eq!(sp.shadowed("foo.service", "/run/generated/foo.service"), "");
        assert_eq!(sp.shadowed("unknown.service", "/anywhere"), "");
    }
}
