//! Adapter YAML config.

use serde::Deserialize;

use not_yet_done_content::AuthSpec;

/// Default per-request timeout (seconds) when `request_timeout_secs` is
/// not set — same rationale as the other REST adapters: long enough for a
/// slow but healthy instance, short enough that a dead connection surfaces
/// instead of freezing the UI.
pub(super) const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 20;

/// Upper bound (seconds) for the *derived* connect timeout when
/// `connect_timeout_secs` is not set: the connect phase gets
/// `min(request_timeout_secs, cap)` so an unreachable host fails fast.
pub(super) const DEFAULT_CONNECT_TIMEOUT_CAP_SECS: u64 = 10;

/// Default lookback window (days) for the timesheet listing when
/// `lookback_days` is not set. The list always time-boxes to
/// `now - lookback_days` — Kimai instances accumulate years of records
/// and the flat view only needs the recent slice.
pub(super) const DEFAULT_LOOKBACK_DAYS: u32 = 92;

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(super) struct KimaiConfig {
    pub(super) url: String,
    #[serde(default)]
    pub(super) name: Option<String>,
    /// `user-api-token` (X-AUTH-USER/X-AUTH-TOKEN header pair, Kimai up
    /// to 2.13) or `bearer-token` (API token, Kimai 2.14+).
    pub(super) auth: AuthSpec,
    /// How many days back the timesheet listing reaches. Defaults to
    /// [`DEFAULT_LOOKBACK_DAYS`].
    #[serde(default)]
    pub(super) lookback_days: Option<u32>,
    /// Hard ceiling (seconds) on every HTTP request, including connect.
    #[serde(default)]
    pub(super) request_timeout_secs: Option<u64>,
    /// Hard ceiling (seconds) on just the connection-establishment phase.
    /// Derived as `min(request_timeout_secs, 10)` when absent.
    #[serde(default)]
    pub(super) connect_timeout_secs: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::{AuthMechanism, CredentialProvider};

    #[test]
    fn parses_user_api_token_config() {
        let yaml = r#"
url: https://kimai.example.invalid
name: Kimai
auth:
  mechanism: user-api-token
  bindings:
    - field: username
      provider: { type: literal, value: alice }
    - field: token
      provider: { type: command, script: get-token.sh }
lookback_days: 30
"#;
        let cfg: KimaiConfig = serde_yaml::from_str(yaml).expect("parses");
        cfg.auth.validate().expect("valid mechanism+bindings");
        assert_eq!(cfg.auth.mechanism, AuthMechanism::UserApiToken);
        assert_eq!(cfg.lookback_days, Some(30));
        assert_eq!(cfg.name.as_deref(), Some("Kimai"));
        assert!(matches!(
            cfg.auth.bindings[1].provider,
            CredentialProvider::Command { .. }
        ));
    }

    #[test]
    fn parses_bearer_token_config_with_defaults() {
        let yaml = r#"
url: https://kimai.example.invalid
auth:
  mechanism: bearer-token
  bindings:
    - field: token
      provider: { type: env, var: SYNTHETIC_KIMAI_TOKEN }
"#;
        let cfg: KimaiConfig = serde_yaml::from_str(yaml).expect("parses");
        cfg.auth.validate().expect("valid");
        assert_eq!(cfg.auth.mechanism, AuthMechanism::BearerToken);
        assert_eq!(cfg.lookback_days, None);
        assert_eq!(cfg.request_timeout_secs, None);
    }

    #[test]
    fn rejects_unknown_fields() {
        let yaml = "url: https://kimai.example.invalid\nfoo: bar\nauth:\n  mechanism: bearer-token\n  bindings:\n    - field: token\n      provider: { type: literal, value: x }\n";
        let res: Result<KimaiConfig, _> = serde_yaml::from_str(yaml);
        assert!(res.is_err(), "unknown top-level field must fail to parse");
    }
}
