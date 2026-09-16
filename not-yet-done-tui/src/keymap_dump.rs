//! Plain-text rendering of the keymap for the terminal (`--keymap`).
//!
//! The shortcut menu's "All tabs" scope already answers "which keys exist and
//! where are they active" — but only inside a running TUI, one screenful at a
//! time. This module takes the very same [`ShortcutRow`]s (see
//! [`crate::app::App::all_shortcut_rows`]) and lays them out for stdout, so
//! the map can be read, grepped and diffed outside the app.
//!
//! Layout: one section per scope, in the order the rows arrive (`Global`
//! first, then tab by tab and level by level), each row as
//! `<key>  <action>  <config path>`. Actions that carry no key at all are
//! collected into a trailing "Unbound" section — the menu's third scope.
//!
//! One thing the dump knows that the menu does not: a tab can be configured,
//! loaded and keymapped and still be absent from `tabs.order`, which is an
//! allowlist. Its bindings are real but unreachable, because no key switches
//! to the tab. Such scopes are marked rather than dropped — dropping them
//! would hide the very config mistake the dump is read to find.

use crate::keymap::ShortcutRow;
use std::collections::HashMap;
use unicode_width::UnicodeWidthStr;

/// Render `rows` as the printable keymap. `filter`, when given, keeps only
/// rows whose key, action name, scope or config path contains it
/// (case-insensitive). `hidden_tabs` names the tabs that `tabs.order` leaves
/// out (see [`crate::app::App::hidden_tab_names`]); every scope belonging to
/// one is marked unreachable.
pub fn render(rows: &[ShortcutRow], filter: Option<&str>, hidden_tabs: &[String]) -> String {
    let needle = filter.map(str::to_lowercase);
    let kept: Vec<&ShortcutRow> = rows
        .iter()
        .filter(|r| match &needle {
            None => true,
            Some(n) => haystack(r).contains(n.as_str()),
        })
        .collect();

    let (bound, unbound): (Vec<&ShortcutRow>, Vec<&ShortcutRow>) =
        kept.into_iter().partition(|r| !r.keys.trim().is_empty());

    let mut out = String::new();
    out.push_str(&format!(
        "{} bindings in {} scopes{}\n",
        bound.len(),
        group_order(&bound).len(),
        match filter {
            Some(f) => format!(" (filter: {f})"),
            None => String::new(),
        }
    ));
    if !hidden_tabs.is_empty() {
        out.push_str(&format!(
            "{} not in tabs.order, so no key switches there: {}\n",
            if hidden_tabs.len() == 1 {
                "One configured tab is"
            } else {
                "Configured tabs are"
            },
            hidden_tabs.join(", "),
        ));
    }

    for scope in group_order(&bound) {
        let mut group: Vec<&&ShortcutRow> = bound.iter().filter(|r| r.scope == scope).collect();
        group.sort_by_key(|r| (sort_key(&r.keys), r.name.to_lowercase()));
        out.push('\n');
        out.push_str(&scope);
        if scope_is_hidden(&scope, hidden_tabs) {
            out.push_str("   (not in tabs.order — unreachable)");
        }
        out.push('\n');
        let key_w = group.iter().map(|r| width(&r.keys)).max().unwrap_or(0);
        let name_w = group.iter().map(|r| width(&r.name)).max().unwrap_or(0);
        for row in group {
            out.push_str(&format!(
                "  {}  {}  {}\n",
                pad(&row.keys, key_w),
                pad(&row.name, name_w),
                source_path(row),
            ));
        }
    }

    if !unbound.is_empty() {
        out.push_str(&format!("\nUnbound ({})\n", unbound.len()));
        let name_w = unbound.iter().map(|r| width(&r.name)).max().unwrap_or(0);
        let scope_w = unbound.iter().map(|r| width(&r.scope)).max().unwrap_or(0);
        let mut group = unbound;
        group.sort_by_key(|r| (r.scope.clone(), r.name.to_lowercase()));
        for row in group {
            out.push_str(&format!(
                "  {}  {}  {}\n",
                pad(&row.name, name_w),
                pad(&row.scope, scope_w),
                source_path(row),
            ));
        }
    }

    out
}

/// Everything a `--keymap <filter>` term is matched against.
fn haystack(row: &ShortcutRow) -> String {
    format!(
        "{} {} {} {}",
        row.keys,
        row.name,
        row.scope,
        source_path(row)
    )
    .to_lowercase()
}

/// Where the binding is declared, as the diagnostic path the config editor
/// and the conflict messages use (`views.tickets.actions[edit sql]`). Empty
/// for the handful of runtime-only claims that carry no source.
fn source_path(row: &ShortcutRow) -> String {
    row.source.as_ref().map(|s| s.human()).unwrap_or_default()
}

