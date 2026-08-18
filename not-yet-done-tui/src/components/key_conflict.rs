//! Shared binding-conflict prompt — "this key is taken, drop the other one?".
//!
//! A recorded chord that collides with a live [`KeyClaim`] never gets written
//! silently. Every host (the shortcut menu, the query/script menus' in-popup
//! recorder, the App's capture overlay) raises the *same* prompt: it lists
//! each colliding binding, and confirming drops them all and applies the
//! pending change. A collision with a read-only binding cannot be resolved,
//! so the prompt then only offers to dismiss.
//!
//! [`KeyClaim`]: crate::keymap::KeyClaim

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::keymap::KeySource;
use crate::ui::theme::Theme;

/// Key hints shown while a conflict prompt is up.
pub const CONFLICT_HINTS: &[(&str, &str)] = &[("y", "apply"), ("n/Esc", "cancel")];

/// One existing binding that collides with a proposed new binding. Carries
/// everything the app needs to drop the colliding alternative, plus the
/// display metadata the prompt shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictItem {
    /// The conflicting shortcut's source (so the app can edit its file).
    pub source: KeySource,
    /// All current bindings of the conflicting shortcut (surface forms).
    pub current: Vec<String>,
    /// The specific alternative of `source` that collides.
    pub drop: String,
    /// Friendly name of the conflicting shortcut, for the prompt text.
    pub name: String,
    /// Whether this binding is editable (and thus removable). If any
    /// conflict is not removable the collision cannot be resolved and the
    /// new binding is refused.
    pub removable: bool,
}

/// A pending y/n prompt shown when a change collides with one or more existing
/// shortcuts. `kind` is the host's own description of what confirming applies
/// — a recorded binding, a batch restore, whatever the host needs back.
#[derive(Debug, Clone)]
pub struct ConflictPrompt<K> {
    /// The change to apply once the collisions are cleared.
    pub kind: K,
    /// Every existing binding that collides with the pending change.
    pub items: Vec<ConflictItem>,
}

/// What a key press did to a conflict prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictReply {
    /// Key consumed, prompt stays up.
    Pending,
    /// Confirmed (y / Return) — apply the change and drop the collisions.
    Apply,
    /// Declined (n / Esc) — leave everything unchanged.
    Dismiss,
}

impl<K> ConflictPrompt<K> {
    pub fn new(kind: K, items: Vec<ConflictItem>) -> Self {
        Self { kind, items }
    }

    /// The collision can be resolved only if every colliding binding can be
    /// removed (all owning shortcuts are editable).
    pub fn resolvable(&self) -> bool {
        self.items.iter().all(|i| i.removable)
    }

    /// Dispatch a key. An unresolvable collision accepts dismissal only.
    pub fn handle_key(&self, key: &str) -> ConflictReply {
        match key {
            "y" | "enter" if self.resolvable() => ConflictReply::Apply,
            "n" | "esc" => ConflictReply::Dismiss,
            _ => ConflictReply::Pending,
        }
    }
}

/// The `⚠ binding conflict` heading every host renders above the prompt.
pub fn conflict_heading(t: &Theme) -> Line<'static> {
    Line::from(vec![Span::styled(
        "\u{26a0} binding conflict",
        Style::default()
            .fg(t.form_accent())
            .add_modifier(Modifier::BOLD),
    )])
}

/// The prompt body: what collides, with what, and the y/n question.
///
/// `summary` names the pending change (`"'ctrl+k l'"`, `"Restoring 2
/// defaults"`) and `question` is the host's apply wording. A read-only
/// collision replaces the question with the reason it cannot be resolved.
pub fn conflict_lines(
    summary: &str,
    items: &[ConflictItem],
    question: &str,
    t: &Theme,
) -> Vec<Line<'static>> {
    let text = Style::default().fg(t.form_text());
    let accent = Style::default()
        .fg(t.form_accent())
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(t.form_hint());
    let resolvable = items.iter().all(|i| i.removable);

    let mut lines: Vec<Line<'static>> = vec![Line::from(vec![
        Span::styled(summary.to_string(), accent),
        Span::styled(" conflicts with:", text),
    ])];
    for item in items {
        let mut spans = vec![
            Span::styled("  \u{2022} ", dim),
            Span::styled(item.drop.clone(), accent),
            Span::styled(" \u{2014} ", dim),
            Span::styled(item.name.clone(), text),
        ];
        if !item.removable {
            spans.push(Span::styled("  (read-only)", dim));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(Span::styled(
        if resolvable {
            question.to_string()
        } else {
            "Read-only bindings can't be removed — press n/Esc.".to_string()
        },
        text,
    )));
    lines
}

/// Width the prompt body needs so no line is truncated.
pub fn conflict_width(summary: &str, items: &[ConflictItem], question: &str) -> usize {
    let mut w = summary.chars().count() + " conflicts with:".chars().count();
    w = w.max(question.chars().count());
    w = w.max("Read-only bindings can't be removed — press n/Esc.".chars().count());
    for item in items {
        let line = 4 + item.drop.chars().count() + 3 + item.name.chars().count();
        w = w.max(if item.removable { line } else { line + 13 });
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::keybindings::GlobalAction;

    fn item(removable: bool) -> ConflictItem {
        ConflictItem {
            source: KeySource::Global(GlobalAction::Quit),
            current: vec!["q".into()],
            drop: "q".into(),
            name: "Quit".into(),
            removable,
        }
    }

    #[test]
    fn resolvable_only_when_every_item_is_removable() {
        assert!(ConflictPrompt::new((), vec![item(true)]).resolvable());
        assert!(!ConflictPrompt::new((), vec![item(true), item(false)]).resolvable());
    }

    #[test]
    fn confirm_needs_a_resolvable_prompt() {
        let ok = ConflictPrompt::new((), vec![item(true)]);
        assert_eq!(ok.handle_key("y"), ConflictReply::Apply);
        assert_eq!(ok.handle_key("enter"), ConflictReply::Apply);
        assert_eq!(ok.handle_key("n"), ConflictReply::Dismiss);
        assert_eq!(ok.handle_key("x"), ConflictReply::Pending);

        let ro = ConflictPrompt::new((), vec![item(false)]);
        assert_eq!(ro.handle_key("y"), ConflictReply::Pending);
        assert_eq!(ro.handle_key("esc"), ConflictReply::Dismiss);
    }

    #[test]
    fn read_only_body_swaps_the_question_for_the_reason() {
        let t = Theme::new(crate::config::ThemeConfig::default());
        let lines = conflict_lines("'q'", &[item(false)], "Bind here? (y/n)", &t);
        let last: String = lines
            .last()
            .unwrap()
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(last.starts_with("Read-only"), "{last}");
        let middle: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(middle.contains("(read-only)"), "{middle}");
    }
}
