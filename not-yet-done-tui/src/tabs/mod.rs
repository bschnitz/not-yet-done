//! View and state type definitions.

pub mod columns;
mod load_state;
pub mod trackings_state;

pub use load_state::LoadState;
pub use trackings_state::{TrackingRow, TrackingsState};

/// Compose a tab-bar label as `icon key name`, placing the key/autonumber
/// hint **between** the icon and the title rather than after it. Empty
/// parts (a tab without an icon, or with no key hint) are dropped so the
/// result never carries double spaces.
pub fn tab_label(icon: &str, key: &str, name: &str) -> String {
    [icon, key, name]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Main tab — the top-level navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Trackings,
    /// Dynamic content tab backed by a ContentAdapter. Index into App::content_views.
    Content(usize),
}

impl Tab {
    pub fn next(&self, content_count: usize) -> Tab {
        let last_content = content_count.saturating_sub(1);
        match self {
            Tab::Trackings if content_count > 0 => Tab::Content(0),
            Tab::Trackings => Tab::Trackings,
            Tab::Content(i) if *i < last_content => Tab::Content(i + 1),
            Tab::Content(_) => Tab::Trackings,
        }
    }

    pub fn prev(&self, content_count: usize) -> Tab {
        let last_content = content_count.saturating_sub(1);
        match self {
            Tab::Trackings if content_count > 0 => Tab::Content(last_content),
            Tab::Trackings => Tab::Trackings,
            Tab::Content(0) => Tab::Trackings,
            Tab::Content(i) => Tab::Content(i - 1),
        }
    }
}

/// The visible, ordered set of top-level tabs plus their autonumber
/// state. Built once at startup / config reload from the active
/// [`TabsConfig`](crate::config::TabsConfig) constellation, or — when no
/// constellation is configured — from the legacy all-tabs order.
///
/// `Tab::Content(idx)` keeps its canonical meaning (an index into
/// `App::content_views`); the layout only decides *which* tabs are
/// shown, *in what order*, and *which digit key* selects each. So the
/// rest of the app keeps switching on `Tab` exactly as before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabLayout {
    /// Visible tabs in display / numbering order.
    order: Vec<Tab>,
    /// Whether autonumber digit keys (`1`..`9`, then `0`) are live. False
    /// in legacy mode, where the fixed `GlobalAction` keys still apply.
    autonumber: bool,
}

impl TabLayout {
    /// Legacy layout: Trackings, then every content tab in slot order.
    /// Autonumber off — the fixed `GlobalAction` tab keys (`1`..`6`) stay
    /// in charge, preserving pre-constellation behaviour.
    pub fn legacy(content_count: usize) -> Self {
        let mut order = vec![Tab::Trackings];
        order.extend((0..content_count).map(Tab::Content));
        Self {
            order,
            autonumber: false,
        }
    }

    /// Constellation layout: exactly the given tabs, in the given order,
    /// with autonumber digit keys active.
    pub fn constellation(order: Vec<Tab>) -> Self {
        Self {
            order,
            autonumber: true,
        }
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.order
    }

    pub fn autonumber(&self) -> bool {
        self.autonumber
    }

    pub fn contains(&self, tab: Tab) -> bool {
        self.order.contains(&tab)
    }

    /// First visible tab — the startup / fallback selection. Defaults to
    /// `Trackings` if the layout is somehow empty.
    pub fn first(&self) -> Tab {
        self.order.first().copied().unwrap_or(Tab::Trackings)
    }

    /// Digit char that selects `tab`, when autonumber is active and the
    /// tab sits within the numberable range (`1`..`9`, then `0` for the
    /// tenth; an eleventh and beyond get none).
    pub fn digit_for(&self, tab: Tab) -> Option<char> {
        if !self.autonumber {
            return None;
        }
        let idx = self.order.iter().position(|t| *t == tab)?;
        digit_for_index(idx)
    }

    /// Resolve a pressed key to a visible tab via the autonumber map.
    /// Returns `None` outside autonumber mode or for non-digit / out-of-
    /// range keys, so the caller can fall through to normal dispatch.
    pub fn tab_for_key(&self, key: &str) -> Option<Tab> {
        if !self.autonumber {
            return None;
        }
        self.order
            .iter()
            .enumerate()
            .find(|(idx, _)| digit_for_index(*idx).is_some_and(|d| d.to_string() == key))
            .map(|(_, t)| *t)
    }

    /// Next visible tab after `current`, wrapping. Falls back to the
    /// first tab when `current` isn't in the layout.
    pub fn next(&self, current: Tab) -> Tab {
        if self.order.is_empty() {
            return current;
        }
        match self.order.iter().position(|t| *t == current) {
            Some(i) => self.order[(i + 1) % self.order.len()],
            None => self.first(),
        }
    }

    /// Previous visible tab before `current`, wrapping.
    pub fn prev(&self, current: Tab) -> Tab {
        if self.order.is_empty() {
            return current;
        }
        match self.order.iter().position(|t| *t == current) {
            Some(i) => self.order[(i + self.order.len() - 1) % self.order.len()],
            None => self.first(),
        }
    }
}

