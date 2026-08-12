//! Generic auth-system primitives shared by all adapters.
//!
//! The system splits two orthogonal concerns:
//!
//! - **Mechanism**: what the *adapter* speaks against the remote API —
//!   `password-login`, `bearer-token`, `cookie`, … Which mechanisms exist
//!   and which fields each needs is **the adapter's** knowledge, published
//!   as a table of [`MechanismSpec`] from its factory; this crate only
//!   carries the descriptor types and checks a config against them
//!   ([`AuthSpec::validate_against`]). Adding a mechanism therefore never
//!   touches this crate.
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

use fieldsmith::Buildable;
use serde::Deserialize;

mod credential_script;
mod orchestrator;
mod resolver;
mod session_store;

pub use credential_script::{ScriptForm, ScriptFormField, ScriptRequest, ScriptRound};
pub use orchestrator::{AuthError, AuthOrchestrator, Clock, ResolvedSession, SystemClock};
pub use resolver::{
    CommandResolver, CredentialError, CredentialResolver, EnvResolver, FileResolver,
    KeyringResolver, LiteralResolver,
};
pub use session_store::{InMemorySessionStore, SessionEntry, SessionStore};

/// One input field a mechanism needs from the outside.
///
/// The adapter states what it needs; where the value comes from is the
/// user's choice, expressed per field as a [`CredentialProvider`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthFieldSpec {
    /// Wire name used in YAML (`- field: username`) and the key the
    /// adapter reads out of the resolved credential map.
    pub name: &'static str,
    /// Display label when this field is prompted for.
    pub label: &'static str,
    /// Whether input is masked (passwords, tokens, cookies).
    pub masked: bool,
    /// A `false` field may be omitted from the config entirely; it is
    /// then simply absent from the resolved credential map.
    pub required: bool,
}

impl AuthFieldSpec {
    /// A field the config must bind.
    pub const fn required(name: &'static str, label: &'static str, masked: bool) -> Self {
        Self {
            name,
            label,
            masked,
            required: true,
        }
    }

    /// A field the config may bind.
    pub const fn optional(name: &'static str, label: &'static str, masked: bool) -> Self {
        Self {
            name,
            label,
            masked,
            required: false,
        }
    }
}

/// One authentication mechanism an adapter implements, as published by
/// its factory (`auth_mechanisms()`).
///
/// This is what makes "which mechanisms exist" adapter-local: the core
/// crate never enumerates them, it only validates a config against the
/// table the adapter hands over, and the config wizard renders the same
/// table so the two cannot drift apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MechanismSpec {
    /// Wire id used in YAML (`mechanism: cookie`). Kebab-case.
    pub id: &'static str,
    /// Display name for the config wizard.
    pub label: &'static str,
    /// One line explaining when to pick this mechanism.
    pub doc: &'static str,
    /// The fields this mechanism needs from the outside.
    pub fields: &'static [AuthFieldSpec],
}

impl MechanismSpec {
    /// The declared field of that name, if the mechanism has one.
    pub fn field(&self, name: &str) -> Option<&AuthFieldSpec> {
        self.fields.iter().find(|f| f.name == name)
    }
}

