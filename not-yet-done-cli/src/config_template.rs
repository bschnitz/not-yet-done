//! Connection-config commands driven by an adapter's declared config schema:
//!
//! * `nyd config template <type>` — print a *static* YAML skeleton (placeholders
//!   + doc comments), non-interactive.
//! * `nyd config build <type>` — *interactively* fill the config field by field
//!   (fieldsmith's stdin driver) and print the completed YAML.
//!
//! Both are the *connection*-config counterpart to `config generate` (which
//! scaffolds a *view* config): where `generate` walks an adapter's runtime
//! protocol, these two reflect the connection config (`url` / `auth` / `db` …)
//! for one adapter **type** from [`AdapterFactory::config_schema`]. The schema
//! is the same associated `Config` type the factory deserializes, so neither
//! the template nor the interactive build can drift from what a real config
//! must contain.
//!
//! Both are factory-by-**type** and never connect — no configured instance, no
//! network, no DB. The finished YAML always goes to **stdout** so
//! `nyd config build jira > jira.config.yaml` just works; skeleton comments,
//! interactive prompts, and guidance go to **stderr**. With no type, each
//! lists the known adapter types.

use std::io::IsTerminal;

use anyhow::{Result, anyhow};
use fieldsmith::{DialoguerPrompter, DriverError, StructSchema, TypeSchema};
use not_yet_done_content::AdapterFactory;
use serde_yaml::{Mapping, Value};

/// Entry point for `nyd config template [<type>]` (`args[2]` is `template`).
pub fn run(args: &[String]) -> Result<()> {
    let atype = adapter_type_arg(args, "template")?;

    let yaml = render_template(atype)?;
    print!("{yaml}");
    if !yaml.ends_with('\n') {
        println!();
    }
    // Guidance to stderr so stdout stays a clean, pipeable config.
    eprintln!();
    eprintln!("# connection-config template for adapter type '{atype}' — no instance was created");
    eprintln!("# fill it in, then reference it from a views/*.yaml `adapter.config: <path>`");
    eprintln!("# (or paste it under `adapter.config_inline:`)");
    eprintln!("# tip: `nyd config build {atype}` fills it in interactively instead");
    // The `auth.mechanism` id is a plain string in the schema, so the skeleton
    // alone cannot say which ids this adapter accepts. Name them here.
    let mechanisms = factory_for(atype)?.auth_mechanisms();
    if !mechanisms.is_empty() {
        eprintln!("# tip: `nyd config auth {atype}` lists the auth mechanisms and their fields");
    }
    Ok(())
}

/// Entry point for `nyd config build [<type>]` (`args[2]` is `build`).
///
/// Interactively walks the adapter's config schema (fieldsmith's stdin driver),
/// prompting for each field, and prints the completed YAML to stdout. Prompts
/// are drawn on stderr, so `nyd config build jira > jira.config.yaml` captures
/// just the config while the wizard runs on the terminal.
pub fn run_build(args: &[String]) -> Result<()> {
    let atype = adapter_type_arg(args, "build")?;

    // The wizard prompts on stderr and reads from stdin; both must be a TTY.
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return Err(anyhow!(
            "`config build` is interactive and needs a terminal; use `config template {atype}` for a static skeleton"
        ));
    }

    // Reflect the schema (factory-by-type, no adapter constructed, no connect).
    let factory = factory_for(atype)?;
    let schema = factory.config_schema();

    // The `auth:` subtree is filled from the adapter's mechanism descriptors
    // rather than from generic reflection — the schema only knows that
    // `mechanism` is a string, not which ids this adapter speaks.
    let mechanisms = factory.auth_mechanisms();
    let auth_key = auth_field_key(&schema).filter(|_| !mechanisms.is_empty());

    eprintln!("Building a '{atype}' adapter config.");
    eprintln!("Answer each field; press Enter to accept a default or skip an optional field.");
    eprintln!();

    let mut prompter = DialoguerPrompter::new();
    let walked = match auth_key {
        Some(key) => without_field(&schema, key),
        None => schema.clone(),
    };
    let Some(value) = finished(fieldsmith::build_value_with(&walked, &mut prompter))? else {
        return abandoned();
    };

    let value = match auth_key {
        Some(key) => {
            let auth = crate::config_auth::build_auth_value(mechanisms, &mut prompter);
            let Some(auth) = finished(auth)? else {
                return abandoned();
            };
            splice_in_declaration_order(&schema, value, key, auth)
        }
        None => value,
    };

    let yaml =
        serde_yaml::to_string(&value).map_err(|e| anyhow!("serialising the built config: {e}"))?;

    print!("{yaml}");
    if !yaml.ends_with('\n') {
        println!();
    }
    eprintln!();
    eprintln!("# built connection config for adapter type '{atype}'");
    eprintln!(
        "# reference it from a views/*.yaml `adapter.config: <path>` (or paste under `adapter.config_inline:`)"
    );
    Ok(())
}