/// Whether `scope` belongs to a tab that `tabs.order` leaves out. Scope
/// labels are `Tab` or `Tab › level › level`, so the tab is the first
/// segment — and `Global` never matches, which is right: the global keys
/// work no matter which tabs are visible.
fn scope_is_hidden(scope: &str, hidden_tabs: &[String]) -> bool {
    let tab = scope.split(" › ").next().unwrap_or(scope);
    hidden_tabs.iter().any(|h| h == tab)
}

/// Scope labels in first-seen order, so the sections follow the tab bar
/// rather than the alphabet.
fn group_order(rows: &[&ShortcutRow]) -> Vec<String> {
    let mut seen: HashMap<&str, ()> = HashMap::new();
    let mut order = Vec::new();
    for row in rows {
        if seen.insert(row.scope.as_str(), ()).is_none() {
            order.push(row.scope.clone());
        }
    }
    order
}

/// Sort keys so single characters come before chords and modifier keys,
/// which is how one scans for a free key: `a`, `b`, … then `ga`, `gb`, …
/// then `ctrl+a`. Digits stay ahead of letters (the tab-switch block).
fn sort_key(keys: &str) -> (u8, String) {
    let first = keys.split(" / ").next().unwrap_or(keys);
    let class = if first.contains('+') {
        2
    } else if first.chars().count() > 1 {
        1
    } else {
        0
    };
    (class, first.to_lowercase())
}

fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Pad `s` to `w` display columns. Uses the terminal width of the string,
/// not its byte length, so a `›` in a scope label does not shift the column.
fn pad(s: &str, w: usize) -> String {
    let mut out = s.to_string();
    for _ in width(s)..w {
        out.push(' ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{KeyScope, KeySource, ShortcutRow};

    fn row(keys: &str, name: &str, scope: &str) -> ShortcutRow {
        ShortcutRow {
            name: name.to_string(),
            keys: keys.to_string(),
            scope: scope.to_string(),
            source: Some(KeySource::TabSwitch {
                tab: name.to_string(),
            }),
            key_scope: Some(KeyScope::Global),
        }
    }

    #[test]
    fn groups_by_scope_in_first_seen_order() {
        let rows = vec![
            row("1", "Switch to Tasks", "Global"),
            row("e", "edit", "Jira"),
            row("2", "Switch to Jira", "Global"),
        ];
        let out = render(&rows, None, &[]);
        let global = out.find("Global").unwrap();
        let jira = out.find("\nJira\n").unwrap();
        assert!(global < jira, "Global section comes first:\n{out}");
        // Both Global rows land in the one section.
        assert!(out[global..jira].contains("Switch to Jira"));
    }

    #[test]
    fn single_keys_sort_before_chords_and_modifiers() {
        let rows = vec![
            row("ctrl+a", "third", "Global"),
            row("ga", "second", "Global"),
            row("b", "first", "Global"),
        ];
        let out = render(&rows, None, &[]);
        let pos = |n: &str| out.find(n).unwrap();
        assert!(pos("first") < pos("second"));
        assert!(pos("second") < pos("third"));
    }

    #[test]
    fn keyless_rows_move_to_the_unbound_section() {
        let rows = vec![
            row("", "toggle-tracking", "Trackings"),
            row("e", "edit", "Jira"),
        ];
        let out = render(&rows, None, &[]);
        assert!(out.contains("Unbound (1)"));
        let unbound = out.find("Unbound (1)").unwrap();
        assert!(out.find("toggle-tracking").unwrap() > unbound);
        assert!(out.find("edit").unwrap() < unbound);
        assert!(out.starts_with("1 bindings in 1 scopes"));
    }

    #[test]
    fn a_tab_outside_tabs_order_is_marked_in_every_one_of_its_scopes() {
        let rows = vec![
            row("1", "Switch to Tasks", "Global"),
            row("e", "edit", "Jira"),
            row("r", "refresh", "systemd"),
            row("C", "chain", "systemd › Services"),
        ];
        let out = render(&rows, None, &["systemd".to_string()]);
        assert!(out.contains("One configured tab is not in tabs.order"));
        assert!(out.contains("systemd   (not in tabs.order — unreachable)"));
        assert!(out.contains("systemd › Services   (not in tabs.order — unreachable)"));
        // The visible tab and the global scope stay unmarked.
        assert!(out.contains("\nJira\n"));
        assert!(out.contains("\nGlobal\n"));
    }

    #[test]
    fn filter_matches_key_name_and_scope() {
        let rows = vec![row("e", "edit", "Jira"), row("d", "delete", "Tasks")];
        assert!(!render(&rows, Some("jira"), &[]).contains("delete"));
        assert!(!render(&rows, Some("delete"), &[]).contains("Jira"));
        assert!(render(&rows, Some("d"), &[]).contains("delete"));
    }
}
