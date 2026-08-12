use ratatui::{Frame, layout::Rect};
use tuirealm::command::{Cmd, CmdResult, Direction};
use tuirealm::component::{AppComponent, Component};
use tuirealm::event::{Event, NoUserEvent};
use tuirealm::props::{AttrValue, Attribute, QueryResult};
use tuirealm::state::{State, StateValue};

use super::{
    LeaderList, LeaderListEvent, LeaderWidth,
    render::{RenderData, render},
};

impl LeaderList {
    /// Resolves the effective line width for `area` given the configured mode.
    fn effective_width(&self, area: Rect) -> u16 {
        let want = match self.width {
            LeaderWidth::Fill => area.width,
            LeaderWidth::Min => self.min_width(),
            LeaderWidth::Fixed(w) => w,
        };
        want.min(area.width)
    }

    /// The event to emit after a page move: cursor-based when selectable,
    /// scroll-based otherwise.
    fn scroll_event(&self) -> LeaderListEvent {
        if self.selectable {
            LeaderListEvent::CursorChanged(self.cursor)
        } else {
            LeaderListEvent::Scrolled(self.scroll_offset)
        }
    }
}

impl Component for LeaderList {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        // The search prompt, title and status line each take one row off the
        // entry window.
        let search_rows = if self.search_enabled { 1 } else { 0 };
        let title_rows = if self.title.is_empty() { 0 } else { 1 };
        let status_rows = if self.status_line { 1 } else { 0 };
        let avail = area
            .height
            .saturating_sub(search_rows + title_rows + status_rows) as usize;
        let visible = match self.max_rows {
            Some(m) => (m as usize).min(avail),
            None => avail,
        };
        // Remember for page up/down, which run without the render area.
        self.page_rows = visible;

        // Keep the cursor within the visible window, then clamp the scroll —
        // all against the filtered (visible) row count.
        let visible_count = self.matches.len();
        if self.selectable && visible > 0 {
            if self.cursor < self.scroll_offset {
                self.scroll_offset = self.cursor;
            } else if self.cursor >= self.scroll_offset + visible {
                self.scroll_offset = self.cursor + 1 - visible;
            }
        }
        self.scroll_offset = self
            .scroll_offset
            .min(visible_count.saturating_sub(visible));

        let data = RenderData {
            entries: &self.entries,
            visible: &self.matches,
            marked: &self.marked,
            marker: &self.marker,
            title: &self.title,
            post: &self.post,
            pre: &self.pre,
            filler: &self.filler,
            cursor: self.cursor,
            scroll_offset: self.scroll_offset,
            selectable: self.selectable,
            focused: self.focused,
            style: &self.style,
            line_width: self.effective_width(area),
            entry_rows: visible,
            show_status: self.status_line,
            show_search: self.search_enabled,
            query: &self.query,
            search_placeholder: &self.search_placeholder,
        };
        render(frame.buffer_mut(), area, &data);
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        match attr {
            Attribute::Focus => Some(QueryResult::Owned(AttrValue::Flag(self.focused))),
            Attribute::Value => Some(QueryResult::Owned(AttrValue::Length(self.cursor))),
            _ => None,
        }
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Focus => {
                if let AttrValue::Flag(f) = value {
                    self.focused = f;
                }
            }
            Attribute::Value => {
                if let AttrValue::Length(i) = value {
                    self.cursor = i;
                    self.clamp_cursor();
                }
            }
            _ => {}
        }
    }

    fn state(&self) -> State {
        State::Single(StateValue::Usize(self.cursor))
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        if !self.selectable {
            return CmdResult::NoChange;
        }
        match cmd {
            Cmd::Move(Direction::Up) => {
                self.move_up();
                CmdResult::Changed(self.state())
            }
            Cmd::Move(Direction::Down) => {
                self.move_down();
                CmdResult::Changed(self.state())
            }
            Cmd::Submit => CmdResult::Submit(self.state()),
            Cmd::Cancel => CmdResult::Batch(vec![]),
            _ => CmdResult::NoChange,
        }
    }
}

