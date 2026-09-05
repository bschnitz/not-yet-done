//! Named arguments for actions.
//!
//! An action used to receive data only through fields the framework had grown
//! for the occasion: five on [`ActionContext`], twenty-eight on the TUI's view
//! YAML, one variant per case on the way back. [`ActionArgs`] is the one
//! channel that replaces the reflex — a small, ordered map of named values
//! that travels with an invocation, from whichever frontend sourced it to
//! whichever adapter consumes it.
//!
//! The rule the map exists to enforce: **new data travels as an argument, not
//! as a new field**. Arguments carry data, action types carry behaviour. The
//! test for a new field is whether a different value would make the action do
//! something *else* (a type) or the *same thing to something else* (an
//! argument).
//!
//! [`ArgValue`] is a closed enum on purpose, not a `serde_json::Value`: every
//! `match` over it stays exhaustive, the YAML deserialiser stays honest about
//! what it accepts, and nothing can smuggle in a nested structure that no one
//! validates.
//!
//! [`ActionContext`]: crate::ActionContext

use std::path::PathBuf;

/// One value carried by a named action argument.
///
/// A frontend that can only produce strings — the CLI's `--arg key=value`, a
/// prompt answer — sends [`ArgValue::Text`] and lets the readers below coerce.
/// Types are never guessed from the shape of a string: `--arg key=007` stays
/// the text `007`, because an inferring command line is a trap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgValue {
    /// A string. What every text-only frontend produces.
    Text(String),
    /// A whole number — limits, depths, page sizes.
    Int(i64),
    /// A flag, so it does not have to travel as `"true"`.
    Bool(bool),
    /// Several strings — ids, labels, keys.
    List(Vec<String>),
    /// A filesystem path, tilde already expanded, so it is resolved once at
    /// construction rather than at every read.
    Path(PathBuf),
}

impl ArgValue {
    /// A path from a string, expanding a leading `~`. The one constructor that
    /// does work, because [`ArgValue::Path`] promises an expanded path.
    pub fn path(input: impl AsRef<str>) -> Self {
        Self::Path(PathBuf::from(crate::download::expand_tilde(input.as_ref())))
    }

    /// The variant's name, for error messages that have to say what arrived.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Text(_) => "text",
            Self::Int(_) => "int",
            Self::Bool(_) => "bool",
            Self::List(_) => "list",
            Self::Path(_) => "path",
        }
    }

    /// The value as a string. Scalars render (`Int` as digits, `Bool` as
    /// `true`/`false`, `Path` lossily); a [`ArgValue::List`] yields `None`,
    /// because joining it would invent a separator the caller never chose.
    pub fn as_text(&self) -> Option<String> {
        match self {
            Self::Text(s) => Some(s.clone()),
            Self::Int(n) => Some(n.to_string()),
            Self::Bool(b) => Some(b.to_string()),
            Self::Path(p) => Some(p.to_string_lossy().into_owned()),
            Self::List(_) => None,
        }
    }

    /// The value as a number: an [`ArgValue::Int`], or text that parses as one
    /// (surrounding whitespace ignored). Anything else is `None`.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(n) => Some(*n),
            Self::Text(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    /// The value as a flag: an [`ArgValue::Bool`], or the text `true`/`false`
    /// in any casing — the same spelling [`crate::ActionInput::Form`] toggles
    /// already use. Anything else is `None`; `1`/`0` are numbers, not flags.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            Self::Text(s) => match s.trim().to_ascii_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
            _ => None,
        }
    }

    /// The value as a list. A lone [`ArgValue::Text`] counts as a one-element
    /// list, so `--arg tags=urgent` satisfies an argument that accepts several.
    /// Numbers, flags and paths do not: a list of one path is a path, and
    /// pretending otherwise hides a config mistake.
    pub fn as_list(&self) -> Option<Vec<String>> {
        match self {
            Self::List(items) => Some(items.clone()),
            Self::Text(s) => Some(vec![s.clone()]),
            _ => None,
        }
    }

    /// The value as a path: an [`ArgValue::Path`], or text (whose leading `~`
    /// is expanded here, once). Anything else is `None`.
    pub fn as_path(&self) -> Option<PathBuf> {
        match self {
            Self::Path(p) => Some(p.clone()),
            Self::Text(s) => Some(PathBuf::from(crate::download::expand_tilde(s))),
            _ => None,
        }
    }
}

