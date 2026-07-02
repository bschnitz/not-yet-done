//! Generic auth-system primitives shared by all adapters.
//!
//! The system splits two orthogonal concerns:
//!
//! - **Mechanism** ([`AuthMechanism`]): what the *adapter* speaks against
//!   the remote API — `password-login`, `bearer-token`, `cookie`, etc.
//!   The adapter declares which one it implements.
//!
//! - **Provider** ([`CredentialProvider`]): where the *runtime* fetches
//!   the credential value from — literal config, prompt, env var, file,
//!   shell command, or OS keyring. Per-field, since one mechanism may
//!   need several fields (username + password) from different sources.
//!
//! On top of that sits [`SessionCachePolicy`] which controls the lifetime
//! of any session token the adapter *derives* from the credentials (e.g.
//! a Taiga JWT). Primary credentials persist through the provider's own
//! storage (keyring entry, file on disk); derived sessions are managed
//! by the orchestrator.

use std::path::PathBuf;

use serde::Deserialize;

mod orchestrator;
mod resolver;
mod session_store;

pub use orchestrator::{AuthError, AuthOrchestrator, Clock, ResolvedSession, SystemClock};
pub use resolver::{
    CommandResolver, CredentialError, CredentialResolver, EnvResolver, FileResolver,
    KeyringResolver, LiteralResolver,
};
pub use session_store::{InMemorySessionStore, SessionEntry, SessionStore};

/// What the adapter speaks against the remote API. The adapter implements
/// exactly one mechanism; the runtime picks fields and ordering from
/// [`AuthSpec::bindings`].
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMechanism {
    /// Adapter exchanges username + password for a derived session
    /// token (JWT, cookie, …) via a login round-trip. Required fields:
    /// `username`, `password`.
    PasswordLogin,
    /// Adapter sends a static bearer token in the `Authorization` header.
    /// Required field: `token`.
    BearerToken,
    /// Adapter sends a static cookie value in the `Cookie` header.
    /// Required field: `cookie`.
    Cookie,
    /// Adapter sends user + token via HTTP Basic. Required fields:
    /// `username`, `token`.
    BasicAuth,
    /// Adapter sends username + a static API token in adapter-defined
    /// request headers (e.g. Kimai's `X-AUTH-USER` / `X-AUTH-TOKEN`
    /// pair). Like `basic-auth` in its field set, but the wire format is
    /// the adapter's own, not HTTP Basic. Required fields: `username`,
    /// `token`.
    UserApiToken,
}

/// Where a credential value comes from. Tagged by `type:` in YAML.
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CredentialProvider {
    /// Value baked into the YAML config. Convenient for non-secret
    /// fields (email, username) and bootstrap; not recommended for
    /// passwords or long-lived tokens.
    Literal { value: String },
    /// User types the value into the TUI on first use. Optional `prefill`
    /// pre-populates the input (e.g. a username already known from the
    /// config).
    Prompt {
        #[serde(default)]
        prefill: Option<String>,
    },
    /// Read from an environment variable. Empty values count as missing.
    Env { var: String },
    /// Read the value from a file. The trailing newline is stripped by
    /// default — set `trim: false` to keep raw bytes.
    File {
        path: PathBuf,
        #[serde(default = "default_file_trim")]
        trim: bool,
    },
    /// Run a shell command and capture stdout as the value. Used for
    /// integrations with `pass`, `op`, `gopass`, `bw`, custom SSO
    /// scripts, etc. The command must exit 0 within `timeout_secs`,
    /// otherwise the runtime retries up to `retries` times.
    Command {
        script: String,
        #[serde(default = "default_command_timeout")]
        timeout_secs: u64,
        #[serde(default = "default_command_retries")]
        retries: u32,
    },
    /// OS keyring entry. On Linux this maps to the secret-service /
    /// libsecret backend (kwallet, gnome-keyring, …); on macOS to the
    /// Keychain; on Windows to Credential Manager.
    Keyring { service: String, account: String },
}

