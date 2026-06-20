//! Unified action enum — the single source of truth for "what should happen"
//! in response to a key press. Produced by `resolve_key`, consumed by `dispatch`.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::config::{
    CommonAction, ContentAction, FormAction, GlobalAction, QueryMenuAction, WindowAction,
};

/// The input mode the app is currently in — determines how keys are resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// Saved filter popup is open (intercepts all keys).
    Popup,
    /// FuzzyFilter text input is active.
    Fuzzy,
    /// Filter form is open.
    FilterForm,
    /// Normal mode — tasks or global actions.
    Normal,
}

/// Every possible action the app can take in response to a key press.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Action {
    /// A global action (tab switching, quit).
    Global(GlobalAction),
    /// A common action (shared between tabs: nav, scroll, fuzzy, search, etc.).
    Common(CommonAction),
    /// A ContentView action (drill open / back / pagination / edit query).
    Content(ContentAction),
    /// A split-pane window-management action.
    Window(WindowAction),
    /// A query-menu-popup action.
    QueryMenu(QueryMenuAction),
    /// A form navigation action (next/prev field, multiselect).
    Form(FormAction),
    /// Text input: insert a character.
    InsertChar(char),
    /// Text input: delete character before cursor.
    Backspace,
    /// Cursor movement.
    CursorLeft,
    CursorRight,
    /// Submit / confirm (Enter).
    Submit,
    /// Escape — context-dependent (close popup, clear fuzzy, close form).
    Escape,
    /// Reset (ctrl+r in filter form).
    Reset,
    /// Toggle (space/enter on multiselect items).
    Toggle,
    /// Recognized key but blocked (e.g. tab switch while form is open).
    Blocked,
    /// Key not recognized — do nothing.
    Noop,
}

impl Action {
    /// Whether this action is allowed inside a user-defined chain in V1.
    /// The whitelist is intentionally small: only operations that compose
    /// cleanly without modal state. Chain-config validation rejects
    /// anything outside this set at load time.
    pub fn is_chainable(&self) -> bool {
        matches!(
            self,
            Action::Window(
                WindowAction::Close
                    | WindowAction::SplitRight
                    | WindowAction::SplitDown
                    | WindowAction::FocusParent
                    | WindowAction::FocusChild,
            ) | Action::Content(
                ContentAction::Open
                    | ContentAction::Back
                    | ContentAction::NextPage
                    | ContentAction::PrevPage,
            ) | Action::Common(
                CommonAction::ListNext
                    | CommonAction::ListPrev
                    | CommonAction::ListFirst
                    | CommonAction::ListLast
                    | CommonAction::ColumnLeft
                    | CommonAction::ColumnRight,
            ),
        )
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::Global(a) => write!(f, "global.{a}"),
            Action::Common(a) => write!(f, "common.{a}"),
            Action::Content(a) => write!(f, "content.{a}"),
            Action::Window(a) => write!(f, "window.{a}"),
            Action::QueryMenu(a) => write!(f, "query_menu.{a}"),
            Action::Form(a) => write!(f, "form.{a}"),
            other => write!(f, "<internal:{other:?}>"),
        }
    }
}

impl FromStr for Action {
    type Err = String;

    /// Parse the dot-notation form `<section>.<action>`. Used to deserialise
    /// `action_chains:` entries in YAML. The section prefix is mandatory —
    /// bare action names are rejected so chain authors must spell out which
    /// scope they mean (helps catch typos like `list_next` that exist in
    /// multiple sections).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (section, name) = s
            .split_once('.')
            .ok_or_else(|| format!("action `{s}` is missing `<section>.` prefix"))?;
        match section {
            "global" => GlobalAction::from_str(name).map(Action::Global),
            "common" => CommonAction::from_str(name).map(Action::Common),
            "content" => ContentAction::from_str(name).map(Action::Content),
            "window" => WindowAction::from_str(name).map(Action::Window),
            "query_menu" => QueryMenuAction::from_str(name).map(Action::QueryMenu),
            "form" => FormAction::from_str(name).map(Action::Form),
            other => Err(format!("unknown action section: `{other}` in `{s}`")),
        }
    }
}

