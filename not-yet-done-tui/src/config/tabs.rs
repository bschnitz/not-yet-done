//! Tab constellations — named, ordered sets of top-level tabs.
//!
//! A *constellation* is a named list of tab display names. The active
//! constellation drives two things at once:
//!
//!   * **Display order** — only the tabs it names are shown, in the
//!     order they appear in the list.
//!   * **Autonumber order** — the same order assigns the switch keys
//!     `1`..`9`, then `0` for a tenth tab; an eleventh and beyond get no
//!     digit (only `Tab`/`Shift+Tab` cycling reaches them).
//!
//! Tabs are referenced by their display name: the built-in `Tasks` /
//! `Trackings` tabs, plus each content view's `tab.name`. Two tabs
//! sharing a name is a hard configuration error — the name would no
//! longer uniquely identify a tab — surfaced as a startup modal.
//!
//! Why this exists: the previous model wired tab-switch keys to fixed
//! `GlobalAction` variants (`1`..`6`, positionally bound to
//! Jira/Taiga/Postgres/Confluence), so a fifth adapter tab (e.g. Stoat)
//! got no key at all. Constellations make the tab set and its numbering
//! data-driven and let a user keep several curated layouts side by side
//! (e.g. a lean `default` vs. a wider `my-corp`).
//!
//! When no constellation is configured (`sets` empty) the feature is
//! dormant: every configured tab is shown in its `order:` with the
//! legacy fixed keys, so existing setups keep working unchanged.

use std::collections::HashMap;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};

/// A single tab constellation: the ordered tab list plus optional
/// presentation/switching metadata (icon + popup shortcut).
///
/// Accepts two YAML shapes so existing plain-list configs keep parsing:
///
/// ```yaml
/// # Shorthand — just the ordered tab list (no icon, no shortcut):
/// default:
///   - Tasks
///   - Trackings
///
/// # Full form — adds an icon (shown in the switch popup) and a
/// # single-key shortcut (pressed in the popup to switch to this set):
/// work:
///   icon: ""
///   shortcut: w
///   tabs:
///     - Tasks
///     - Jira
/// ```
#[derive(Debug, Clone, Serialize)]
pub struct TabSet {
    /// Glyph shown next to the set name in the switch popup. `None`
    /// renders the name without a leading icon.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Human-friendly display name shown in the switch popup (e.g.
    /// `Work`). `None` falls back to the constellation's key under
    /// `sets`, so a slug-style key (`work`) can present as `Work`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Single key that switches to this set while the switch popup is
    /// open (e.g. `w`). `None` means the set is only reachable via the
    /// popup's arrow-key selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<String>,
    /// Ordered list of tab display names that make up this constellation.
    pub tabs: Vec<String>,
}

/// Wire representation used only for deserialization: a constellation is
/// either a bare sequence of tab names (shorthand) or the full mapping.
#[derive(Deserialize)]
#[serde(untagged)]
enum TabSetRepr {
    List(Vec<String>),
    Full {
        #[serde(default)]
        icon: Option<String>,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        shortcut: Option<String>,
        tabs: Vec<String>,
    },
}

impl<'de> Deserialize<'de> for TabSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match TabSetRepr::deserialize(deserializer)? {
            TabSetRepr::List(tabs) => Ok(TabSet {
                icon: None,
                label: None,
                shortcut: None,
                tabs,
            }),
            TabSetRepr::Full {
                icon,
                label,
                shortcut,
                tabs,
            } => {
                if let Some(s) = &shortcut {
                    if s.chars().count() != 1 {
                        return Err(de::Error::custom(format!(
                            "tab set shortcut \"{s}\" must be exactly one character"
                        )));
                    }
                }
                Ok(TabSet {
                    icon,
                    label,
                    shortcut,
                    tabs,
                })
            }
        }
    }
}

/// The `tabs:` section of `tui.yaml`.
///
/// ```yaml
/// tabs:
///   active: work
///   sets:
///     work:
///       icon: ""
///       shortcut: w
///       tabs:
///         - Tasks
///         - Trackings
///         - Jira
///         - Taiga
///     personal:
///       icon: ""
///       shortcut: p
///       tabs:
///         - Tasks
///         - Trackings
///         - Stoat
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabsConfig {
    /// Which constellation is currently visible. Defaults to `default`.
    /// Mutated in memory when the user switches via the tab-set popup;
    /// the change is session-only unless persisted back to `tui.yaml`.
    #[serde(default = "default_active")]
    pub active: String,
    /// Constellation name → its definition. Empty by default, which keeps
    /// the feature dormant (legacy all-tabs order + fixed keys).
    #[serde(default)]
    pub sets: HashMap<String, TabSet>,
}

fn default_active() -> String {
    "default".to_string()
}

impl Default for TabsConfig {
    fn default() -> Self {
        Self {
            active: default_active(),
            sets: HashMap::new(),
        }
    }
}

impl TabsConfig {
    /// True when at least one constellation is defined — i.e. the user
    /// has opted into the tab-set / autonumber behaviour.
    pub fn is_active(&self) -> bool {
        !self.sets.is_empty()
    }

    /// The ordered tab-name list for the active constellation, if it
    /// exists. `None` means the configured `active` name has no matching
    /// entry under `sets` (a soft error: callers fall back to the legacy
    /// layout and warn).
    pub fn active_set(&self) -> Option<&[String]> {
        self.sets.get(&self.active).map(|s| s.tabs.as_slice())
    }

    /// All constellations in a deterministic display order for the switch
    /// popup: sets carrying a `shortcut` first (ordered by that key), then
    /// the remainder alphabetically. `HashMap` iteration order is
    /// unspecified, so we impose one here for a stable popup layout.
    pub fn sets_sorted(&self) -> Vec<(&String, &TabSet)> {
        let mut entries: Vec<(&String, &TabSet)> = self.sets.iter().collect();
        entries.sort_by(|(a_name, a), (b_name, b)| {
            match (a.shortcut.as_deref(), b.shortcut.as_deref()) {
                (Some(x), Some(y)) => x.cmp(y).then_with(|| a_name.cmp(b_name)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a_name.cmp(b_name),
            }
        });
        entries
    }
}
