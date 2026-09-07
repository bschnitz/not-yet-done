//! Backend config block for a `backend: office365-web` connection.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

use not_yet_done_content::CredentialProvider;
use not_yet_done_content::auth::{CredentialPrompts, CredentialResolver};
use not_yet_done_office365_web::{Answers, BrowserConfig, SessionConfig};

/// The `config:` sub-tree of an `office365-web` connection entry, e.g.
///
/// ```yaml
/// account_key: work           # sessions with the same key share one browser
/// name: "Work"                # "Account" column label (optional)
/// account: user@example.com   # what the flow signs on as ({{data.account}})
/// profile_dir: ~/.local/state/not_yet_done/office365-web/work
/// headless: true              # resting hidden; shows the window for a second factor
/// auto_headed: true           # (default) and hides it again once the run is over
/// secrets:                    # what the flow may ask for, and where it comes from
///   calendar-password:
///     type: command
///     script: pass show work/example/password
/// ```
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct Office365WebConfig {
    /// Registry key: connections sharing a key share one browser session.
    pub(crate) account_key: String,
    /// Display label for the "Account" column. Defaults to the connection id.
    #[serde(default)]
    pub(crate) name: Option<String>,
    /// The account the flow signs on as — its `{{data.account}}`. Optional only
    /// because the browser's own values file may say it; with two connections
    /// each names its own.
    #[serde(default)]
    pub(crate) account: Option<String>,
    /// Persistent browser profile directory (login/SSO survives here). `~/` is
    /// expanded against `$HOME`.
    pub(crate) profile_dir: String,
    /// Resting display mode. `true` (default) keeps the browser hidden for
    /// every silent poll; `false` always shows the window (fully manual setups).
    #[serde(default = "default_true")]
    pub(crate) headless: bool,
    /// When resting hidden, put the window up the moment the sign-in has
    /// something to say to you (typically the second factor), then take it
    /// down once the run is over. Defaults to `true`. Set `false` to never pop
    /// a window: what the run says then reaches you through the event bus
    /// alone (see the `office365-web:mfa:number-match` binding).
    #[serde(default = "default_true")]
    pub(crate) auto_headed: bool,
    /// The flow a calendar read runs: a name on the browser's shelf or a path
    /// to a flow directory. Default `nyd-calendar`.
    #[serde(default)]
    pub(crate) flow: Option<String>,
    /// The `drunken-browser` binary (default: found on `PATH`).
    #[serde(default)]
    pub(crate) browser: Option<String>,
    /// The secrets the flow may ask for — by the name the flow declares under
    /// `requires: secrets:` — and how to obtain each. A run asks only for a
    /// secret the browser's own values file says to ask for; the value is
    /// resolved here and handed to the run as the answer. Any credential
    /// provider works; `command` wrapping `pass`/`op` is the intended default.
    #[serde(default)]
    pub(crate) secrets: BTreeMap<String, CredentialProvider>,
}

fn default_true() -> bool {
    true
}

impl Office365WebConfig {
    /// Build one credential resolver per configured secret. Called before
    /// [`into_session_config`](Self::into_session_config), while the providers
    /// are still available.
    ///
    /// `prompts` is what a `script` provider asks the user through, so a
    /// locked password store raises a form in the frontend rather than
    /// `gpg`'s own `pinentry` window.
    pub(crate) fn build_secret_resolvers(
        &self,
        prompts: Option<&CredentialPrompts>,
    ) -> Result<BTreeMap<String, Box<dyn CredentialResolver>>, String> {
        self.secrets
            .iter()
            .map(|(name, provider)| {
                provider
                    .build_resolver_with(prompts)
                    .map(|resolver| (name.clone(), resolver))
            })
            .collect()
    }

    /// Whether any configured provider may raise a dialog while it
    /// resolves — the cue for the backend to offer a prompt stream at all.
    pub(crate) fn can_prompt(&self) -> bool {
        self.secrets.values().any(CredentialProvider::can_prompt)
    }

