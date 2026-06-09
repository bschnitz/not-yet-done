//! Adapter YAML config.
//!
//! Auth lives in the unified `AuthSpec` from `not-yet-done-content` —
//! mechanism + bindings + session-cache policy. Stoat uses
//! `password-login` with `email` + `password` bindings.

use serde::Deserialize;

use not_yet_done_content::AuthSpec;

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(super) struct StoatConfig {
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
    fn parses_password_login_with_username_and_password() {
        // PasswordLogin fixes the field names to username + password;
        // Stoat carries the login email in the `username` field.
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
        cfg.auth.validate().expect("valid mechanism+bindings");
        assert_eq!(cfg.auth.mechanism, AuthMechanism::PasswordLogin);
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
}
