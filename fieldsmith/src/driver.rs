//! Interactive, recursive builder over a [`TypeSchema`] (feature `stdin`).
//!
//! The driver walks a schema and asks one question per leaf, descending into
//! nested structs, offering a menu for enum variants, and looping over lists.
//! It assembles a [`serde_yaml::Value`] and then hands it to the type's own
//! `Deserialize` — so the config's existing deserialization stays the single
//! authority on how the collected answers become a value.
//!
//! Prompting is abstracted behind [`Prompter`]: [`DialoguerPrompter`] drives a
//! real terminal, while [`ScriptedPrompter`] replays canned answers so the walk
//! is unit-testable without a TTY.

use serde::de::DeserializeOwned;
use serde_yaml::{Mapping, Number, Value};
use std::collections::VecDeque;
use std::fmt;

use crate::Buildable;
use crate::schema::{
    EnumSchema, EnumTag, FieldSchema, Kind, ScalarHint, StructSchema, TypeSchema, VariantKind,
};

/// The outcome of a single prompt: either an answer, or the user pressing
/// Escape to abandon the whole build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer<T> {
    /// The user answered.
    Value(T),
    /// The user cancelled (Escape); the driver aborts with
    /// [`DriverError::Cancelled`].
    Cancelled,
}

/// Something that can ask the user questions.
///
/// Each method returns `Ok(Answer::Value(_))` for an answer,
/// `Ok(Answer::Cancelled)` when the user pressed Escape, or `Err(message)` when
/// prompting genuinely fails (a broken TTY, or a scripted prompter running out
/// of answers); the driver wraps the latter in [`DriverError::Prompt`].
pub trait Prompter {
    /// Ask for a single line of text. `default` pre-fills the answer;
    /// `allow_empty` permits an empty response (used to skip optional fields).
    fn text(
        &mut self,
        label: &str,
        help: Option<&str>,
        default: Option<&str>,
        allow_empty: bool,
    ) -> Result<Answer<String>, String>;

    /// Ask a yes/no question.
    fn confirm(
        &mut self,
        label: &str,
        help: Option<&str>,
        default: bool,
    ) -> Result<Answer<bool>, String>;

    /// Ask the user to pick one of `items`, returning its index.
    fn select(
        &mut self,
        label: &str,
        help: Option<&str>,
        items: &[&str],
        default: usize,
    ) -> Result<Answer<usize>, String>;

    /// Report a recoverable problem with the previous answer before re-asking
    /// (e.g. a value that did not parse). Default: a no-op, so headless
    /// prompters ignore it.
    fn error(&mut self, _msg: &str) {}
}

/// Something went wrong while building a value interactively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverError {
    /// A prompt could not be answered.
    Prompt(String),
    /// The collected answers did not deserialize into the target type.
    Deserialize(String),
    /// The schema contains a shape the driver cannot handle.
    Unsupported(String),
    /// The user pressed Escape to abandon the build.
    Cancelled,
}

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DriverError::Prompt(s) => write!(f, "prompt failed: {s}"),
            DriverError::Deserialize(s) => write!(f, "could not build value: {s}"),
            DriverError::Unsupported(s) => write!(f, "unsupported schema: {s}"),
            DriverError::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for DriverError {}

/// Ask a prompt, propagating a genuine failure as [`DriverError::Prompt`] and a
/// user cancellation as an early `return Err(DriverError::Cancelled)`.
macro_rules! ask {
    ($e:expr) => {
        match $e.map_err(DriverError::Prompt)? {
            Answer::Value(v) => v,
            Answer::Cancelled => return Err(DriverError::Cancelled),
        }
    };
}

/// Build a value of `T` interactively, using the given prompter.
pub fn build_with<T>(prompter: &mut dyn Prompter) -> Result<T, DriverError>
where
    T: Buildable + DeserializeOwned,
{
    let value = build_value_with(&T::schema(), prompter)?;
    serde_yaml::from_value(value).map_err(|e| DriverError::Deserialize(e.to_string()))
}

/// Build a value of `T` interactively on the real terminal.
pub fn build_stdin<T>() -> Result<T, DriverError>
where
    T: Buildable + DeserializeOwned,
{
    let mut prompter = DialoguerPrompter::new();
    build_with(&mut prompter)
}

