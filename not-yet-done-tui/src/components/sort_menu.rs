//! Sort menu — every sortable column in one list, the sorted ones first.
//!
//! A second UI path onto the same sort state the `S` sort-hint mode edits:
//! the hint mode is a fast one-column gesture (pick a label, pick a
//! direction), this menu shows the *whole* sort spec at once so a
//! multi-column sort can be re-ordered without rebuilding it key by key.
//! Both end in `App::commit_sort`, so there is one sort mechanic, not two.
//!
//! Invariant: the sorted entries are a **prefix** of the list, in sort
//! order, and the unsorted ones follow in the view's natural column order.
//! That is what makes the list readable as the sort spec — rank 1, 2, 3 —
//! and it is why `a`/`d` on an unsorted entry moves it up into the block
//! and `0` drops it back out.
//!
//! Nothing is applied while the menu is open: a live apply would trigger
//! one reload per keystroke on adapter-side sorts. Enter commits, Esc
//! discards.

use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};

use not_yet_done_content::{ColumnSchema, SortDirection, SortKey};

use crate::config::{CommonAction, KeyBindingConfig};
use crate::ui::popup_utils::{hints_height, render_hints_bar, render_popup_frame};
use crate::ui::theme::Theme;

/// One row: a sortable column, plus its place in the sort if it has one.
#[derive(Debug, Clone)]
struct SortEntry {
    key: String,
    label: String,
    /// `None` = not part of the sort. Entries carrying a direction form a
    /// prefix of the list, in sort order.
    direction: Option<SortDirection>,
    /// Index in the view's `columns`, so an entry that leaves the
    /// sorted block falls back into its natural place below it.
    origin: usize,
}

/// What a key press did to the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortMenuOutcome {
    /// Key consumed, menu stays open.
    Consumed,
    /// Esc — close and keep the view's current sort.
    Cancelled,
    /// Enter — close and apply this sort spec.
    Applied(Vec<SortKey>),
}

pub struct SortMenu {
    theme: Arc<Theme>,
    entries: Vec<SortEntry>,
    cursor: usize,
    hints: Vec<(String, &'static str)>,
}

impl SortMenu {
    /// Build the menu from the level's columns and its current sort.
    ///
    /// Only columns the adapter declares [`ColumnSchema::sortable`] are
    /// offered — a column nobody can sort by must not appear in a menu whose
    /// every entry promises a sort. Columns named by `current` that the level
    /// no longer offers are dropped just the same: a stale saved sort must not
    /// resurrect a gone column.
    pub fn new(
        theme: Arc<Theme>,
        columns: &[ColumnSchema],
        current: &[SortKey],
        kb: &KeyBindingConfig,
    ) -> Self {
        let columns: Vec<&ColumnSchema> = columns.iter().filter(|c| c.sortable).collect();
        let mut entries: Vec<SortEntry> = Vec::with_capacity(columns.len());
        for key in current {
            if let Some((origin, col)) = columns
                .iter()
                .enumerate()
                .find(|(_, c)| c.key == key.column)
            {
                entries.push(SortEntry {
                    key: col.key.clone(),
                    label: col.display_label().to_string(),
                    direction: Some(key.direction),
                    origin,
                });
            }
        }
        for (origin, col) in columns.iter().enumerate() {
            if entries.iter().any(|e| e.key == col.key) {
                continue;
            }
            entries.push(SortEntry {
                key: col.key.clone(),
                label: col.display_label().to_string(),
                direction: None,
                origin,
            });
        }

        let hints = vec![
            (kb.common.label(&CommonAction::ListPrev), "up"),
            (kb.common.label(&CommonAction::ListNext), "down"),
            ("C-k/C-j".to_string(), "move"),
            ("a".to_string(), "asc"),
            ("d".to_string(), "desc"),
            ("0".to_string(), "clear"),
            ("↵".to_string(), "apply"),
            ("Esc".to_string(), "cancel"),
        ];

        Self {
            theme,
            entries,
            cursor: 0,
            hints,
        }
    }