/// User-configured chain bindings: each key string maps to either a
/// validated chain of [`Action`]s or `None` (chain explicitly disabled at
/// this scope, no fallback up the resolution chain). Deserializes from
/// YAML where chains are written as lists of dot-notation strings, e.g.:
///
/// ```yaml
/// action_chains:
///   "ctrl+n": [window.focus_parent, common.list_next, content.open]
///   "ctrl+p": ~        # disabled at this scope
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActionChains(pub HashMap<String, Option<Vec<Action>>>);

impl ActionChains {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Look up `key`. Three outcomes:
    /// - `Some(Some(chain))` — run the chain at this scope.
    /// - `Some(None)` — explicitly disabled at this scope, **don't** fall back.
    /// - `None` — not defined here, caller should look at the next scope.
    pub fn lookup(&self, key: &str) -> Option<&Option<Vec<Action>>> {
        self.0.get(key)
    }
}

/// Walk a stack of [`ActionChains`] scopes innermost-first (`scopes[0]`
/// is most specific) and return the effective entry for `key`. Used by
/// both the runtime dispatcher and the config-time validator so the
/// "child wins over view wins over global, `None` disables" rule lives
/// in one place.
pub fn resolve_chain_in_scopes<'a>(
    scopes: &[&'a ActionChains],
    key: &str,
) -> Option<&'a Option<Vec<Action>>> {
    for s in scopes {
        if let Some(entry) = s.lookup(key) {
            return Some(entry);
        }
    }
    None
}

/// Collect every key defined anywhere in `scopes` and return the subset
/// whose effective resolution is `Some(chain)`. Disabled entries
/// (`None` at the innermost scope) drop out. Each entry carries the
/// index in `scopes` of the scope that actually defined the live chain
/// — callers (e.g. the validator) use that to attribute the binding to
/// its source scope without re-walking.
pub fn effective_chains_in_scopes<'a>(
    scopes: &[&'a ActionChains],
) -> HashMap<String, (usize, &'a Vec<Action>)> {
    let mut keys: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in scopes {
        keys.extend(s.0.keys().cloned());
    }
    let mut out = HashMap::new();
    for key in keys {
        for (i, s) in scopes.iter().enumerate() {
            if let Some(entry) = s.lookup(&key) {
                if let Some(chain) = entry {
                    out.insert(key, (i, chain));
                }
                break;
            }
        }
    }
    out
}

impl Serialize for ActionChains {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let m: HashMap<&String, Option<Vec<String>>> = self
            .0
            .iter()
            .map(|(k, v)| {
                (
                    k,
                    v.as_ref()
                        .map(|chain| chain.iter().map(|a| a.to_string()).collect()),
                )
            })
            .collect();
        m.serialize(s)
    }
}

impl<'de> Deserialize<'de> for ActionChains {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de;
        let raw: HashMap<String, Option<Vec<String>>> = HashMap::deserialize(d)?;
        let mut resolved: HashMap<String, Option<Vec<Action>>> = HashMap::new();
        for (key, chain_opt) in raw {
            let chain = match chain_opt {
                None => None,
                Some(tokens) => {
                    let mut actions = Vec::with_capacity(tokens.len());
                    for token in tokens {
                        let action = Action::from_str(&token).map_err(|e| {
                            de::Error::custom(format!(
                                "action_chains[{key}]: {e}"
                            ))
                        })?;
                        if !action.is_chainable() {
                            return Err(de::Error::custom(format!(
                                "action_chains[{key}]: `{token}` is not chainable in V1"
                            )));
                        }
                        actions.push(action);
                    }
                    Some(actions)
                }
            };
            resolved.insert(key, chain);
        }
        Ok(ActionChains(resolved))
    }
}

/// Determine the current input mode from app state.
pub fn input_mode(
    popup_open: bool,
    fuzzy_active: bool,
    filter_form_open: bool,
) -> InputMode {
    if popup_open {
        InputMode::Popup
    } else if fuzzy_active {
        InputMode::Fuzzy
    } else if filter_form_open {
        InputMode::FilterForm
    } else {
        InputMode::Normal
    }
}