impl From<&str> for ArgValue {
    fn from(value: &str) -> Self {
        Self::Text(value.to_string())
    }
}

impl From<String> for ArgValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<i64> for ArgValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<bool> for ArgValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

/// The named arguments of one action invocation.
///
/// Insertion-ordered, because the order is what a frontend shows the user and
/// what `help` prints — a map that re-sorts would scramble both. Lookups are a
/// linear scan: an action carries a handful of arguments, never a table's
/// worth, and the honesty of an ordered `Vec` beats a hash map's constant here.
///
/// Layered filling is [`ActionArgs::merge`]: defaults first, then the view
/// config, then whatever the invocation supplied. Later wins, and a key keeps
/// the position it first appeared in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActionArgs(Vec<(String, ArgValue)>);

impl ActionArgs {
    /// An empty set — what every action that takes no arguments carries.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set `key`, replacing any value already stored under it **in place**, so
    /// a later layer overrides an earlier one without reordering the set.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<ArgValue>) {
        let key = key.into();
        let value = value.into();
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key, value)),
        }
    }

    /// Builder form of [`ActionArgs::insert`], for assembling a set inline.
    pub fn with(mut self, key: impl Into<String>, value: impl Into<ArgValue>) -> Self {
        self.insert(key, value);
        self
    }

    /// Lay `other` over `self`: every key it carries wins, keys it does not
    /// mention survive untouched. The primitive behind the fill order
    /// (parameter default → view config → invocation).
    pub fn merge(&mut self, other: &ActionArgs) {
        for (key, value) in other.iter() {
            self.insert(key.clone(), value.clone());
        }
    }

    /// The raw value under `key`, if any.
    pub fn get(&self, key: &str) -> Option<&ArgValue> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Whether `key` was supplied at all — distinct from it being supplied
    /// empty, which is a value the user chose.
    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// [`ArgValue::as_text`] on `key`.
    pub fn text(&self, key: &str) -> Option<String> {
        self.get(key).and_then(ArgValue::as_text)
    }

    /// [`ArgValue::as_int`] on `key`.
    pub fn int(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(ArgValue::as_int)
    }

    /// [`ArgValue::as_bool`] on `key`.
    pub fn bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(ArgValue::as_bool)
    }

    /// [`ArgValue::as_list`] on `key`.
    pub fn list(&self, key: &str) -> Option<Vec<String>> {
        self.get(key).and_then(ArgValue::as_list)
    }

    /// [`ArgValue::as_path`] on `key`.
    pub fn path(&self, key: &str) -> Option<PathBuf> {
        self.get(key).and_then(ArgValue::as_path)
    }

    /// Whether anything was supplied.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many arguments were supplied.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The pairs in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &ArgValue)> {
        self.0.iter().map(|(k, v)| (k, v))
    }

    /// The keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(k, _)| k.as_str())
    }
}

impl<K: Into<String>, V: Into<ArgValue>> FromIterator<(K, V)> for ActionArgs {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut args = Self::new();
        for (key, value) in iter {
            args.insert(key, value);
        }
        args
    }
}

// ---------------------------------------------------------------------------
// YAML
// ---------------------------------------------------------------------------

/// A YAML value becomes the [`ArgValue`] its **YAML type** names: a bare
/// `true` is a [`ArgValue::Bool`], `20` an [`ArgValue::Int`], a quoted or
/// unquoted string a [`ArgValue::Text`], a sequence of scalars an
/// [`ArgValue::List`]. That is the parser's type, not a guess from the shape
/// of a string: `"20"` in quotes stays text, and the declared parameter kind
/// coerces it later if the action wants a number. A path is never produced
/// here — YAML has no path type — so a `path` parameter arrives as text and
/// is read as a path by [`ArgKind::Path`].
///
/// Maps, decimals and `null` are refused: an argument is a scalar or a list
/// of scalars, and a key with no value is a key to leave out.
impl<'de> serde::Deserialize<'de> for ArgValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ArgValueVisitor)
    }
}

struct ArgValueVisitor;

