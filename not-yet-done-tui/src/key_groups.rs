//! Folding a chord group into a single bar entry.
//!
//! A which-key group ([`WhichKeyGroup`]) names everything bound under one
//! chord prefix. With `collapse_in_bars` set, the action and status bars stop
//! listing the group's chords one by one and show the group itself instead —
//! `o Open …` rather than `o f open file`, `o d open dir`, …
//!
//! The bars work with rendered key labels, not with claims, so that is where
//! the match happens: [`label_in_group`] decides whether a label like `o f`
//! (or an alternatives label like `⌫/h`) belongs to a prefix, and
//! [`collapse`] rewrites a list of bar entries in one pass. Both bars and all
//! three entry kinds (hints, query favorites, script shortcuts) go through
//! [`collapse`], so a group folds the same way wherever it shows up.

use crate::config::keybindings::binding_steps;
use crate::config::tui_config::WhichKeyGroup;

/// Does the bar label `label` sit under the chord prefix `prefix`?
///
/// True for every deeper chord (`o f`, `o d`) and **false for the prefix key
/// on its own**: a view that binds plain `o` to an action of its own does not
/// open the group's menu there, so folding that entry away would hide a key
/// the popup could never explain. Both sides are parsed with
/// [`binding_steps`], the parser the dispatcher itself uses, so the legacy
/// concatenated form matches just as well: `zm` sits under `z`.
///
/// Alternatives are checked one by one: `⌫/h` belongs to a group as soon as
/// one of its keys does, because the bar would otherwise advertise a folded
/// key through the back door. Brackets around a label (`[s]`, the form the
/// app-global status hints use) are stripped first.
pub fn label_in_group(label: &str, prefix: &str) -> bool {
    let steps = binding_steps(prefix);
    if steps.is_empty() {
        return false;
    }
    let label = label
        .strip_prefix('[')
        .and_then(|l| l.strip_suffix(']'))
        .unwrap_or(label);
    label.split('/').any(|alt| {
        let keys = binding_steps(alt);
        keys.len() > steps.len() && keys[..steps.len()] == steps[..]
    })
}

/// Replace every entry that belongs to a collapsing group with one entry for
/// the group, at the position of the first entry it swallowed. Entries
/// outside every group keep their place and their order.
///
/// `key_of` reads an entry's rendered key label, `entry` builds the
/// replacement from the group. When a label matches several groups the
/// longest prefix wins, so a narrower `o p` group still shows up inside a
/// broader `o` one.
pub fn collapse<T, K, E>(groups: &[WhichKeyGroup], items: Vec<T>, key_of: K, entry: E) -> Vec<T>
where
    K: Fn(&T) -> &str,
    E: Fn(&WhichKeyGroup) -> T,
{
    let collapsing = collapsing(groups);
    if collapsing.is_empty() {
        return items;
    }
    let mut out = Vec::with_capacity(items.len());
    let mut placed: Vec<&str> = Vec::new();
    for item in items {
        match best_group(&collapsing, key_of(&item)) {
            Some(group) => {
                if !placed.contains(&group.prefix.as_str()) {
                    placed.push(&group.prefix);
                    out.push(entry(group));
                }
            }
            None => out.push(item),
        }
    }
    out
}

/// [`collapse`] for a list that has no room for a group entry of its own:
/// drops the grouped entries and hands the groups that lost one back, in
/// config order, so the caller can surface them where they do belong. The
/// action bar folds its query favorites and script shortcuts this way — the
/// group shows up once, among the hints, not once per bar section.
pub fn strip<'a, T, K>(
    groups: &'a [WhichKeyGroup],
    items: Vec<T>,
    key_of: K,
) -> (Vec<T>, Vec<&'a WhichKeyGroup>)
where
    K: Fn(&T) -> &str,
{
    let collapsing = collapsing(groups);
    if collapsing.is_empty() {
        return (items, Vec::new());
    }
    let mut kept = Vec::with_capacity(items.len());
    let mut hit: Vec<&WhichKeyGroup> = Vec::new();
    for item in items {
        match best_group(&collapsing, key_of(&item)) {
            Some(group) => {
                if !hit.iter().any(|g| g.prefix == group.prefix) {
                    hit.push(group);
                }
            }
            None => kept.push(item),
        }
    }
    (kept, hit)
}

/// The group a key label belongs to, over *every* configured group — folding
/// groups and merely-named ones alike. [`collapse`] and [`strip`] only ever
/// look at the folding ones; the shortcut overview groups by name, so it asks
/// here. Same longest-prefix rule.
pub fn group_of<'a>(groups: &'a [WhichKeyGroup], label: &str) -> Option<&'a WhichKeyGroup> {
    let named: Vec<&WhichKeyGroup> = groups
        .iter()
        .filter(|g| !g.prefix.trim().is_empty())
        .collect();
    best_group(&named, label)
}

/// The groups that actually fold something, in config order.
fn collapsing(groups: &[WhichKeyGroup]) -> Vec<&WhichKeyGroup> {
    groups
        .iter()
        .filter(|g| g.collapse_in_bars && !g.prefix.trim().is_empty())
        .collect()
}

/// The group a bar label falls into: the one with the longest matching
/// prefix, so a narrower `o p` group still shows up inside a broader `o` one.
fn best_group<'a>(collapsing: &[&'a WhichKeyGroup], label: &str) -> Option<&'a WhichKeyGroup> {
    collapsing
        .iter()
        .filter(|g| label_in_group(label, &g.prefix))
        .max_by_key(|g| g.prefix.split_whitespace().count())
        .copied()
}