/// Render the connection-config YAML template for one adapter type. Looks up
/// the factory by type (constructing no adapter, opening no connection) and
/// reflects its `config_schema()` into a template. Errors — with the list of
/// known types — when the type is unknown.
fn render_template(atype: &str) -> Result<String> {
    Ok(fieldsmith::yaml_template(
        &factory_for(atype)?.config_schema(),
    ))
}

/// The single optional `<type>` positional every schema-reflecting `config`
/// subcommand takes. Missing type or a stray argument errors, naming the
/// subcommand and the types there are.
pub(crate) fn adapter_type_arg<'a>(args: &'a [String], sub: &str) -> Result<&'a str> {
    let mut adapter_type: Option<&str> = None;
    for a in &args[3..] {
        if a.starts_with('-') {
            return Err(anyhow!("unknown flag '{a}'"));
        }
        if adapter_type.is_some() {
            return Err(anyhow!("unexpected extra argument '{a}'"));
        }
        adapter_type = Some(a);
    }
    adapter_type.ok_or_else(|| {
        eprintln!("name an adapter type: `nyd config {sub} <type>`");
        eprintln!("known types: {}", known_types().join(", "));
        anyhow!("no adapter type given")
    })
}

/// The factory registered for an adapter type — by type alone, so nothing is
/// constructed and nothing connects. Errors list the known types.
pub(crate) fn factory_for(atype: &str) -> Result<Box<dyn AdapterFactory>> {
    let mut factories = not_yet_done_host::factories();
    factories.remove(atype).ok_or_else(|| {
        anyhow!(
            "no adapter type '{atype}' (known: {})",
            known_types().join(", ")
        )
    })
}

/// The config key holding the shared [`AuthSpec`](not_yet_done_content::AuthSpec),
/// if this adapter's config has one at the top level. Keyed on the reflected
/// type name rather than on the spelling of the key, so an adapter that calls
/// it something else is still recognised.
fn auth_field_key(schema: &TypeSchema) -> Option<&'static str> {
    schema
        .as_struct()?
        .fields
        .iter()
        .find(|f| f.type_name == "AuthSpec")
        .map(|f| f.key)
}

/// The schema minus one field — what the generic walk covers when the auth
/// section is filled from the descriptors instead.
fn without_field(schema: &TypeSchema, key: &str) -> TypeSchema {
    match schema {
        TypeSchema::Struct(s) => {
            let mut reduced: StructSchema = s.clone();
            reduced.fields.retain(|f| f.key != key);
            TypeSchema::Struct(reduced)
        }
        other => other.clone(),
    }
}

/// Put `auth` back where the config type declares it. The wizard asks for it
/// last (a mechanism choice reads better after the connection details), but
/// the printed YAML should read like the struct it will be parsed into.
fn splice_in_declaration_order(schema: &TypeSchema, built: Value, key: &str, auth: Value) -> Value {
    match (schema.as_struct(), built) {
        (Some(s), Value::Mapping(mut m)) => {
            m.insert(Value::String(key.to_string()), auth);
            let mut out = Mapping::new();
            for f in &s.fields {
                if let Some(v) = m.remove(f.key) {
                    out.insert(Value::String(f.key.to_string()), v);
                }
            }
            // Anything the schema did not name keeps its relative order at the end.
            for (k, v) in m {
                out.insert(k, v);
            }
            Value::Mapping(out)
        }
        (_, other) => other,
    }
}

/// Fold one interactive step into "a value, a clean abandon (`None`), or a
/// hard error" — Escape/Ctrl-C is the user changing their mind, not a failure.
fn finished<T>(r: std::result::Result<T, DriverError>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(DriverError::Cancelled) => Ok(None),
        Err(e) => Err(anyhow!("interactive build did not complete: {e}")),
    }
}

/// The abandon path: say so on stderr and leave stdout empty, so a redirected
/// `config build … > file` does not end up with half a config.
fn abandoned() -> Result<()> {
    eprintln!();
    eprintln!("aborted — no config written.");
    Ok(())
}