impl AppComponent<LeaderListEvent, NoUserEvent> for LeaderList {
    fn on(&mut self, ev: &Event<NoUserEvent>) -> Option<LeaderListEvent> {
        let Event::Keyboard(key_ev) = ev else {
            return None;
        };
        let key_ev = *key_ev;

        // Fuzzy filter: printable keys (without Ctrl/Alt) type into the query,
        // Backspace edits it. Captured first so plain letters never reach the
        // navigation keymap — movement lives on the arrows / Ctrl-j/Ctrl-k.
        if self.search_enabled {
            use tuirealm::event::{Key, KeyModifiers};
            let mods = key_ev.modifiers;
            match key_ev.code {
                Key::Char(c) if !mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                    self.push_search(c);
                    return Some(LeaderListEvent::CursorChanged(self.cursor));
                }
                Key::Backspace => {
                    self.backspace_search();
                    return Some(LeaderListEvent::CursorChanged(self.cursor));
                }
                _ => {}
            }
        }

        // Paging works whenever the list can scroll — also without selection.
        if self.keymap.page_down.matches(&key_ev) {
            return self.page_move(true).then(|| self.scroll_event());
        }
        if self.keymap.page_up.matches(&key_ev) {
            return self.page_move(false).then(|| self.scroll_event());
        }

        if !self.selectable {
            return None;
        }

        let cmd = if self.keymap.move_up.matches(&key_ev) {
            Cmd::Move(Direction::Up)
        } else if self.keymap.move_down.matches(&key_ev) {
            Cmd::Move(Direction::Down)
        } else if self.keymap.confirm.matches(&key_ev) {
            Cmd::Submit
        } else if self.keymap.cancel.matches(&key_ev) {
            Cmd::Cancel
        } else {
            return None;
        };