impl<'de> serde::de::Visitor<'de> for ArgValueVisitor {
    type Value = ArgValue;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a string, a whole number, a boolean, or a list of those")
    }

    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<ArgValue, E> {
        Ok(ArgValue::Bool(v))
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<ArgValue, E> {
        Ok(ArgValue::Int(v))
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<ArgValue, E> {
        i64::try_from(v)
            .map(ArgValue::Int)
            .map_err(|_| E::custom(format!("{v} does not fit a 64-bit signed integer")))
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<ArgValue, E> {
        Err(E::custom(format!(
            "{v} is a decimal; an argument carries a whole number or text (quote it to pass it as text)"
        )))
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<ArgValue, E> {
        Ok(ArgValue::Text(v.to_string()))
    }

    fn visit_string<E: serde::de::Error>(self, v: String) -> Result<ArgValue, E> {
        Ok(ArgValue::Text(v))
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<ArgValue, E> {
        Err(E::custom(
            "an argument carries a value; drop the key to leave it unset",
        ))
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<ArgValue, E> {
        self.visit_unit()
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<ArgValue, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element::<ArgValue>()? {
            match item.as_text() {
                Some(text) => items.push(text),
                None => {
                    return Err(serde::de::Error::custom(
                        "a list argument holds scalars, not nested lists",
                    ));
                }
            }
        }
        Ok(ArgValue::List(items))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, _map: A) -> Result<ArgValue, A::Error> {
        Err(serde::de::Error::custom(
            "a mapping is not an argument value; an argument is a scalar or a list of scalars",
        ))
    }
}

/// A YAML mapping of names to values, in the order written.
impl<'de> serde::Deserialize<'de> for ActionArgs {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(ActionArgsVisitor)
    }
}

struct ActionArgsVisitor;

impl<'de> serde::de::Visitor<'de> for ActionArgsVisitor {
    type Value = ActionArgs;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a mapping of argument names to values")
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<ActionArgs, A::Error> {
        let mut args = ActionArgs::new();
        while let Some((key, value)) = map.next_entry::<String, ArgValue>()? {
            if key.trim().is_empty() {
                return Err(serde::de::Error::custom(
                    "an argument name must not be empty",
                ));
            }
            args.insert(key, value);
        }
        Ok(args)
    }
}

// ---------------------------------------------------------------------------
// Placeholders
// ---------------------------------------------------------------------------

impl ActionArgs {
    /// Expand `{name}` placeholders in every text-bearing value — a
    /// [`ArgValue::Text`], a [`ArgValue::Path`], each item of an
    /// [`ArgValue::List`]; numbers and flags carry no text and pass through.
    /// `resolve` answers a placeholder's name with its value, or `None` when
    /// it knows no such name. Every unresolved placeholder becomes an
    /// [`ArgProblem::Unresolved`] and the whole set is refused: a path with a
    /// hole in it must not be silently created.
    ///
    /// This runs on the **parsed** value, never on YAML text. Substituting
    /// into the raw file once turned a search string containing `#` into a
    /// comment and produced a null query without a word of complaint.
    ///
    /// A placeholder is a `{`, a name, a `}`. A name starts with a letter or
    /// an underscore and continues with letters, digits, `_`, `-`, `.` or
    /// `:` — the colon is what lets `{cell:<key>}` address a field. Braces
    /// around anything else (`{}`, `{ x }`, `{1}`) are not placeholders and
    /// stay as they are.
    pub fn expand(
        &self,
        resolve: impl Fn(&str) -> Option<String>,
    ) -> Result<ActionArgs, Vec<ArgProblem>> {
        let mut problems = Vec::new();
        let mut out = ActionArgs::new();
        for (key, value) in self.iter() {
            let expanded = match value {
                ArgValue::Text(s) => ArgValue::Text(expand_text(s, key, &resolve, &mut problems)),
                ArgValue::Path(p) => ArgValue::Path(PathBuf::from(expand_text(
                    &p.to_string_lossy(),
                    key,
                    &resolve,
                    &mut problems,
                ))),
                ArgValue::List(items) => ArgValue::List(
                    items
                        .iter()
                        .map(|s| expand_text(s, key, &resolve, &mut problems))
                        .collect(),
                ),
                ArgValue::Int(_) | ArgValue::Bool(_) => value.clone(),
            };
            out.insert(key.clone(), expanded);
        }
        if problems.is_empty() {
            Ok(out)
        } else {
            Err(problems)
        }
    }
}

/// Expand the placeholders in one string; each one nobody resolves is
/// recorded against `key` and contributes nothing to the output.
fn expand_text(
    text: &str,
    key: &str,
    resolve: &impl Fn(&str) -> Option<String>,
    problems: &mut Vec<ArgProblem>,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if is_placeholder_name(&after[..close]) => {
                let name = &after[..close];
                match resolve(name) {
                    Some(value) => out.push_str(&value),
                    None => problems.push(ArgProblem::Unresolved {
                        key: key.to_string(),
                        placeholder: name.to_string(),
                    }),
                }
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn is_placeholder_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_only_frontends_still_satisfy_typed_arguments() {
        let args: ActionArgs = [("limit", "20"), ("force", "TRUE"), ("tag", "urgent")]
            .into_iter()
            .collect();

        assert_eq!(args.int("limit"), Some(20));
        assert_eq!(args.bool("force"), Some(true));
        assert_eq!(args.list("tag"), Some(vec!["urgent".to_string()]));
    }

    #[test]
    fn a_list_is_not_a_string_and_a_flag_is_not_a_list() {
        let args = ActionArgs::new()
            .with("tags", ArgValue::List(vec!["a".into(), "b".into()]))
            .with("force", true);

        // Joining a list would invent a separator the caller never chose.
        assert_eq!(args.text("tags"), None);
        // A one-element list of a flag hides a config mistake instead of
        // reporting it.
        assert_eq!(args.list("force"), None);
        assert_eq!(args.text("force"), Some("true".to_string()));
    }

    #[test]
    fn nothing_is_guessed_from_the_shape_of_a_string() {
        let args = ActionArgs::new().with("build", "007");

        // Still text — the number only appears where a number was asked for.
        assert_eq!(args.get("build"), Some(&ArgValue::Text("007".into())));
        assert_eq!(args.text("build"), Some("007".to_string()));
        assert_eq!(args.int("build"), Some(7));
    }

    #[test]
    fn a_later_layer_wins_without_reordering_the_set() {
        let mut args: ActionArgs = [("buffer", "default.md"), ("limit", "10")]
            .into_iter()
            .collect();
        let from_config: ActionArgs = [("buffer", "ticket.edit.md")].into_iter().collect();

        args.merge(&from_config);

        assert_eq!(args.text("buffer"), Some("ticket.edit.md".to_string()));
        assert_eq!(args.text("limit"), Some("10".to_string()));
        assert_eq!(args.keys().collect::<Vec<_>>(), ["buffer", "limit"]);
    }

    #[test]
    fn a_path_argument_is_expanded_once_not_at_every_read() {
        let home = std::env::var("HOME").expect("HOME");
        let args = ActionArgs::new().with("buffer", ArgValue::path("~/drafts/ticket.md"));

        assert_eq!(
            args.path("buffer"),
            Some(PathBuf::from(format!("{home}/drafts/ticket.md")))
        );
        // Text reaching a path reader is expanded too — the same courtesy, so
        // a CLI-supplied path behaves like a configured one.
        let typed = ActionArgs::new().with("buffer", "~/drafts/ticket.md");
        assert_eq!(typed.path("buffer"), args.path("buffer"));
    }

    #[test]
    fn an_absent_argument_and_an_empty_one_are_different_answers() {
        let args = ActionArgs::new().with("note", "");

        assert!(args.contains("note"));
        assert_eq!(args.text("note"), Some(String::new()));
        assert!(!args.contains("missing"));
        assert_eq!(args.text("missing"), None);
    }
}

// ---------------------------------------------------------------------------
// Declaration: what an action says it accepts
// ---------------------------------------------------------------------------

/// The type a [`ParamSpec`] asks for, named apart from [`ArgValue`] because a
/// declaration carries no value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgKind {
    Text,
    Int,
    Bool,
    List,
    Path,
}

impl ArgKind {
    /// The name used in messages and in `help`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Int => "int",
            Self::Bool => "bool",
            Self::List => "list",
            Self::Path => "path",
        }
    }

    /// Read `value` as this kind, applying the coercions in [`ArgValue`].
    /// `None` means the value cannot be this kind — a rejection, not an
    /// absence.
    fn read(self, value: &ArgValue) -> Option<ArgValue> {
        match self {
            Self::Text => value.as_text().map(ArgValue::Text),
            Self::Int => value.as_int().map(ArgValue::Int),
            Self::Bool => value.as_bool().map(ArgValue::Bool),
            Self::List => value.as_list().map(ArgValue::List),
            Self::Path => value.as_path().map(ArgValue::Path),
        }
    }
}

/// One parameter an action accepts.
///
/// Declaring is optional, and what it buys is worth naming: `help` can print
/// the parameter, a frontend can validate a typo instead of ignoring it, and
/// the value an adapter reads back is already the declared type rather than
/// whatever text the command line produced. An action that declares nothing
/// takes its arguments as they come — see [`resolve_args`].
///
/// Unlike a form field, a parameter is **optional by default**. A form asks
/// the user to fill it in, so requiring is the norm there; a parameter mostly
/// overrides something that already has a sensible value, so requiring is the
/// exception and has to be asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamSpec {
    /// The key the value travels under in [`ActionArgs`].
    pub key: String,
    /// Human-readable description, shown by `help`.
    pub label: String,
    /// The type the value is read as.
    pub kind: ArgKind,
    /// Whether the action refuses to run without it.
    pub required: bool,
    /// Value used when the invocation supplies none.
    pub default: Option<ArgValue>,
}

impl ParamSpec {
    /// A parameter of the given kind — optional, with no default.
    pub fn new(key: impl Into<String>, label: impl Into<String>, kind: ArgKind) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind,
            required: false,
            default: None,
        }
    }

    /// A text parameter.
    pub fn text(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(key, label, ArgKind::Text)
    }

    /// A whole-number parameter.
    pub fn int(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(key, label, ArgKind::Int)
    }

    /// A flag parameter.
    pub fn bool(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(key, label, ArgKind::Bool)
    }

    /// A list parameter.
    pub fn list(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(key, label, ArgKind::List)
    }

    /// A path parameter.
    pub fn path(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(key, label, ArgKind::Path)
    }

    /// The action refuses to run without this parameter.
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// The value to use when the invocation supplies none.
    pub fn with_default(mut self, value: impl Into<ArgValue>) -> Self {
        self.default = Some(value.into());
        self
    }
}

