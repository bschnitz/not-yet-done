//! The `auth:` half of an adapter's connection config, driven by the
//! mechanism descriptors the adapter publishes.
//!
//! `config build` cannot get this section from generic schema reflection:
//! since mechanisms became adapter-local, [`AuthSpec::mechanism`] is a plain
//! `String` on the wire, so the schema knows the *shape* (`mechanism`,
//! `session_cache`, `bindings`) but not which ids are valid nor which fields
//! each one needs. That knowledge lives in
//! [`AdapterFactory::auth_mechanisms`](not_yet_done_content::AdapterFactory::auth_mechanisms),
//! and this module is what renders it:
//!
//! * [`build_auth_value`] — the interactive walk: pick a mechanism, then bind
//!   each of its fields to a credential provider,
//! * [`render_auth_section`] — the same table as text, for
//!   `nyd config auth <type>`.
//!
//! Both read the descriptor the factory also validates against, so what the
//! wizard offers and what the adapter accepts cannot drift apart.

use anyhow::Result;
use fieldsmith::{Answer, Buildable, DriverError, Prompter, TypeSchema};
use not_yet_done_content::{
    AuthFieldSpec, CredentialBinding, CredentialProvider, MechanismSpec, SessionCachePolicy,
};
use serde_yaml::{Mapping, Value};

/// Entry point for `nyd config auth [<type>]` (`args[2]` is `auth`) — print
/// the adapter type's mechanisms and their fields, connecting to nothing.
pub fn run(args: &[String]) -> Result<()> {
    let atype = crate::config_template::adapter_type_arg(args, "auth")?;
    let factory = crate::config_template::factory_for(atype)?;
    let mechanisms = factory.auth_mechanisms();
    if mechanisms.is_empty() {
        println!(
            "adapter type '{atype}' has no authentication — its config has no `auth:` section"
        );
        return Ok(());
    }
    print!("{}", render_auth_section(atype, mechanisms));
    Ok(())
}

/// The mechanisms of one adapter type as human-readable text: every id with
/// its label, its one-line doc, and the fields a config has to bind.
pub fn render_auth_section(atype: &str, mechanisms: &[MechanismSpec]) -> String {
    let mut out = format!("Authentication for adapter type '{atype}'\n\n");
    out.push_str("Pick one id for `auth.mechanism`; bind its fields under `auth.bindings`:\n\n");
    out.push_str(&mechanism_lines(mechanisms));
    out.push_str(&provider_lines());
    out.push_str(&format!(
        "`nyd config build {atype}` asks these questions and writes the block.\n"
    ));
    out
}

/// One indented block per mechanism — id, label, wrapped doc, fields. Shared
/// by `config auth` and the wizard's legend so the menu and the listing say
/// the same thing.
fn mechanism_lines(mechanisms: &[MechanismSpec]) -> String {
    let mut out = String::new();
    for m in mechanisms {
        out.push_str(&format!("  {} — {}\n", m.id, m.label));
        for line in wrap(m.doc, 68) {
            out.push_str(&format!("      {line}\n"));
        }
        for f in m.fields {
            out.push_str(&format!("      - {}\n", field_line(f)));
        }
        out.push('\n');
    }
    out
}

/// The credential providers, read out of the enum's own schema: a new
/// provider (or a reworded doc comment) reaches this listing without anyone
/// remembering to edit it.
fn provider_lines() -> String {
    let TypeSchema::Enum(e) = <CredentialProvider as Buildable>::schema() else {
        return String::new();
    };
    let mut out = String::from("Each binding names where its value comes from:\n\n");
    for v in &e.variants {
        out.push_str(&format!("  {}\n", v.name));
        for line in v
            .doc
            .map(first_sentence)
            .into_iter()
            .flat_map(|d| wrap(d, 68))
        {
            out.push_str(&format!("      {line}\n"));
        }
    }
    out.push('\n');
    out
}

/// Up to the first full stop — enough of a doc comment to tell providers
/// apart in a listing, without printing the whole paragraph.
fn first_sentence(doc: &str) -> &str {
    let bytes = doc.as_bytes();
    for (i, c) in doc.char_indices() {
        if c == '.' && bytes.get(i + 1).is_none_or(|b| b.is_ascii_whitespace()) {
            return &doc[..=i];
        }
    }
    doc
}

