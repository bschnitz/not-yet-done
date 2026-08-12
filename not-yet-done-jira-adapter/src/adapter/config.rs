//! Adapter YAML config: auth strategy (via the unified [`AuthSpec`]),
//! endpoint, optional DB override.
//!
//! Auth lives in the unified `AuthSpec` from `not-yet-done-content` —
//! mechanism + bindings + session-cache policy. Which mechanisms this
//! adapter implements, and which fields each needs, is published from
//! `auth_bridge::MECHANISMS` and listed by `nyd config auth jira`.
//!
//! Pre-orchestrator configs (`auth: { type: cookie-script, script: ... }`)
//! no longer parse and need to be rewritten as
//! `auth: { mechanism: cookie, bindings: [...] }`.

use fieldsmith::Buildable;
use serde::Deserialize;

use not_yet_done_content::AuthSpec;

#[derive(Deserialize, Buildable)]
#[serde(deny_unknown_fields)]
pub struct JiraConfig {
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
    /// Glyph shown in the `bookmarked` marker column for a bookmarked issue
    /// (non-bookmarked rows stay blank). Defaults to [`DEFAULT_BOOKMARK_MARKER`]
    /// when unset. Any string is allowed — a Nerd-Font glyph, an emoji, or
    /// plain ASCII like `*`.
    #[serde(default)]
    pub(super) bookmark_marker: Option<String>,
    /// Base directory under which the `edit (markdown)` and `export workspace`
    /// actions materialise a persistent per-ticket folder
    /// (`<base>/<KEY>-<slug>/ticket.md` + `attachments/`). A leading `~` is
    /// expanded. When unset, defaults to `<data-local>/not_yet_done/jira/
    /// <instance>/tickets`.
    #[serde(default)]
    pub(super) ticket_workspace: Option<String>,
}

/// Default `bookmarked`-column glyph when `bookmark_marker` is unset.
pub(super) const DEFAULT_BOOKMARK_MARKER: &str = "★";

#[derive(Deserialize, Buildable, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct DbConfig {
    pub(super) url: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::auth_bridge::MECHANISMS;
    use not_yet_done_content::{CredentialProvider, SessionCachePolicy};

    /// The example under `docs/examples/views/` is the first thing a user
    /// copies, so it has to parse and validate like any real config. It
    /// once documented fields (`email`, `session_id`) this struct had long
    /// stopped having — nothing caught that until someone tried it.
    #[test]
    fn the_shipped_example_config_parses() {
        let yaml = include_str!("../../../docs/examples/views/jira-adapter.yaml");
        let cfg: JiraConfig = serde_yaml::from_str(yaml).expect("example parses");
        cfg.auth
            .validate_against(MECHANISMS)
            .expect("example is a valid spec");
    }

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
        cfg.auth.validate_against(MECHANISMS).expect("valid spec");
        assert_eq!(cfg.auth.mechanism, "cookie");
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
        cfg.auth.validate_against(MECHANISMS).expect("valid spec");
        assert_eq!(cfg.auth.mechanism, "basic-auth");
    }

    #[test]
    fn rejects_legacy_cookie_script_shape() {
        let yaml =
            "url: https://jira.example.invalid\nauth:\n  type: cookie-script\n  script: /s\n";
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

    /// The `config_schema()` hook derives its template from the same
    /// `type Config` the factory parses, so the two can never drift. Here
    /// we assert the reflected schema renders a non-empty YAML template
    /// that names the config's top-level fields and is itself valid YAML.
    #[test]
    fn schema_renders_a_valid_yaml_template() {
        use fieldsmith::Buildable;

        let template = fieldsmith::yaml_template(&JiraConfig::schema());
        assert!(!template.trim().is_empty(), "template must not be empty");
        assert!(
            template.contains("url") && template.contains("auth"),
            "template should mention the config's fields:\n{template}"
        );
        serde_yaml::from_str::<serde_yaml::Value>(&template)
            .expect("rendered template must be valid YAML");
    }

    /// The config parses fine on its own — what rejects it is the
    /// adapter's mechanism table, so a mechanism this adapter never
    /// implements fails at build time instead of at first login.
    #[test]
    fn rejects_a_mechanism_this_adapter_does_not_implement() {
        let yaml = r#"
url: https://jira.example.invalid
auth:
  mechanism: password-login
  bindings:
    - field: username
      provider: { type: literal, value: alice }
    - field: password
      provider: { type: prompt }
"#;
        let cfg: JiraConfig = serde_yaml::from_str(yaml).expect("parses");
        let err = cfg
            .auth
            .validate_against(MECHANISMS)
            .expect_err("mechanism is not implemented here");
        assert!(
            err.contains("password-login"),
            "names the rejected mechanism: {err}"
        );
        assert!(err.contains("cookie"), "names a supported one: {err}");
    }
}