/// The label a collapsed group carries in the bars: its title, or a bare `…`
/// when it has none — the entry still has to say that more keys follow.
pub fn group_label(group: &WhichKeyGroup) -> String {
    group
        .title
        .clone()
        .unwrap_or_else(|| "\u{2026}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(prefix: &str, title: Option<&str>, collapse: bool) -> WhichKeyGroup {
        WhichKeyGroup {
            prefix: prefix.to_string(),
            title: title.map(|t| t.to_string()),
            collapse_in_bars: collapse,
        }
    }

    #[test]
    fn a_label_belongs_to_its_prefix_and_to_nothing_else() {
        assert!(label_in_group("o f", "o"));
        assert!(label_in_group("ctrl+k l", "ctrl+k"));
        assert!(!label_in_group("p", "o"));
        assert!(!label_in_group("o f", "o d"));
        // The legacy concatenated chord form parses to the same steps.
        assert!(label_in_group("zm", "z"));
        assert!(!label_in_group("zm", "g"));
        // An atomic named key is one step, not a chord under `e`.
        assert!(!label_in_group("enter", "e"));
        // Bracketed labels (the app-global status hints) match too.
        assert!(label_in_group("[zr]", "z"));
    }

    /// A view may bind the prefix key on its own — `o` opens the notes in the
    /// tasks view. That binding runs on press instead of opening the group's
    /// menu, so it keeps its own bar entry.
    #[test]
    fn the_bare_prefix_key_is_a_binding_of_its_own_not_part_of_the_group() {
        assert!(!label_in_group("o", "o"));
        assert!(!label_in_group("ctrl+k", "ctrl+k"));
        let groups = vec![group("o", Some("Open ..."), true)];
        let items = vec![
            ("o".to_string(), "notes".to_string()),
            ("o f".to_string(), "open file".to_string()),
        ];
        let out = collapse(
            &groups,
            items,
            |(k, _): &(String, String)| k.as_str(),
            |g| (g.prefix.clone(), group_label(g)),
        );
        assert_eq!(
            out,
            vec![
                ("o".to_string(), "notes".to_string()),
                ("o".to_string(), "Open ...".to_string()),
            ]
        );
    }

    #[test]
    fn any_alternative_under_the_prefix_pulls_the_label_in() {
        assert!(label_in_group("\u{232b}/o f", "o"));
        assert!(!label_in_group("\u{232b}/h", "o"));
    }

    #[test]
    fn collapsing_keeps_order_and_folds_each_group_once() {
        let groups = vec![group("o", Some("Open ..."), true)];
        let items = vec![
            ("q".to_string(), "queries".to_string()),
            ("o f".to_string(), "open file".to_string()),
            ("o d".to_string(), "open dir".to_string()),
            ("J".to_string(), "jump".to_string()),
        ];
        let out = collapse(
            &groups,
            items,
            |(k, _): &(String, String)| k.as_str(),
            |g| (g.prefix.clone(), group_label(g)),
        );
        assert_eq!(
            out,
            vec![
                ("q".to_string(), "queries".to_string()),
                ("o".to_string(), "Open ...".to_string()),
                ("J".to_string(), "jump".to_string()),
            ]
        );
    }

    #[test]
    fn a_group_without_collapse_leaves_the_bar_alone() {
        let groups = vec![group("o", Some("Open ..."), false)];
        let items = vec![("o f".to_string(), "open file".to_string())];
        let out = collapse(
            &groups,
            items.clone(),
            |(k, _): &(String, String)| k.as_str(),
            |g| (g.prefix.clone(), group_label(g)),
        );
        assert_eq!(out, items);
    }

    #[test]
    fn the_longest_matching_prefix_wins() {
        let groups = vec![
            group("o", Some("Open ..."), true),
            group("o p", Some("Open project ..."), true),
        ];
        let items = vec![
            ("o f".to_string(), "open file".to_string()),
            ("o p x".to_string(), "open project x".to_string()),
        ];
        let out = collapse(
            &groups,
            items,
            |(k, _): &(String, String)| k.as_str(),
            |g| (g.prefix.clone(), group_label(g)),
        );
        assert_eq!(
            out,
            vec![
                ("o".to_string(), "Open ...".to_string()),
                ("o p".to_string(), "Open project ...".to_string()),
            ]
        );
    }

    /// The overview groups by name, so `group_of` must see a group the bars
    /// ignore — and still pick the longest matching prefix.
    #[test]
    fn group_of_sees_every_group_not_just_the_folding_ones() {
        let groups = vec![
            group("o", Some("Open ..."), false),
            group("o p", Some("Open project ..."), false),
        ];
        assert_eq!(
            group_of(&groups, "o f").map(|g| g.prefix.as_str()),
            Some("o")
        );
        assert_eq!(
            group_of(&groups, "o p x").map(|g| g.prefix.as_str()),
            Some("o p")
        );
        assert!(group_of(&groups, "o").is_none(), "the bare prefix key");
        assert!(group_of(&groups, "q q").is_none());
        assert!(group_of(&[group("", None, true)], "o f").is_none());
    }

    #[test]
    fn a_titleless_group_still_says_that_more_keys_follow() {
        assert_eq!(group_label(&group("o", None, true)), "\u{2026}");
    }
}