/// Break a mechanism's one-line doc into terminal-width lines. The descriptor
/// carries it as a single sentence on purpose (a menu item needs one line), so
/// the wrapping belongs here rather than in the adapter's table.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// One field as `name (Label, masked, optional)` — the trailing notes only
/// where they say something beyond the name.
fn field_line(f: &AuthFieldSpec) -> String {
    let mut notes = vec![f.label.to_string()];
    if f.masked {
        notes.push("masked".into());
    }
    if !f.required {
        notes.push("optional".into());
    }
    format!("{} ({})", f.name, notes.join(", "))
}

/// Interactively build the `auth:` mapping for an adapter that publishes
/// `mechanisms`: choose one, then bind each of its fields to a credential
/// provider. Optional fields are offered but skipped by default, and the
/// session-cache policy is only asked for when the user wants something other
/// than the default.
///
/// The result is a `serde_yaml` mapping shaped like
/// [`AuthSpec`](not_yet_done_content::AuthSpec); the caller splices it into
/// the config it built from the rest of the schema.
pub fn build_auth_value(
    mechanisms: &[MechanismSpec],
    p: &mut dyn Prompter,
) -> Result<Value, DriverError> {
    let m = choose_mechanism(mechanisms, p)?;

    let mut auth = Mapping::new();
    auth.insert("mechanism".into(), Value::String(m.id.to_string()));

    if ask(p.confirm(
        "auth.session_cache: change the session-cache policy?",
        Some("Default: until-rejected — keep a derived session until the server refuses it."),
        false,
    ))? {
        let schema = <SessionCachePolicy as Buildable>::schema();
        let v = fieldsmith::build_value_with_ctx(&schema, p, "auth.session_cache")?;
        auth.insert("session_cache".into(), v);
    }

    let mut bindings = Vec::new();
    for f in m.fields {
        if !f.required
            && !ask(p.confirm(
                &format!("auth.{}: bind the optional field `{}`?", f.name, f.name),
                Some(f.label),
                false,
            ))?
        {
            continue;
        }
        bindings.push(binding_value(f, p)?);
    }

    // `script-result` names no script of its own — one script serves every
    // field that uses it, so it is asked for once, here, after the bindings
    // revealed whether it is needed at all. Asking earlier would mean asking
    // for a script nobody binds to; `AuthSpec::validate_against` rejects both
    // halves of that pairing when they come apart.
    if bindings.iter().any(uses_script_result) {
        let script = ask(p.text(
            "auth.script",
            Some(
                "Command that supplies the `script-result` fields. It is run once per \
                 login round with the wanted field names on stdin and answers with the \
                 values, or with a form to ask you something first.",
            ),
            None,
            false,
        ))?;
        auth.insert("script".into(), Value::String(script));
    }

    auth.insert("bindings".into(), Value::Sequence(bindings));

    Ok(Value::Mapping(auth))
}

/// Whether a built binding took its value from the auth block's script.
/// Reads the produced YAML rather than the enum so it stays correct however
/// fieldsmith renders the variant — the wire id is what the config carries.
fn uses_script_result(binding: &Value) -> bool {
    binding
        .get("provider")
        .and_then(|p| p.get("type"))
        .and_then(Value::as_str)
        == Some("script-result")
}

/// Offer the mechanisms as a menu, preceded by the same block
/// `nyd config auth <type>` prints.
///
/// The menu items stay short on purpose: the terminal prompter only shows a
/// field's help *after* it has been answered — too late for a choice — but a
/// menu item long enough to carry the doc would wrap, and dialoguer redraws
/// its menu by counting lines. So the docs go above the menu, as a legend.
fn choose_mechanism<'a>(
    mechanisms: &'a [MechanismSpec],
    p: &mut dyn Prompter,
) -> Result<&'a MechanismSpec, DriverError> {
    if mechanisms.is_empty() {
        return Err(DriverError::Unsupported(
            "this adapter publishes no auth mechanisms".into(),
        ));
    }
    eprintln!("Authentication — this adapter implements:");
    eprintln!();
    eprint!("{}", mechanism_lines(mechanisms));
    let items: Vec<String> = mechanisms
        .iter()
        .map(|m| format!("{} [{}]", m.label, m.id))
        .collect();
    let refs: Vec<&str> = items.iter().map(String::as_str).collect();
    let idx = ask(p.select("auth.mechanism", None, &refs, 0))?;
    mechanisms
        .get(idx)
        .ok_or_else(|| DriverError::Prompt(format!("mechanism index {idx} out of range")))
}

