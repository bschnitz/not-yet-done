//! What the distribution *meant* a unit's enablement to be — and where the
//! machine has drifted away from it.
//!
//! # Why this is a parser and not a property read
//!
//! Every other column in this adapter is something systemd hands over. The
//! preset is not: there is no `GetUnitFilePreset` on the manager interface, and
//! `UnitFilePreset` exists only as a property of an already-*loaded* unit —
//! precisely the population the unit-files level exists to look past. A unit
//! that has never been started has no object path to ask.
//!
//! `systemctl` has the same problem and solves it the same way: it walks the
//! preset directories itself and answers locally. So does this module, and the
//! rules it follows are `systemd.preset(5)`'s, not an approximation of them:
//!
//! * Four directories, highest priority first — `/etc`, `/run`,
//!   `/usr/local/lib`, `/usr/lib`. A file in a higher one **replaces** the
//!   same-named file in a lower one rather than adding to it.
//! * All surviving files are then sorted by **filename**, regardless of which
//!   directory they came from, which is why everyone prefixes them `50-`.
//! * Within that order, the **first matching line wins** — later lines about
//!   the same unit are dead.
//! * A symlink to `/dev/null` in `/etc` switches a vendor file off entirely.
//! * A unit no line matches is **enabled**. That is systemd's default, and it
//!   is the reason [`drift`] is loud on a distribution that ships no
//!   default-off preset: `systemctl preset-all` really would switch those on.
//!
//! # What it is for
//!
//! Two columns and one verb. `preset` says what the policy wants, `drift` says
//! how the unit disagrees with it, and `preset` the verb finally has a visible
//! meaning — before this module it did something you could not predict from the
//! row in front of you, which is why phase 1 left it out.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobMatcher};

use crate::config::Manager;

/// The preset directories for *user* units, highest priority first.
const USER_PRESET_DIRS: &[&str] = &[
    "/etc/systemd/user-preset",
    "/run/systemd/user-preset",
    "/usr/local/lib/systemd/user-preset",
    "/usr/lib/systemd/user-preset",
];

/// The same four directories for *system* units.
///
/// Not a detail: the two policies routinely disagree by default. A
/// distribution that ships `disable *` for system units — most do — while
/// leaving no user-preset file at all means reading the wrong one turns every
/// disabled system unit into a unit that "should be enabled". An empty policy
/// is not a neutral fallback here; it is the opposite answer.
const SYSTEM_PRESET_DIRS: &[&str] = &[
    "/etc/systemd/system-preset",
    "/run/systemd/system-preset",
    "/usr/local/lib/systemd/system-preset",
    "/usr/lib/systemd/system-preset",
];

/// What the preset policy says about a unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Enable,
    Disable,
    /// "Leave whatever is there alone" — a unit with this preset can never
    /// drift, because the policy has no opinion to drift from.
    Ignore,
}

impl Preset {
    /// The word the column shows.
    ///
    /// Past tense, matching the `state` column it sits next to: comparing
    /// `enabled` with `enable` reads like a typo, and the two columns are read
    /// against each other or not at all. `systemctl list-unit-files` prints the
    /// same word in its own PRESET column.
    pub fn word(self) -> &'static str {
        match self {
            Preset::Enable => "enabled",
            Preset::Disable => "disabled",
            Preset::Ignore => "ignored",
        }
    }

    fn from_verb(verb: &str) -> Option<Self> {
        match verb {
            "enable" => Some(Preset::Enable),
            "disable" => Some(Preset::Disable),
            "ignore" => Some(Preset::Ignore),
            _ => None,
        }
    }
}

/// One `enable`/`disable`/`ignore` line.
struct Rule {
    verb: Preset,
    /// The pattern itself, plus one literal matcher per instance name listed
    /// after it (`enable getty@.service tty1 tty2`).
    matchers: Vec<GlobMatcher>,
}

impl Rule {
    fn matches(&self, unit: &str) -> bool {
        self.matchers.iter().any(|m| m.is_match(unit))
    }
}

/// The merged preset policy — every rule of every file, in the order systemd
/// would consult them.
#[derive(Default)]
pub struct Policy {
    rules: Vec<Rule>,
}

