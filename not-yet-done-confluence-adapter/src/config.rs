//! Adapter YAML config: auth strategy (via the unified [`AuthSpec`]),
//! endpoint, optional DB override. CF-2a slice — `auth` is parsed but not
//! yet wired (the bridge lands in CF-2b).
//!
//! `manual_connect` is a view-level flag (`AdapterConfig.manual_connect`
//! in the TUI's view config), not part of the adapter's own YAML — same
//! shape as Jira/Taiga.

use serde::Deserialize;

use not_yet_done_content::AuthSpec;

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfluenceConfig {
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) name: Option<String>,
    pub(crate) auth: AuthSpec,
    /// Consumed by [`crate::client::ConfluenceClient`] once the auth
    /// bridge lands in CF-2b. Already parsed so user configs don't have
    /// to change between phases.
    #[serde(default)]
    #[allow(dead_code)]
    pub(crate) accept_invalid_certs: bool,
    /// Optional override for the cache backing store. Sea-orm-compatible
    /// URL (`sqlite:///path?mode=rwc`, `postgres://user:pw@host/db`, ...).
    /// Falls back to a private SQLite file under the user's local data dir.
    #[serde(default)]
    pub(crate) db: Option<DbConfig>,
    /// Optional whitelist of space keys to surface in the spaces sub-tab.
    /// `None` (omitted) keeps the historic behaviour of listing every
    /// space the user can read. `Some(keys)` filters the API result and
    /// re-orders it so spaces appear in the YAML-provided sequence.
    /// Reason this exists: Crowd-SSO Confluence instances routinely
    /// expose hundreds of spaces but a user typically curates a handful
    /// — letting the list balloon to 200+ entries makes the tree mode
    /// unusable, both for browsing and for the initial fetch latency.
    /// Missing keys (typos / spaces the user lost access to) are
    /// silently dropped with a warn-log so a single bad entry doesn't
    /// brick the whole listing.
    #[serde(default)]
    pub(crate) space_keys: Option<Vec<String>>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub(crate) struct DbConfig {
    pub(crate) url: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::{AuthMechanism, CredentialProvider, SessionCachePolicy};

    #[test]
    fn parses_cookie_via_command_provider() {
        let yaml = r#"
url: https://wiki.example.invalid/confluence
auth:
  mechanism: cookie
  bindings:
    - field: cookie
      provider:
        type: command
        script: /usr/local/bin/get-cookie.sh
        timeout_secs: 60
        retries: 5
"#;
        let cfg: ConfluenceConfig = serde_yaml::from_str(yaml).expect("parses");
        cfg.auth.validate().expect("valid spec");
        assert_eq!(cfg.auth.mechanism, AuthMechanism::Cookie);
        assert_eq!(cfg.auth.bindings.len(), 1);
        match &cfg.auth.bindings[0].provider {
            CredentialProvider::Command {
                script,
                timeout_secs,
                retries,
            } => {
                assert_eq!(script, "/usr/local/bin/get-cookie.sh");
                assert_eq!(*timeout_secs, 60);
                assert_eq!(*retries, 5);
            }
            other => panic!("unexpected provider: {other:?}"),
        }
        assert_eq!(cfg.auth.session_cache, SessionCachePolicy::UntilRejected);
    }

    #[test]
    fn parses_with_db_override_and_invalid_certs() {
        let yaml = r#"
url: https://wiki.example.invalid
name: wiki
accept_invalid_certs: true
db:
  url: "sqlite:///tmp/confluence-test.sqlite?mode=rwc"
auth:
  mechanism: cookie
  bindings:
    - field: cookie
      provider: { type: literal, value: "JSESSIONID=synthetic" }
"#;
        let cfg: ConfluenceConfig = serde_yaml::from_str(yaml).expect("parses");
        cfg.auth.validate().expect("valid spec");
        assert!(cfg.accept_invalid_certs);
        let db = cfg.db.expect("db present");
        assert!(db.url.contains("/tmp/confluence-test.sqlite"));
    }

    #[test]
    fn rejects_missing_auth() {
        let yaml = "url: https://wiki.example.invalid\nname: wiki\n";
        let res: Result<ConfluenceConfig, _> = serde_yaml::from_str(yaml);
        assert!(
            res.is_err(),
            "auth is mandatory — config without it must fail"
        );
    }

    #[test]
    fn parses_space_keys_whitelist() {
        let yaml = r#"
url: https://wiki.example.invalid
auth:
  mechanism: cookie
  bindings:
    - field: cookie
      provider: { type: literal, value: "JSESSIONID=x" }
space_keys:
  - ALPHA
  - BETA
"#;
        let cfg: ConfluenceConfig = serde_yaml::from_str(yaml).expect("parses");
        cfg.auth.validate().expect("valid spec");
        let keys = cfg.space_keys.expect("space_keys present");
        assert_eq!(keys, vec!["ALPHA".to_string(), "BETA".to_string()]);
    }

    #[test]
    fn omitting_space_keys_yields_none() {
        let yaml = r#"
url: https://wiki.example.invalid
auth:
  mechanism: cookie
  bindings:
    - field: cookie
      provider: { type: literal, value: "JSESSIONID=x" }
"#;
        let cfg: ConfluenceConfig = serde_yaml::from_str(yaml).expect("parses");
        assert!(cfg.space_keys.is_none());
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let yaml = r#"
url: https://wiki.example.invalid
auth:
  mechanism: cookie
  bindings:
    - field: cookie
      provider: { type: literal, value: x }
foo: bar
"#;
        let err = serde_yaml::from_str::<ConfluenceConfig>(yaml)
            .err()
            .expect("must reject unknown top-level field");
        assert!(err.to_string().contains("foo"), "error mentions foo: {err}");
    }
}