/// One `bindings:` entry: the field name, its provider, and the descriptor's
/// `label` / `masked` — but only where they differ from what the runtime
/// would infer from the field name anyway, so the generated YAML carries
/// exactly the overrides that matter.
fn binding_value(f: &AuthFieldSpec, p: &mut dyn Prompter) -> Result<Value, DriverError> {
    let schema = <CredentialProvider as Buildable>::schema();
    let provider = fieldsmith::build_value_with_ctx(&schema, p, &format!("auth.{}", f.name))?;

    let inferred = CredentialBinding {
        field: f.name.to_string(),
        provider: CredentialProvider::Prompt { prefill: None },
        label: None,
        masked: None,
    };

    let mut b = Mapping::new();
    b.insert("field".into(), Value::String(f.name.to_string()));
    b.insert("provider".into(), provider);
    if inferred.effective_label() != f.label {
        b.insert("label".into(), Value::String(f.label.to_string()));
    }
    if inferred.effective_masked() != f.masked {
        b.insert("masked".into(), Value::Bool(f.masked));
    }
    Ok(Value::Mapping(b))
}

/// Ask one prompt, mapping a cancellation onto [`DriverError::Cancelled`] —
/// the same abandon path fieldsmith's own walk takes, so the caller handles
/// "user pressed Escape" once for the whole build.
fn ask<T>(r: Result<Answer<T>, String>) -> Result<T, DriverError> {
    match r.map_err(DriverError::Prompt)? {
        Answer::Value(v) => Ok(v),
        Answer::Cancelled => Err(DriverError::Cancelled),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fieldsmith::ScriptedPrompter;
    use not_yet_done_content::AuthSpec;

    /// A stand-in for what an adapter publishes — deliberately local, like
    /// the real tables are.
    const MECHANISMS: &[MechanismSpec] = &[
        MechanismSpec {
            id: "cookie",
            label: "Session cookie",
            doc: "Send a ready-made Cookie header.",
            fields: &[AuthFieldSpec::required("cookie", "Cookie header", true)],
        },
        MechanismSpec {
            id: "password-login",
            label: "Username and password",
            doc: "Log in and keep the derived session.",
            fields: &[
                AuthFieldSpec::required("username", "Username", false),
                AuthFieldSpec::required("password", "Password", true),
                AuthFieldSpec::optional("otp", "One-time code", true),
            ],
        },
    ];

    /// The walk produces YAML that the real `AuthSpec` accepts *and* that
    /// passes the adapter's own validation — the wizard cannot offer a
    /// config the factory would reject.
    #[test]
    fn scripted_walk_builds_a_spec_that_validates() {
        // mechanism → password-login; no session-cache change; username via
        // literal, password via prompt, otp skipped.
        let mut p = ScriptedPrompter::new()
            .push_select(1) // auth.mechanism
            .push_confirm(false) // session_cache: keep the default
            .push_select(0) // username: literal
            .push_text("alice") // literal value
            .push_select(1) // password: prompt
            .push_text("") // prompt prefill (optional)
            .push_confirm(false); // otp: skip the optional field

        let value = build_auth_value(MECHANISMS, &mut p).expect("walk completes");
        let yaml = serde_yaml::to_string(&value).expect("serialises");
        let spec: AuthSpec = serde_yaml::from_str(&yaml).expect("parses as AuthSpec");
        spec.validate_against(MECHANISMS)
            .expect("the built spec passes the adapter's own check");

        assert_eq!(spec.mechanism, "password-login");
        assert_eq!(spec.bindings.len(), 2, "otp was skipped: {yaml}");
        assert_eq!(spec.session_cache, SessionCachePolicy::UntilRejected);
    }

    /// An optional field is bound when the user asks for it.
    #[test]
    fn optional_field_is_bound_on_request() {
        let mut p = ScriptedPrompter::new()
            .push_select(1)
            .push_confirm(false)
            .push_select(0)
            .push_text("alice")
            .push_select(1)
            .push_text("")
            .push_confirm(true) // otp: yes
            .push_select(1) // otp via prompt
            .push_text("");

        let value = build_auth_value(MECHANISMS, &mut p).expect("walk completes");
        let spec: AuthSpec = serde_yaml::from_value(value).expect("parses as AuthSpec");
        spec.validate_against(MECHANISMS).expect("valid");
        assert!(
            spec.bindings.iter().any(|b| b.field == "otp"),
            "otp binding present: {:?}",
            spec.bindings
        );
    }

    /// Index of the `script-result` variant in the provider menu — the walk
    /// offers the enum's variants in declaration order.
    const SCRIPT_RESULT: usize = 5;

    /// Several fields can take their value from the one script, and the
    /// wizard asks for that script exactly once — the pairing the real
    /// `AuthSpec` insists on.
    #[test]
    fn one_script_is_asked_for_once_however_many_fields_use_it() {
        let mut p = ScriptedPrompter::new()
            .push_select(1) // password-login
            .push_confirm(false) // keep the default session cache
            .push_select(SCRIPT_RESULT) // username from the script
            .push_select(SCRIPT_RESULT) // password from the same script
            .push_confirm(false) // otp: skip
            .push_text("creds.py"); // auth.script — asked once

        let value = build_auth_value(MECHANISMS, &mut p).expect("walk completes");
        let spec: AuthSpec = serde_yaml::from_value(value).expect("parses as AuthSpec");
        spec.validate_against(MECHANISMS)
            .expect("script and bindings agree");

        assert_eq!(spec.script.as_deref(), Some("creds.py"));
        assert_eq!(spec.bindings.len(), 2);
        assert!(
            spec.bindings
                .iter()
                .all(|b| matches!(b.provider, CredentialProvider::ScriptResult)),
            "both fields bound to the script: {:?}",
            spec.bindings
        );
    }

    /// No binding wants a script, so none is asked for — an unused `script:`
    /// would be rejected by the adapter's own check.
    #[test]
    fn no_script_result_binding_means_no_script_question() {
        let mut p = ScriptedPrompter::new()
            .push_select(0) // cookie
            .push_confirm(false)
            .push_select(1) // cookie via prompt
            .push_text(""); // prompt prefill — the only text in the walk

        let value = build_auth_value(MECHANISMS, &mut p).expect("walk completes");
        let spec: AuthSpec = serde_yaml::from_value(value).expect("parses as AuthSpec");
        spec.validate_against(MECHANISMS).expect("valid");
        assert_eq!(spec.script, None);
    }

    /// The descriptor's label survives into the YAML when the runtime's
    /// name-based guess would get it wrong — and stays out of it otherwise.
    #[test]
    fn only_diverging_label_and_masked_are_written() {
        let mut p = ScriptedPrompter::new()
            .push_select(0) // cookie mechanism
            .push_confirm(false)
            .push_select(1) // cookie via prompt
            .push_text("");

        let value = build_auth_value(MECHANISMS, &mut p).expect("walk completes");
        let yaml = serde_yaml::to_string(&value).expect("serialises");
        // `cookie` → "Cookie" by the title-case rule, but the adapter calls
        // it "Cookie header".
        assert!(yaml.contains("label: Cookie header"), "yaml: {yaml}");
        // Masked is already what the name-based default infers for `cookie`.
        assert!(!yaml.contains("masked:"), "no redundant override: {yaml}");
    }

    /// Escape anywhere in the walk abandons the whole build.
    #[test]
    fn cancelling_the_mechanism_choice_aborts() {
        struct Cancels;
        impl Prompter for Cancels {
            fn text(
                &mut self,
                _: &str,
                _: Option<&str>,
                _: Option<&str>,
                _: bool,
            ) -> Result<Answer<String>, String> {
                Ok(Answer::Cancelled)
            }
            fn confirm(
                &mut self,
                _: &str,
                _: Option<&str>,
                _: bool,
            ) -> Result<Answer<bool>, String> {
                Ok(Answer::Cancelled)
            }
            fn select(
                &mut self,
                _: &str,
                _: Option<&str>,
                _: &[&str],
                _: usize,
            ) -> Result<Answer<usize>, String> {
                Ok(Answer::Cancelled)
            }
        }
        let err = build_auth_value(MECHANISMS, &mut Cancels).expect_err("aborts");
        assert_eq!(err, DriverError::Cancelled);
    }

    /// The loop closed against the real descriptor tables: for every
    /// registered adapter and every mechanism it publishes, the wizard's
    /// output passes that adapter's own `validate_against`. A new adapter is
    /// covered the moment it is registered.
    #[test]
    fn every_registered_mechanism_builds_a_spec_its_adapter_accepts() {
        let mut checked = 0;
        for (atype, factory) in not_yet_done_host::factories() {
            let mechanisms = factory.auth_mechanisms();
            for (i, m) in mechanisms.iter().enumerate() {
                let mut p = ScriptedPrompter::new()
                    .push_select(i) // this mechanism
                    .push_confirm(false); // keep the default session cache
                for f in m.fields {
                    if f.required {
                        p = p.push_select(0).push_text("x"); // literal provider
                    } else {
                        p = p.push_confirm(false); // skip the optional field
                    }
                }
                let value = build_auth_value(mechanisms, &mut p)
                    .unwrap_or_else(|e| panic!("{atype}/{}: walk failed: {e}", m.id));
                let spec: AuthSpec = serde_yaml::from_value(value)
                    .unwrap_or_else(|e| panic!("{atype}/{}: not an AuthSpec: {e}", m.id));
                spec.validate_against(mechanisms)
                    .unwrap_or_else(|e| panic!("{atype}/{}: rejected by the adapter: {e}", m.id));
                assert_eq!(spec.mechanism, m.id);
                checked += 1;
            }
        }
        assert!(checked >= 5, "the authenticating adapters were covered");
    }

    /// The provider half of the listing comes from the enum, so it names
    /// every provider a binding may use — including the parameterless
    /// `script-result`, which a hand-written list would be likeliest to
    /// forget.
    #[test]
    fn rendered_section_lists_every_credential_provider() {
        let text = render_auth_section("demo", MECHANISMS);
        for id in [
            "literal",
            "prompt",
            "env",
            "file",
            "command",
            "script-result",
            "keyring",
        ] {
            assert!(
                text.contains(&format!("\n  {id}\n")),
                "missing {id}: {text}"
            );
        }
        assert!(
            text.contains("Take this field's value out of"),
            "the provider's own doc is shown: {text}"
        );
    }

    #[test]
    fn first_sentence_stops_at_the_first_full_stop() {
        assert_eq!(first_sentence("One. Two. Three."), "One.");
        assert_eq!(
            first_sentence("Wrapped over\ntwo lines. Rest."),
            "Wrapped over\ntwo lines."
        );
        assert_eq!(first_sentence("No full stop"), "No full stop");
    }

    #[test]
    fn wrap_breaks_on_word_boundaries_and_keeps_long_words_whole() {
        assert_eq!(wrap("one two three four", 8), ["one two", "three", "four"]);
        assert_eq!(wrap("supercalifragilistic", 8), ["supercalifragilistic"]);
        assert!(wrap("", 8).is_empty());
    }

    #[test]
    fn rendered_section_names_ids_fields_and_optionality() {
        let text = render_auth_section("demo", MECHANISMS);
        assert!(text.contains("cookie — Session cookie"), "{text}");
        assert!(text.contains("password-login"), "{text}");
        assert!(
            text.contains("otp (One-time code, masked, optional)"),
            "field notes: {text}"
        );
        assert!(
            text.contains("username (Username)"),
            "a plain field carries no notes: {text}"
        );
    }
}