impl Policy {
    /// Read this manager's preset policy off this machine.
    ///
    /// A missing directory is not an error — most machines have one preset file
    /// in `/usr/lib` and nothing else. No policy at all means every unit's
    /// preset is `enable`, which is what systemd would do too; which is exactly
    /// why the manager has to pick the right directories rather than default to
    /// one of them. See [`SYSTEM_PRESET_DIRS`].
    pub fn load_for(manager: Manager) -> Self {
        let dirs = match manager {
            Manager::User => USER_PRESET_DIRS,
            Manager::System => SYSTEM_PRESET_DIRS,
        };
        Self::load_from(&dirs.iter().map(Path::new).collect::<Vec<_>>())
    }

    /// The same, from an explicit list of directories — highest priority first.
    pub fn load_from(dirs: &[&Path]) -> Self {
        let files = merge(dirs);
        Self::parse(
            files
                .into_iter()
                .filter_map(|path| std::fs::read_to_string(&path).ok()),
        )
    }

    /// Build a policy from file contents already in the order systemd would
    /// read them. The seam the tests use, and the reason [`merge`] is a
    /// function of its own.
    pub fn parse(files: impl IntoIterator<Item = String>) -> Self {
        let mut rules = Vec::new();
        for text in files {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                    continue;
                }
                let mut words = line.split_whitespace();
                let Some(verb) = words.next().and_then(Preset::from_verb) else {
                    // Not a directive we know. systemd warns and carries on;
                    // so do we, because one bad line must not void a policy.
                    continue;
                };
                let Some(pattern) = words.next() else {
                    continue;
                };
                let mut matchers = Vec::new();
                if let Some(m) = matcher(pattern) {
                    matchers.push(m);
                }
                // `enable foo@.service a b` means foo@a.service and foo@b.service.
                for instance in words {
                    if let Some(name) = instantiate(pattern, instance)
                        && let Some(m) = matcher(&name)
                    {
                        matchers.push(m);
                    }
                }
                if !matchers.is_empty() {
                    rules.push(Rule { verb, matchers });
                }
            }
        }
        Self { rules }
    }

    /// What the policy says about one unit — first match wins, `enable` when
    /// nothing matches.
    pub fn query(&self, unit: &str) -> Preset {
        self.rules
            .iter()
            .find(|r| r.matches(unit))
            .map_or(Preset::Enable, |r| r.verb)
    }
}

/// Compile one shell-style pattern. `/` never appears in a unit name, so the
/// path semantics globset would otherwise apply are irrelevant here.
fn matcher(pattern: &str) -> Option<GlobMatcher> {
    Glob::new(pattern).ok().map(|g| g.compile_matcher())
}

/// `foo@.service` + `tty1` → `foo@tty1.service`.
fn instantiate(pattern: &str, instance: &str) -> Option<String> {
    let (head, tail) = pattern.split_once("@.")?;
    Some(format!("{head}@{instance}.{tail}"))
}

/// The preset files that survive the merge, in the order they are read.
///
/// Same name in two directories: the higher-priority one wins outright, which
/// is how `/etc` overrides a vendor file. A symlink to `/dev/null` is how the
/// administrator switches one off, and it survives the merge as the winner and
/// is then dropped — dropping it earlier would let the vendor file underneath
/// come back.
fn merge(dirs: &[&Path]) -> Vec<PathBuf> {
    let mut winners: BTreeMap<String, PathBuf> = BTreeMap::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".preset") {
                continue;
            }
            winners.entry(name).or_insert_with(|| entry.path());
        }
    }
    // BTreeMap already holds them in lexicographic filename order — the order
    // systemd reads them in, across directories.
    winners.into_values().filter(|p| !is_devnull(p)).collect()
}

fn is_devnull(path: &Path) -> bool {
    std::fs::read_link(path).is_ok_and(|t| t == Path::new("/dev/null"))
}

// ---------------------------------------------------------------------------
// Drift
// ---------------------------------------------------------------------------

