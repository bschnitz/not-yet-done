//! Backend config block for a `backend: office365-web` connection.

use std::path::PathBuf;

use serde::Deserialize;

use not_yet_done_content::CredentialProvider;
use not_yet_done_content::auth::CredentialResolver;
use not_yet_done_office365_web::{SessionConfig, SidecarConfig};

/// Prompt text shown to the user when the sign-in raises an MFA challenge, when
/// no `mfa.prompt` override is configured. Kept generic on purpose: for the
/// common number-match flow the actual number arrives as the request's
/// *detail* line, rendered above this text.
pub(crate) const DEFAULT_MFA_PROMPT: &str = "Multi-factor authentication is required to sign in. Approve the request in \
     your authenticator app, then press Enter.";

/// The username + password resolvers built from a connection's credential
/// providers. Either may be absent (fully manual login); the username falls
/// back to `login_hint` when no `username:` provider is configured.
pub(crate) struct CredentialResolvers {
    pub(crate) username: Option<Box<dyn CredentialResolver>>,
    pub(crate) password: Option<Box<dyn CredentialResolver>>,
}

/// The `config:` sub-tree of an `office365-web` connection entry, e.g.
///
/// ```yaml
/// account_key: work           # sessions with the same key share one browser
/// name: "Work"                # "Account" column label (optional)
/// login_hint: user@example.com
/// profile_dir: ~/.local/state/not_yet_done/office365-web/work
/// headless: true             # resting invisible; auto-shows a window for MFA
/// auto_headed: true           # (default) drop back to headless after sign-in
/// password:                   # optional: drives the sign-in unattended
///   type: command
///   script: pass show work/example/password
/// ```
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct Office365WebConfig {
    /// Registry key: connections sharing a key share one browser session.
    pub(crate) account_key: String,
    /// Display label for the "Account" column. Defaults to the connection id.
    #[serde(default)]
    pub(crate) name: Option<String>,
    /// UPN to prefill on the interactive login (optional). Also the default
    /// username for the account picker / email field when no `username:`
    /// credential provider is set.
    #[serde(default)]
    pub(crate) login_hint: Option<String>,
    /// Persistent browser profile directory (login/SSO survives here). `~/` is
    /// expanded against `$HOME`.
    pub(crate) profile_dir: String,
    /// Resting display mode. `true` (default) keeps the browser invisible for
    /// every silent poll; `false` always shows the window (fully manual setups).
    #[serde(default = "default_true")]
    pub(crate) headless: bool,
    /// When resting headless, briefly surface a visible window the moment the
    /// sign-in needs the user (typically MFA), then drop back to headless once
    /// it completes. Defaults to `true`. Set `false` to never pop a window: a
    /// lapsed headless session then reports a login error instead (only sensible
    /// for accounts whose session rarely expires).
    #[serde(default = "default_true")]
    pub(crate) auto_headed: bool,
    /// Entry URL (optional; the sidecar defaults to the Outlook web calendar).
    #[serde(default)]
    pub(crate) start_url: Option<String>,
    /// Override the sidecar entry script path (else `NYD_OFFICE365_SIDECAR`).
    #[serde(default)]
    pub(crate) sidecar_script: Option<String>,
    /// Override the Node binary (default `node`).
    #[serde(default)]
    pub(crate) node_bin: Option<String>,
    /// How to obtain the account username/UPN (optional). Any credential
    /// provider works; a `literal` or `command` (e.g. `pass`) is typical. When
    /// absent, `login_hint` is used.
    #[serde(default)]
    pub(crate) username: Option<CredentialProvider>,
    /// How to obtain the account password (optional). Any credential provider
    /// works; `command` wrapping `pass`/`op` is the intended default. When
    /// absent, the sign-in stays manual (a headed window opens for the user).
    #[serde(default)]
    pub(crate) password: Option<CredentialProvider>,
    /// How the multi-factor-authentication challenge is surfaced to the user
    /// (optional). Absent = the defaults: [`DEFAULT_MFA_PROMPT`] text and the
    /// wrapper's built-in retry count.
    #[serde(default)]
    pub(crate) mfa: Option<MfaConfig>,
}

/// Per-connection MFA prompt configuration. Both fields are independently
/// optional so a config can override just the wording, just the retry count, or
/// both.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct MfaConfig {
    /// Prompt text shown when the sign-in needs the user during MFA. Defaults
    /// to [`DEFAULT_MFA_PROMPT`].
    #[serde(default)]
    pub(crate) prompt: Option<String>,
    /// How many times the sidecar re-raises the challenge if the user is too
    /// slow and the underlying attempt lapses before an answer arrives.
    /// Defaults to the wrapper's [`SidecarConfig`] default.
    #[serde(default)]
    pub(crate) max_retries: Option<u32>,
}

fn default_true() -> bool {
    true
}