/// The registered adapter type names, sorted for stable help/error output.
fn known_types() -> Vec<String> {
    let mut types: Vec<String> = not_yet_done_host::factories().into_keys().collect();
    types.sort_unstable();
    types
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_types_include_the_core_adapters() {
        let types = known_types();
        for expected in ["jira", "tasks"] {
            assert!(
                types.iter().any(|t| t == expected),
                "expected adapter type '{expected}' among {types:?}"
            );
        }
    }

    #[test]
    fn template_for_a_known_type_is_non_empty_valid_yaml() {
        let yaml = render_template("jira").expect("jira type is known");
        assert!(!yaml.trim().is_empty(), "template must not be empty");
        serde_yaml::from_str::<serde_yaml::Value>(&yaml)
            .expect("rendered template must be valid YAML");
    }

    #[test]
    fn unknown_type_errors_and_lists_known() {
        let err = render_template("does-not-exist").unwrap_err().to_string();
        assert!(err.contains("no adapter type"), "message: {err}");
        assert!(err.contains("known:"), "message lists known types: {err}");
    }

    /// An authenticating adapter offers the wizard both halves: an `AuthSpec`
    /// field to fill from the descriptors, and a non-empty mechanism table to
    /// fill it from.
    #[test]
    fn an_authenticating_adapter_exposes_both_halves() {
        let factory = factory_for("jira").expect("jira type is known");
        let key = auth_field_key(&factory.config_schema()).expect("jira config has an AuthSpec");
        assert_eq!(key, "auth");
        assert!(
            !factory.auth_mechanisms().is_empty(),
            "jira publishes mechanisms"
        );
    }

    /// A local adapter has no auth at all — nothing to splice, and the walk
    /// stays exactly what it was before the wizard learnt about mechanisms.
    #[test]
    fn a_local_adapter_has_no_auth_section() {
        let factory = factory_for("tasks").expect("tasks type is known");
        assert!(factory.auth_mechanisms().is_empty(), "tasks has no auth");
        assert!(auth_field_key(&factory.config_schema()).is_none());
    }

    #[test]
    fn without_field_drops_only_that_field() {
        let schema = factory_for("jira").expect("known").config_schema();
        let before = schema.as_struct().expect("struct").fields.len();
        let reduced = without_field(&schema, "auth");
        let fields = &reduced.as_struct().expect("struct").fields;
        assert_eq!(fields.len(), before - 1);
        assert!(!fields.iter().any(|f| f.key == "auth"));
    }

    /// The wizard asks for auth last; the printed YAML still reads in the
    /// order the config type declares.
    #[test]
    fn splice_restores_declaration_order() {
        let schema = factory_for("jira").expect("known").config_schema();
        let declared: Vec<&str> = schema
            .as_struct()
            .expect("struct")
            .fields
            .iter()
            .map(|f| f.key)
            .collect();

        // Everything but `auth`, in declaration order, plus the auth block
        // appended last — what the two-step build produces.
        let mut built = Mapping::new();
        for key in declared.iter().filter(|k| **k != "auth") {
            built.insert(Value::String((*key).to_string()), Value::String("x".into()));
        }
        let spliced = splice_in_declaration_order(
            &schema,
            Value::Mapping(built),
            "auth",
            Value::String("<auth>".into()),
        );

        let keys: Vec<String> = spliced
            .as_mapping()
            .expect("mapping")
            .keys()
            .map(|k| k.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(keys, declared, "keys follow the struct's own order");
    }

    fn args(rest: &[&str]) -> Vec<String> {
        let mut v = vec!["nyd".to_string(), "config".to_string()];
        v.extend(rest.iter().map(|s| s.to_string()));
        v
    }

    #[test]
    fn build_without_a_type_errors() {
        let err = run_build(&args(&["build"])).unwrap_err().to_string();
        assert!(err.contains("no adapter type"), "message: {err}");
    }

    #[test]
    fn build_refuses_without_a_terminal() {
        // The test harness has no TTY, so the interactive guard must fire
        // before anything tries to prompt.
        let err = run_build(&args(&["build", "jira"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("terminal"), "message: {err}");
        assert!(
            err.contains("config template jira"),
            "suggests the static fallback: {err}"
        );
    }

    /// The schema → prompt → YAML path the interactive command relies on,
    /// driven headlessly by fieldsmith's scripted prompter.
    #[test]
    fn scripted_build_walks_schema_to_roundtrippable_yaml() {
        use fieldsmith::{Buildable, ScriptedPrompter, build_value_with};

        #[derive(fieldsmith::Buildable, serde::Deserialize)]
        struct Demo {
            host: String,
            port: u16,
        }

        let mut prompter = ScriptedPrompter::new()
            .push_text("db.example.invalid")
            .push_text("5432");
        let value = build_value_with(&Demo::schema(), &mut prompter).expect("walk completes");
        let yaml = serde_yaml::to_string(&value).expect("serialises");
        assert!(yaml.contains("host: db.example.invalid"), "yaml: {yaml}");

        let demo: Demo = serde_yaml::from_str(&yaml).expect("round-trips into the real type");
        assert_eq!(demo.port, 5432);
        assert_eq!(demo.host, "db.example.invalid");
    }
}
