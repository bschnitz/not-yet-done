//! The protection list — the units a disruptive action refuses to touch.
//!
//! A user manager is not a collection of interchangeable services. A handful of
//! them *are* the session: stop `dbus.socket` and nothing else on the bus will
//! answer again; stop the slice a unit lives in and everything in it goes with
//! it. Those are one keystroke away in a table, and a confirmation prompt is a
//! poor guard against a keystroke — the muscle memory that pressed the key
//! presses `y` too.
//!
//! So the list does not ask. A disruptive verb on a protected unit comes back
//! as [`ContentError`](not_yet_done_content::ContentError)-shaped refusal, and
//! the user has `systemctl` if they genuinely mean it.
//!
//! # What "disruptive" means
//!
//! Not "changes something" — *interrupts something that is running, or removes
//! it from the session it is holding up*. Starting a protected unit is fine (it
//! is already running, or should be). Reloading it is fine. Stopping, killing,
//! restarting, freezing and masking it are not. Each verb declares which it is;
//! see [`crate::control::Verb::disruptive`].
//!
//! # Why it is additive by default
//!
//! `protect:` adds to the built-in list and `unprotect:` removes from it, rather
//! than one key replacing it wholesale. A replacing key means a config that
//! names one extra unit silently drops every default — the failure mode
//! being protected against is precisely the one nobody notices until the
//! session is gone. Lifting a default stays possible, but it has to be said.

use crate::config::Manager;

/// The units a fresh instance protects **on the user manager**.
///
/// Deliberately short. Everything here either *is* the session's plumbing or
/// takes the rest of it down: this is not a list of "important" units, which
/// would be a matter of taste and would grow until the tab could not do
/// anything.
pub const USER_PROTECTED: &[&str] = &[
    // Every cgroup grouping: a slice or scope is not a service you restart, and
    // stopping one kills everything inside it.
    "*.slice",
    "*.scope",
    // The session bus itself — including the socket this adapter is talking
    // over. Both implementations are named, because which one a machine runs
    // is not something the user picked: `dbus-broker.service` is the default on
    // most current distributions, `dbus.service` the older reference daemon.
    // Listing only one leaves the other unguarded on exactly the machines that
    // have it.
    "dbus.socket",
    "dbus.service",
    "dbus-broker.service",
    // The targets the user manager's own dependency graph hangs off.
    "default.target",
    "basic.target",
    "sockets.target",
    "timers.target",
    "paths.target",
    "graphical-session.target",
    "graphical-session-pre.target",
];

/// The units a fresh instance protects **on the system manager**.
///
/// The same idea, a different machine. `graphical-session.target` is not what
/// holds up a system, and `default.target` means something else here — so the
/// list is not the user one with entries bolted on, it is the answer to the
/// same question asked of the other manager.
///
/// What is deliberately **absent** is worth as much as what is present:
/// `NetworkManager.service` and `sshd.service` can cut the very session a
/// remote user is holding, and they are still not here. They are exactly the
/// services one legitimately restarts, and a list that protected them would
/// make the tab useless for its main job. Protection is for what takes the
/// *machine* down, not for what is merely expensive to get wrong; `protect:`
/// in the adapter config is where a machine that is only ever reached over SSH
/// says so.
pub const SYSTEM_PROTECTED: &[&str] = &[
    // As above, and `init.scope` — PID 1's own scope — falls under this.
    "*.slice",
    "*.scope",
    // The system bus. Same two implementations, same reason.
    "dbus.socket",
    "dbus.service",
    "dbus-broker.service",
    // The log. Stopping the socket does not merely stop logging: everything
    // that writes to it blocks or loses its output, including the journal this
    // tab reads.
    "systemd-journald.service",
    "systemd-journald.socket",
    // Sessions, seats and the lock that lets anyone log in again.
    "systemd-logind.service",
    // The targets the system's own graph hangs off, early to late.
    "sysinit.target",
    "basic.target",
    "multi-user.target",
    "graphical.target",
    "default.target",
];

/// The built-in list for `manager`.
pub fn defaults_for(manager: Manager) -> &'static [&'static str] {
    match manager {
        Manager::User => USER_PROTECTED,
        Manager::System => SYSTEM_PROTECTED,
    }
}