    /// Build the wrapper's [`SessionConfig`] from this connection config.
    /// Secrets are resolved separately (see
    /// [`build_secret_resolvers`](Self::build_secret_resolvers)) and injected
    /// on the async path, so this leaves `answers` empty.
    pub(crate) fn into_session_config(self) -> SessionConfig {
        let mut browser = BrowserConfig::default();
        if let Some(flow) = self.flow {
            browser.flow = flow;
        }
        if let Some(bin) = self.browser {
            browser.bin = expand_tilde(&bin);
        }
        let facts = self
            .account
            .into_iter()
            .map(|account| ("account".to_string(), account))
            .collect();
        SessionConfig {
            account_key: self.account_key,
            profile_dir: expand_tilde(&self.profile_dir),
            headless: self.headless,
            auto_headed: self.auto_headed,
            facts,
            answers: Answers::new(),
            browser,
        }
    }
}

/// Expand a leading `~/` against `$HOME`; otherwise return the path verbatim.
fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config() {
        let yaml = "account_key: work\nprofile_dir: /tmp/p\n";
        let cfg: Office365WebConfig = serde_yaml::from_str(yaml).expect("parses");
        assert_eq!(cfg.account_key, "work");
        assert!(cfg.headless, "headless defaults to true");
        assert!(cfg.auto_headed, "auto_headed defaults to true");
        let sc = cfg.into_session_config();
        assert_eq!(sc.account_key, "work");
        assert_eq!(sc.profile_dir, PathBuf::from("/tmp/p"));
        assert_eq!(sc.browser.flow, "nyd-calendar");
        assert_eq!(sc.browser.bin, PathBuf::from("drunken-browser"));
        assert!(sc.facts.is_empty(), "no account → no fact");
        assert!(
            sc.answers.is_empty(),
            "answers are resolved on the async path"
        );
    }

    #[test]
    fn rejects_unknown_fields() {
        let yaml = "account_key: work\nprofile_dir: /tmp/p\nbogus: 1\n";
        assert!(serde_yaml::from_str::<Office365WebConfig>(yaml).is_err());
    }

    #[test]
    fn the_sidecar_fields_are_gone() {
        for old in [
            "login_hint: x",
            "sidecar_script: x",
            "node_bin: x",
            "start_url: x",
        ] {
            let yaml = format!("account_key: work\nprofile_dir: /tmp/p\n{old}\n");
            assert!(
                serde_yaml::from_str::<Office365WebConfig>(&yaml).is_err(),
                "{old} should be refused"
            );
        }
    }

    #[test]
    fn account_flow_and_browser_reach_the_session_config() {
        let yaml = r#"
account_key: work
profile_dir: /tmp/p
account: user@example.com
flow: /home/me/flows/calendar
browser: ~/.cargo/bin/drunken-browser
"#;
        let cfg: Office365WebConfig = serde_yaml::from_str(yaml).expect("parses");
        let sc = cfg.into_session_config();
        assert_eq!(sc.facts["account"], "user@example.com");
        assert_eq!(sc.browser.flow, "/home/me/flows/calendar");
        assert!(sc.browser.bin.ends_with(".cargo/bin/drunken-browser"));
    }

    #[test]
    fn parses_secret_providers_and_builds_resolvers() {
        let yaml = r#"
account_key: work
profile_dir: /tmp/p
secrets:
  calendar-password:
    type: command
    script: pass show example/password
"#;
        let cfg: Office365WebConfig = serde_yaml::from_str(yaml).expect("parses");
        assert!(matches!(
            cfg.secrets.get("calendar-password"),
            Some(CredentialProvider::Command { .. })
        ));
        let resolvers = cfg.build_secret_resolvers(None).expect("builds");
        assert_eq!(resolvers.len(), 1);
        assert!(resolvers.contains_key("calendar-password"));
        assert!(!cfg.can_prompt(), "a command provider asks nothing");
    }

    #[test]
    fn expands_tilde() {
        // SAFETY: single-threaded test; sets HOME only for this assertion.
        unsafe { std::env::set_var("HOME", "/home/tester") };
        assert_eq!(expand_tilde("~/x/y"), PathBuf::from("/home/tester/x/y"));
        assert_eq!(expand_tilde("/abs"), PathBuf::from("/abs"));
    }
}