/// Why a set of arguments does not satisfy an action's declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgProblem {
    /// A key the action does not declare. Carries the declared keys, because
    /// the useful half of "unknown argument" is the list of known ones.
    Unknown { key: String, known: Vec<String> },
    /// A required parameter nobody supplied and that has no default.
    Missing { key: String },
    /// A value that cannot be read as the declared kind.
    WrongType {
        key: String,
        expected: ArgKind,
        got: ArgValue,
    },
    /// A `{placeholder}` in a configured value that nothing resolved (see
    /// [`ActionArgs::expand`]).
    Unresolved { key: String, placeholder: String },
}

impl std::fmt::Display for ArgProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown { key, known } if known.is_empty() => {
                write!(f, "unknown argument '{key}' (this action takes none)")
            }
            Self::Unknown { key, known } => {
                write!(f, "unknown argument '{key}' (known: {})", known.join(", "))
            }
            Self::Missing { key } => write!(f, "missing required argument '{key}'"),
            Self::WrongType { key, expected, got } => write!(
                f,
                "argument '{key}' expects {}, got {}",
                expected.name(),
                got.type_name()
            ),
            Self::Unresolved { key, placeholder } => write!(
                f,
                "argument '{key}': nothing resolves the placeholder {{{placeholder}}}"
            ),
        }
    }
}

