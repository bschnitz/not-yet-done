//! The `systemd` adapter's `config:` block.
//!
//! Deliberately tiny. What is *shown* is a query the user switches at runtime,
//! not configuration (see `docs/plan-systemd-adapter.md`, decision D2), so the
//! only things left here are which manager to talk to and how long to wait for
//! it.
//!
//! The exception is the protection list, which is not about what is *shown* but
//! about what may be *done* — see [`crate::protect`].
//!
//! ```yaml
//! adapter:
//!   type: systemd
//!   config:
//!     manager: user       # user | system
//!     timeout_secs: 10    # deadline per D-Bus call
//!     protect:            # refuse disruptive verbs on these, too
//!       - ssh-agent.service
//!     unprotect:          # lift one built-in entry, spelled exactly
//!       - "*.slice"
//! ```

use fieldsmith::Buildable;
use serde::Deserialize;

/// Default deadline for a single D-Bus call, in seconds.
pub const DEFAULT_TIMEOUT_SECS: u64 = 10;

/// Which systemd manager an instance talks to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Manager {
    /// The per-user manager on the session bus — `systemctl --user`.
    #[default]
    User,
    /// The system manager on the system bus — `systemctl`. Read-only until the
    /// privilege question is answered (phase 7); listing needs no privileges.
    System,
}

impl Manager {
    /// Parse the configured word. `None` for anything else, so a typo is
    /// rejected at config-load time rather than silently meaning `user`.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "user" => Some(Manager::User),
            "system" => Some(Manager::System),
            _ => None,
        }
    }

    /// The word the user wrote, for labels and error messages.
    pub fn as_str(self) -> &'static str {
        match self {
            Manager::User => "user",
            Manager::System => "system",
        }
    }
}

/// The adapter-level configuration. Every field is optional, so a bare
/// `adapter: { type: systemd }` is a valid instance against the user manager.
#[derive(Debug, Default, Deserialize, Buildable)]
#[serde(deny_unknown_fields)]
pub struct SystemdConfig {
    /// Which manager to talk to — `user` or `system`. Defaults to `user`.
    ///
    /// The field exists from day one even though the user manager is the only
    /// one this phase exercises: a second manager is then an additive change
    /// rather than a schema break.
    ///
    /// Carried as a string (not an enum) so it reaches the config schema and
    /// the `config build` wizard the way every other adapter's choice fields
    /// do; [`SystemdConfig::manager`] is what resolves it.
    #[serde(default)]
    pub manager: Option<String>,
    /// Deadline for a single D-Bus call, in seconds. Defaults to
    /// [`DEFAULT_TIMEOUT_SECS`]. `0` disables the deadline.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Units to protect **in addition to** the built-in list — a unit name or a
    /// `*.<suffix>`. A disruptive verb (stop, restart, kill, mask, freeze)
    /// refuses to touch them.
    ///
    /// Additive rather than replacing, so naming one unit here cannot silently
    /// drop the defaults; see [`crate::protect`].
    #[serde(default)]
    pub protect: Vec<String>,
    /// Entries to drop from the built-in list, spelled exactly as
    /// [`DEFAULT_PROTECTED`](crate::protect::DEFAULT_PROTECTED) spells them.
    ///
    /// The escape hatch, and deliberately an explicit one: lifting protection
    /// should read like a decision in the config, not like an omission.
    #[serde(default)]
    pub unprotect: Vec<String>,
}

impl SystemdConfig {
    /// The configured manager, or the default. An unrecognised word falls
    /// back to the default rather than failing the instance — `validate`
    /// is where a typo is named.
    pub fn manager(&self) -> Manager {
        self.manager
            .as_deref()
            .and_then(Manager::parse)
            .unwrap_or_default()
    }

    /// Reject a `manager:` that is neither `user` nor `system`, so the mistake
    /// surfaces as a sentence instead of as a tab pointing at the wrong bus.
    pub fn validate(&self) -> std::result::Result<(), String> {
        if let Some(raw) = self.manager.as_deref()
            && Manager::parse(raw).is_none()
        {
            return Err(format!(
                "unknown manager {raw:?} — expected \"user\" or \"system\""
            ));
        }
        // An `unprotect:` entry that matches nothing is almost certainly a
        // misspelling, and the consequence of a misspelling here is that the
        // protection the user meant to lift is still in place — which they only
        // find out at the moment they wanted the verb to work.
        let known: Vec<&str> = crate::protect::DEFAULT_PROTECTED
            .iter()
            .copied()
            .chain(self.protect.iter().map(String::as_str))
            .collect();
        if let Some(stray) = self
            .unprotect
            .iter()
            .find(|u| !known.contains(&u.trim()))
        {
            return Err(format!(
                "unprotect: {stray:?} is not on the protection list — it must be spelled exactly \
                 as the entry it lifts"
            ));
        }
        Ok(())
    }

    /// The protection list this instance runs with.
    pub fn protection(&self) -> crate::protect::Protection {
        crate::protect::Protection::new(&self.protect, &self.unprotect)
    }

    /// The configured per-call deadline, or the default. `None` = no deadline.
    pub fn timeout(&self) -> Option<std::time::Duration> {
        match self.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS) {
            0 => None,
            secs => Some(std::time::Duration::from_secs(secs)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_is_the_user_manager_with_the_default_deadline() {
        let cfg: SystemdConfig = serde_yaml::from_str("{}").unwrap();
        assert_eq!(cfg.manager(), Manager::User);
        assert_eq!(cfg.timeout(), Some(std::time::Duration::from_secs(10)));
    }

    #[test]
    fn parses_both_managers_and_rejects_anything_else() {
        let cfg: SystemdConfig = serde_yaml::from_str("manager: system").unwrap();
        assert_eq!(cfg.manager(), Manager::System);
        assert!(cfg.validate().is_ok());

        let typo: SystemdConfig = serde_yaml::from_str("manager: root").unwrap();
        let err = typo.validate().unwrap_err();
        assert!(err.contains("root"), "error should quote the word: {err}");

        assert!(serde_yaml::from_str::<SystemdConfig>("bogus: 1").is_err());
    }

    #[test]
    fn a_misspelled_unprotect_entry_is_refused() {
        let good: SystemdConfig =
            serde_yaml::from_str("protect: [a.service]\nunprotect: [\"*.slice\"]").unwrap();
        assert!(good.validate().is_ok());
        assert!(good.protection().covers("a.service"));
        assert!(!good.protection().covers("app.slice"));

        // Lifting something that was never on the list would look like it
        // worked while changing nothing.
        let typo: SystemdConfig = serde_yaml::from_str("unprotect: [dbus.sockett]").unwrap();
        let err = typo.validate().unwrap_err();
        assert!(err.contains("dbus.sockett"), "must quote the entry: {err}");
    }

    #[test]
    fn zero_timeout_means_no_deadline() {
        let cfg: SystemdConfig = serde_yaml::from_str("timeout_secs: 0").unwrap();
        assert_eq!(cfg.timeout(), None);
    }
}