/// `auth_token` / `auth-token` → `Auth Token`. The label a field gets
/// when nothing better was declared for it.
pub(crate) fn title_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut up = true;
    for c in name.chars() {
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

/// Comma-separated ids, for error messages that tell the user what they
/// could have written instead.
fn id_list(mechanisms: &[MechanismSpec]) -> String {
    mechanisms
        .iter()
        .map(|m| format!("`{}`", m.id))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Where a credential value comes from. Tagged by `type:` in YAML.
#[derive(Deserialize, Buildable, Clone, Debug, PartialEq, Eq)]
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
    /// Take this field's value out of what the auth block's `script`
    /// returns, which may ask the user something on the way.
    ///
    /// See [`AuthSpec::script`] and the
    /// [`credential_script`](crate::auth) protocol for the round shape.
    ///
    /// Carries no parameters on purpose: the binding's `field` name is
    /// the key looked up in the script's result, and the script itself is
    /// named once per auth block rather than once per field. Several
    /// fields therefore cost one invocation, not one each — which is the
    /// whole point when every invocation unlocks a password store.
    ScriptResult,
    /// OS keyring entry. On Linux this maps to the secret-service /
    /// libsecret backend (kwallet, gnome-keyring, …); on macOS to the
    /// Keychain; on Windows to Credential Manager.
    Keyring { service: String, account: String },
}

impl CredentialProvider {
    /// Whether resolving this provider needs the orchestrator rather than
    /// a standalone [`CredentialResolver`]: `prompt` because only the
    /// frontend can answer it, `script-result` because the script is
    /// shared by several bindings and may ask the frontend on the way.
    /// Both go through the
    /// [`AdapterStatus::NeedsCreds`](crate::AdapterStatus::NeedsCreds)
    /// contract; everything else builds its resolver up front.
    pub fn needs_frontend(&self) -> bool {
        matches!(
            self,
            CredentialProvider::Prompt { .. } | CredentialProvider::ScriptResult
        )
    }
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
fn default_script_timeout() -> u64 {
    120
}

/// One field of the active mechanism, paired with its provider. The
/// `field` name must match one the mechanism declares (see
/// [`MechanismSpec::fields`]).
#[derive(Deserialize, Buildable, Clone, Debug, PartialEq, Eq)]
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
        match &self.label {
            Some(l) => l.clone(),
            None => title_case(&self.field),
        }
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
#[derive(Deserialize, Buildable, Clone, Debug, PartialEq, Eq)]
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
#[derive(Deserialize, Buildable, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuthSpec {
    /// Id of the chosen mechanism, checked against the adapter's own
    /// table in [`AuthSpec::validate_against`] — this crate knows no
    /// mechanisms of its own.
    pub mechanism: String,
    #[serde(default)]
    pub session_cache: SessionCachePolicy,
    /// Shell command supplying every `script-result` binding at once.
    ///
    /// Sits here rather than on the provider because it is invoked
    /// **once** for all of them: several `script:` keys would read as
    /// several invocations, which is exactly what this is meant to avoid.
    /// Run through `sh -c`, so `~` and arguments work.
    #[serde(default)]
    pub script: Option<String>,
    /// Deadline for one round of the credential script. Generous by
    /// default — a round may unlock a password store or wait out an SSO
    /// bounce.
    #[serde(default = "default_script_timeout")]
    #[builder(default = 120)]
    pub script_timeout_secs: u64,
    pub bindings: Vec<CredentialBinding>,
}

impl AuthSpec {
    /// Check this config against the mechanisms an adapter publishes:
    /// the id must be one of them, and `bindings` must cover every
    /// required field of it, with no duplicates and nothing the
    /// mechanism does not declare.
    ///
    /// Factories call this while building an adapter from its config, so
    /// a mechanism the adapter cannot speak is rejected with the list of
    /// ones it can — instead of surfacing at the first login attempt.
    pub fn validate_against(&self, mechanisms: &[MechanismSpec]) -> Result<(), String> {
        let Some(m) = mechanisms.iter().find(|m| m.id == self.mechanism) else {
            if mechanisms.is_empty() {
                return Err(format!(
                    "mechanism `{}`: this adapter has no authentication",
                    self.mechanism
                ));
            }
            return Err(format!(
                "unknown mechanism `{}`; this adapter supports {}",
                self.mechanism,
                id_list(mechanisms)
            ));
        };

        let mut seen: Vec<&str> = Vec::with_capacity(self.bindings.len());
        for b in &self.bindings {
            if seen.contains(&b.field.as_str()) {
                return Err(format!("duplicate binding for field `{}`", b.field));
            }
            seen.push(b.field.as_str());
            if m.field(&b.field).is_none() {
                return Err(format!(
                    "field `{}` is not used by mechanism `{}`; expected one of {}",
                    b.field,
                    m.id,
                    m.fields
                        .iter()
                        .map(|f| format!("`{}`", f.name))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        for f in m.fields.iter().filter(|f| f.required) {
            if !seen.contains(&f.name) {
                return Err(format!(
                    "mechanism `{}` requires a binding for field `{}`",
                    m.id, f.name
                ));
            }
        }

        // `script` and `script-result` only mean anything together, and
        // half of the pair is a silent no-op: a script nobody reads, or a
        // binding waiting on a script that was never named. Both are
        // typos worth catching while the config is read.
        let uses_script = self
            .bindings
            .iter()
            .any(|b| matches!(b.provider, CredentialProvider::ScriptResult));
        match (&self.script, uses_script) {
            (None, true) => {
                return Err(
                    "a binding uses the `script-result` provider, but no `script` is set on \
                     the auth block"
                        .to_string(),
                );
            }
            (Some(_), false) => {
                return Err(
                    "`script` is set on the auth block, but no binding uses the \
                     `script-result` provider"
                        .to_string(),
                );
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stand-in for what an adapter publishes. Deliberately spelled out
    /// here rather than shared from the library: the point of the
    /// descriptors is that this table belongs to the adapter.
    const MECHANISMS: &[MechanismSpec] = &[
        MechanismSpec {
            id: "password-login",
            label: "Username and password",
            doc: "Log in with username and password, keep the derived session.",
            fields: &[
                AuthFieldSpec::required("username", "Username", false),
                AuthFieldSpec::required("password", "Password", true),
                AuthFieldSpec::optional("otp", "One-time code", true),
            ],
        },
        MechanismSpec {
            id: "bearer-token",
            label: "Bearer token",
            doc: "Send a static token in the Authorization header.",
            fields: &[AuthFieldSpec::required("token", "Token", true)],
        },
        MechanismSpec {
            id: "cookie",
            label: "Session cookie",
            doc: "Send a ready-made Cookie header.",
            fields: &[AuthFieldSpec::required("cookie", "Cookie header", true)],
        },
        MechanismSpec {
            id: "basic-auth",
            label: "HTTP Basic",
            doc: "Send username and token via HTTP Basic.",
            fields: &[
                AuthFieldSpec::required("username", "Username", false),
                AuthFieldSpec::required("token", "Token", true),
            ],
        },
        MechanismSpec {
            id: "user-api-token",
            label: "User + API token",
            doc: "Send username and token in adapter-defined headers.",
            fields: &[
                AuthFieldSpec::required("username", "Username", false),
                AuthFieldSpec::required("token", "Token", true),
            ],
        },
    ];

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
        spec.validate_against(MECHANISMS).expect("valid");
        assert_eq!(spec.mechanism, "password-login");
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
        spec.validate_against(MECHANISMS).expect("valid");
        match &spec.bindings[0].provider {
            CredentialProvider::Command {
                script,
                timeout_secs,
                retries,
            } => {
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
        spec.validate_against(MECHANISMS).expect("valid");
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
        spec.validate_against(MECHANISMS).expect("valid");
        assert_eq!(spec.mechanism, "basic-auth");
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
        spec.validate_against(MECHANISMS).expect("valid");
        assert_eq!(spec.mechanism, "user-api-token");
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
        let err = spec
            .validate_against(MECHANISMS)
            .expect_err("should reject missing username");
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
        let err = spec
            .validate_against(MECHANISMS)
            .expect_err("should reject missing password");
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
        let err = spec
            .validate_against(MECHANISMS)
            .expect_err("should reject username on bearer-token");
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
        let err = spec
            .validate_against(MECHANISMS)
            .expect_err("should reject duplicate username");
        assert!(err.contains("duplicate"), "error mentions duplicate: {err}");
    }

    /// Goal 3: a mechanism the adapter does not implement is rejected
    /// while its config is read, and the message names what it does
    /// implement — the user should not have to grep the source.
    #[test]
    fn rejects_a_mechanism_this_adapter_does_not_implement() {
        let yaml = r#"
mechanism: kerberos
bindings:
  - field: token
    provider: { type: literal, value: x }
"#;
        let spec = parse(yaml);
        let cookie_only = &MECHANISMS[2..3];
        let err = spec
            .validate_against(cookie_only)
            .expect_err("should reject kerberos");
        assert!(err.contains("kerberos"), "names the rejected id: {err}");
        assert!(err.contains("`cookie`"), "names the supported ids: {err}");
    }

    /// An adapter without authentication says so instead of listing an
    /// empty set of alternatives.
    #[test]
    fn rejects_any_mechanism_when_the_adapter_has_no_auth() {
        let yaml = r#"
mechanism: cookie
bindings:
  - field: cookie
    provider: { type: literal, value: x }
"#;
        let err = parse(yaml)
            .validate_against(&[])
            .expect_err("should reject");
        assert!(err.contains("no authentication"), "got: {err}");
    }

    #[test]
    fn optional_fields_may_be_omitted_and_may_be_bound() {
        let without = r#"
mechanism: password-login
bindings:
  - field: username
    provider: { type: literal, value: alice }
  - field: password
    provider: { type: prompt }
"#;
        parse(without)
            .validate_against(MECHANISMS)
            .expect("otp is optional");

        let with = r#"
mechanism: password-login
bindings:
  - field: username
    provider: { type: literal, value: alice }
  - field: password
    provider: { type: prompt }
  - field: otp
    provider: { type: prompt }
"#;
        parse(with)
            .validate_against(MECHANISMS)
            .expect("otp may be bound");
    }

    /// The motivating shape: one script, two fields, one invocation.
    #[test]
    fn one_script_feeds_several_script_result_bindings() {
        let yaml = r#"
mechanism: user-api-token
script: ~/.config/not_yet_done/scripts/pass_credentials.py timetrack
bindings:
  - field: username
    provider: { type: script-result }
  - field: token
    provider: { type: script-result }
"#;
        let spec = parse(yaml);
        spec.validate_against(MECHANISMS).expect("valid");
        assert_eq!(
            spec.script.as_deref(),
            Some("~/.config/not_yet_done/scripts/pass_credentials.py timetrack")
        );
        assert_eq!(spec.script_timeout_secs, 120);
        assert!(
            spec.bindings
                .iter()
                .all(|b| b.provider == CredentialProvider::ScriptResult)
        );
        assert!(spec.bindings[0].provider.needs_frontend());
    }

    /// Half a pair is a typo, not a configuration: the binding would wait
    /// on a script nobody named.
    #[test]
    fn script_result_without_a_script_is_rejected() {
        let yaml = r#"
mechanism: bearer-token
bindings:
  - field: token
    provider: { type: script-result }
"#;
        let err = parse(yaml)
            .validate_against(MECHANISMS)
            .expect_err("should reject");
        assert!(err.contains("no `script` is set"), "got: {err}");
    }

    /// …and the other half is a script nobody reads.
    #[test]
    fn a_script_nothing_binds_to_is_rejected() {
        let yaml = r#"
mechanism: bearer-token
script: /opt/get-token.sh
bindings:
  - field: token
    provider: { type: literal, value: x }
"#;
        let err = parse(yaml)
            .validate_against(MECHANISMS)
            .expect_err("should reject");
        assert!(err.contains("no binding uses"), "got: {err}");
    }

    #[test]
    fn only_prompt_and_script_result_need_a_frontend() {
        assert!(CredentialProvider::Prompt { prefill: None }.needs_frontend());
        assert!(CredentialProvider::ScriptResult.needs_frontend());
        assert!(
            !CredentialProvider::Command {
                script: "x".into(),
                timeout_secs: 1,
                retries: 1,
            }
            .needs_frontend()
        );
        assert!(!CredentialProvider::Literal { value: "x".into() }.needs_frontend());
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