fn is_printable(key: &str) -> bool {
    let mut chars = key.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if !c.is_control())
}

/// Pure key resolution: map a key string to an Action based on the current
/// mode and keybindings. No state mutation happens here.
pub fn resolve_key(
    key: &str,
    mode: InputMode,
    keybindings: &crate::config::KeyBindingConfig,
    form_visible: bool,
) -> Action {
    match mode {
        InputMode::Popup => resolve_popup_key(key, keybindings),
        InputMode::Fuzzy => resolve_fuzzy_key(key, keybindings),
        InputMode::FilterForm => resolve_filter_form_key(key, keybindings),
        InputMode::Normal => resolve_normal_key(key, keybindings, form_visible),
    }
}

fn resolve_popup_key(key: &str, kb: &crate::config::KeyBindingConfig) -> Action {
    // Use form keybindings for navigation (default: ctrl+j / ctrl+k).
    for (action, binding) in &kb.form.bindings {
        if binding.matches(key) {
            match action {
                FormAction::Next | FormAction::MultiselectNext => return Action::Form(FormAction::Next),
                FormAction::Prev | FormAction::MultiselectPrev => return Action::Form(FormAction::Prev),
            }
        }
    }
    match key {
        "esc" => Action::Escape,
        "enter" => Action::Submit,
        "down" => Action::Form(FormAction::Next),
        "up" => Action::Form(FormAction::Prev),
        "left" => Action::CursorLeft,
        "right" => Action::CursorRight,
        "backspace" => Action::Backspace,
        ch if is_printable(ch) => Action::InsertChar(ch.chars().next().unwrap()),
        _ => Action::Noop,
    }
}

fn resolve_fuzzy_key(key: &str, kb: &crate::config::KeyBindingConfig) -> Action {
    // Configurable fuzzy keys from common section.
    for (action, binding) in &kb.common.bindings {
        if binding.matches(key) {
            match action {
                CommonAction::FuzzyFilterAccept => return Action::Common(CommonAction::FuzzyFilterAccept),
                CommonAction::FuzzyFilterClear => return Action::Common(CommonAction::FuzzyFilterClear),
                CommonAction::FuzzyFilterCancel => return Action::Common(CommonAction::FuzzyFilterCancel),
                _ => {}
            }
        }
    }
    match key {
        "backspace" => Action::Backspace,
        "left" => Action::CursorLeft,
        "right" => Action::CursorRight,
        ch if is_printable(ch) => Action::InsertChar(ch.chars().next().unwrap()),
        _ => Action::Noop,
    }
}

fn resolve_filter_form_key(key: &str, kb: &crate::config::KeyBindingConfig) -> Action {
    // Check form keybindings first.
    for (action, binding) in &kb.form.bindings {
        if binding.matches(key) {
            return Action::Form(action.clone());
        }
    }
    // Check for common-level close.
    for (action, binding) in &kb.common.bindings {
        if binding.matches(key) && *action == CommonAction::FormClose {
            return Action::Common(CommonAction::FormClose);
        }
    }
    match key {
        "left" => Action::CursorLeft,
        "right" => Action::CursorRight,
        "backspace" => Action::Backspace,
        "enter" | " " => Action::Toggle,
        "ctrl+r" => Action::Reset,
        ch if is_printable(ch) => Action::InsertChar(ch.chars().next().unwrap()),
        _ => Action::Noop,
    }
}