/// Map a 0-based tab position to its autonumber key: `1`..`9` for the
/// first nine, `0` for the tenth, nothing beyond.
fn digit_for_index(idx: usize) -> Option<char> {
    match idx {
        0..=8 => Some((b'1' + idx as u8) as char),
        9 => Some('0'),
        _ => None,
    }
}

/// Build a [`TabLayout`] from the active constellation.
///
/// `available` is the full list of selectable tabs as
/// `(display_name, Tab)` in their natural order (built-ins first, then
/// content tabs by slot). Resolution:
///
///   * Two tabs sharing a display name → `Err` (hard config error; the
///     name can no longer identify a tab uniquely).
///   * No constellation configured → legacy layout.
///   * Active constellation missing, or it resolves to zero known tabs →
///     legacy layout, with the reason returned via `warn` for logging.
///   * Otherwise → the named tabs, in order; names matching no tab are
///     skipped (and reported through `warn`).
pub fn resolve_tab_layout(
    cfg: &crate::config::TabsConfig,
    available: &[(String, Tab)],
    content_count: usize,
    mut warn: impl FnMut(String),
) -> Result<TabLayout, String> {
    // Integrity check first — duplicate names are fatal regardless of
    // whether any constellation is configured.
    let mut by_name: std::collections::HashMap<&str, Tab> = std::collections::HashMap::new();
    for (name, tab) in available {
        if by_name.insert(name.as_str(), *tab).is_some() {
            return Err(format!(
                "Two tabs share the name \"{name}\". Tab names must be unique \
                 (built-in Trackings plus each view's `tab.name`) so a \
                 constellation can reference them unambiguously. Rename one."
            ));
        }
    }

    if !cfg.is_active() {
        return Ok(TabLayout::legacy(content_count));
    }

    let Some(names) = cfg.active_set() else {
        warn(format!(
            "tab constellation \"{}\" is not defined under `tabs.sets` — \
             showing all tabs in their default order",
            cfg.active
        ));
        return Ok(TabLayout::legacy(content_count));
    };

    let mut order = Vec::new();
    for name in names {
        match by_name.get(name.as_str()) {
            Some(tab) => order.push(*tab),
            None => warn(format!(
                "tab constellation \"{}\" references unknown tab \"{name}\" — skipped",
                cfg.active
            )),
        }
    }

    if order.is_empty() {
        warn(format!(
            "tab constellation \"{}\" resolved to no known tabs — \
             showing all tabs in their default order",
            cfg.active
        ));
        return Ok(TabLayout::legacy(content_count));
    }

    Ok(TabLayout::constellation(order))
}

/// Sub-view within the Trackings tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackingsSubView {
    Normal,
    Condensed,
    Tree,
}

impl TrackingsSubView {
    pub const ALL: &'static [TrackingsSubView] = &[
        TrackingsSubView::Normal,
        TrackingsSubView::Condensed,
        TrackingsSubView::Tree,
    ];

    pub fn title(&self) -> &'static str {
        match self {
            TrackingsSubView::Normal => "normal",
            TrackingsSubView::Condensed => "condensed",
            TrackingsSubView::Tree => "tree",
        }
    }
}

#[cfg(test)]
mod tab_label_tests {
    use super::tab_label;

    #[test]
    fn key_goes_between_icon_and_name() {
        assert_eq!(tab_label("✅", "1", "Tasks"), "✅ 1 Tasks");
    }

    #[test]
    fn empty_parts_are_dropped_without_double_spaces() {
        assert_eq!(tab_label("", "1", "Tasks"), "1 Tasks");
        assert_eq!(tab_label("✅", "", "Tasks"), "✅ Tasks");
        assert_eq!(tab_label("", "", "Tasks"), "Tasks");
    }
}

#[cfg(test)]
mod tab_layout_tests {
    use super::*;
    use crate::config::TabsConfig;
    use std::collections::HashMap;

    /// 3 content tabs: Jira, Taiga, Analytics DB (slots 0,1,2).
    fn available() -> Vec<(String, Tab)> {
        vec![
            ("Trackings".into(), Tab::Trackings),
            ("Jira".into(), Tab::Content(0)),
            ("Taiga".into(), Tab::Content(1)),
            ("Analytics DB".into(), Tab::Content(2)),
        ]
    }

    fn no_warn() -> impl FnMut(String) {
        |_w| {}
    }

    fn cfg_with(active: &str, sets: &[(&str, &[&str])]) -> TabsConfig {
        let mut map = HashMap::new();
        for (name, tabs) in sets {
            map.insert(
                name.to_string(),
                crate::config::tabs::TabSet {
                    icon: None,
                    label: None,
                    shortcut: None,
                    tabs: tabs.iter().map(|s| s.to_string()).collect(),
                },
            );
        }
        TabsConfig {
            active: active.to_string(),
            sets: map,
        }
    }

