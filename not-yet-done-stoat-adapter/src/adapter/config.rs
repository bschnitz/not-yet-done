//! Adapter YAML config.
//!
//! Auth lives in the unified `AuthSpec` from `not-yet-done-content` —
//! mechanism + bindings + session-cache policy. Which mechanisms this
//! adapter implements, and which fields each needs, is published from
//! `auth_bridge::MECHANISMS` and listed by `nyd config auth stoat`.

use fieldsmith::Buildable;
use serde::Deserialize;

use not_yet_done_content::AuthSpec;

#[derive(Deserialize, Buildable, Debug)]
#[serde(deny_unknown_fields)]
pub struct StoatConfig {
    /// Base domain; `/api` and `/ws` are self-discovered via `GET /api/`.
    pub(super) url: String,
    #[serde(default)]
    pub(super) name: Option<String>,
    pub(super) auth: AuthSpec,
    /// Optional override for the backing store (session token + sort
    /// state). Sea-orm-compatible URL; falls back to a private SQLite
    /// file under the user's local data dir.
    #[serde(default)]
    pub(super) db: Option<DbConfig>,
}

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
    /// copies, so it has to parse and validate like any real config.
    #[test]
    fn the_shipped_example_config_parses() {
        let yaml = include_str!("../../../docs/examples/views/stoat-adapter.yaml");
        let cfg: StoatConfig = serde_yaml::from_str(yaml).expect("example parses");
        cfg.auth
            .validate_against(MECHANISMS)
            .expect("example is a valid spec");
    }

    #[test]
    fn parses_password_login_with_username_and_password() {
        // Stoat's `password-login` declares the fields username +
        // password; the login email goes into the `username` field.
        let yaml = r#"
url: https://chat.example.invalid
name: example
auth:
  mechanism: password-login
  bindings:
    - field: username
      provider: { type: prompt }
    - field: password
      provider: { type: prompt }
"#;
        let cfg: StoatConfig = serde_yaml::from_str(yaml).expect("parses");
        cfg.auth
            .validate_against(MECHANISMS)
            .expect("valid mechanism+bindings");
        assert_eq!(cfg.auth.mechanism, "password-login");
        assert_eq!(cfg.auth.session_cache, SessionCachePolicy::UntilRejected);
        assert_eq!(cfg.auth.bindings.len(), 2);
        assert!(matches!(
            cfg.auth.bindings[1].provider,
            CredentialProvider::Prompt { .. }
        ));
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let yaml = "url: https://chat.example.invalid\nfoo: bar\nauth:\n  mechanism: password-login\n  bindings: []\n";
        let res: Result<StoatConfig, _> = serde_yaml::from_str(yaml);
        assert!(res.is_err(), "deny_unknown_fields must reject `foo`");
    }

    /// The config parses fine on its own — what rejects it is the
    /// adapter's mechanism table, so a mechanism this adapter never
    /// implements fails at build time instead of at first login.
    #[test]
    fn rejects_a_mechanism_this_adapter_does_not_implement() {
        let yaml = r#"
url: https://chat.example.invalid
auth:
  mechanism: bearer-token
  bindings:
    - field: token
      provider: { type: prompt }
"#;
        let cfg: StoatConfig = serde_yaml::from_str(yaml).expect("parses");
        let err = cfg
            .auth
            .validate_against(MECHANISMS)
            .expect_err("mechanism is not implemented here");
        assert!(
            err.contains("bearer-token"),
            "names the rejected mechanism: {err}"
        );
        assert!(
            err.contains("password-login"),
            "names a supported one: {err}"
        );
    }
}