fn resolve_normal_key(
    key: &str,
    kb: &crate::config::KeyBindingConfig,
    form_visible: bool,
) -> Action {
    // Common keybindings (shared between tabs).
    for (action, binding) in &kb.common.bindings {
        if binding.matches(key) {
            return Action::Common(action.clone());
        }
    }

    // Global keybindings.
    for (action, binding) in &kb.global.bindings {
        if binding.matches(key) {
            // Block tab switching while a form is open.
            if form_visible {
                match action {
                    GlobalAction::TabJira
                    | GlobalAction::TabTaiga
                    | GlobalAction::TabPostgres
                    | GlobalAction::TabConfluence
                    | GlobalAction::TabNext
                    | GlobalAction::TabPrev => return Action::Blocked,
                    _ => {}
                }
            }
            return Action::Global(action.clone());
        }
    }

    Action::Noop
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::KeyBindingConfig;

    fn default_kb() -> KeyBindingConfig {
        KeyBindingConfig::default()
    }

    #[test]
    fn normal_mode_quit() {
        let kb = default_kb();
        let action = resolve_key("ctrl+c", InputMode::Normal, &kb, false);
        assert_eq!(action, Action::Global(GlobalAction::Quit));
    }

    #[test]
    fn normal_mode_tab_switch_blocked_while_form_open() {
        let kb = default_kb();
        // A tab-switch global key (e.g. "3" → Jira) is blocked while a form
        // is open. (Digit keys 1/2 are no longer fixed tab bindings since the
        // legacy Tasks/Trackings tabs were removed.)
        let action = resolve_key("3", InputMode::Normal, &kb, true);
        assert_eq!(action, Action::Blocked);
    }

    #[test]
    fn popup_mode_enter_submits() {
        let kb = default_kb();
        let action = resolve_key("enter", InputMode::Popup, &kb, false);
        assert_eq!(action, Action::Submit);
    }

    #[test]
    fn popup_mode_char_inserts() {
        let kb = default_kb();
        let action = resolve_key("x", InputMode::Popup, &kb, false);
        assert_eq!(action, Action::InsertChar('x'));
    }

    #[test]
    fn popup_mode_ctrl_j_navigates_down() {
        let kb = default_kb();
        // Default form next is ctrl+j.
        let action = resolve_key("ctrl+j", InputMode::Popup, &kb, false);
        assert_eq!(action, Action::Form(FormAction::Next));
    }

    #[test]
    fn popup_mode_ctrl_k_navigates_up() {
        let kb = default_kb();
        let action = resolve_key("ctrl+k", InputMode::Popup, &kb, false);
        assert_eq!(action, Action::Form(FormAction::Prev));
    }

    #[test]
    fn popup_mode_j_inserts_char() {
        let kb = default_kb();
        let action = resolve_key("j", InputMode::Popup, &kb, false);
        assert_eq!(action, Action::InsertChar('j'));
    }

    #[test]
    fn fuzzy_mode_accept_key() {
        let kb = default_kb();
        // Default fuzzy accept is "enter".
        let action = resolve_key("enter", InputMode::Fuzzy, &kb, false);
        assert_eq!(action, Action::Common(CommonAction::FuzzyFilterAccept));
    }

    #[test]
    fn fuzzy_mode_cancel_key() {
        let kb = default_kb();
        // Default fuzzy cancel is "esc".
        let action = resolve_key("esc", InputMode::Fuzzy, &kb, false);
        assert_eq!(action, Action::Common(CommonAction::FuzzyFilterCancel));
    }

    #[test]
    fn fuzzy_mode_char_inserts() {
        let kb = default_kb();
        let action = resolve_key("a", InputMode::Fuzzy, &kb, false);
        assert_eq!(action, Action::InsertChar('a'));
    }

    #[test]
    fn filter_form_next_field() {
        let kb = default_kb();
        // Default form next is "ctrl+j".
        let action = resolve_key("ctrl+j", InputMode::FilterForm, &kb, true);
        assert_eq!(action, Action::Form(FormAction::Next));
    }

    #[test]
    fn filter_form_close() {
        let kb = default_kb();
        let action = resolve_key("esc", InputMode::FilterForm, &kb, true);
        assert_eq!(action, Action::Common(CommonAction::FormClose));
    }

    #[test]
    fn unknown_key_is_noop() {
        let kb = default_kb();
        let action = resolve_key("f24", InputMode::Normal, &kb, false);
        assert_eq!(action, Action::Noop);
    }

    // ── Action FromStr / Display / chainable ────────────────────────────

    #[test]
    fn action_from_str_dot_notation_window() {
        assert_eq!(
            Action::from_str("window.focus_parent").unwrap(),
            Action::Window(WindowAction::FocusParent),
        );
        assert_eq!(
            Action::from_str("window.split_right").unwrap(),
            Action::Window(WindowAction::SplitRight),
        );
    }

    #[test]
    fn action_from_str_dot_notation_content_and_common() {
        assert_eq!(
            Action::from_str("content.open").unwrap(),
            Action::Content(ContentAction::Open),
        );
        assert_eq!(
            Action::from_str("common.list_next").unwrap(),
            Action::Common(CommonAction::ListNext),
        );
    }

    #[test]
    fn action_from_str_requires_section_prefix() {
        let err = Action::from_str("list_next").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn action_from_str_unknown_section_errors() {
        let err = Action::from_str("bogus.thing").unwrap_err();
        assert!(err.contains("section") && err.contains("bogus"), "got: {err}");
    }

    #[test]
    fn action_from_str_unknown_action_errors() {
        let err = Action::from_str("window.fly").unwrap_err();
        assert!(err.contains("fly"), "got: {err}");
    }

    #[test]
    fn action_display_round_trips_via_from_str() {
        let cases = [
            Action::Window(WindowAction::FocusParent),
            Action::Window(WindowAction::Close),
            Action::Content(ContentAction::Back),
            Action::Common(CommonAction::ListPrev),
        ];
        for a in cases {
            let s = a.to_string();
            let parsed = Action::from_str(&s).expect(&s);
            assert_eq!(parsed, a, "round-trip failed for `{s}`");
        }
    }

    #[test]
    fn action_chains_deserialise_chain_and_disabled() {
        let yaml = r#"
"ctrl+n": [window.focus_parent, common.list_next, content.open]
"ctrl+p": ~
"#;
        let chains: ActionChains = serde_yaml::from_str(yaml).unwrap();
        let n = chains.lookup("ctrl+n").unwrap().as_ref().unwrap();
        assert_eq!(
            n,
            &vec![
                Action::Window(WindowAction::FocusParent),
                Action::Common(CommonAction::ListNext),
                Action::Content(ContentAction::Open),
            ]
        );
        assert!(chains.lookup("ctrl+p").unwrap().is_none());
        assert!(chains.lookup("ctrl+x").is_none());
    }

    #[test]
    fn action_chains_reject_non_chainable_action() {
        let yaml = r#"
"ctrl+x": [global.quit]
"#;
        let err = serde_yaml::from_str::<ActionChains>(yaml).unwrap_err();
        assert!(err.to_string().contains("not chainable"), "got: {err}");
    }

    #[test]
    fn action_chains_reject_unknown_action() {
        let yaml = r#"
"ctrl+x": [content.warp]
"#;
        let err = serde_yaml::from_str::<ActionChains>(yaml).unwrap_err();
        assert!(err.to_string().contains("warp"), "got: {err}");
    }

    #[test]
    fn action_chains_reject_missing_section_prefix() {
        let yaml = r#"
"ctrl+x": [list_next]
"#;
        let err = serde_yaml::from_str::<ActionChains>(yaml).unwrap_err();
        assert!(err.to_string().contains("missing"), "got: {err}");
    }

    #[test]
    fn chainable_whitelist_v1() {
        // Allowed
        assert!(Action::Window(WindowAction::Close).is_chainable());
        assert!(Action::Window(WindowAction::FocusParent).is_chainable());
        assert!(Action::Window(WindowAction::FocusChild).is_chainable());
        assert!(Action::Content(ContentAction::Open).is_chainable());
        assert!(Action::Common(CommonAction::ListNext).is_chainable());
        // Not in V1 set
        assert!(!Action::Content(ContentAction::EditQuery).is_chainable());
        assert!(!Action::Common(CommonAction::ColumnConfig).is_chainable());
        assert!(!Action::Global(GlobalAction::Quit).is_chainable());
    }
}