    #[test]
    fn no_constellation_yields_legacy_layout() {
        let cfg = TabsConfig::default(); // empty sets
        let layout = resolve_tab_layout(&cfg, &available(), 3, no_warn()).unwrap();
        assert_eq!(
            layout.tabs(),
            &[
                Tab::Trackings,
                Tab::Content(0),
                Tab::Content(1),
                Tab::Content(2)
            ]
        );
        // Legacy mode: digits stay with the fixed GlobalAction keys.
        assert!(!layout.autonumber());
        assert_eq!(layout.digit_for(Tab::Trackings), None);
        assert_eq!(layout.tab_for_key("1"), None);
    }

    #[test]
    fn constellation_orders_and_numbers_named_tabs() {
        let cfg = cfg_with(
            "default",
            &[("default", &["Trackings", "Jira", "Analytics DB"])],
        );
        let layout = resolve_tab_layout(&cfg, &available(), 3, no_warn()).unwrap();
        assert_eq!(
            layout.tabs(),
            &[Tab::Trackings, Tab::Content(0), Tab::Content(2)]
        );
        assert!(layout.autonumber());
        // Taiga (Content(1)) is hidden — not in the constellation.
        assert!(!layout.contains(Tab::Content(1)));
        // Order → digits 1,2,3.
        assert_eq!(layout.digit_for(Tab::Trackings), Some('1'));
        assert_eq!(layout.digit_for(Tab::Content(2)), Some('3'));
        assert_eq!(layout.tab_for_key("2"), Some(Tab::Content(0)));
        assert_eq!(layout.tab_for_key("3"), Some(Tab::Content(2)));
        // No fourth visible tab → key "4" maps nowhere.
        assert_eq!(layout.tab_for_key("4"), None);
    }

    #[test]
    fn duplicate_tab_name_is_hard_error() {
        let mut avail = available();
        avail.push(("Jira".into(), Tab::Content(3))); // name collision
        let cfg = TabsConfig::default();
        let err = resolve_tab_layout(&cfg, &avail, 4, no_warn()).unwrap_err();
        assert!(err.contains("Jira"), "error names the duplicate: {err}");
    }

    #[test]
    fn unknown_name_in_constellation_is_skipped_with_warning() {
        let cfg = cfg_with("default", &[("default", &["Trackings", "Ghost", "Jira"])]);
        let mut warnings = Vec::new();
        let layout =
            resolve_tab_layout(&cfg, &available(), 3, |w| warnings.push(w)).unwrap();
        assert_eq!(layout.tabs(), &[Tab::Trackings, Tab::Content(0)]);
        assert!(
            warnings.iter().any(|w| w.contains("Ghost")),
            "warned about the unknown tab: {warnings:?}"
        );
    }

    #[test]
    fn missing_active_set_falls_back_to_legacy_with_warning() {
        let cfg = cfg_with("my-corp", &[("default", &["Trackings", "Jira"])]);
        let mut warnings = Vec::new();
        let layout =
            resolve_tab_layout(&cfg, &available(), 3, |w| warnings.push(w)).unwrap();
        assert!(!layout.autonumber(), "fell back to legacy");
        assert_eq!(layout.tabs().len(), 4);
        assert!(warnings.iter().any(|w| w.contains("my-corp")));
    }

    #[test]
    fn next_prev_wrap_within_visible_order() {
        let cfg = cfg_with("default", &[("default", &["Trackings", "Jira", "Taiga"])]);
        let layout = resolve_tab_layout(&cfg, &available(), 3, no_warn()).unwrap();
        assert_eq!(layout.next(Tab::Trackings), Tab::Content(0));
        assert_eq!(layout.next(Tab::Content(1)), Tab::Trackings); // wrap
        assert_eq!(layout.prev(Tab::Trackings), Tab::Content(1)); // wrap back
        // A tab outside the layout snaps to first.
        assert_eq!(layout.next(Tab::Content(2)), Tab::Trackings);
    }

    #[test]
    fn tenth_tab_gets_zero_eleventh_gets_nothing() {
        // 11 tabs in one set; only 1..9 then 0 are numberable.
        let names: Vec<String> = (0..11).map(|i| format!("T{i}")).collect();
        let avail: Vec<(String, Tab)> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.clone(), Tab::Content(i)))
            .collect();
        let mut sets = HashMap::new();
        sets.insert(
            "default".to_string(),
            crate::config::tabs::TabSet {
                icon: None,
                label: None,
                shortcut: None,
                tabs: names.clone(),
            },
        );
        let cfg = TabsConfig {
            active: "default".into(),
            sets,
        };
        let layout = resolve_tab_layout(&cfg, &avail, 11, no_warn()).unwrap();
        assert_eq!(layout.digit_for(Tab::Content(8)), Some('9'));
        assert_eq!(layout.digit_for(Tab::Content(9)), Some('0'));
        assert_eq!(layout.digit_for(Tab::Content(10)), None);
        assert_eq!(layout.tab_for_key("0"), Some(Tab::Content(9)));
    }
}