fn default_file_trim() -> bool {
    true
}
fn default_command_timeout() -> u64 {
    30
}
fn default_command_retries() -> u32 {
    3
}

/// One field of the active mechanism, paired with its provider. The
/// `field` name must match what the mechanism expects (see
/// [`AuthMechanism`] docs).
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CredentialBinding {
    pub field: String,
    pub provider: CredentialProvider,
    /// Display label when this field is prompted. Defaults to a
    /// title-cased version of `field`.
    #[serde(default)]
    pub label: Option<String>,
    /// Whether the input should be masked (passwords, tokens). Defaults
    /// based on field name: `password`, `token`, `secret`, `cookie` →
    /// masked; everything else → not masked.
    #[serde(default)]
    pub masked: Option<bool>,
}

impl CredentialBinding {
    /// Resolved display label, applying the convention-based default.
    pub fn effective_label(&self) -> String {
        if let Some(l) = &self.label {
            return l.clone();
        }
        let mut out = String::with_capacity(self.field.len());
        let mut up = true;
        for c in self.field.chars() {
            if c == '_' || c == '-' {
                out.push(' ');
                up = true;
            } else if up {
                out.extend(c.to_uppercase());
                up = false;
            } else {
                out.push(c);
            }
        }
        out
    }

    /// Resolved masked flag, applying the convention-based default.
    pub fn effective_masked(&self) -> bool {
        if let Some(m) = self.masked {
            return m;
        }
        matches!(
            self.field.as_str(),
            "password" | "token" | "secret" | "cookie" | "api_key" | "api-key"
        )
    }
}

/// Lifetime policy for the session token an adapter derives from
/// credentials (JWT, login cookie, …). Has no effect on adapters whose
/// mechanism doesn't derive a session (`bearer-token`, `cookie`,
/// `basic-auth` with literal providers).
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SessionCachePolicy {
    /// (a) In-memory only. Session is rebuilt every adapter start; no
    /// disk persistence.
    None,
    /// (b) Persist; expire after `ttl_secs` of wall-clock age, even
    /// across restarts.
    Ttl { ttl_secs: u64 },
    /// (c) Persist; expire after `ttl_secs` *or* on app close —
    /// whichever comes first.
    TtlOrClose { ttl_secs: u64 },
    /// (d) Persist forever; only invalidated when the server rejects
    /// the session (HTTP 401 / 403). Default policy.
    UntilRejected,
    /// (e) Persist forever; only invalidated by an explicit user action
    /// (`forget session`).
    Explicit,
}

impl Default for SessionCachePolicy {
    fn default() -> Self {
        SessionCachePolicy::UntilRejected
    }
}

/// Top-level auth section in an adapter's YAML config.
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuthSpec {
    pub mechanism: AuthMechanism,
    #[serde(default)]
    pub session_cache: SessionCachePolicy,
    pub bindings: Vec<CredentialBinding>,
}

