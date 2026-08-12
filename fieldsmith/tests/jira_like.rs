//! Phase-0 proof: a JiraConfig-shaped struct (scalars + `Option` + nested,
//! no enums) reflects into a schema, renders a YAML template, and builds.

use fieldsmith::{Buildable, Kind, ScalarHint, TypeSchema, yaml_template};
use serde::Deserialize;

#[derive(Buildable, Deserialize, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
struct DbCfg {
    /// Sea-orm-compatible cache URL.
    url: String,
}

/// Jira adapter configuration.
#[derive(Buildable, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct JiraLike {
    /// Base URL of your Jira instance.
    #[builder(default = "https://your-jira.example.com")]
    url: String,
    /// Optional display name for this instance.
    #[serde(default)]
    #[builder(default = "My Jira")]
    name: Option<String>,
    /// Trust self-signed TLS certificates.
    #[serde(default)]
    #[builder(default = false)]
    accept_invalid_certs: bool,
    /// Optional cache DB override.
    #[serde(default)]
    db: Option<DbCfg>,
}

#[test]
fn schema_describes_every_field() {
    let ts = JiraLike::schema();
    let schema = ts.as_struct().expect("JiraLike is a struct");
    assert_eq!(schema.name, "JiraLike");
    assert_eq!(schema.doc, Some("Jira adapter configuration."));
    assert_eq!(schema.fields.len(), 4);

    let url = &schema.fields[0];
    assert_eq!(url.key, "url");
    assert!(!url.optional);
    assert_eq!(url.default, Some("https://your-jira.example.com"));
    assert!(matches!(url.kind, Kind::Scalar(ScalarHint::Str)));

    let name = &schema.fields[1];
    assert_eq!(name.key, "name");
    assert!(name.optional);
    assert_eq!(name.default, Some("My Jira"));
    assert!(matches!(name.kind, Kind::Scalar(ScalarHint::Str)));

    let certs = &schema.fields[2];
    assert_eq!(certs.key, "accept_invalid_certs");
    assert!(matches!(certs.kind, Kind::Scalar(ScalarHint::Bool)));
    assert_eq!(certs.default, Some("false"));

    let db = &schema.fields[3];
    assert_eq!(db.key, "db");
    assert!(db.optional);
    match &db.kind {
        Kind::Nested(TypeSchema::Struct(sub)) => {
            assert_eq!(sub.name, "DbCfg");
            assert_eq!(sub.fields.len(), 1);
            assert_eq!(sub.fields[0].key, "url");
        }
        other => panic!("expected nested struct, got {other:?}"),
    }
}

#[test]
fn builder_applies_defaults_and_checks_required() {
    // A field with no default and no serde default is required.
    let err = DbCfgBuilder::new().build().unwrap_err();
    assert_eq!(err, fieldsmith::BuildError::MissingField("url"));

    // Every JiraLike field has a default or is optional → builds with nothing set.
    let cfg = JiraLikeBuilder::new()
        .build()
        .expect("builds from defaults");
    assert_eq!(cfg.url, "https://your-jira.example.com"); // builder default = real default
    assert_eq!(cfg.name.as_deref(), Some("My Jira")); // Option with default → Some(default)
    assert!(!cfg.accept_invalid_certs);
    assert_eq!(cfg.db, None);

    // Explicit overrides, including the nested struct.
    let cfg = JiraLikeBuilder::new()
        .url("https://jira.acme.test")
        .name("Acme Jira")
        .accept_invalid_certs(true)
        .db(DbCfg {
            url: "sqlite::memory:".to_string(),
        })
        .build()
        .expect("builds");
    assert_eq!(cfg.name.as_deref(), Some("Acme Jira"));
    assert!(cfg.accept_invalid_certs);
    assert_eq!(cfg.db.unwrap().url, "sqlite::memory:");
}

#[test]
fn yaml_template_is_a_fillable_skeleton() {
    let yaml = yaml_template(&JiraLike::schema());

    // Struct + field docs become comments.
    assert!(yaml.contains("# Jira adapter configuration."));
    assert!(yaml.contains("# Base URL of your Jira instance."));

    // Required scalar shows its default as a working value.
    assert!(yaml.contains("url: https://your-jira.example.com"));

    // Optional scalar is commented out.
    assert!(yaml.contains("#name: My Jira"));

    // Bool default rendered verbatim.
    assert!(yaml.contains("accept_invalid_certs: false"));

    // Optional nested block: note + header + indented required child placeholder.
    assert!(yaml.contains("# optional"));
    assert!(yaml.contains("db:"));
    assert!(yaml.contains("  url: <string>"));

    // The template must round-trip as valid YAML.
    let _: serde_yaml::Value = serde_yaml::from_str(&yaml).expect("template is valid YAML");
}