/// Which units are off limits for disruptive verbs.
#[derive(Clone, Debug)]
pub struct Protection {
    manager: Manager,
    patterns: Vec<String>,
}

impl Default for Protection {
    fn default() -> Self {
        Self::new(Manager::default(), &[], &[])
    }
}

impl Protection {
    /// The manager's built-in list, plus `added`, minus `lifted`.
    ///
    /// `lifted` is matched against the pattern *as written*, not against the
    /// units it covers: lifting `*.slice` is one decision, and it should read
    /// like the one thing it is rather than requiring the user to guess which
    /// entry a given unit came from.
    pub fn new(manager: Manager, added: &[String], lifted: &[String]) -> Self {
        let patterns = defaults_for(manager)
            .iter()
            .map(|p| (*p).to_string())
            .chain(added.iter().cloned())
            .filter(|p| !lifted.iter().any(|l| l.trim() == p))
            .collect();
        Self { manager, patterns }
    }

    /// Whether `unit` is protected.
    ///
    /// A pattern is either a whole unit name or a `*.<suffix>` covering a unit
    /// type. That is the entire language on purpose: a general glob invites
    /// `*`, and a protection list that matches everything is a tab that does
    /// nothing while looking like it works.
    pub fn covers(&self, unit: &str) -> bool {
        self.patterns.iter().any(|p| match p.strip_prefix("*.") {
            Some(suffix) => unit.ends_with(suffix) && unit.len() > suffix.len(),
            None => p == unit,
        })
    }

    /// The refusal, phrased so it says *why* and what the way around is.
    ///
    /// The "why" is not the same sentence on both managers, and saying the
    /// wrong one would be worse than saying nothing: a system unit on this
    /// list is not holding up *a session*, it is holding up the machine every
    /// session on it runs in.
    pub fn refusal(&self, unit: &str, verb_label: &str) -> String {
        let holds = match self.manager {
            Manager::User => "it holds up the session",
            Manager::System => "it holds up the machine",
        };
        format!(
            "{verb_label} refused: {unit} is on the protection list — {holds}. \
             Lift it with unprotect: in the adapter config, or use systemctl."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_cover_the_session_plumbing() {
        let p = Protection::default();
        assert!(p.covers("dbus.socket"));
        // Whichever of the two D-Bus daemons the machine runs.
        assert!(p.covers("dbus.service"));
        assert!(p.covers("dbus-broker.service"));
        assert!(p.covers("app.slice"));
        assert!(p.covers("session.scope"));
        assert!(p.covers("default.target"));
        assert!(!p.covers("backup-photos.service"));
        // A bare suffix is not a unit name, so it must not match the pattern.
        assert!(!p.covers("slice"));
    }

    #[test]
    fn protect_adds_and_unprotect_lifts() {
        let p = Protection::new(
            Manager::User,
            &["ssh-agent.service".into()],
            &["*.slice".into()],
        );
        assert!(p.covers("ssh-agent.service"));
        assert!(!p.covers("app.slice"));
        // Lifting one entry leaves the rest of the defaults standing.
        assert!(p.covers("dbus.socket"));
    }

    #[test]
    fn each_manager_protects_what_holds_up_its_own_machine() {
        let user = Protection::new(Manager::User, &[], &[]);
        let system = Protection::new(Manager::System, &[], &[]);

        // Both are a session's plumbing in their own way.
        for p in [&user, &system] {
            assert!(p.covers("dbus.socket"));
            assert!(p.covers("app.slice"));
            assert!(p.covers("init.scope"));
        }

        // What only the user manager hangs off.
        assert!(user.covers("graphical-session.target"));
        assert!(!system.covers("graphical-session.target"));

        // What only the system has at all.
        assert!(system.covers("systemd-journald.socket"));
        assert!(system.covers("systemd-logind.service"));
        assert!(system.covers("multi-user.target"));
        assert!(!user.covers("systemd-logind.service"));

        // Deliberately unprotected on both: the services one restarts on
        // purpose, however much it stings over SSH.
        assert!(!system.covers("sshd.service"));
        assert!(!system.covers("NetworkManager.service"));
    }
}
