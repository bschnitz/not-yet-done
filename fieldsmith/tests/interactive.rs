//! Phase-1 proof: an internally-tagged enum + a list build interactively from
//! canned answers, via `ScriptedPrompter` (no TTY).

#![cfg(feature = "stdin")]

use fieldsmith::{Buildable, EnumTag, ScriptedPrompter, build_with};
use serde::Deserialize;

/// How a secret is obtained.
#[derive(Buildable, Deserialize, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Cred {
    /// Read the secret from an environment variable.
    Env {
        /// Name of the variable.
        var: String,
    },
    /// Run a command; its stdout is the secret.
    Command {
        /// Command line to run.
        script: String,
        /// Abort after this many seconds.
        timeout_secs: Option<u64>,
    },
}

/// Auth block with a credential and extra headers.
#[derive(Buildable, Deserialize, Debug, PartialEq)]
struct AuthLike {
    /// How to obtain the secret.
    cred: Cred,
    /// Extra HTTP headers, verbatim.
    #[serde(default)]
    headers: Vec<String>,
}

#[test]
fn enum_schema_reflects_tag_and_variants() {
    let ts = Cred::schema();
    let e = ts.as_enum().expect("Cred is an enum");
    assert_eq!(e.name, "Cred");
    assert_eq!(e.tag, EnumTag::Internal("type"));
    let names: Vec<&str> = e.variants.iter().map(|v| v.name).collect();
    assert_eq!(names, ["env", "command"]); // rename_all = snake_case
}

#[test]
fn builds_command_variant_with_list() {
    // Order per prompt kind: select(variant) → texts → confirms(list loop).
    let mut p = ScriptedPrompter::new()
        .push_select(1) // pick `command`
        .push_text("run.sh") // command.script
        .push_text("30") // command.timeout_secs
        .push_confirm(true) // add a header
        .push_text("X-A: 1")
        .push_confirm(true) // add another
        .push_text("X-B: 2")
        .push_confirm(false); // stop

    let auth: AuthLike = build_with(&mut p).expect("builds");
    assert_eq!(
        auth,
        AuthLike {
            cred: Cred::Command {
                script: "run.sh".to_string(),
                timeout_secs: Some(30),
            },
            headers: vec!["X-A: 1".to_string(), "X-B: 2".to_string()],
        }
    );
}

#[test]
fn builds_env_variant_skipping_optional_and_empty_list() {
    let mut p = ScriptedPrompter::new()
        .push_select(0) // pick `env`
        .push_text("TOKEN_VAR") // env.var
        .push_confirm(false); // add no headers

    let auth: AuthLike = build_with(&mut p).expect("builds");
    assert_eq!(
        auth,
        AuthLike {
            cred: Cred::Env {
                var: "TOKEN_VAR".to_string(),
            },
            headers: vec![],
        }
    );
}

#[test]
fn omits_optional_scalar_when_left_blank() {
    let mut p = ScriptedPrompter::new()
        .push_select(1) // `command`
        .push_text("run.sh") // script
        .push_text("") // timeout_secs left blank → omitted → None
        .push_confirm(false); // no headers

    let auth: AuthLike = build_with(&mut p).expect("builds");
    assert_eq!(
        auth.cred,
        Cred::Command {
            script: "run.sh".to_string(),
            timeout_secs: None,
        }
    );
}

#[test]
fn retries_a_scalar_until_it_parses() {
    // command.timeout_secs is an integer; a non-numeric answer is rejected and
    // the driver re-asks (Prompter::error is a no-op here) rather than aborting.
    let mut p = ScriptedPrompter::new()
        .push_select(1) // command
        .push_text("run.sh") // script
        .push_text("soon") // timeout_secs: not an integer → retry
        .push_text("45") // timeout_secs: valid this time
        .push_confirm(false); // no headers

    let auth: AuthLike = build_with(&mut p).expect("builds after a retry");
    assert_eq!(
        auth.cred,
        Cred::Command {
            script: "run.sh".to_string(),
            timeout_secs: Some(45),
        }
    );
}

#[test]
fn schema_carries_concrete_type_names() {
    // ScalarHint collapses every integer to `Int`; type_name keeps the exact
    // Rust type so a prompt can show `timeout_secs (u64)`.
    let ts = Cred::schema();
    let e = ts.as_enum().expect("Cred is an enum");
    let command = e.variants.iter().find(|v| v.name == "command").unwrap();
    let fields = match &command.kind {
        fieldsmith::VariantKind::Struct(f) => f,
        _ => panic!("command is a struct variant"),
    };
    let by_key = |k: &str| fields.iter().find(|f| f.key == k).unwrap();
    assert_eq!(by_key("script").type_name, "String");
    assert_eq!(by_key("timeout_secs").type_name, "u64"); // Option-unwrapped

    // A Vec field reports the container type; the driver descends per item.
    let auth = AuthLike::schema();
    let s = auth.as_struct().expect("AuthLike is a struct");
    let headers = s.fields.iter().find(|f| f.key == "headers").unwrap();
    assert_eq!(headers.type_name, "Vec");
}

/// The template renderer must also cope with a top-level enum schema.
#[test]
fn enum_template_shows_variant_menu() {
    let yaml = fieldsmith::yaml_template(&Cred::schema());
    assert!(yaml.contains("# one of: env | command"));
    // Internally tagged: the tag line names the first variant.
    assert!(yaml.contains("type: env"));
}