/// Whether a unit-file state can be compared with a preset at all.
///
/// A `static` unit has no `[Install]` section to enable, a `generated` or
/// `transient` one has no file anybody wrote, and an `alias` is another unit
/// under a second name. `systemctl list-unit-files` prints `-` in its preset
/// column for exactly these; this returns `false` for them and the cell stays
/// empty, which says the same thing more quietly.
pub fn preset_applies(state: &str) -> bool {
    !matches!(
        state,
        "static" | "transient" | "generated" | "alias" | "bad" | ""
    )
}

/// How a unit disagrees with the policy — **named as the fix**, not as the
/// complaint.
///
/// The value is what `systemctl preset` would do to this unit, so a row that
/// says `should-disable` is a row where pressing the preset verb disables it.
/// That is also why the empty string is the common case: a machine in line with
/// its distribution has an empty column, and the audit is "sort by drift".
///
/// Only `enabled` and `disabled` can drift. `indirect` (enabled through another
/// unit's `Also=`) has a preset but no enablement of its own to compare, and
/// everything [`preset_applies`] rejects has neither.
pub fn drift(state: &str, preset: Preset) -> &'static str {
    match (state, preset) {
        ("enabled" | "enabled-runtime", Preset::Disable) => "should-disable",
        ("disabled", Preset::Enable) => "should-enable",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two scopes must not be able to collapse into one another.
    ///
    /// This is the shape of a bug that shipped: the system tab read the *user*
    /// preset directories, found nothing there, and an empty policy means
    /// "enable everything" — so 177 units the distribution wants off were
    /// reported as drifting towards on. An empty policy is not a neutral
    /// answer, which is why the directories are picked by manager and not
    /// defaulted to.
    #[test]
    fn each_manager_reads_its_own_preset_directories() {
        let dirs_for = |m| match m {
            Manager::User => USER_PRESET_DIRS,
            Manager::System => SYSTEM_PRESET_DIRS,
        };
        assert!(
            dirs_for(Manager::User)
                .iter()
                .all(|d| d.ends_with("user-preset"))
        );
        assert!(
            dirs_for(Manager::System)
                .iter()
                .all(|d| d.ends_with("system-preset"))
        );
        // Same four locations, same priority order, different last segment.
        assert_eq!(
            dirs_for(Manager::User).len(),
            dirs_for(Manager::System).len()
        );
        for (u, s) in dirs_for(Manager::User)
            .iter()
            .zip(dirs_for(Manager::System))
        {
            assert_eq!(
                u.trim_end_matches("user-preset"),
                s.trim_end_matches("system-preset")
            );
        }
    }

    /// What the system policy of a typical distribution actually says, and what
    /// it must produce: `disable *` plus a named exception list.
    #[test]
    fn a_disable_star_policy_leaves_a_disabled_unit_undrifted() {
        let policy = Policy::parse([
            "enable getty@.service\nenable remote-fs.target\n".to_string(),
            "disable *\n".to_string(),
        ]);
        assert_eq!(policy.query("accounts-daemon.service"), Preset::Disable);
        assert_eq!(drift("disabled", Preset::Disable), "");
        // And the exception still wins, because it is read first.
        assert_eq!(policy.query("getty@.service"), Preset::Enable);
        assert_eq!(drift("disabled", Preset::Enable), "should-enable");
    }

    fn policy(text: &str) -> Policy {
        Policy::parse([text.to_string()])
    }

    #[test]
    fn an_unmatched_unit_is_enabled_because_that_is_systemds_default() {
        let p = policy("enable foo.service\n");
        assert_eq!(p.query("bar.service"), Preset::Enable);
        // And with no policy at all.
        assert_eq!(Policy::default().query("bar.service"), Preset::Enable);
    }

    #[test]
    fn the_first_matching_line_wins_and_the_rest_are_dead() {
        let p = policy("enable foo.service\ndisable *\ndisable foo.service\n");
        assert_eq!(p.query("foo.service"), Preset::Enable);
        // The catchall still governs everything the first line did not claim.
        assert_eq!(p.query("bar.service"), Preset::Disable);
    }

    #[test]
    fn comments_blank_lines_and_nonsense_do_not_void_a_policy() {
        let p = policy("# a comment\n; another\n\n  \nnonsense here\ndisable *\n");
        assert_eq!(p.query("anything.service"), Preset::Disable);
    }

    #[test]
    fn wildcards_are_shell_style() {
        let p = policy("disable avahi-daemon.*\nenable *.timer\n");
        assert_eq!(p.query("avahi-daemon.socket"), Preset::Disable);
        assert_eq!(p.query("avahi-daemon.service"), Preset::Disable);
        assert_eq!(p.query("avahi.service"), Preset::Enable);
    }

    #[test]
    fn instance_names_after_a_template_enable_those_instances() {
        let p = policy("enable dirsrv@.service foo bar\ndisable *\n");
        assert_eq!(p.query("dirsrv@foo.service"), Preset::Enable);
        assert_eq!(p.query("dirsrv@bar.service"), Preset::Enable);
        // An instance nobody listed falls through to the catchall.
        assert_eq!(p.query("dirsrv@baz.service"), Preset::Disable);
    }

    #[test]
    fn files_are_read_in_filename_order_across_directories() {
        let dir = tempfile::tempdir().unwrap();
        let etc = dir.path().join("etc");
        let usr = dir.path().join("usr");
        std::fs::create_dir_all(&etc).unwrap();
        std::fs::create_dir_all(&usr).unwrap();
        std::fs::write(etc.join("00-admin.preset"), "enable ssh.service\n").unwrap();
        std::fs::write(usr.join("99-default.preset"), "disable *\n").unwrap();
        let p = Policy::load_from(&[etc.as_path(), usr.as_path()]);
        assert_eq!(p.query("ssh.service"), Preset::Enable, "00- is read first");
        assert_eq!(p.query("other.service"), Preset::Disable);
    }

    #[test]
    fn a_higher_directory_replaces_a_same_named_file_rather_than_adding_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let etc = dir.path().join("etc");
        let usr = dir.path().join("usr");
        std::fs::create_dir_all(&etc).unwrap();
        std::fs::create_dir_all(&usr).unwrap();
        std::fs::write(etc.join("50-policy.preset"), "enable ssh.service\n").unwrap();
        std::fs::write(
            usr.join("50-policy.preset"),
            "disable ssh.service\ndisable *\n",
        )
        .unwrap();
        let p = Policy::load_from(&[etc.as_path(), usr.as_path()]);
        assert_eq!(p.query("ssh.service"), Preset::Enable);
        // The vendor file is gone entirely, so its catchall is gone too.
        assert_eq!(p.query("other.service"), Preset::Enable);
    }

    #[test]
    fn a_devnull_symlink_switches_a_vendor_file_off() {
        let dir = tempfile::tempdir().unwrap();
        let etc = dir.path().join("etc");
        let usr = dir.path().join("usr");
        std::fs::create_dir_all(&etc).unwrap();
        std::fs::create_dir_all(&usr).unwrap();
        std::fs::write(usr.join("90-vendor.preset"), "disable *\n").unwrap();
        std::os::unix::fs::symlink("/dev/null", etc.join("90-vendor.preset")).unwrap();
        let p = Policy::load_from(&[etc.as_path(), usr.as_path()]);
        assert_eq!(
            p.query("anything.service"),
            Preset::Enable,
            "the vendor catchall must not come back"
        );
    }

    #[test]
    fn only_enabled_and_disabled_can_drift() {
        assert_eq!(drift("enabled", Preset::Disable), "should-disable");
        assert_eq!(drift("disabled", Preset::Enable), "should-enable");
        assert_eq!(drift("enabled", Preset::Enable), "");
        assert_eq!(drift("disabled", Preset::Disable), "");
        assert_eq!(drift("enabled", Preset::Ignore), "");
        assert_eq!(drift("indirect", Preset::Enable), "");
        assert_eq!(drift("static", Preset::Enable), "");
        assert_eq!(drift("masked", Preset::Enable), "");
    }

    #[test]
    fn a_state_with_no_install_section_has_no_preset_to_show() {
        for state in ["static", "transient", "generated", "alias", "bad", ""] {
            assert!(!preset_applies(state), "{state} should have no preset");
        }
        for state in ["enabled", "disabled", "masked", "linked", "indirect"] {
            assert!(preset_applies(state), "{state} should have one");
        }
    }
}
