use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

// ---------------------------------------------------------------------------
// KeyBinding
// ---------------------------------------------------------------------------

/// A key combination, e.g. "q", "ctrl+c", "tab", "shift+tab"
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyBinding(pub String);

impl KeyBinding {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns a short display label, e.g. "[q]"
    pub fn display_label(&self) -> String {
        format!("[{}]", self.0)
    }
}

// ---------------------------------------------------------------------------
// Action
// ---------------------------------------------------------------------------

/// All configurable actions in the TUI.
///
/// We derive Hash/Eq for use as a HashMap key, and implement Serialize /
/// Deserialize manually so that the enum serialises as a plain snake_case
/// string — which is required for it to work as a YAML mapping key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    TabWelcome,
    TabTasks,
    TabTrackings,
    TabNext,
    TabPrev,
}

impl Action {
    fn as_str(&self) -> &'static str {
        match self {
            Action::Quit         => "quit",
            Action::TabWelcome   => "tab_welcome",
            Action::TabTasks     => "tab_tasks",
            Action::TabTrackings => "tab_trackings",
            Action::TabNext      => "tab_next",
            Action::TabPrev      => "tab_prev",
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Action {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "quit"          => Ok(Action::Quit),
            "tab_welcome"   => Ok(Action::TabWelcome),
            "tab_tasks"     => Ok(Action::TabTasks),
            "tab_trackings" => Ok(Action::TabTrackings),
            "tab_next"      => Ok(Action::TabNext),
            "tab_prev"      => Ok(Action::TabPrev),
            other           => Err(format!("unknown action: {}", other)),
        }
    }
}

impl Serialize for Action {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Action {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Action::from_str(&raw).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// KeyBindingConfig
// ---------------------------------------------------------------------------

/// Maps each action to a key binding.
///
/// Serialises as a YAML mapping with snake_case action names as keys:
///
/// ```yaml
/// bindings:
///   quit: q
///   tab_welcome: "1"
///   tab_tasks: "2"
///   tab_trackings: "3"
///   tab_next: tab
///   tab_prev: shift+tab
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBindingConfig {
    #[serde(default = "default_bindings")]
    pub bindings: HashMap<Action, KeyBinding>,
}

fn default_bindings() -> HashMap<Action, KeyBinding> {
    let mut map = HashMap::new();
    map.insert(Action::Quit,         KeyBinding::new("q"));
    map.insert(Action::TabWelcome,   KeyBinding::new("1"));
    map.insert(Action::TabTasks,     KeyBinding::new("2"));
    map.insert(Action::TabTrackings, KeyBinding::new("3"));
    map.insert(Action::TabNext,      KeyBinding::new("tab"));
    map.insert(Action::TabPrev,      KeyBinding::new("shift+tab"));
    map
}

impl Default for KeyBindingConfig {
    fn default() -> Self {
        Self {
            bindings: default_bindings(),
        }
    }
}

impl KeyBindingConfig {
    pub fn get(&self, action: &Action) -> Option<&KeyBinding> {
        self.bindings.get(action)
    }

    /// Returns the display label for an action, e.g. "[q]". Falls back to "[?]".
    pub fn label(&self, action: &Action) -> String {
        self.get(action)
            .map(|k| k.display_label())
            .unwrap_or_else(|| "[?]".to_string())
    }
}
