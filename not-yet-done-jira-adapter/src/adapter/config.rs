//! Adapter YAML config: auth strategy (via the unified [`AuthSpec`]),
//! endpoint, optional DB override.
//!
//! Auth lives in the unified `AuthSpec` from `not-yet-done-content` —
//! mechanism + bindings + session-cache policy. Pre-orchestrator configs
//! (`auth: { type: cookie-script, script: ... }`) no longer parse and
//! need to be rewritten as `auth: { mechanism: cookie, bindings: [...] }`.

use serde::Deserialize;

use not_yet_done_content::AuthSpec;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct JiraConfig {
    pub(super) url: String,
    #[serde(default)]
    pub(super) name: Option<String>,
    pub(super) auth: AuthSpec,
    #[serde(default)]
    pub(super) accept_invalid_certs: bool,
    /// Optional override for the cache backing store. Sea-orm-compatible URL
    /// (`sqlite:///path?mode=rwc`, `postgres://user:pw@host/db`, ...). Falls
    /// back to a private SQLite file under the user's local data dir.
    #[serde(default)]
    pub(super) db: Option<DbConfig>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub(super) struct DbConfig {
    pub(super) url: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::{AuthMechanism, CredentialProvider, SessionCachePolicy};

    #[test]
    fn parses_cookie_via_command_provider() {
        let yaml = r#"
url: https://jira.example.invalid
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
        let cfg: JiraConfig = serde_yaml::from_str(yaml).expect("parses");
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
    fn parses_basic_auth_with_literal_token() {
        let yaml = r#"
url: https://jira.example.invalid
auth:
  mechanism: basic-auth
  bindings:
    - field: username
      provider: { type: literal, value: alice@example.invalid }
    - field: token
      provider: { type: literal, value: synthetic-token }
"#;
        let cfg: JiraConfig = serde_yaml::from_str(yaml).expect("parses");
        cfg.auth.validate().expect("valid spec");
        assert_eq!(cfg.auth.mechanism, AuthMechanism::BasicAuth);
    }

    #[test]
    fn rejects_legacy_cookie_script_shape() {
        let yaml = "url: https://jira.example.invalid\nauth:\n  type: cookie-script\n  script: /s\n";
        let res: Result<JiraConfig, _> = serde_yaml::from_str(yaml);
        assert!(
            res.is_err(),
            "legacy `type: cookie-script` shape must fail under new AuthSpec"
        );
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let yaml = r#"
url: https://jira.example.invalid
auth:
  mechanism: cookie
  bindings:
    - field: cookie
      provider: { type: literal, value: x }
foo: bar
"#;
        let err = serde_yaml::from_str::<JiraConfig>(yaml)
            .err()
            .expect("must reject unknown top-level field");
        assert!(err.to_string().contains("foo"), "error mentions foo: {err}");
    }
}
