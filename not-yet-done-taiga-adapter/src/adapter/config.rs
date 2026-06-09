//! Adapter YAML config.
//!
//! Auth lives in the unified `AuthSpec` from `not-yet-done-content` —
//! mechanism + bindings + session-cache policy. Pre-orchestrator
//! configs (`auth: { source: prompt }`) no longer parse and need to be
//! rewritten as `auth: { mechanism: password-login, bindings: [...] }`.

use serde::Deserialize;

use not_yet_done_content::AuthSpec;

/// Default per-request timeout (seconds) when `request_timeout_secs` is
/// not set in the YAML. Chosen as a balance: long enough that a slow but
/// healthy Taiga instance still answers, short enough that a dead
/// connection surfaces an error within a few breaths instead of freezing
/// the UI indefinitely.
pub(super) const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 20;

/// Upper bound (seconds) for the *derived* connect timeout when
/// `connect_timeout_secs` is not set: the connect phase gets
/// `min(request_timeout_secs, DEFAULT_CONNECT_TIMEOUT_CAP_SECS)` so an
/// unreachable host fails fast rather than eating the full request
/// budget just to open a socket. Only a cap on the *default* — an
/// explicit `connect_timeout_secs` overrides it (e.g. a high-latency
/// link where the TCP/TLS handshake legitimately needs longer).
pub(super) const DEFAULT_CONNECT_TIMEOUT_CAP_SECS: u64 = 10;

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub(super) struct TaigaConfig {
    pub(super) url: String,
    #[serde(default)]
    pub(super) name: Option<String>,
    pub(super) auth: AuthSpec,
    /// Optional override for the cache backing store. Sea-orm-compatible URL.
    /// Falls back to a private SQLite file under the user's local data dir.
    #[serde(default)]
    pub(super) db: Option<DbConfig>,
    /// Hard ceiling (seconds) on every HTTP request to the Taiga API,
    /// including the time spent establishing the connection.
    ///
    /// Why this exists: without a timeout a dead keep-alive socket (server
    /// silently dropped the connection, network went away mid-session)
    /// makes `reqwest` wait forever, and because the editor-open path
    /// blocks on that request the whole TUI freezes with no way out. The
    /// timeout converts "hangs forever" into "fails after N seconds", at
    /// which point the adapter reconnects + retries once and otherwise
    /// surfaces a normal error. Lower it on a fast LAN, raise it for a
    /// slow link. Defaults to [`DEFAULT_REQUEST_TIMEOUT_SECS`].
    #[serde(default)]
    pub(super) request_timeout_secs: Option<u64>,
    /// Hard ceiling (seconds) on just the connection-establishment phase
    /// (DNS + TCP + TLS), separate from the overall request budget above.
    ///
    /// Why a separate knob: by default this is derived as
    /// `min(request_timeout_secs, 10)` so an unreachable host fails fast.
    /// But on a high-latency or VPN-gated link the handshake can
    /// legitimately take longer than 10 s — there the derived cap would
    /// abort a *healthy* connection before it ever opens. Set this
    /// explicitly (e.g. `connect_timeout_secs: 30`) to lift that cap.
    /// Omit it on a normal LAN/WAN; the derived default is right there.
    #[serde(default)]
    pub(super) connect_timeout_secs: Option<u64>,
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
    fn parses_password_login_with_prompt_password() {
        let yaml = r#"
url: https://taiga.example.invalid
auth:
  mechanism: password-login
  bindings:
    - field: username
      provider:
        type: literal
        value: alice
    - field: password
      provider:
        type: prompt
"#;
        let cfg: TaigaConfig = serde_yaml::from_str(yaml).expect("parses");
        cfg.auth.validate().expect("valid mechanism+bindings");
        assert_eq!(cfg.auth.mechanism, AuthMechanism::PasswordLogin);
        assert_eq!(cfg.auth.session_cache, SessionCachePolicy::UntilRejected);
        assert_eq!(cfg.auth.bindings.len(), 2);
        assert!(matches!(
            cfg.auth.bindings[0].provider,
            CredentialProvider::Literal { .. }
        ));
        assert!(matches!(
            cfg.auth.bindings[1].provider,
            CredentialProvider::Prompt { .. }
        ));
    }

    #[test]
    fn parses_env_provided_credentials_with_explicit_ttl() {
        let yaml = r#"
url: https://taiga.example.invalid
auth:
  mechanism: password-login
  session_cache:
    kind: ttl
    ttl_secs: 3600
  bindings:
    - field: username
      provider: { type: env, var: SYNTHETIC_TAIGA_USER }
    - field: password
      provider: { type: env, var: SYNTHETIC_TAIGA_PW }
"#;
        let cfg: TaigaConfig = serde_yaml::from_str(yaml).expect("parses");
        cfg.auth.validate().expect("valid");
        assert_eq!(
            cfg.auth.session_cache,
            SessionCachePolicy::Ttl { ttl_secs: 3600 }
        );
    }

    #[test]
    fn rejects_legacy_source_tagged_auth() {
        let yaml = "url: https://taiga.example.invalid\nauth:\n  source: prompt\n";
        let res: Result<TaigaConfig, _> = serde_yaml::from_str(yaml);
        assert!(
            res.is_err(),
            "legacy `source: prompt` shape must fail under new AuthSpec"
        );
    }

    #[test]
    fn request_timeout_defaults_to_none_and_parses_when_set() {
        let base = r#"
url: https://taiga.example.invalid
auth:
  mechanism: password-login
  bindings:
    - field: username
      provider: { type: literal, value: alice }
    - field: password
      provider: { type: prompt }
"#;
        let cfg: TaigaConfig = serde_yaml::from_str(base).expect("parses");
        assert_eq!(
            cfg.request_timeout_secs, None,
            "absent key falls back to DEFAULT_REQUEST_TIMEOUT_SECS at use site"
        );

        let with_timeout = format!("{base}request_timeout_secs: 5\n");
        let cfg: TaigaConfig = serde_yaml::from_str(&with_timeout).expect("parses");
        assert_eq!(cfg.request_timeout_secs, Some(5));
        assert_eq!(
            cfg.connect_timeout_secs, None,
            "connect ceiling absent → derived as min(request, cap) at use site"
        );

        let with_connect = format!("{base}connect_timeout_secs: 30\n");
        let cfg: TaigaConfig = serde_yaml::from_str(&with_connect).expect("parses");
        assert_eq!(cfg.connect_timeout_secs, Some(30));
    }
}
