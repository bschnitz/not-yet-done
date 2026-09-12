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
//! names one extra unit silently drops the eleven defaults — the failure mode
//! being protected against is precisely the one nobody notices until the
//! session is gone. Lifting a default stays possible, but it has to be said.

/// The units a fresh instance protects.
///
/// Deliberately short. Everything here either *is* the session's plumbing or
/// takes the rest of it down: this is not a list of "important" units, which
/// would be a matter of taste and would grow until the tab could not do
/// anything.
pub const DEFAULT_PROTECTED: &[&str] = &[
    // Every cgroup grouping: a slice or scope is not a service you restart, and
    // stopping one kills everything inside it.
    "*.slice",
    "*.scope",
    // The session bus itself — including the socket this adapter is talking
    // over.
    "dbus.socket",
    "dbus.service",
    // The targets the user manager's own dependency graph hangs off.
    "default.target",
    "basic.target",
    "sockets.target",
    "timers.target",
    "paths.target",
    "graphical-session.target",
    "graphical-session-pre.target",
];

/// Which units are off limits for disruptive verbs.
#[derive(Clone, Debug)]
pub struct Protection {
    patterns: Vec<String>,
}

impl Default for Protection {
    fn default() -> Self {
        Self::new(&[], &[])
    }
}

impl Protection {
    /// The built-in list, plus `added`, minus `lifted`.
    ///
    /// `lifted` is matched against the pattern *as written*, not against the
    /// units it covers: lifting `*.slice` is one decision, and it should read
    /// like the one thing it is rather than requiring the user to guess which
    /// entry a given unit came from.
    pub fn new(added: &[String], lifted: &[String]) -> Self {
        let patterns = DEFAULT_PROTECTED
            .iter()
            .map(|p| (*p).to_string())
            .chain(added.iter().cloned())
            .filter(|p| !lifted.iter().any(|l| l.trim() == p))
            .collect();
        Self { patterns }
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
    pub fn refusal(&self, unit: &str, verb_label: &str) -> String {
        format!(
            "{verb_label} refused: {unit} is on the protection list — it holds up the \
             session. Lift it with unprotect: in the adapter config, or use systemctl."
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
        assert!(p.covers("app.slice"));
        assert!(p.covers("session.scope"));
        assert!(p.covers("default.target"));
        assert!(!p.covers("backup-photos.service"));
        // A bare suffix is not a unit name, so it must not match the pattern.
        assert!(!p.covers("slice"));
    }

    #[test]
    fn protect_adds_and_unprotect_lifts() {
        let p = Protection::new(&["ssh-agent.service".into()], &["*.slice".into()]);
        assert!(p.covers("ssh-agent.service"));
        assert!(!p.covers("app.slice"));
        // Lifting one entry leaves the rest of the defaults standing.
        assert!(p.covers("dbus.socket"));
    }
}
