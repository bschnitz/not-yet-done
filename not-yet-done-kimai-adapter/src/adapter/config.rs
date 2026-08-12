//! Adapter YAML config.

use fieldsmith::Buildable;
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

#[derive(Deserialize, Buildable, Debug)]
#[serde(deny_unknown_fields)]
pub struct KimaiConfig {
    pub(super) url: String,
    #[serde(default)]
    pub(super) name: Option<String>,
    /// Mechanism + bindings. The mechanisms this adapter implements and
    /// the fields each one needs are published by its factory (see
    /// `auth_bridge::MECHANISMS`) and listed by `nyd config auth kimai`
    /// — repeating them here would only drift from that table.
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
    use crate::adapter::auth_bridge::MECHANISMS;
    use not_yet_done_content::CredentialProvider;

    /// The example under `docs/examples/views/` is the first thing a user
    /// copies, so it has to parse and validate like any real config.
    #[test]
    fn the_shipped_example_config_parses() {
        let yaml = include_str!("../../../docs/examples/views/kimai-adapter.yaml");
        let cfg: KimaiConfig = serde_yaml::from_str(yaml).expect("example parses");
        cfg.auth
            .validate_against(MECHANISMS)
            .expect("example is a valid spec");
    }

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
        cfg.auth
            .validate_against(MECHANISMS)
            .expect("valid mechanism+bindings");
        assert_eq!(cfg.auth.mechanism, "user-api-token");
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
        cfg.auth.validate_against(MECHANISMS).expect("valid");
        assert_eq!(cfg.auth.mechanism, "bearer-token");
        assert_eq!(cfg.lookback_days, None);
        assert_eq!(cfg.request_timeout_secs, None);
    }

    #[test]
    fn rejects_unknown_fields() {
        let yaml = "url: https://kimai.example.invalid\nfoo: bar\nauth:\n  mechanism: bearer-token\n  bindings:\n    - field: token\n      provider: { type: literal, value: x }\n";
        let res: Result<KimaiConfig, _> = serde_yaml::from_str(yaml);
        assert!(res.is_err(), "unknown top-level field must fail to parse");
    }

    /// The config parses fine on its own — what rejects it is the
    /// adapter's mechanism table, so a mechanism this adapter never
    /// implements fails at build time instead of at first login.
    #[test]
    fn rejects_a_mechanism_this_adapter_does_not_implement() {
        let yaml = r#"
url: https://kimai.example.invalid
auth:
  mechanism: cookie
  bindings:
    - field: cookie
      provider: { type: literal, value: x }
"#;
        let cfg: KimaiConfig = serde_yaml::from_str(yaml).expect("parses");
        let err = cfg
            .auth
            .validate_against(MECHANISMS)
            .expect_err("mechanism is not implemented here");
        assert!(
            err.contains("cookie"),
            "names the rejected mechanism: {err}"
        );
        assert!(
            err.contains("user-api-token"),
            "names a supported one: {err}"
        );
    }
}
