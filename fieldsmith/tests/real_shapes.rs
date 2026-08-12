//! Integration proof against a real-world config shape.
//!
//! This mirrors the *structure* of the not-yet-done Jira adapter config and its
//! shared auth types (invented field semantics, no real data): a top-level
//! struct nesting another struct that holds two internally-tagged enums and a
//! `Vec` of structs, plus `PathBuf`, `Option`, and serde-default fields. If
//! fieldsmith reflects, templates, and interactively builds *this*, it covers
//! the adapters.

#![cfg(feature = "stdin")]

use fieldsmith::{Buildable, EnumTag, Kind, ScalarHint, ScriptedPrompter, TypeSchema, build_with};
use serde::Deserialize;
use std::path::PathBuf;

fn default_trim() -> bool {
    true
}
fn default_timeout() -> u64 {
    30
}
fn default_retries() -> u32 {
    3
}

/// Which authentication mechanism the adapter uses.
#[derive(Buildable, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum AuthMechanism {
    PasswordLogin,
    BearerToken,
    Cookie,
    BasicAuth,
    UserApiToken,
}

/// How long a resolved session is reused.
#[derive(Buildable, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum SessionCachePolicy {
    None,
    Ttl {
        ttl_secs: u64,
    },
    TtlOrClose {
        ttl_secs: u64,
    },
    #[default]
    UntilRejected,
    Explicit,
}

/// Where a single credential value comes from.
#[derive(Buildable, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
enum CredentialProvider {
    Literal {
        value: String,
    },
    Prompt {
        #[serde(default)]
        prefill: Option<String>,
    },
    Env {
        var: String,
    },
    File {
        path: PathBuf,
        #[serde(default = "default_trim")]
        trim: bool,
    },
    Command {
        script: String,
        #[serde(default = "default_timeout")]
        timeout_secs: u64,
        #[serde(default = "default_retries")]
        retries: u32,
    },
    Keyring {
        service: String,
        account: String,
    },
}

/// Binds one auth field to a credential source.
#[derive(Buildable, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CredentialBinding {
    /// The auth field this fills (e.g. token, username).
    field: String,
    provider: CredentialProvider,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    masked: Option<bool>,
}

/// The adapter's authentication block.
#[derive(Buildable, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct AuthSpec {
    mechanism: AuthMechanism,
    #[serde(default)]
    session_cache: SessionCachePolicy,
    bindings: Vec<CredentialBinding>,
}

/// Cache DB override.
#[derive(Buildable, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct DbConfig {
    url: String,
}

/// Jira adapter configuration.
#[derive(Buildable, Deserialize, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
struct JiraConfig {
    /// Base URL of your Jira instance.
    url: String,
    #[serde(default)]
    name: Option<String>,
    auth: AuthSpec,
    #[serde(default)]
    accept_invalid_certs: bool,
    #[serde(default)]
    db: Option<DbConfig>,
}

#[test]
fn schema_reflects_the_whole_tree() {
    let ts = JiraConfig::schema();
    let s = ts.as_struct().unwrap();
    // auth → AuthSpec, whose `bindings` is a list of nested structs.
    let auth = s.fields.iter().find(|f| f.key == "auth").unwrap();
    let auth_schema = match &auth.kind {
        Kind::Nested(inner) => inner.as_struct().unwrap(),
        other => panic!("expected nested auth, got {other:?}"),
    };
    let bindings = auth_schema
        .fields
        .iter()
        .find(|f| f.key == "bindings")
        .unwrap();
    let provider = match &bindings.kind {
        Kind::List(inner) => match inner.as_ref() {
            Kind::Nested(TypeSchema::Struct(binding)) => {
                binding.fields.iter().find(|f| f.key == "provider").unwrap()
            }
            other => panic!("expected list of binding structs, got {other:?}"),
        },
        other => panic!("expected list, got {other:?}"),
    };
    // provider → internally-tagged enum; the File variant's path is a scalar.
    let provider_enum = match &provider.kind {
        Kind::Nested(TypeSchema::Enum(e)) => e,
        other => panic!("expected provider enum, got {other:?}"),
    };
    assert_eq!(provider_enum.tag, EnumTag::Internal("type"));
    let file = provider_enum
        .variants
        .iter()
        .find(|v| v.name == "file")
        .unwrap();
    match &file.kind {
        fieldsmith::VariantKind::Struct(fields) => {
            let path = fields.iter().find(|f| f.key == "path").unwrap();
            assert!(matches!(path.kind, Kind::Scalar(ScalarHint::Str))); // PathBuf → scalar
        }
        other => panic!("expected struct variant, got {other:?}"),
    }
}

#[test]
fn template_renders_valid_yaml() {
    let yaml = fieldsmith::yaml_template(&JiraConfig::schema());
    assert!(yaml.contains("# Base URL of your Jira instance."));
    assert!(yaml.contains("url:"));
    assert!(yaml.contains("auth:"));
    // The internally-tagged provider enum surfaces its menu + tag line.
    assert!(yaml.contains("# one of:"));
    let _: serde_yaml::Value = serde_yaml::from_str(&yaml).expect("template is valid YAML");
}

#[test]
fn interactively_builds_a_realistic_config() {
    // Prompt order (execution order per kind):
    //   texts:   url, name(skip), binding.field, env.var, label(skip)
    //   selects: mechanism, session_cache, provider
    //   confirms: add-binding, masked, add-another, accept_invalid_certs, db
    let mut p = ScriptedPrompter::new()
        .push_text("https://jira.example.test")
        .push_text("") // name → None
        .push_select(1) // mechanism = bearer-token
        .push_select(3) // session_cache = until-rejected (unit variant)
        .push_confirm(true) // add a binding
        .push_text("token") // binding.field
        .push_select(2) // provider = env
        .push_text("JIRA_TOKEN") // env.var
        .push_text("") // label → None
        .push_confirm(false) // masked = false
        .push_confirm(false) // no more bindings
        .push_confirm(false) // accept_invalid_certs = false
        .push_confirm(false); // no db

    let cfg: JiraConfig = build_with(&mut p).expect("builds");
    assert_eq!(
        cfg,
        JiraConfig {
            url: "https://jira.example.test".to_string(),
            name: None,
            auth: AuthSpec {
                mechanism: AuthMechanism::BearerToken,
                session_cache: SessionCachePolicy::UntilRejected,
                bindings: vec![CredentialBinding {
                    field: "token".to_string(),
                    provider: CredentialProvider::Env {
                        var: "JIRA_TOKEN".to_string(),
                    },
                    label: None,
                    masked: Some(false),
                }],
            },
            accept_invalid_certs: false,
            db: None,
        }
    );
}