/// Check `supplied` against what the action declares, and return the arguments
/// the action should actually see: declared defaults first, the invocation's
/// values over them, every value already read as its declared kind — so an
/// adapter that asked for a number gets [`ArgValue::Int`] whatever the
/// frontend was able to produce.
///
/// **An action that declares nothing is not validated**: `supplied` comes back
/// untouched. Validation is what declaring buys, and making it retroactive
/// would break every adapter that reads an argument it never announced.
///
/// Problems are collected rather than returned one at a time: a user who
/// mistyped two arguments should learn both on the first run.
pub fn resolve_args(
    params: &[ParamSpec],
    supplied: &ActionArgs,
) -> Result<ActionArgs, Vec<ArgProblem>> {
    if params.is_empty() {
        return Ok(supplied.clone());
    }

    let known: Vec<String> = params.iter().map(|p| p.key.clone()).collect();
    let mut problems = Vec::new();
    for key in supplied.keys() {
        if !known.iter().any(|k| k == key) {
            problems.push(ArgProblem::Unknown {
                key: key.to_string(),
                known: known.clone(),
            });
        }
    }

    // Declaration order, so the resolved set reads the way `help` prints it.
    let mut resolved = ActionArgs::new();
    for param in params {
        let raw = supplied.get(&param.key).or(param.default.as_ref());
        let Some(raw) = raw else {
            if param.required {
                problems.push(ArgProblem::Missing {
                    key: param.key.clone(),
                });
            }
            continue;
        };
        match param.kind.read(raw) {
            Some(value) => resolved.insert(param.key.clone(), value),
            None => problems.push(ArgProblem::WrongType {
                key: param.key.clone(),
                expected: param.kind,
                got: raw.clone(),
            }),
        }
    }

    if problems.is_empty() {
        Ok(resolved)
    } else {
        Err(problems)
    }
}