    /// Number of leading entries that carry a direction — the size of the
    /// sorted block.
    fn sorted_len(&self) -> usize {
        self.entries
            .iter()
            .take_while(|e| e.direction.is_some())
            .count()
    }

    /// The sort spec as it currently reads in the menu.
    pub fn result(&self) -> Vec<SortKey> {
        self.entries
            .iter()
            .filter_map(|e| {
                e.direction.map(|direction| SortKey {
                    column: e.key.clone(),
                    direction,
                })
            })
            .collect()
    }

    fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn move_down(&mut self) {
        if self.cursor + 1 < self.entries.len() {
            self.cursor += 1;
        }
    }

    /// Give the entry under the cursor a direction. An unsorted entry joins
    /// the sorted block at its end — the same "append" the sort-hint mode
    /// does — and the cursor follows it there.
    fn set_direction(&mut self, direction: SortDirection) {
        let Some(entry) = self.entries.get_mut(self.cursor) else {
            return;
        };
        if entry.direction.is_some() {
            entry.direction = Some(direction);
            return;
        }
        let mut entry = self.entries.remove(self.cursor);
        entry.direction = Some(direction);
        let target = self.sorted_len();
        self.entries.insert(target, entry);
        self.cursor = target;
    }

    /// Take the entry under the cursor out of the sort. It drops back below
    /// the sorted block, into its natural column position.
    fn clear_direction(&mut self) {
        if self
            .entries
            .get(self.cursor)
            .is_none_or(|e| e.direction.is_none())
        {
            return;
        }
        let mut entry = self.entries.remove(self.cursor);
        entry.direction = None;
        let sorted = self.sorted_len();
        let target = self.entries[sorted..]
            .iter()
            .position(|e| e.origin > entry.origin)
            .map(|p| sorted + p)
            .unwrap_or(self.entries.len());
        self.entries.insert(target, entry);
        self.cursor = target;
    }

    /// Re-order within the sorted block only: an unsorted entry has no rank
    /// to move, and pushing one into the block silently would hide the
    /// direction it would then need.
    fn reorder_up(&mut self) {
        if self.cursor == 0 || self.cursor >= self.sorted_len() {
            return;
        }
        self.entries.swap(self.cursor, self.cursor - 1);
        self.cursor -= 1;
    }

    fn reorder_down(&mut self) {
        if self.cursor + 1 >= self.sorted_len() {
            return;
        }
        self.entries.swap(self.cursor, self.cursor + 1);
        self.cursor += 1;
    }

    /// Dispatch a key. Every key is consumed while the menu is open so
    /// nothing leaks to the table behind it.
    pub fn handle_key(&mut self, key: &str, kb: &KeyBindingConfig) -> SortMenuOutcome {
        if key == "esc"
            || kb
                .common
                .get(&CommonAction::FormClose)
                .is_some_and(|b| b.matches(key))
        {
            return SortMenuOutcome::Cancelled;
        }
        if key == "enter" {
            return SortMenuOutcome::Applied(self.result());
        }
        // Reorder before navigation: `ctrl+j`/`ctrl+k` must not be eaten by
        // a nav binding that happens to include them.
        match key {
            "ctrl+k" => {
                self.reorder_up();
                return SortMenuOutcome::Consumed;
            }
            "ctrl+j" => {
                self.reorder_down();
                return SortMenuOutcome::Consumed;
            }
            "a" | "+" => {
                self.set_direction(SortDirection::Asc);
                return SortMenuOutcome::Consumed;
            }
            "d" | "-" => {
                self.set_direction(SortDirection::Desc);
                return SortMenuOutcome::Consumed;
            }
            "0" | "c" => {
                self.clear_direction();
                return SortMenuOutcome::Consumed;
            }
            _ => {}
        }
        if kb
            .common
            .get(&CommonAction::ListPrev)
            .is_some_and(|b| b.matches(key))
        {
            self.move_up();
        } else if kb
            .common
            .get(&CommonAction::ListNext)
            .is_some_and(|b| b.matches(key))
        {
            self.move_down();
        }
        SortMenuOutcome::Consumed
    }

