use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

// ---------------------------------------------------------------------------
// KeyBinding
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyBinding(pub String);

impl KeyBinding {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn display_label(&self) -> String {
        format!("[{}]", self.0)
    }
}

// ---------------------------------------------------------------------------
// Macro: Serialize/Deserialize via Display/FromStr
// ---------------------------------------------------------------------------

macro_rules! impl_string_serde {
    ($t:ty) => {
        impl Serialize for $t {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.to_string())
            }
        }
        impl<'de> Deserialize<'de> for $t {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(d)?;
                <$t>::from_str(&raw).map_err(serde::de::Error::custom)
            }
        }
    };
}

// ---------------------------------------------------------------------------
// GlobalAction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GlobalAction {
    Quit,
    TabWelcome,
    TabTasks,
    TabTrackings,
    TabNext,
    TabPrev,
}

impl GlobalAction {
    fn as_str(&self) -> &'static str {
        match self {
            GlobalAction::Quit => "quit",
            GlobalAction::TabWelcome => "tab_welcome",
            GlobalAction::TabTasks => "tab_tasks",
            GlobalAction::TabTrackings => "tab_trackings",
            GlobalAction::TabNext => "tab_next",
            GlobalAction::TabPrev => "tab_prev",
        }
    }
}

impl fmt::Display for GlobalAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for GlobalAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "quit" => Ok(GlobalAction::Quit),
            "tab_welcome" => Ok(GlobalAction::TabWelcome),
            "tab_tasks" => Ok(GlobalAction::TabTasks),
            "tab_trackings" => Ok(GlobalAction::TabTrackings),
            "tab_next" => Ok(GlobalAction::TabNext),
            "tab_prev" => Ok(GlobalAction::TabPrev),
            other => Err(format!("unknown global action: {}", other)),
        }
    }
}

impl_string_serde!(GlobalAction);

// ---------------------------------------------------------------------------
// FormAction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FormAction {
    Next,
    Prev,
    MultiselectNext,
    MultiselectPrev,
}

impl FormAction {
    fn as_str(&self) -> &'static str {
        match self {
            FormAction::Next => "next",
            FormAction::Prev => "prev",
            FormAction::MultiselectNext => "multiselect_next",
            FormAction::MultiselectPrev => "multiselect_prev",
        }
    }
}

impl fmt::Display for FormAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for FormAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "next" => Ok(FormAction::Next),
            "prev" => Ok(FormAction::Prev),
            "multiselect_next" => Ok(FormAction::MultiselectNext),
            "multiselect_prev" => Ok(FormAction::MultiselectPrev),
            other => Err(format!("unknown form action: {}", other)),
        }
    }
}

impl_string_serde!(FormAction);

// ---------------------------------------------------------------------------
// TasksAction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TasksAction {
    ViewList,
    ViewTree,
    FormFilter,
    FormAdd,
    FormDelete,
    FormClose,
    /// Move selection down in the task list.
    ListNext,
    /// Move selection up in the task list.
    ListPrev,
}

impl TasksAction {
    fn as_str(&self) -> &'static str {
        match self {
            TasksAction::ViewList => "view_list",
            TasksAction::ViewTree => "view_tree",
            TasksAction::FormFilter => "form_filter",
            TasksAction::FormAdd => "form_add",
            TasksAction::FormDelete => "form_delete",
            TasksAction::FormClose => "form_close",
            TasksAction::ListNext => "list_next",
            TasksAction::ListPrev => "list_prev",
        }
    }
}

impl fmt::Display for TasksAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TasksAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "view_list" => Ok(TasksAction::ViewList),
            "view_tree" => Ok(TasksAction::ViewTree),
            "form_filter" => Ok(TasksAction::FormFilter),
            "form_add" => Ok(TasksAction::FormAdd),
            "form_delete" => Ok(TasksAction::FormDelete),
            "form_close" => Ok(TasksAction::FormClose),
            "list_next" => Ok(TasksAction::ListNext),
            "list_prev" => Ok(TasksAction::ListPrev),
            other => Err(format!("unknown tasks action: {}", other)),
        }
    }
}

impl_string_serde!(TasksAction);

// ---------------------------------------------------------------------------
// KeyBindingSection<A>
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct KeyBindingSection<A: Eq + std::hash::Hash> {
    pub bindings: HashMap<A, KeyBinding>,
}

impl<A: Eq + std::hash::Hash> KeyBindingSection<A> {
    pub fn get(&self, action: &A) -> Option<&KeyBinding> {
        self.bindings.get(action)
    }

    pub fn label(&self, action: &A) -> String {
        self.get(action)
            .map(|k| k.display_label())
            .unwrap_or_else(|| "[?]".to_string())
    }
}

impl<A> Serialize for KeyBindingSection<A>
where
    A: Eq + std::hash::Hash + Serialize,
{
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.bindings.serialize(serializer)
    }
}

impl<'de, A> Deserialize<'de> for KeyBindingSection<A>
where
    A: Eq + std::hash::Hash + Deserialize<'de>,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let bindings = HashMap::<A, KeyBinding>::deserialize(deserializer)?;
        Ok(Self { bindings })
    }
}

impl Default for KeyBindingSection<GlobalAction> {
    fn default() -> Self {
        let mut m = HashMap::new();
        m.insert(GlobalAction::Quit, KeyBinding::new("q"));
        m.insert(GlobalAction::TabWelcome, KeyBinding::new("1"));
        m.insert(GlobalAction::TabTasks, KeyBinding::new("2"));
        m.insert(GlobalAction::TabTrackings, KeyBinding::new("3"));
        m.insert(GlobalAction::TabNext, KeyBinding::new("tab"));
        m.insert(GlobalAction::TabPrev, KeyBinding::new("shift+tab"));
        Self { bindings: m }
    }
}

impl Default for KeyBindingSection<TasksAction> {
    fn default() -> Self {
        let mut m = HashMap::new();
        m.insert(TasksAction::ViewList, KeyBinding::new("l"));
        m.insert(TasksAction::ViewTree, KeyBinding::new("t"));
        m.insert(TasksAction::FormFilter, KeyBinding::new("f"));
        m.insert(TasksAction::FormAdd, KeyBinding::new("a"));
        m.insert(TasksAction::FormDelete, KeyBinding::new("d"));
        m.insert(TasksAction::FormClose, KeyBinding::new("esc"));
        m.insert(TasksAction::ListNext, KeyBinding::new("j"));
        m.insert(TasksAction::ListPrev, KeyBinding::new("k"));
        Self { bindings: m }
    }
}

impl Default for KeyBindingSection<FormAction> {
    fn default() -> Self {
        let mut m = HashMap::new();
        m.insert(FormAction::Next, KeyBinding::new("ctrl+j"));
        m.insert(FormAction::Prev, KeyBinding::new("ctrl+k"));
        m.insert(FormAction::MultiselectNext, KeyBinding::new("tab"));
        m.insert(FormAction::MultiselectPrev, KeyBinding::new("shift+tab"));
        Self { bindings: m }
    }
}

// ---------------------------------------------------------------------------
// Top-level KeyBindingConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KeyBindingConfig {
    #[serde(default)]
    pub global: KeyBindingSection<GlobalAction>,
    #[serde(default)]
    pub tasks: KeyBindingSection<TasksAction>,
    #[serde(default)]
    pub form: KeyBindingSection<FormAction>,
}
