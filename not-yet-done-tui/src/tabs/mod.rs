//! View and state type definitions.

/// Compose a tab-bar label as `icon key name`, placing the key/autonumber
/// hint **between** the icon and the title rather than after it. Empty
/// parts (a tab without an icon, or with no key hint) are dropped so the
/// result never carries double spaces.
pub fn tab_label(icon: &str, key: &str, name: &str) -> String {
    tab_label_with_marker("", icon, key, name)
}

/// [`tab_label`] with an unread marker in front of the icon — the same
/// order the tree uses inside the view (marker, then type icon, then the
/// name), so the two read as one convention. An empty marker (nothing
/// unread, or the glyph configured away) collapses to plain [`tab_label`].
pub fn tab_label_with_marker(marker: &str, icon: &str, key: &str, name: &str) -> String {
    [marker, icon, key, name]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// One entry of the main tab bar: the composed label plus the emphasis the
/// tab asks for while its view holds unread items.
pub struct MainTab {
    pub tab: Tab,
    pub label: String,
    /// Style patch layered on top of the bar's normal active/inactive style
    /// when the tab is unread. `None` = nothing unread, bar style untouched.
    pub unread: Option<ratatui::style::Style>,
}

/// Main tab — the top-level navigation. Since the built-in tabs were
/// retired in favour of [`ContentAdapter`](not_yet_done_content)-backed
/// tabs, every tab is now a `Content(idx)` indexing into
/// `App::content_views`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Dynamic content tab backed by a ContentAdapter. Index into App::content_views.
    Content(usize),
}

impl Tab {
    pub fn next(&self, content_count: usize) -> Tab {
        let last_content = content_count.saturating_sub(1);
        match self {
            Tab::Content(i) if *i < last_content => Tab::Content(i + 1),
            Tab::Content(_) => Tab::Content(0),
        }
    }

    pub fn prev(&self, content_count: usize) -> Tab {
        let last_content = content_count.saturating_sub(1);
        match self {
            Tab::Content(0) => Tab::Content(last_content),
            Tab::Content(i) => Tab::Content(i - 1),
        }
    }
}

/// The visible, ordered set of top-level tabs. Built once at startup /
/// config reload from the [`TabsConfig`](crate::config::TabsConfig)
/// order, or — when none is configured — from every content tab in slot
/// order.
///
/// `Tab::Content(idx)` keeps its canonical meaning (an index into
/// `App::content_views`); the layout only decides *which* tabs are
/// shown, *in what order*, and *which digit key* (`1`..`9`, then `0`)
/// selects each. So the rest of the app keeps switching on `Tab` exactly
/// as before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabLayout {
    /// Visible tabs in display / numbering order.
    order: Vec<Tab>,
}

impl TabLayout {
    /// Every content tab in slot order — the fallback when no explicit
    /// tab order is configured.
    pub fn all_tabs(content_count: usize) -> Self {
        let order = (0..content_count).map(Tab::Content).collect();
        Self { order }
    }

    /// Exactly the given tabs, in the given order.
    pub fn ordered(order: Vec<Tab>) -> Self {
        Self { order }
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.order
    }

    pub fn contains(&self, tab: Tab) -> bool {
        self.order.contains(&tab)
    }

    /// First visible tab — the startup / fallback selection. Defaults to
    /// the first content tab if the layout is somehow empty.
    pub fn first(&self) -> Tab {
        self.order.first().copied().unwrap_or(Tab::Content(0))
    }

    /// Digit char that selects `tab`, when the tab sits within the
    /// numberable range (`1`..`9`, then `0` for the tenth; an eleventh
    /// and beyond get none).
    pub fn digit_for(&self, tab: Tab) -> Option<char> {
        let idx = self.order.iter().position(|t| *t == tab)?;
        digit_for_index(idx)
    }