/// The problems as one message, for a frontend that has a single line to say
/// it in.
pub fn describe_problems(problems: &[ArgProblem]) -> String {
    problems
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod param_tests {
    use super::*;

    fn params() -> Vec<ParamSpec> {
        vec![
            ParamSpec::path("buffer", "file the editor opens"),
            ParamSpec::int("limit", "how many rows").with_default(50i64),
            ParamSpec::text("reason", "why").required(),
        ]
    }

    #[test]
    fn resolution_normalises_to_the_declared_type() {
        let supplied: ActionArgs = [("limit", "20"), ("reason", "cleanup")]
            .into_iter()
            .collect();

        let resolved = resolve_args(&params(), &supplied).unwrap();

        // The command line only had text; the adapter sees the declared type.
        assert_eq!(resolved.get("limit"), Some(&ArgValue::Int(20)));
        // Declaration order, not the order the user typed.
        assert_eq!(resolved.keys().collect::<Vec<_>>(), ["limit", "reason"]);
    }

    #[test]
    fn a_default_fills_in_and_an_invocation_overrides_it() {
        let empty: ActionArgs = [("reason", "cleanup")].into_iter().collect();
        let resolved = resolve_args(&params(), &empty).unwrap();
        assert_eq!(resolved.int("limit"), Some(50));

        let given: ActionArgs = [("reason", "cleanup"), ("limit", "5")]
            .into_iter()
            .collect();
        let resolved = resolve_args(&params(), &given).unwrap();
        assert_eq!(resolved.int("limit"), Some(5));
    }

    #[test]
    fn every_problem_is_reported_on_the_first_run() {
        let supplied: ActionArgs = [("limt", "20"), ("buffer", "true")].into_iter().collect();

        let problems = resolve_args(&params(), &supplied).unwrap_err();
        let message = describe_problems(&problems);

        // The typo, and the required argument it was meant to be — both.
        assert!(message.contains("unknown argument 'limt'"), "{message}");
        assert!(
            message.contains("known: buffer, limit, reason"),
            "{message}"
        );
        assert!(
            message.contains("missing required argument 'reason'"),
            "{message}"
        );
    }

    #[test]
    fn a_value_that_cannot_be_the_declared_kind_is_refused() {
        let supplied: ActionArgs = [("reason", "cleanup"), ("limit", "soon")]
            .into_iter()
            .collect();

        let problems = resolve_args(&params(), &supplied).unwrap_err();

        assert_eq!(
            problems,
            vec![ArgProblem::WrongType {
                key: "limit".into(),
                expected: ArgKind::Int,
                got: ArgValue::Text("soon".into()),
            }]
        );
    }

    #[test]
    fn an_action_that_declares_nothing_takes_its_arguments_as_they_come() {
        let supplied: ActionArgs = [("anything", "at all")].into_iter().collect();

        // Validating retroactively would break every adapter reading an
        // argument it never announced.
        assert_eq!(resolve_args(&[], &supplied).unwrap(), supplied);
    }
}

#[cfg(test)]
mod yaml_tests {
    use super::*;

    #[test]
    fn a_yaml_value_has_the_parsers_type_not_a_guessed_one() {
        let args: ActionArgs = serde_yaml::from_str(
            "limit: 20\nforce: true\nbuild: \"007\"\ntags: [a, b]\nbuffer: ~/x.md\n",
        )
        .unwrap();

        assert_eq!(args.get("limit"), Some(&ArgValue::Int(20)));
        assert_eq!(args.get("force"), Some(&ArgValue::Bool(true)));
        assert_eq!(args.get("build"), Some(&ArgValue::Text("007".into())));
        assert_eq!(
            args.get("tags"),
            Some(&ArgValue::List(vec!["a".into(), "b".into()]))
        );
        // YAML has no path type: a path arrives as text and the declared
        // parameter kind turns it into one.
        assert_eq!(args.get("buffer"), Some(&ArgValue::Text("~/x.md".into())));
    }

    #[test]
    fn the_order_written_is_the_order_kept() {
        let args: ActionArgs = serde_yaml::from_str("z: 1\na: 2\nm: 3\n").unwrap();
        assert_eq!(args.keys().collect::<Vec<_>>(), vec!["z", "a", "m"]);
    }

    #[test]
    fn a_mapping_a_decimal_and_a_null_are_refused() {
        let err = serde_yaml::from_str::<ActionArgs>("nested: {a: 1}\n").unwrap_err();
        assert!(err.to_string().contains("mapping"), "{err}");

        let err = serde_yaml::from_str::<ActionArgs>("ratio: 0.5\n").unwrap_err();
        assert!(err.to_string().contains("decimal"), "{err}");

        let err = serde_yaml::from_str::<ActionArgs>("unset: ~\n").unwrap_err();
        assert!(err.to_string().contains("drop the key"), "{err}");
    }

    #[test]
    fn a_list_holds_scalars_only() {
        let err = serde_yaml::from_str::<ActionArgs>("tags: [[a], b]\n").unwrap_err();
        assert!(err.to_string().contains("nested lists"), "{err}");

        // Scalars of any kind are fine and become their text.
        let args: ActionArgs = serde_yaml::from_str("ids: [1, true, x]\n").unwrap();
        assert_eq!(
            args.list("ids"),
            Some(vec!["1".into(), "true".into(), "x".into()])
        );
    }
}

#[cfg(test)]
mod placeholder_tests {
    use super::*;

    fn resolver(name: &str) -> Option<String> {
        match name {
            "node_id" => Some("ABC-1".into()),
            "workspace" => Some("/data/jira".into()),
            n => n.strip_prefix("cell:").and_then(|k| match k {
                "summary" => Some("fix the thing".into()),
                _ => None,
            }),
        }
    }

    #[test]
    fn placeholders_expand_on_every_text_bearing_value() {
        let args = ActionArgs::new()
            .with(
                "buffer",
                ArgValue::path("{workspace}/{node_id}/ticket.edit.md"),
            )
            .with("title", "{node_id}: {cell:summary}")
            .with(
                "labels",
                ArgValue::List(vec!["{node_id}".into(), "plain".into()]),
            )
            .with("limit", 20i64);

        let out = args.expand(resolver).unwrap();

        assert_eq!(
            out.path("buffer"),
            Some(PathBuf::from("/data/jira/ABC-1/ticket.edit.md"))
        );
        assert_eq!(out.text("title").as_deref(), Some("ABC-1: fix the thing"));
        assert_eq!(
            out.list("labels"),
            Some(vec!["ABC-1".into(), "plain".into()])
        );
        assert_eq!(out.int("limit"), Some(20));
    }

    #[test]
    fn an_unresolved_placeholder_refuses_the_whole_set() {
        let args = ActionArgs::new()
            .with("buffer", "{workspace}/{cell:missing}/x")
            .with("other", "{nobody}");

        let problems = args.expand(resolver).unwrap_err();

        assert_eq!(problems.len(), 2);
        assert_eq!(
            problems[0],
            ArgProblem::Unresolved {
                key: "buffer".into(),
                placeholder: "cell:missing".into()
            }
        );
        assert_eq!(
            problems[0].to_string(),
            "argument 'buffer': nothing resolves the placeholder {cell:missing}"
        );
    }

    #[test]
    fn braces_around_anything_else_are_left_alone() {
        let args = ActionArgs::new().with("fmt", "{} { x } {1} {node_id} {");
        let out = args.expand(resolver).unwrap();
        assert_eq!(out.text("fmt").as_deref(), Some("{} { x } {1} ABC-1 {"));
    }
}