impl Office365WebConfig {
    /// Build the credential resolvers from this config's providers. Called
    /// before [`into_session_config`], while the providers are still available.
    pub(crate) fn build_credential_resolvers(&self) -> Result<CredentialResolvers, String> {
        let build = |p: &Option<CredentialProvider>| -> Result<_, String> {
            p.as_ref()
                .map(CredentialProvider::build_resolver)
                .transpose()
        };
        Ok(CredentialResolvers {
            username: build(&self.username)?,
            password: build(&self.password)?,
        })
    }

    /// The prompt text to show when this connection raises an MFA challenge,
    /// falling back to [`DEFAULT_MFA_PROMPT`]. Borrows `self`, so it is read
    /// before [`into_session_config`] consumes the config.
    pub(crate) fn mfa_prompt(&self) -> String {
        self.mfa
            .as_ref()
            .and_then(|m| m.prompt.clone())
            .unwrap_or_else(|| DEFAULT_MFA_PROMPT.to_string())
    }

    /// Build the wrapper's [`SessionConfig`] from this connection config.
    /// Credentials are resolved separately (see [`build_credential_resolvers`])
    /// and injected on the async path, so this leaves `credentials` unset.
    pub(crate) fn into_session_config(self) -> SessionConfig {
        let mut sidecar = SidecarConfig::default();
        if let Some(script) = self.sidecar_script {
            sidecar.script = PathBuf::from(script);
        }
        if let Some(node) = self.node_bin {
            sidecar.node_bin = PathBuf::from(node);
        }
        // Only override the wrapper's built-in retry count when the config asks.
        if let Some(retries) = self.mfa.as_ref().and_then(|m| m.max_retries) {
            sidecar.mfa_max_retries = retries;
        }
        SessionConfig {
            account_key: self.account_key,
            login_hint: self.login_hint,
            profile_dir: expand_tilde(&self.profile_dir),
            headless: self.headless,
            auto_headed: self.auto_headed,
            start_url: self.start_url,
            credentials: None,
            sidecar,
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
    }

    #[test]
    fn rejects_unknown_fields() {
        let yaml = "account_key: work\nprofile_dir: /tmp/p\nbogus: 1\n";
        assert!(serde_yaml::from_str::<Office365WebConfig>(yaml).is_err());
    }

    #[test]
    fn parses_credential_providers_and_builds_resolvers() {
        let yaml = r#"
account_key: work
profile_dir: /tmp/p
login_hint: user@example.com
password:
  type: command
  script: pass show example/password
"#;
        let cfg: Office365WebConfig = serde_yaml::from_str(yaml).expect("parses");
        assert!(matches!(
            cfg.password,
            Some(CredentialProvider::Command { .. })
        ));
        assert!(cfg.username.is_none(), "username provider is optional");
        let resolvers = cfg.build_credential_resolvers().expect("builds");
        assert!(resolvers.password.is_some());
        assert!(
            resolvers.username.is_none(),
            "no username provider → falls back to login_hint at resolve time"
        );
    }

    #[test]
    fn credentials_default_to_none_in_session_config() {
        let yaml = "account_key: work\nprofile_dir: /tmp/p\n";
        let cfg: Office365WebConfig = serde_yaml::from_str(yaml).expect("parses");
        let sc = cfg.into_session_config();
        assert!(
            sc.credentials.is_none(),
            "credentials are injected on the async path, not here"
        );
    }

    #[test]
    fn mfa_defaults_when_absent() {
        let yaml = "account_key: work\nprofile_dir: /tmp/p\n";
        let cfg: Office365WebConfig = serde_yaml::from_str(yaml).expect("parses");
        assert_eq!(cfg.mfa_prompt(), DEFAULT_MFA_PROMPT);
        let sc = cfg.into_session_config();
        assert_eq!(
            sc.sidecar.mfa_max_retries,
            SidecarConfig::default().mfa_max_retries,
            "no mfa block → wrapper default retry count"
        );
    }

    #[test]
    fn mfa_overrides_prompt_and_retries() {
        let yaml = r#"
account_key: work
profile_dir: /tmp/p
mfa:
  prompt: "Approve on your phone"
  max_retries: 5
"#;
        let cfg: Office365WebConfig = serde_yaml::from_str(yaml).expect("parses");
        assert_eq!(cfg.mfa_prompt(), "Approve on your phone");
        let sc = cfg.into_session_config();
        assert_eq!(sc.sidecar.mfa_max_retries, 5);
    }

    #[test]
    fn mfa_rejects_unknown_fields() {
        let yaml = "account_key: work\nprofile_dir: /tmp/p\nmfa:\n  bogus: 1\n";
        assert!(serde_yaml::from_str::<Office365WebConfig>(yaml).is_err());
    }

    #[test]
    fn expands_tilde() {
        // SAFETY: single-threaded test; sets HOME only for this assertion.
        unsafe { std::env::set_var("HOME", "/home/tester") };
        assert_eq!(expand_tilde("~/x/y"), PathBuf::from("/home/tester/x/y"));
        assert_eq!(expand_tilde("/abs"), PathBuf::from("/abs"));
    }
}