impl AuthSpec {
    /// Verify that `bindings` covers exactly the fields the mechanism
    /// requires, with no duplicates and no extras. Adapters call this
    /// during config parsing; failures abort startup with a clear
    /// message instead of failing later at auth time.
    pub fn validate(&self) -> Result<(), String> {
        let required: &[&str] = match self.mechanism {
            AuthMechanism::PasswordLogin => &["username", "password"],
            AuthMechanism::BearerToken => &["token"],
            AuthMechanism::Cookie => &["cookie"],
            AuthMechanism::BasicAuth => &["username", "token"],
            AuthMechanism::UserApiToken => &["username", "token"],
        };

        let mut seen: Vec<&str> = Vec::with_capacity(self.bindings.len());
        for b in &self.bindings {
            if seen.contains(&b.field.as_str()) {
                return Err(format!("duplicate binding for field `{}`", b.field));
            }
            seen.push(b.field.as_str());
            if !required.contains(&b.field.as_str()) {
                return Err(format!(
                    "field `{}` is not used by mechanism `{:?}`; expected one of {:?}",
                    b.field, self.mechanism, required
                ));
            }
        }
        for r in required {
            if !seen.contains(r) {
                return Err(format!(
                    "mechanism `{:?}` requires a binding for field `{}`",
                    self.mechanism, r
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> AuthSpec {
        serde_yaml::from_str(yaml).expect("yaml parses")
    }

    #[test]
    fn password_login_with_keyring_username_and_prompt_password() {
        let yaml = r#"
mechanism: password-login
session_cache:
  kind: until-rejected
bindings:
  - field: username
    provider:
      type: keyring
      service: nyd_taiga
      account: alice
  - field: password
    provider:
      type: prompt
"#;
        let spec = parse(yaml);
        spec.validate().expect("valid");
        assert_eq!(spec.mechanism, AuthMechanism::PasswordLogin);
        assert_eq!(spec.session_cache, SessionCachePolicy::UntilRejected);
        assert_eq!(spec.bindings.len(), 2);
        assert!(matches!(
            spec.bindings[0].provider,
            CredentialProvider::Keyring { .. }
        ));
        assert!(matches!(
            spec.bindings[1].provider,
            CredentialProvider::Prompt { prefill: None }
        ));
    }

    #[test]
    fn cookie_via_command_with_explicit_timeout() {
        let yaml = r#"
mechanism: cookie
session_cache:
  kind: none
bindings:
  - field: cookie
    provider:
      type: command
      script: /opt/get-cookie.sh
      timeout_secs: 10
      retries: 5
"#;
        let spec = parse(yaml);
        spec.validate().expect("valid");
        match &spec.bindings[0].provider {
            CredentialProvider::Command { script, timeout_secs, retries } => {
                assert_eq!(script, "/opt/get-cookie.sh");
                assert_eq!(*timeout_secs, 10);
                assert_eq!(*retries, 5);
            }
            other => panic!("unexpected provider: {other:?}"),
        }
    }

    #[test]
    fn bearer_token_with_env_provider() {
        let yaml = r#"
mechanism: bearer-token
bindings:
  - field: token
    provider:
      type: env
      var: SYNTHETIC_API_TOKEN
"#;
        let spec = parse(yaml);
        spec.validate().expect("valid");
        // session_cache default applies.
        assert_eq!(spec.session_cache, SessionCachePolicy::UntilRejected);
        match &spec.bindings[0].provider {
            CredentialProvider::Env { var } => assert_eq!(var, "SYNTHETIC_API_TOKEN"),
            other => panic!("unexpected provider: {other:?}"),
        }
    }

    #[test]
    fn basic_auth_with_literal_email_and_keyring_token() {
        let yaml = r#"
mechanism: basic-auth
bindings:
  - field: username
    provider:
      type: literal
      value: alice@example.invalid
  - field: token
    provider:
      type: keyring
      service: nyd_jira
      account: alice
"#;
        let spec = parse(yaml);
        spec.validate().expect("valid");
        assert_eq!(spec.mechanism, AuthMechanism::BasicAuth);
    }

    #[test]
    fn user_api_token_with_command_providers() {
        let yaml = r#"
mechanism: user-api-token
bindings:
  - field: username
    provider:
      type: command
      script: secret-tool lookup service timetrack field user
  - field: token
    provider:
      type: command
      script: secret-tool lookup service timetrack field token
"#;
        let spec = parse(yaml);
        spec.validate().expect("valid");
        assert_eq!(spec.mechanism, AuthMechanism::UserApiToken);
    }

    #[test]
    fn user_api_token_requires_both_fields() {
        let yaml = r#"
mechanism: user-api-token
bindings:
  - field: token
    provider: { type: literal, value: x }
"#;
        let spec = parse(yaml);
        let err = spec.validate().expect_err("should reject missing username");
        assert!(err.contains("username"), "error mentions username: {err}");
    }

    #[test]
    fn ttl_policy_with_seconds() {
        let yaml = r#"
mechanism: password-login
session_cache:
  kind: ttl
  ttl_secs: 28800
bindings:
  - field: username
    provider: { type: literal, value: alice }
  - field: password
    provider: { type: prompt }
"#;
        let spec = parse(yaml);
        assert_eq!(
            spec.session_cache,
            SessionCachePolicy::Ttl { ttl_secs: 28800 }
        );
    }

    #[test]
    fn rejects_missing_required_field() {
        let yaml = r#"
mechanism: password-login
bindings:
  - field: username
    provider: { type: literal, value: alice }
"#;
        let spec = parse(yaml);
        let err = spec.validate().expect_err("should reject missing password");
        assert!(err.contains("password"), "error mentions password: {err}");
    }

    #[test]
    fn rejects_extra_field_for_mechanism() {
        let yaml = r#"
mechanism: bearer-token
bindings:
  - field: token
    provider: { type: literal, value: x }
  - field: username
    provider: { type: literal, value: alice }
"#;
        let spec = parse(yaml);
        let err = spec.validate().expect_err("should reject username on bearer-token");
        assert!(err.contains("username"), "error mentions username: {err}");
    }

    #[test]
    fn rejects_duplicate_field() {
        let yaml = r#"
mechanism: password-login
bindings:
  - field: username
    provider: { type: literal, value: alice }
  - field: username
    provider: { type: prompt }
  - field: password
    provider: { type: prompt }
"#;
        let spec = parse(yaml);
        let err = spec.validate().expect_err("should reject duplicate username");
        assert!(err.contains("duplicate"), "error mentions duplicate: {err}");
    }

    #[test]
    fn rejects_unknown_provider_type() {
        let yaml = r#"
mechanism: bearer-token
bindings:
  - field: token
    provider: { type: clipboard }
"#;
        let res: Result<AuthSpec, _> = serde_yaml::from_str(yaml);
        assert!(res.is_err(), "unknown provider variant must fail to parse");
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let yaml = r#"
mechanism: bearer-token
foo: bar
bindings:
  - field: token
    provider: { type: literal, value: x }
"#;
        let res: Result<AuthSpec, _> = serde_yaml::from_str(yaml);
        assert!(res.is_err(), "unknown top-level field must fail to parse");
    }

    #[test]
    fn effective_label_title_cases_field_name() {
        let b = CredentialBinding {
            field: "auth_token".into(),
            provider: CredentialProvider::Prompt { prefill: None },
            label: None,
            masked: None,
        };
        assert_eq!(b.effective_label(), "Auth Token");
    }

    #[test]
    fn effective_label_explicit_overrides() {
        let b = CredentialBinding {
            field: "username".into(),
            provider: CredentialProvider::Prompt { prefill: None },
            label: Some("Login".into()),
            masked: None,
        };
        assert_eq!(b.effective_label(), "Login");
    }

    #[test]
    fn effective_masked_defaults_by_field_name() {
        let p = CredentialBinding {
            field: "password".into(),
            provider: CredentialProvider::Prompt { prefill: None },
            label: None,
            masked: None,
        };
        assert!(p.effective_masked());

        let u = CredentialBinding {
            field: "username".into(),
            provider: CredentialProvider::Prompt { prefill: None },
            label: None,
            masked: None,
        };
        assert!(!u.effective_masked());
    }

    #[test]
    fn effective_masked_explicit_overrides_default() {
        let b = CredentialBinding {
            field: "username".into(),
            provider: CredentialProvider::Prompt { prefill: None },
            label: None,
            masked: Some(true),
        };
        assert!(b.effective_masked());
    }

    #[test]
    fn file_provider_defaults() {
        let yaml = r#"
mechanism: bearer-token
bindings:
  - field: token
    provider:
      type: file
      path: /tmp/synthetic-token
"#;
        let spec = parse(yaml);
        match &spec.bindings[0].provider {
            CredentialProvider::File { path, trim } => {
                assert_eq!(path.to_str().unwrap(), "/tmp/synthetic-token");
                assert!(*trim, "trim defaults to true");
            }
            other => panic!("unexpected provider: {other:?}"),
        }
    }
}
