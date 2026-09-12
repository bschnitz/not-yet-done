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
//!   shell command, an auth plugin, or the OS keyring. Per-field, since
//!   one mechanism may need several fields (username + password) from
//!   different sources.
//!
//! A provider is never a layer everything passes through: an adapter whose
//! login fits in Rust publishes a mechanism for it (`password-login`
//! derives its own session) and meets none of this.
//!
//! On top of that sits [`SessionCachePolicy`] which controls the lifetime
//! of any session token the adapter *derives* from the credentials (e.g.
//! a Taiga JWT). Primary credentials persist through the provider's own
//! storage (keyring entry, file on disk); derived sessions are managed
//! by the orchestrator.

use std::path::PathBuf;

use fieldsmith::Buildable;
use serde::Deserialize;

mod auth_plugin;
mod credential_script;
mod orchestrator;
mod resolver;
mod session_store;

pub use auth_plugin::{
    PROTOCOL as PLUGIN_PROTOCOL, PluginLine, PluginSaid, PluginSession, ToPlugin,
};
pub use credential_script::{ScriptForm, ScriptFormField, ScriptRequest, ScriptRound};
pub use orchestrator::{AuthError, AuthOrchestrator, Clock, ResolvedSession, SystemClock};
pub use resolver::{
    CommandResolver, CredentialError, CredentialPrompts, CredentialResolver, EnvResolver,
    FileResolver, KeyringResolver, LiteralResolver, ScriptResolver,
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

/// Comma-separated names, for error messages that tell the user what they
/// could have written instead.
fn id_list_of(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
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
    /// Run a credential script for *this one slot*, speaking the same
    /// round protocol as [`ScriptResult`](Self::ScriptResult) (see the
    /// [`credential_script`](crate::auth) module).
    ///
    /// The difference is where it may be written: `script-result` needs
    /// an `auth:` block whose orchestrator owns the dialog, this one
    /// needs nothing but a provider slot — so a connection config that
    /// has no auth block (a calendar backend, an SSH hop) can still ask
    /// the user for the password store's passphrase instead of letting
    /// `gpg` open its own `pinentry` window behind the frontend's back.
    ///
    /// The question travels as a [`PromptRequest`](crate::PromptRequest)
    /// on the adapter's prompt stream, so it needs an adapter that wired
    /// one up. Without a stream an *unlocked* store still resolves; a
    /// locked one fails loudly rather than hanging.
    Script {
        script: String,
        /// Which key of the script's `result` this slot takes. May be
        /// omitted when the script returns exactly one value — with
        /// several, naming one is the only way to say which.
        #[serde(default)]
        field: Option<String>,
        #[serde(default = "default_script_timeout")]
        timeout_secs: u64,
    },
    /// Take this field's value from an [`auth plugin`](crate::auth::PluginSession)
    /// named in the auth block's `plugins:` — a helper process that stays
    /// alive for the whole login, reports what it is doing while it runs,
    /// and may say that it is waiting on the user doing something
    /// elsewhere (see ADR 0010).
    ///
    /// For the login a Rust library cannot perform: an interactive SSO
    /// bounce that only a real browser gets through. The round protocol
    /// behind [`ScriptResult`](Self::ScriptResult) cannot serve it, since
    /// it starts a fresh process per round and a half-finished browser
    /// session does not survive one.
    ///
    /// `use` names the entry rather than repeating the command, so
    /// several fields of one login share a single invocation — the same
    /// economy `script-result` exists for, and the reason one browser
    /// opens instead of three.
    Plugin {
        #[serde(rename = "use")]
        name: String,
    },
    /// OS keyring entry. On Linux this maps to the secret-service /
    /// libsecret backend (kwallet, gnome-keyring, …); on macOS to the
    /// Keychain; on Windows to Credential Manager.
    Keyring { service: String, account: String },
}

impl CredentialProvider {
    /// What to tell the user while this provider is being asked for
    /// `field` — the login's slowest step is usually one of these, and
    /// "Connecting…" alone does not say which.
    ///
    /// Worded from the user's side: they know they configured a script for
    /// their cookie, not that a `CommandResolver` is spawning a shell.
    pub fn progress_step(&self, field: &str) -> String {
        match self {
            Self::Literal { .. } => format!("reading {field} from the config"),
            Self::Prompt { .. } => format!("waiting for {field}"),
            Self::Env { var } => format!("reading {field} from ${var}"),
            Self::File { .. } => format!("reading {field} from its file"),
            Self::Command { .. } => format!("running the {field} script"),
            Self::ScriptResult | Self::Script { .. } => {
                format!("asking the credential script for {field}")
            }
            // Only until the plugin says otherwise: it names its own steps
            // from here on, which is the whole point of it.
            Self::Plugin { name } => format!("starting the `{name}` auth plugin for {field}"),
            Self::Keyring { .. } => format!("reading {field} from the keyring"),
        }
    }

    /// The attempt count and deadline this provider runs under, as
    /// `(max_retries, timeout_secs)`. `(1, 0)` means neither limit exists —
    /// reading a file has no deadline worth showing, while a script that may
    /// wait minutes for a browser login has both.
    pub fn progress_limits(&self) -> (u32, u64) {
        match self {
            Self::Command {
                timeout_secs,
                retries,
                ..
            } => ((*retries).max(1), *timeout_secs),
            Self::Script { timeout_secs, .. } => (1, *timeout_secs),
            _ => (1, 0),
        }
    }

    /// Whether resolving this provider needs the orchestrator rather than
    /// a standalone [`CredentialResolver`]: `prompt` because only the
    /// frontend can answer it, `script-result` and `plugin` because the
    /// helper is shared by several bindings and may ask the frontend on
    /// the way. All three go through the
    /// [`AdapterStatus::NeedsCreds`](crate::AdapterStatus::NeedsCreds)
    /// contract; everything else builds its resolver up front.
    ///
    /// A plugin needs the orchestrator for a second reason: it reports its
    /// steps while it runs, and the orchestrator is what holds the
    /// [`StatusReporter`](crate::StatusReporter) they are reported on.
    pub fn needs_frontend(&self) -> bool {
        matches!(
            self,
            CredentialProvider::Prompt { .. }
                | CredentialProvider::ScriptResult
                | CredentialProvider::Plugin { .. }
        )
    }

    /// Whether this provider may raise a dialog of its own while it
    /// resolves. Unlike [`needs_frontend`](Self::needs_frontend) this is
    /// not a demand for the orchestrator — the resolver is built like any
    /// other — but the cue for an adapter to offer a
    /// [`PromptRequest`](crate::PromptRequest) stream at all, so a
    /// connection that never asks contributes no stream.
    pub fn can_prompt(&self) -> bool {
        matches!(self, CredentialProvider::Script { .. })
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
fn default_plugin_timeout() -> u64 {
    60
}
fn default_plugin_attention_timeout() -> u64 {
    300
}

/// One auth plugin the auth block may draw on.
///
/// A list of named entries rather than a YAML map because `bindings:`
/// already keys by a name inside the item, and because the schema a
/// config wizard is generated from has no map shape — only scalars,
/// nested types and lists.
#[derive(Deserialize, Buildable, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginSpec {
    /// What a binding points at with `provider: { type: plugin, use: … }`.
    pub name: String,
    /// The command line, run through `sh -c`, so `~` and arguments work.
    pub command: String,
    /// Deadline for one **step**, refreshed by every step the plugin
    /// reports: progress is the heartbeat, and a separate liveness ping
    /// would be a second truth about the same thing. A whole-login
    /// deadline would have to be as long as the slowest login and would
    /// then forgive a plugin that hung on its first step.
    #[serde(default = "default_plugin_timeout")]
    #[builder(default = 60)]
    pub timeout_secs: u64,
    /// Deadline while the plugin says it is waiting on the user. Longer,
    /// because a person at a second factor takes as long as they take and
    /// a login must not fail underneath them.
    #[serde(default = "default_plugin_attention_timeout")]
    #[builder(default = 300)]
    pub attention_timeout_secs: u64,
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
    /// The auth plugins bindings may name with `use:`.
    ///
    /// A table rather than one `plugin:` key beside `script:`, because one
    /// adapter may want a browser for its session cookie and something
    /// else entirely for another slot. Fields naming the same entry are
    /// served by one invocation; fields naming different entries get
    /// different plugins.
    #[serde(default)]
    pub plugins: Vec<PluginSpec>,
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

        self.validate_plugins()?;
        Ok(())
    }

    /// The same pairing check for `plugins:` and the `plugin` provider,
    /// plus the two invariants a named table brings with it: a name is
    /// what a binding points at, so an empty or a repeated one leaves a
    /// binding pointing at nothing or at either of two things.
    fn validate_plugins(&self) -> Result<(), String> {
        let mut seen: Vec<&str> = Vec::with_capacity(self.plugins.len());
        for plugin in &self.plugins {
            if plugin.name.trim().is_empty() {
                return Err("a plugin on the auth block has no `name`".to_string());
            }
            if seen.contains(&plugin.name.as_str()) {
                return Err(format!("duplicate plugin `{}`", plugin.name));
            }
            seen.push(&plugin.name);
            if plugin.command.trim().is_empty() {
                return Err(format!("plugin `{}` has no `command`", plugin.name));
            }
        }

        let mut used: Vec<&str> = Vec::new();
        for binding in &self.bindings {
            let CredentialProvider::Plugin { name } = &binding.provider else {
                continue;
            };
            if !seen.contains(&name.as_str()) {
                return Err(format!(
                    "field `{}` uses the plugin `{name}`, which the auth block does not \
                     declare{}",
                    binding.field,
                    if seen.is_empty() {
                        String::new()
                    } else {
                        format!("; it declares {}", id_list_of(&seen))
                    }
                ));
            }
            if !used.contains(&name.as_str()) {
                used.push(name);
            }
        }

        // A declared plugin nobody uses is a silent no-op, the same typo
        // `script` without `script-result` is.
        if let Some(idle) = seen.iter().find(|name| !used.contains(*name)) {
            return Err(format!(
                "plugin `{idle}` is declared on the auth block, but no binding uses it"
            ));
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

    /// The shape a connection config writes when it has no `auth:` block
    /// at all: one slot, one script, everything else defaulted.
    #[test]
    fn standalone_script_provider_parses_with_defaults() {
        let yaml = r#"
type: script
script: >-
  ~/.config/not_yet_done/scripts/pass_credentials.py
  password=example/service/pass
"#;
        let provider: CredentialProvider = serde_yaml::from_str(yaml).expect("yaml parses");
        match &provider {
            CredentialProvider::Script {
                script,
                field,
                timeout_secs,
            } => {
                assert!(script.ends_with("password=example/service/pass"));
                assert!(field.is_none(), "one value needs no name");
                assert_eq!(*timeout_secs, 120);
            }
            other => panic!("unexpected provider: {other:?}"),
        }
        // It may ask, but an unlocked store answers without anyone
        // listening — so it is not in the "needs a frontend" set.
        assert!(provider.can_prompt());
        assert!(!provider.needs_frontend());
    }

    // --- auth plugins (ADR 0010) ------------------------------------------

    /// The shape the `cookie` mechanism's own doc has always promised: an
    /// SSO login fetched by a helper, with the adapter never talking to a
    /// browser itself.
    #[test]
    fn a_cookie_can_be_fetched_by_a_named_plugin() {
        let spec = parse(
            r#"
mechanism: cookie
plugins:
  - name: sso
    command: nyd-auth-drunken --flow jira-sso
    timeout_secs: 90
bindings:
  - field: cookie
    provider: { type: plugin, use: sso }
"#,
        );
        spec.validate_against(MECHANISMS).expect("a sound config");
        assert_eq!(spec.plugins[0].name, "sso");
        assert_eq!(spec.plugins[0].timeout_secs, 90);
        // Only the per-step deadline was written; a person at a second
        // factor still gets the longer one.
        assert_eq!(spec.plugins[0].attention_timeout_secs, 300);
        let provider = &spec.bindings[0].provider;
        assert!(
            provider.needs_frontend(),
            "the orchestrator holds both the dialog and the status channel"
        );
        assert!(
            provider.build_resolver().is_err(),
            "a plugin is not a standalone resolver"
        );
    }

    #[test]
    fn a_binding_cannot_name_a_plugin_the_auth_block_never_declared() {
        let spec = parse(
            r#"
mechanism: cookie
plugins:
  - name: sso
    command: nyd-auth-drunken --flow jira-sso
bindings:
  - field: cookie
    provider: { type: plugin, use: sso-typo }
"#,
        );
        let err = spec.validate_against(MECHANISMS).expect_err("a typo");
        assert!(err.contains("`sso-typo`"), "{err}");
        assert!(err.contains("it declares `sso`"), "{err}");
    }

    /// The mirror of the `script` / `script-result` pairing check: half of
    /// the pair is a silent no-op either way round.
    #[test]
    fn a_declared_plugin_nobody_uses_is_a_typo_and_not_a_spare() {
        let spec = parse(
            r#"
mechanism: cookie
plugins:
  - name: sso
    command: nyd-auth-drunken --flow jira-sso
bindings:
  - field: cookie
    provider: { type: literal, value: "session=abc" }
"#,
        );
        let err = spec
            .validate_against(MECHANISMS)
            .expect_err("an idle plugin");
        assert!(err.contains("no binding uses it"), "{err}");
    }

    /// Two fields, one plugin: the economy the whole named table exists
    /// for — three entries would open three browsers for one login.
    #[test]
    fn two_fields_may_share_one_plugin() {
        let spec = parse(
            r#"
mechanism: basic-auth
plugins:
  - name: sso
    command: nyd-auth-drunken --flow sso
bindings:
  - field: username
    provider: { type: plugin, use: sso }
  - field: token
    provider: { type: plugin, use: sso }
"#,
        );
        spec.validate_against(MECHANISMS).expect("a sound config");
    }

    #[test]
    fn two_plugins_cannot_answer_to_the_same_name() {
        let spec = parse(
            r#"
mechanism: cookie
plugins:
  - name: sso
    command: one
  - name: sso
    command: another
bindings:
  - field: cookie
    provider: { type: plugin, use: sso }
"#,
        );
        let err = spec.validate_against(MECHANISMS).expect_err("an ambiguity");
        assert!(err.contains("duplicate plugin `sso`"), "{err}");
    }
}