        match self.perform(cmd) {
            CmdResult::Changed(State::Single(StateValue::Usize(idx))) => {
                Some(LeaderListEvent::CursorChanged(idx))
            }
            CmdResult::Submit(State::Single(StateValue::Usize(idx))) => {
                Some(LeaderListEvent::Selected(idx))
            }
            CmdResult::Batch(_) => Some(LeaderListEvent::Cancelled),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tuirealm::event::{Key, KeyEvent};

    fn toc() -> LeaderList {
        LeaderList::default()
            .with_entries(vec![
                ("Introduction", "1"),
                ("Getting Started", "7"),
                ("Advanced Topics", "142"),
            ])
            .with_affixes("", " ", " .")
            .with_selectable(true)
    }

    #[test]
    fn min_width_is_longest_left_plus_right() {
        let l = toc();
        // "Advanced Topics" (15) + post "" (0) + pre " ." (2) + "142" (3) = 20
        assert_eq!(l.min_width(), 20);
    }

    #[test]
    fn min_width_empty_is_zero() {
        assert_eq!(LeaderList::default().min_width(), 0);
    }

    #[test]
    fn min_width_accounts_for_a_wide_title() {
        // A title wider than the widest entry line (20) drives min_width.
        let title = "A long section heading that wins";
        let l = toc().with_title(title);
        assert!(title.len() > 20);
        assert_eq!(l.min_width() as usize, title.len());
    }

    #[test]
    fn cursor_moves_within_bounds() {
        let mut l = toc();
        assert_eq!(l.selected(), 0);
        l.move_up();
        assert_eq!(l.selected(), 0, "cannot move above the first entry");
        l.move_down();
        l.move_down();
        l.move_down();
        assert_eq!(l.selected(), 2, "cannot move past the last entry");
    }

    #[test]
    fn not_selectable_ignores_commands() {
        let mut l = toc().with_selectable(false);
        assert_eq!(l.perform(Cmd::Move(Direction::Down)), CmdResult::NoChange);
        assert_eq!(l.selected(), 0);
    }

    #[test]
    fn submit_emits_selected_event() {
        let mut l = toc();
        assert_eq!(l.perform(Cmd::Submit), CmdResult::Submit(l.state()));
    }

    fn long_list(n: usize) -> LeaderList {
        LeaderList::default().with_entries(
            (0..n)
                .map(|i| (format!("Item {i}"), i.to_string()))
                .collect(),
        )
    }

    #[test]
    fn scrollable_when_entries_exceed_max_rows() {
        // Before the first render page_rows is 0, so page_size falls back to
        // max_rows.
        let l = long_list(10).with_max_rows(3);
        assert_eq!(l.page_size(), 3);
        assert!(l.is_scrollable());
        assert!(!long_list(3).with_max_rows(3).is_scrollable());
    }

    #[test]
    fn page_down_scrolls_window_when_not_selectable() {
        let mut l = long_list(10).with_max_rows(3);
        assert!(l.page_move(true));
        assert_eq!(l.scroll_offset, 3);
        assert!(l.page_move(true));
        assert_eq!(l.scroll_offset, 6);
        // Clamped to len - page = 10 - 3 = 7.
        assert!(l.page_move(true));
        assert_eq!(l.scroll_offset, 7);
        assert!(!l.page_move(true), "already at the bottom");
    }

    #[test]
    fn page_down_moves_cursor_a_page_when_selectable() {
        let mut l = long_list(10).with_max_rows(3).with_selectable(true);
        assert!(l.page_move(true));
        assert_eq!(l.selected(), 3);
        assert_eq!(l.scroll_offset, 0, "scroll follows on next view, not here");
        l.page_move(false);
        assert_eq!(l.selected(), 0);
    }

    fn key(code: Key) -> Event<NoUserEvent> {
        Event::Keyboard(KeyEvent {
            code,
            modifiers: tuirealm::event::KeyModifiers::NONE,
        })
    }

    fn searchable() -> LeaderList {
        LeaderList::default()
            .with_entries(vec![
                ("Delete", "d"),
                ("Delete Comment", "D"),
                ("Open", "enter"),
                ("Quit", "q"),
            ])
            .with_selectable(true)
            .with_search(true)
    }

    #[test]
    fn typing_filters_to_matching_entries() {
        let mut l = searchable();
        // All four rows are visible before typing.
        assert_eq!(l.matches.len(), 4);
        l.on(&key(Key::Char('d')));
        l.on(&key(Key::Char('e')));
        assert_eq!(l.search_query(), "de");
        // "Delete" and "Delete Comment" both match "de"; "Open"/"Quit" don't.
        assert_eq!(l.matches.len(), 2);
        // Cursor is on the top (best) match, mapped back to the entry index.
        assert_eq!(l.selected_index(), Some(0));
    }

    #[test]
    fn backspace_widens_and_clear_restores() {
        let mut l = searchable();
        l.on(&key(Key::Char('x'))); // matches nothing
        assert_eq!(l.matches.len(), 0);
        assert_eq!(l.selected_index(), None);
        l.on(&key(Key::Backspace));
        assert_eq!(l.search_query(), "");
        assert_eq!(l.matches.len(), 4, "empty query shows everything again");
    }

    #[test]
    fn dot_prefix_filters_by_keys_not_label() {
        let mut l = searchable();
        // ". e" searches the keys column: only "Open" (bound to `enter`)
        // matches — despite "Delete"/"Delete Comment" containing `e` in their
        // labels.
        for c in ['.', ' ', 'e'] {
            l.on(&key(Key::Char(c)));
        }
        assert_eq!(l.search_query(), ". e");
        assert_eq!(l.matches.len(), 1);
        assert_eq!(l.selected_index(), Some(2), "the `enter`-bound row");
    }

    #[test]
    fn dot_prefix_does_not_match_label_text() {
        let mut l = searchable();
        // "delete" hits two labels, but under the `.` key-mode it matches no
        // keys column, so nothing survives.
        for c in ['.', ' ', 'd', 'e', 'l', 'e', 't', 'e'] {
            l.on(&key(Key::Char(c)));
        }
        assert_eq!(l.matches.len(), 0);
    }

    #[test]
    fn ctrl_j_navigates_and_is_not_typed() {
        let mut l = searchable();
        let ev = Event::Keyboard(KeyEvent {
            code: Key::Char('j'),
            modifiers: tuirealm::event::KeyModifiers::CONTROL,
        });
        l.on(&ev);
        assert_eq!(l.search_query(), "", "Ctrl-j must not enter the filter");
        assert_eq!(l.selected(), 1, "Ctrl-j moves the cursor down");
    }
}
