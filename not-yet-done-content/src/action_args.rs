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