    /// Row text after the rank column: `Label` plus the direction word for
    /// sorted entries.
    fn row_text(entry: &SortEntry) -> String {
        match entry.direction {
            Some(SortDirection::Asc) => format!("{} ↑ asc", entry.label),
            Some(SortDirection::Desc) => format!("{} ↓ desc", entry.label),
            None => entry.label.clone(),
        }
    }

    fn hints_as_refs(&self) -> Vec<(&str, &str)> {
        self.hints.iter().map(|(k, d)| (k.as_str(), *d)).collect()
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let t = Arc::clone(&self.theme);

        let text_w = self
            .entries
            .iter()
            .map(|e| Self::row_text(e).chars().count() + 4)
            .max()
            .unwrap_or(0);
        let popup_w = ((text_w as u16) + 4)
            .max(34)
            .min(area.width.saturating_sub(4));
        let hint_refs = self.hints_as_refs();
        let hints_h = hints_height(&hint_refs, popup_w.saturating_sub(2));
        let popup_h = self.entries.len() as u16 + 2 + hints_h;

        let inner = render_popup_frame(frame, area, &t, "Sort", popup_w, popup_h);
        if inner.height == 0 || inner.width == 0 {
            return;
        }

        {
            let buf = frame.buffer_mut();
            let items_height = inner.height.saturating_sub(hints_h) as usize;

            for (i, entry) in self.entries.iter().enumerate() {
                if i >= items_height {
                    break;
                }
                let row_y = inner.y + i as u16;
                let is_cursor = i == self.cursor;
                let bg = if is_cursor { t.surface_2() } else { t.bg() };

                for cx in inner.left()..inner.right() {
                    if let Some(cell) = buf.cell_mut(Position::new(cx, row_y)) {
                        cell.set_char(' ');
                        cell.set_style(Style::default().bg(bg));
                    }
                }

                // Rank column: the sort position for sorted entries, blank
                // for the rest — so the block reads as an ordered list.
                let rank = match entry.direction {
                    Some(_) => format!("{}. ", i + 1),
                    None => "   ".to_string(),
                };
                let mut cx = inner.left() + 1;
                let rank_style = Style::default().fg(t.text_dim()).bg(bg);
                for ch in rank.chars() {
                    if cx >= inner.right() {
                        break;
                    }
                    if let Some(cell) = buf.cell_mut(Position::new(cx, row_y)) {
                        cell.set_char(ch);
                        cell.set_style(rank_style);
                    }
                    cx += 1;
                }

                let style = if entry.direction.is_some() {
                    Style::default()
                        .fg(t.text_high())
                        .bg(bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(t.text_med()).bg(bg)
                };
                for ch in Self::row_text(entry).chars() {
                    if cx >= inner.right() {
                        break;
                    }
                    if let Some(cell) = buf.cell_mut(Position::new(cx, row_y)) {
                        cell.set_char(ch);
                        cell.set_style(style);
                    }
                    cx += 1;
                }
            }
        } // drop buf borrow

        render_hints_bar(frame, inner, &t, &hint_refs, hints_h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Arc<Theme> {
        Arc::new(Theme::new(crate::config::ThemeConfig::default()))
    }

    fn columns() -> Vec<ColumnSchema> {
        ["key", "summary", "updated"]
            .iter()
            .map(|k| ColumnSchema::new(*k, k.to_uppercase()))
            .collect()
    }

    #[test]
    fn a_column_the_adapter_cannot_sort_is_not_offered() {
        let cols = vec![
            ColumnSchema::new("key", "KEY"),
            ColumnSchema::new("attachments", "Attachm.").unsortable(),
        ];
        let menu = SortMenu::new(theme(), &cols, &[], &KeyBindingConfig::default());
        assert_eq!(menu.entries.len(), 1);
        assert_eq!(menu.entries[0].key, "key");
    }

    fn menu(current: &[SortKey]) -> SortMenu {
        SortMenu::new(theme(), &columns(), current, &KeyBindingConfig::default())
    }

    fn asc(col: &str) -> SortKey {
        SortKey {
            column: col.to_string(),
            direction: SortDirection::Asc,
        }
    }

    fn desc(col: &str) -> SortKey {
        SortKey {
            column: col.to_string(),
            direction: SortDirection::Desc,
        }
    }

    fn keys(menu: &SortMenu) -> Vec<String> {
        menu.entries.iter().map(|e| e.key.clone()).collect()
    }

    #[test]
    fn the_sorted_columns_lead_the_list_in_sort_order() {
        let m = menu(&[desc("updated"), asc("key")]);
        assert_eq!(keys(&m), ["updated", "key", "summary"]);
        assert_eq!(m.sorted_len(), 2);
        assert_eq!(m.result(), vec![desc("updated"), asc("key")]);
    }

    #[test]
    fn a_sort_on_a_column_the_view_dropped_is_ignored() {
        let m = menu(&[asc("gone"), asc("key")]);
        assert_eq!(m.result(), vec![asc("key")]);
    }

    #[test]
    fn giving_an_unsorted_column_a_direction_appends_it_to_the_block() {
        let kb = KeyBindingConfig::default();
        let mut m = menu(&[asc("key")]);
        m.handle_key("j", &kb); // -> summary (first unsorted)
        m.handle_key("d", &kb);
        assert_eq!(m.result(), vec![asc("key"), desc("summary")]);
        // The cursor followed the entry into the block.
        assert_eq!(m.cursor, 1);
    }

    #[test]
    fn flipping_the_direction_keeps_the_rank() {
        let kb = KeyBindingConfig::default();
        let mut m = menu(&[asc("key"), asc("summary")]);
        m.handle_key("j", &kb); // -> summary
        m.handle_key("d", &kb);
        assert_eq!(m.result(), vec![asc("key"), desc("summary")]);
    }

    #[test]
    fn clearing_drops_the_column_back_into_its_natural_place() {
        let kb = KeyBindingConfig::default();
        let mut m = menu(&[asc("updated"), asc("key")]);
        // cursor on `updated`, which is column 2 of the natural order.
        m.handle_key("0", &kb);
        assert_eq!(m.result(), vec![asc("key")]);
        assert_eq!(keys(&m), ["key", "summary", "updated"]);
    }

    #[test]
    fn reordering_moves_an_entry_inside_the_sorted_block() {
        let kb = KeyBindingConfig::default();
        let mut m = menu(&[asc("key"), desc("summary")]);
        m.handle_key("j", &kb); // -> summary
        m.handle_key("ctrl+k", &kb);
        assert_eq!(m.result(), vec![desc("summary"), asc("key")]);
        assert_eq!(m.cursor, 0);
    }

    #[test]
    fn an_unsorted_entry_cannot_be_reordered_into_the_block() {
        let kb = KeyBindingConfig::default();
        let mut m = menu(&[asc("key")]);
        m.handle_key("j", &kb); // -> summary, unsorted
        m.handle_key("ctrl+k", &kb);
        assert_eq!(keys(&m), ["key", "summary", "updated"]);
        assert_eq!(m.result(), vec![asc("key")]);
    }

    #[test]
    fn esc_discards_and_enter_applies() {
        let kb = KeyBindingConfig::default();
        let mut m = menu(&[asc("key")]);
        m.handle_key("d", &kb);
        assert_eq!(m.handle_key("esc", &kb), SortMenuOutcome::Cancelled);
        assert_eq!(
            m.handle_key("enter", &kb),
            SortMenuOutcome::Applied(vec![desc("key")])
        );
    }

    #[test]
    fn stray_keys_are_swallowed_so_nothing_leaks_to_the_table() {
        let kb = KeyBindingConfig::default();
        let mut m = menu(&[]);
        assert_eq!(m.handle_key("2", &kb), SortMenuOutcome::Consumed);
        assert_eq!(m.result(), vec![]);
    }
}
