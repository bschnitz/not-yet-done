//! When a configured adapter instance is allowed to connect on its own.
//!
//! Connecting is the side-effecting step: it can open an SSH tunnel, spend a
//! VPN round-trip, or put a credential dialog in front of the user. So *when*
//! it happens is a per-instance decision, written in the `adapter:` block of a
//! view file:
//!
//! ```yaml
//! adapter:
//!   type: jira
//!   auto_connect: startup   # never | on_open | startup
//! ```
//!
//! The three answers, in order of eagerness:
//!
//! | Value      | The instance connects …                                  |
//! |------------|----------------------------------------------------------|
//! | `never`    | only when the user triggers a `reload` action (default)   |
//! | `on_open`  | the first time its tab is opened                          |
//! | `startup`  | as the app comes up, whether its tab is visited or not    |
//!
//! Only the TUI has a notion of "opening a tab", so only the TUI honours the
//! distinction; the CLI and Waybar run one request and exit, and connect
//! regardless.
//!
//! The older boolean spelling `manual_connect: true|false` still parses and
//! means `never` / `startup` respectively — see
//! `not_yet_done_host::AdapterInstance::connect_mode`.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// How eagerly one adapter instance connects. See the [module docs](self).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoConnect {
    /// Never on its own — the user presses the view's `reload` key.
    ///
    /// The default, because an unconfigured instance must not be the one that
    /// opens a tunnel or asks for a password unasked.
    #[default]
    #[serde(alias = "manual")]
    Never,
    /// On the first visit to the instance's tab. The connection is still the
    /// user's doing (they switched to the tab) but costs them no extra key.
    #[serde(alias = "on_switch", alias = "tab")]
    OnOpen,
    /// While the app starts, in the background, before any tab is visited.
    /// The eager choice: right for cheap local stores, and for the one remote
    /// instance whose data should be there the moment its tab is.
    #[serde(alias = "eager")]
    Startup,
}

impl AutoConnect {
    /// The value's spelling in a view file — what [`FromStr`] takes back.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::OnOpen => "on_open",
            Self::Startup => "startup",
        }
    }

    /// True when nothing but an explicit `reload` may start the connection.
    pub fn is_manual(self) -> bool {
        matches!(self, Self::Never)
    }

    /// True when the instance connects without its tab ever being opened —
    /// the one mode whose load (and whose credential prompt) can belong to a
    /// tab the user is not looking at.
    pub fn is_eager(self) -> bool {
        matches!(self, Self::Startup)
    }

    /// True when opening the instance's tab is what triggers the load.
    pub fn is_on_open(self) -> bool {
        matches!(self, Self::OnOpen)
    }
}

impl fmt::Display for AutoConnect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AutoConnect {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "never" | "manual" => Ok(Self::Never),
            "on_open" | "on_switch" | "tab" => Ok(Self::OnOpen),
            "startup" | "eager" => Ok(Self::Startup),
            other => Err(format!(
                "unknown auto_connect `{other}` (expected never, on_open or startup)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_the_cautious_one() {
        assert_eq!(AutoConnect::default(), AutoConnect::Never);
        assert!(AutoConnect::default().is_manual());
    }

    #[test]
    fn every_value_round_trips_through_its_spelling() {
        for mode in [
            AutoConnect::Never,
            AutoConnect::OnOpen,
            AutoConnect::Startup,
        ] {
            assert_eq!(mode.as_str().parse::<AutoConnect>(), Ok(mode));
            assert_eq!(
                serde_yaml::from_str::<AutoConnect>(mode.as_str()).unwrap(),
                mode,
                "yaml spelling of {mode}"
            );
        }
    }

    #[test]
    fn a_typo_names_the_values_it_expected() {
        let err = "on-open".parse::<AutoConnect>().unwrap_err();
        assert!(err.contains("never"), "{err}");
        assert!(err.contains("on_open"), "{err}");
        assert!(err.contains("startup"), "{err}");
    }

    #[test]
    fn only_startup_connects_behind_the_users_back() {
        assert!(AutoConnect::Startup.is_eager());
        assert!(!AutoConnect::OnOpen.is_eager());
        assert!(!AutoConnect::Never.is_eager());
        assert!(AutoConnect::OnOpen.is_on_open());
    }
}
