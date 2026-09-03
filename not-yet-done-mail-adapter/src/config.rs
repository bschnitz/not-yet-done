//! Adapter YAML config.
//!
//! Unlike the single-connection adapters, one `mail` instance holds **many
//! accounts**: mail is one workflow, and six tabs for six mailboxes is the
//! wrong shape for it. The layout follows the calendar adapter — a list of
//! connections under one instance — with one difference: an account's
//! credentials are a plain [`AuthSpec`], so `nyd config auth mail` describes
//! them and every existing credential provider works per account.
//!
//! Which mechanisms are implemented, and which fields each needs, is published
//! from [`crate::auth::MECHANISMS`] and validated per account in the factory.

use fieldsmith::Buildable;
use serde::Deserialize;

use not_yet_done_content::{AuthSpec, RetryConfig};

/// How many messages one page holds when the view asks for no explicit window.
pub(crate) const DEFAULT_PAGE_SIZE: u32 = 50;

/// How long one IMAP command may take before the adapter gives up on it.
///
/// Generous, because a `SEARCH` over a mailbox of a hundred thousand messages
/// on a busy server is slow but not broken. It exists for the other case: a
/// connection that is neither answering nor closing, where without a deadline
/// the actor waits forever — and, since IMAP runs one command at a time,
/// takes the whole account's queue down with it.
pub(crate) const DEFAULT_COMMAND_TIMEOUT_SECS: u64 = 60;

/// How the connection to the IMAP server is secured. Stated, never guessed
/// from the port: a bridge on loopback speaks plain text on purpose, and
/// silently "upgrading" it would break it rather than protect anyone.
#[derive(Deserialize, Buildable, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Security {
    /// Implicit TLS from the first byte (the usual port 993).
    #[default]
    Tls,
    /// Plain connect, then `STARTTLS` before login (the usual port 143).
    Starttls,
    /// No transport security at all. Only sensible for a local bridge.
    None,
}

impl Security {
    /// The port to use when the account names none.
    pub(crate) fn default_port(self) -> u16 {
        match self {
            Security::Tls => 993,
            Security::Starttls | Security::None => 143,
        }
    }
}

/// The `config:` block of a `type: mail` instance.
#[derive(Deserialize, Buildable, Debug)]
#[serde(deny_unknown_fields)]
pub struct MailConfig {
    /// Label of the instance — the root node and the tab title.
    #[serde(default)]
    pub(crate) name: Option<String>,
    /// One entry per mailbox. At least one is required; ids must be unique,
    /// because an account's id is the first segment of every node id below it.
    pub(crate) accounts: Vec<AccountConfig>,
    /// Messages per page when the view asks for no explicit window. May be
    /// overridden per account.
    #[serde(default)]
    pub(crate) page_size: Option<u32>,
    /// How often a request that produced no answer at all is sent again.
    /// Inherited by every account that names no `retry` of its own.
    #[serde(default)]
    pub(crate) retry: RetryConfig,
    /// Seconds one IMAP command may take. `0` switches the deadline off —
    /// for a server that is genuinely that slow, and at the price of a
    /// stalled connection hanging the account's whole queue.
    #[serde(default)]
    pub(crate) command_timeout_secs: Option<u64>,
    /// Optional override for the backing store (envelope cache, phase 4).
    #[serde(default)]
    pub(crate) db: Option<DbConfig>,
}

/// One mailbox: where it lives, how it is secured, and who logs in.
#[derive(Deserialize, Buildable, Debug)]
#[serde(deny_unknown_fields)]
pub struct AccountConfig {
    /// Stable, unique within the instance. It is the first segment of every
    /// node id under this account, so renaming it invalidates saved cursors —
    /// pick something short and permanent (`work`, `private`).
    pub(crate) id: String,
    /// Human-readable label for the account row and its subtab. Defaults to
    /// [`AccountConfig::id`].
    #[serde(default)]
    pub(crate) name: Option<String>,
    /// The account's own e-mail address. Displayed, and used as the `From`
    /// when sending (phase 7) — never as the login name, which is whatever
    /// the `username` credential field says.
    #[serde(default)]
    pub(crate) address: Option<String>,
    pub(crate) host: String,
    /// Defaults to 993 for `tls`, 143 otherwise.
    #[serde(default)]
    pub(crate) port: Option<u16>,
    #[serde(default)]
    pub(crate) security: Security,
    /// Accept a certificate that does not validate. Only for a server with a
    /// self-signed certificate; a bridge on loopback wants `security: none`
    /// instead, which never looks at a certificate in the first place.
    #[serde(default)]
    pub(crate) accept_invalid_certs: bool,
    /// The folder a view opens on when it names none.
    #[serde(default = "default_folder")]
    pub(crate) default_folder: String,
    /// Folder paths never listed. Entries ending in `/*` (or the server's own
    /// delimiter) hide a whole subtree.
    #[serde(default)]
    pub(crate) exclude_folders: Vec<String>,
    pub(crate) auth: AuthSpec,
    /// Per-account override of the instance's `page_size`.
    #[serde(default)]
    pub(crate) page_size: Option<u32>,
    /// Per-account override of the instance's `retry`.
    #[serde(default)]
    pub(crate) retry: Option<RetryConfig>,
    /// Per-account override of the instance's `command_timeout_secs`.
    #[serde(default)]
    pub(crate) command_timeout_secs: Option<u64>,
}