/// Interactively collect a [`serde_yaml::Value`] for a schema known only at
/// runtime, using the given prompter.
///
/// This is the type-erased counterpart to [`build_with`]: callers that hold a
/// [`TypeSchema`] (e.g. reflected from a boxed trait object) but not the
/// concrete `T` get back the assembled YAML value, which they can serialise or
/// feed to their own `Deserialize`. No concrete type is required, so nothing
/// re-serialises the value — it is produced directly from the schema walk.
pub fn build_value_with(
    schema: &TypeSchema,
    prompter: &mut dyn Prompter,
) -> Result<Value, DriverError> {
    walk_type(schema, prompter, "")
}

/// Like [`build_value_with`], but every prompt is labelled under `ctx`.
///
/// For a caller that embeds one walk inside a larger form of its own: with
/// `ctx = "auth.password"` the variant menu of a nested enum reads
/// `auth.password: choose variant` instead of a bare `Choose
/// CredentialProvider`, which is ambiguous once the same type is walked for
/// several fields in a row.
pub fn build_value_with_ctx(
    schema: &TypeSchema,
    prompter: &mut dyn Prompter,
    ctx: &str,
) -> Result<Value, DriverError> {
    walk_type(schema, prompter, ctx)
}

/// Interactively collect a [`serde_yaml::Value`] for a runtime schema on the
/// real terminal — the type-erased counterpart to [`build_stdin`].
pub fn build_value_stdin(schema: &TypeSchema) -> Result<Value, DriverError> {
    let mut prompter = DialoguerPrompter::new();
    build_value_with(schema, &mut prompter)
}

// ---------------------------------------------------------------------------
// The walk — schema → serde_yaml::Value
// ---------------------------------------------------------------------------

fn walk_type(schema: &TypeSchema, p: &mut dyn Prompter, ctx: &str) -> Result<Value, DriverError> {
    match schema {
        TypeSchema::Struct(s) => walk_struct(s, p, ctx),
        TypeSchema::Enum(e) => walk_enum(e, p, ctx),
    }
}

fn walk_struct(s: &StructSchema, p: &mut dyn Prompter, ctx: &str) -> Result<Value, DriverError> {
    let mut map = Mapping::new();
    for field in &s.fields {
        let label = if ctx.is_empty() {
            field.key.to_string()
        } else {
            format!("{ctx}.{}", field.key)
        };
        if let Some(v) = walk_kind(
            &field.kind,
            p,
            &label,
            field.doc,
            field.optional,
            field.default,
            Some(field.type_name),
        )? {
            map.insert(Value::String(field.key.to_string()), v);
        }
    }
    Ok(Value::Mapping(map))
}