    /// Resolve a pressed key to a visible tab via the autonumber map.
    /// Returns `None` for non-digit / out-of-range keys, so the caller
    /// can fall through to normal dispatch.
    ///
    /// Dispatch now resolves tab switches through the App's effective
    /// bindings (`tab.key` overrides plus digits), so this positional-only
    /// lookup is retained for its unit tests and as a layout query.
    #[allow(dead_code)]
    pub fn tab_for_key(&self, key: &str) -> Option<Tab> {
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
pub(crate) fn digit_for_index(idx: usize) -> Option<char> {
    match idx {
        0..=8 => Some((b'1' + idx as u8) as char),
        9 => Some('0'),
        _ => None,
    }
}

/// Build a [`TabLayout`] from the configured tab order.
///
/// `available` is the full list of selectable tabs as
/// `(display_name, Tab)` in their natural order (content tabs by slot).
/// Resolution:
///
///   * Two tabs sharing a display name → `Err` (hard config error; the
///     name can no longer identify a tab uniquely).
///   * No order configured → all tabs in slot order.
///   * Configured order resolves to zero known tabs → all tabs in slot
///     order, with the reason returned via `warn` for logging.
///   * Otherwise → the named tabs, in order; names matching no tab are
///     skipped (and reported through `warn`).
pub fn resolve_tab_layout(
    cfg: &crate::config::TabsConfig,
    available: &[(String, Tab)],
    content_count: usize,
    mut warn: impl FnMut(String),
) -> Result<TabLayout, String> {
    // Integrity check first — duplicate names are fatal regardless of
    // whether an explicit order is configured.
    let mut by_name: std::collections::HashMap<&str, Tab> = std::collections::HashMap::new();
    for (name, tab) in available {
        if by_name.insert(name.as_str(), *tab).is_some() {
            return Err(format!(
                "Two tabs share the name \"{name}\". Tab names must be unique \
                 (each view's `tab.name`) so `tabs.order` can reference them \
                 unambiguously. Rename one."
            ));
        }
    }

    if !cfg.is_active() {
        return Ok(TabLayout::all_tabs(content_count));
    }

    let mut order = Vec::new();
    for name in cfg.order() {
        match by_name.get(name.as_str()) {
            Some(tab) => order.push(*tab),
            None => warn(format!(
                "tabs.order references unknown tab \"{name}\" — skipped"
            )),
        }
    }

    if order.is_empty() {
        warn(
            "tabs.order resolved to no known tabs — showing all tabs in \
             their default order"
                .to_string(),
        );
        return Ok(TabLayout::all_tabs(content_count));
    }

    Ok(TabLayout::ordered(order))
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

    #[test]
    fn unread_marker_leads_the_label() {
        use super::tab_label_with_marker;
        // Same order as the tree row: marker, then type icon, then the name.
        assert_eq!(
            tab_label_with_marker("🔔", "💬", "9", "Stoat"),
            "🔔 💬 9 Stoat"
        );
        // Nothing unread (or the glyph configured away) → the plain label.
        assert_eq!(tab_label_with_marker("", "💬", "9", "Stoat"), "💬 9 Stoat");
    }
}

#[cfg(test)]
mod tab_layout_tests {
    use super::*;
    use crate::config::TabsConfig;

    /// 4 content tabs: Trackings, Jira, Taiga, Analytics DB (slots 0..3).
    /// Every tab is a ContentAdapter tab now — there are no built-ins.
    fn available() -> Vec<(String, Tab)> {
        vec![
            ("Trackings".into(), Tab::Content(0)),
            ("Jira".into(), Tab::Content(1)),
            ("Taiga".into(), Tab::Content(2)),
            ("Analytics DB".into(), Tab::Content(3)),
        ]
    }

    fn no_warn() -> impl FnMut(String) {
        |_w| {}
    }

    fn cfg_with(order: &[&str]) -> TabsConfig {
        TabsConfig {
            order: order.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn no_order_shows_all_tabs_in_slot_order() {
        let cfg = TabsConfig::default(); // empty order
        let layout = resolve_tab_layout(&cfg, &available(), 4, no_warn()).unwrap();
        assert_eq!(
            layout.tabs(),
            &[
                Tab::Content(0),
                Tab::Content(1),
                Tab::Content(2),
                Tab::Content(3)
            ]
        );
        // Autonumber is always live now.
        assert_eq!(layout.digit_for(Tab::Content(0)), Some('1'));
        assert_eq!(layout.tab_for_key("1"), Some(Tab::Content(0)));
    }

    #[test]
    fn order_selects_and_numbers_named_tabs() {
        let cfg = cfg_with(&["Trackings", "Jira", "Analytics DB"]);
        let layout = resolve_tab_layout(&cfg, &available(), 4, no_warn()).unwrap();
        // Trackings=Content(0), Jira=Content(1), Analytics DB=Content(3).
        assert_eq!(
            layout.tabs(),
            &[Tab::Content(0), Tab::Content(1), Tab::Content(3)]
        );
        // Taiga (Content(2)) is hidden — not in the order.
        assert!(!layout.contains(Tab::Content(2)));
        // Order → digits 1,2,3.
        assert_eq!(layout.digit_for(Tab::Content(0)), Some('1'));
        assert_eq!(layout.digit_for(Tab::Content(3)), Some('3'));
        assert_eq!(layout.tab_for_key("2"), Some(Tab::Content(1)));
        assert_eq!(layout.tab_for_key("3"), Some(Tab::Content(3)));
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
    fn unknown_name_in_order_is_skipped_with_warning() {
        let cfg = cfg_with(&["Trackings", "Ghost", "Jira"]);
        let mut warnings = Vec::new();
        let layout = resolve_tab_layout(&cfg, &available(), 4, |w| warnings.push(w)).unwrap();
        assert_eq!(layout.tabs(), &[Tab::Content(0), Tab::Content(1)]);
        assert!(
            warnings.iter().any(|w| w.contains("Ghost")),
            "warned about the unknown tab: {warnings:?}"
        );
    }

    #[test]
    fn order_of_only_unknown_tabs_falls_back_to_all_with_warning() {
        let cfg = cfg_with(&["Ghost", "Phantom"]);
        let mut warnings = Vec::new();
        let layout = resolve_tab_layout(&cfg, &available(), 4, |w| warnings.push(w)).unwrap();
        assert_eq!(layout.tabs().len(), 4, "fell back to all tabs");
        assert!(warnings.iter().any(|w| w.contains("no known tabs")));
    }

    #[test]
    fn next_prev_wrap_within_visible_order() {
        let cfg = cfg_with(&["Trackings", "Jira", "Taiga"]);
        let layout = resolve_tab_layout(&cfg, &available(), 4, no_warn()).unwrap();
        // Visible order: Content(0), Content(1), Content(2).
        assert_eq!(layout.next(Tab::Content(0)), Tab::Content(1));
        assert_eq!(layout.next(Tab::Content(2)), Tab::Content(0)); // wrap
        assert_eq!(layout.prev(Tab::Content(0)), Tab::Content(2)); // wrap back
        // A tab outside the layout snaps to first.
        assert_eq!(layout.next(Tab::Content(3)), Tab::Content(0));
    }

    #[test]
    fn tenth_tab_gets_zero_eleventh_gets_nothing() {
        // 11 tabs; only 1..9 then 0 are numberable.
        let names: Vec<String> = (0..11).map(|i| format!("T{i}")).collect();
        let avail: Vec<(String, Tab)> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.clone(), Tab::Content(i)))
            .collect();
        let cfg = TabsConfig {
            order: names.clone(),
            ..Default::default()
        };
        let layout = resolve_tab_layout(&cfg, &avail, 11, no_warn()).unwrap();
        assert_eq!(layout.digit_for(Tab::Content(8)), Some('9'));
        assert_eq!(layout.digit_for(Tab::Content(9)), Some('0'));
        assert_eq!(layout.digit_for(Tab::Content(10)), None);
        assert_eq!(layout.tab_for_key("0"), Some(Tab::Content(9)));
    }
}