fn default_folder() -> String {
    "INBOX".to_string()
}

#[derive(Deserialize, Buildable, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DbConfig {
    pub(crate) url: String,
}

impl MailConfig {
    /// Reject the two mistakes that would otherwise surface as a confusing
    /// runtime failure: no account at all, and two accounts sharing an id
    /// (which would make one shadow the other's node ids).
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.accounts.is_empty() {
            return Err("`accounts:` must name at least one mailbox".into());
        }
        let mut seen: Vec<&str> = Vec::new();
        for acc in &self.accounts {
            if acc.id.trim().is_empty() {
                return Err("every account needs a non-empty `id:`".into());
            }
            if acc.id.contains('/') {
                return Err(format!(
                    "account id `{}` must not contain `/` — it is the first segment of every node id",
                    acc.id
                ));
            }
            if seen.contains(&acc.id.as_str()) {
                return Err(format!("duplicate account id `{}`", acc.id));
            }
            seen.push(&acc.id);
        }
        Ok(())
    }
}

impl AccountConfig {
    pub(crate) fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    pub(crate) fn port(&self) -> u16 {
        self.port.unwrap_or_else(|| self.security.default_port())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::MECHANISMS;

    const ONE_ACCOUNT: &str = r#"
accounts:
  - id: work
    host: imap.example.invalid
    auth:
      mechanism: password
      bindings:
        - field: username
          provider: { type: literal, value: me@example.invalid }
        - field: password
          provider: { type: prompt }
"#;

    #[test]
    fn an_account_defaults_to_implicit_tls_on_993() {
        let cfg: MailConfig = serde_yaml::from_str(ONE_ACCOUNT).expect("parses");
        cfg.validate().expect("valid");
        assert_eq!(cfg.accounts[0].security, Security::Tls);
        assert_eq!(cfg.accounts[0].port(), 993);
        assert_eq!(cfg.accounts[0].default_folder, "INBOX");
    }

    /// A bridge on loopback speaks plain text on purpose. `security: none`
    /// must parse and must pick the plain port — refusing it here would make
    /// the whole account unreachable.
    #[test]
    fn plaintext_on_loopback_is_configurable() {
        let yaml = r#"
accounts:
  - id: bridge
    host: 127.0.0.1
    security: none
    auth:
      mechanism: password
      bindings:
        - field: username
          provider: { type: prompt }
        - field: password
          provider: { type: prompt }
"#;
        let cfg: MailConfig = serde_yaml::from_str(yaml).expect("parses");
        cfg.validate().expect("valid");
        assert_eq!(cfg.accounts[0].security, Security::None);
        assert_eq!(cfg.accounts[0].port(), 143);
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let res: Result<MailConfig, _> = serde_yaml::from_str(&format!("foo: bar\n{ONE_ACCOUNT}"));
        assert!(res.is_err(), "deny_unknown_fields must reject `foo`");
    }

    #[test]
    fn rejects_duplicate_account_ids() {
        // A duplicate id parses perfectly well — what rejects it is the
        // guard, because the id is the first segment of every node id under
        // the account and a shadowed one would swallow the other's messages.
        let dup = r#"
accounts:
  - id: work
    host: a.invalid
    auth: { mechanism: password, bindings: [] }
  - id: work
    host: b.invalid
    auth: { mechanism: password, bindings: [] }
"#;
        let cfg: MailConfig = serde_yaml::from_str(dup).expect("parses");
        let err = cfg.validate().expect_err("duplicate id");
        assert!(err.contains("work"), "names the offending id: {err}");
    }

    #[test]
    fn rejects_a_mechanism_this_adapter_does_not_implement() {
        let yaml = r#"
accounts:
  - id: work
    host: imap.example.invalid
    auth:
      mechanism: cookie
      bindings:
        - field: cookie
          provider: { type: prompt }
"#;
        let cfg: MailConfig = serde_yaml::from_str(yaml).expect("parses");
        let err = cfg.accounts[0]
            .auth
            .validate_against(MECHANISMS)
            .expect_err("mechanism is not implemented here");
        assert!(err.contains("cookie"), "names the rejected one: {err}");
        assert!(err.contains("password"), "names a supported one: {err}");
    }

    #[test]
    fn the_shipped_example_config_parses() {
        let yaml = include_str!("../../docs/examples/views/mail-adapter.yaml");
        let cfg: MailConfig = serde_yaml::from_str(yaml).expect("example parses");
        cfg.validate().expect("example is valid");
        for acc in &cfg.accounts {
            acc.auth
                .validate_against(MECHANISMS)
                .unwrap_or_else(|e| panic!("account `{}`: {e}", acc.id));
        }
    }
}