fn walk_enum(e: &EnumSchema, p: &mut dyn Prompter, ctx: &str) -> Result<Value, DriverError> {
    let items: Vec<&str> = e.variants.iter().map(|v| v.name).collect();
    let label = if ctx.is_empty() {
        format!("Choose {}", e.name)
    } else {
        format!("{ctx}: choose variant")
    };
    let idx = ask!(p.select(&label, e.doc, &items, 0));
    let variant = e
        .variants
        .get(idx)
        .ok_or_else(|| DriverError::Prompt(format!("variant index {idx} out of range")))?;
    let inner_ctx = format!("{ctx}.{}", variant.name);

    match e.tag {
        EnumTag::External => match &variant.kind {
            VariantKind::Unit => Ok(Value::String(variant.name.to_string())),
            VariantKind::Newtype(inner) => {
                let v = walk_kind(inner, p, &inner_ctx, None, false, None, None)?
                    .unwrap_or(Value::Null);
                Ok(single(variant.name, v))
            }
            VariantKind::Struct(fields) => {
                let inner = walk_struct(&sub_schema(variant.name, fields), p, &inner_ctx)?;
                Ok(single(variant.name, inner))
            }
        },
        EnumTag::Internal(tag) => {
            let mut map = match &variant.kind {
                VariantKind::Unit => Mapping::new(),
                VariantKind::Struct(fields) => {
                    match walk_struct(&sub_schema(variant.name, fields), p, &inner_ctx)? {
                        Value::Mapping(m) => m,
                        _ => Mapping::new(),
                    }
                }
                VariantKind::Newtype(_) => {
                    return Err(DriverError::Unsupported(format!(
                        "internally-tagged newtype variant `{}` is not supported",
                        variant.name
                    )));
                }
            };
            map.insert(
                Value::String(tag.to_string()),
                Value::String(variant.name.to_string()),
            );
            Ok(Value::Mapping(map))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_kind(
    kind: &Kind,
    p: &mut dyn Prompter,
    label: &str,
    help: Option<&str>,
    optional: bool,
    default: Option<&str>,
    type_name: Option<&str>,
) -> Result<Option<Value>, DriverError> {
    match kind {
        Kind::Scalar(hint) => walk_scalar(*hint, p, label, help, optional, default, type_name),
        Kind::Nested(ts) => {
            if optional {
                let want = ask!(p.confirm(&format!("Configure {label}?"), help, false));
                if !want {
                    return Ok(None);
                }
            }
            Ok(Some(walk_type(ts, p, label)?))
        }
        Kind::List(inner) => {
            let mut items = Vec::new();
            loop {
                let first = items.is_empty();
                let add = ask!(p.confirm(
                    &format!("Add an item to {label}?"),
                    help.filter(|_| first),
                    first,
                ));
                if !add {
                    break;
                }
                let item_label = format!("{label}[{}]", items.len());
                if let Some(v) = walk_kind(inner, p, &item_label, None, false, None, None)? {
                    items.push(v);
                }
            }
            Ok(Some(Value::Sequence(items)))
        }
    }
}

/// Decorate a scalar's prompt label with its concrete type, e.g. `port (u32)`.
/// Booleans are asked as a yes/no `confirm`, where a type tag adds nothing.
fn scalar_label(label: &str, type_name: Option<&str>, hint: ScalarHint) -> String {
    match (type_name, hint) {
        (Some(tn), h) if h != ScalarHint::Bool => format!("{label} ({tn})"),
        _ => label.to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_scalar(
    hint: ScalarHint,
    p: &mut dyn Prompter,
    label: &str,
    help: Option<&str>,
    optional: bool,
    default: Option<&str>,
    type_name: Option<&str>,
) -> Result<Option<Value>, DriverError> {
    let shown = scalar_label(label, type_name, hint);
    match hint {
        ScalarHint::Bool => {
            let d = default.map(|s| s == "true").unwrap_or(false);
            let b = ask!(p.confirm(&shown, help, d));
            Ok(Some(Value::Bool(b)))
        }
        ScalarHint::Int => loop {
            let raw = ask!(p.text(&shown, help, default, optional));
            let t = raw.trim();
            if t.is_empty() {
                return Ok(None);
            }
            if let Ok(i) = t.parse::<i64>() {
                return Ok(Some(Value::Number(Number::from(i))));
            }
            if let Ok(u) = t.parse::<u64>() {
                return Ok(Some(Value::Number(Number::from(u))));
            }
            p.error(&format!("`{t}` is not an integer — try again"));
        },
        ScalarHint::Float => loop {
            let raw = ask!(p.text(&shown, help, default, optional));
            let t = raw.trim();
            if t.is_empty() {
                return Ok(None);
            }
            match t.parse::<f64>() {
                Ok(f) => return Ok(Some(Value::Number(Number::from(f)))),
                Err(_) => p.error(&format!("`{t}` is not a number — try again")),
            }
        },
        ScalarHint::Str | ScalarHint::Other(_) => {
            let raw = ask!(p.text(&shown, help, default, optional));
            if optional && raw.trim().is_empty() {
                return Ok(None);
            }
            Ok(Some(Value::String(raw)))
        }
    }
}

/// A one-entry mapping `{ key: value }`.
fn single(key: &str, value: Value) -> Value {
    let mut m = Mapping::new();
    m.insert(Value::String(key.to_string()), value);
    Value::Mapping(m)
}

/// A throwaway struct schema for an enum variant's named fields.
fn sub_schema(name: &'static str, fields: &[FieldSchema]) -> StructSchema {
    StructSchema {
        name,
        doc: None,
        fields: fields.to_vec(),
    }
}

// ---------------------------------------------------------------------------
// Prompter implementations
// ---------------------------------------------------------------------------

/// A [`Prompter`] backed by `dialoguer` for real terminal interaction.
pub struct DialoguerPrompter {
    theme: dialoguer::theme::ColorfulTheme,
}

impl DialoguerPrompter {
    /// Create a prompter with dialoguer's default colourful theme.
    pub fn new() -> Self {
        Self {
            theme: dialoguer::theme::ColorfulTheme::default(),
        }
    }
}

impl Default for DialoguerPrompter {
    fn default() -> Self {
        Self::new()
    }
}

/// Map a dialoguer/console error to either a user cancellation (Ctrl-C, which
/// surfaces as an interrupted read) or a genuine prompt failure.
fn interrupt_is_cancel<T>(e: dialoguer::Error) -> Result<Answer<T>, String> {
    let dialoguer::Error::IO(io) = e;
    if io.kind() == std::io::ErrorKind::Interrupted {
        Ok(Answer::Cancelled)
    } else {
        Err(io.to_string())
    }
}

impl DialoguerPrompter {
    /// Print the field's description on its own dimmed line(s) directly below
    /// the answered prompt, then a blank line separating it from the next
    /// field — the "description underneath, blank line between fields" layout.
    fn describe(&self, help: Option<&str>) {
        if let Some(h) = help {
            for line in h.split('\n') {
                eprintln!("  {}", console::style(line).dim());
            }
        }
        eprintln!();
    }
}

impl Prompter for DialoguerPrompter {
    fn text(
        &mut self,
        label: &str,
        help: Option<&str>,
        default: Option<&str>,
        allow_empty: bool,
    ) -> Result<Answer<String>, String> {
        let mut input = dialoguer::Input::<String>::with_theme(&self.theme)
            .with_prompt(label)
            .allow_empty(allow_empty);
        if let Some(d) = default {
            input = input.default(d.to_string());
        }
        // dialoguer's text Input has no Escape handling; Ctrl-C (interrupt) is
        // the only in-band cancel, which we map to Answer::Cancelled.
        match input.interact_text() {
            Ok(v) => {
                self.describe(help);
                Ok(Answer::Value(v))
            }
            Err(e) => interrupt_is_cancel(e),
        }
    }

    fn confirm(
        &mut self,
        label: &str,
        help: Option<&str>,
        default: bool,
    ) -> Result<Answer<bool>, String> {
        let outcome = dialoguer::Confirm::with_theme(&self.theme)
            .with_prompt(label)
            .default(default)
            .interact_opt()
            .map_err(|e| e.to_string())?;
        self.describe(help);
        Ok(match outcome {
            Some(b) => Answer::Value(b),
            None => Answer::Cancelled,
        })
    }

    fn select(
        &mut self,
        label: &str,
        help: Option<&str>,
        items: &[&str],
        default: usize,
    ) -> Result<Answer<usize>, String> {
        let outcome = dialoguer::Select::with_theme(&self.theme)
            .with_prompt(label)
            .items(items)
            .default(default)
            .interact_opt()
            .map_err(|e| e.to_string())?;
        self.describe(help);
        Ok(match outcome {
            Some(i) => Answer::Value(i),
            None => Answer::Cancelled,
        })
    }

    fn error(&mut self, msg: &str) {
        eprintln!("  {}", console::style(msg).red());
    }
}

/// A [`Prompter`] that replays canned answers — for tests, no TTY needed.
///
/// Each kind of prompt draws from its own queue in call order. Running a queue
/// dry yields an error, which surfaces as [`DriverError::Prompt`].
#[derive(Debug, Default)]
pub struct ScriptedPrompter {
    texts: VecDeque<String>,
    confirms: VecDeque<bool>,
    selects: VecDeque<usize>,
}

impl ScriptedPrompter {
    /// An empty scripted prompter; feed answers with the `push_*` builders.
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a text answer.
    pub fn push_text(mut self, s: impl Into<String>) -> Self {
        self.texts.push_back(s.into());
        self
    }

    /// Queue a yes/no answer.
    pub fn push_confirm(mut self, b: bool) -> Self {
        self.confirms.push_back(b);
        self
    }

    /// Queue a menu selection (variant index).
    pub fn push_select(mut self, i: usize) -> Self {
        self.selects.push_back(i);
        self
    }
}

impl Prompter for ScriptedPrompter {
    fn text(
        &mut self,
        label: &str,
        _help: Option<&str>,
        _default: Option<&str>,
        _allow_empty: bool,
    ) -> Result<Answer<String>, String> {
        self.texts
            .pop_front()
            .map(Answer::Value)
            .ok_or_else(|| format!("scripted: no text answer left for `{label}`"))
    }

    fn confirm(
        &mut self,
        label: &str,
        _help: Option<&str>,
        _default: bool,
    ) -> Result<Answer<bool>, String> {
        self.confirms
            .pop_front()
            .map(Answer::Value)
            .ok_or_else(|| format!("scripted: no confirm answer left for `{label}`"))
    }

    fn select(
        &mut self,
        label: &str,
        _help: Option<&str>,
        _items: &[&str],
        _default: usize,
    ) -> Result<Answer<usize>, String> {
        self.selects
            .pop_front()
            .map(Answer::Value)
            .ok_or_else(|| format!("scripted: no select answer left for `{label}`"))
    }
}
