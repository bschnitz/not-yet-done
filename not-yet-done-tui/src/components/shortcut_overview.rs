//! Shortcut overview — every shortcut of the current context on one page,
//! grouped and read-only.
//!
//! Opened by [`GlobalAction::ShortcutOverview`] (default `f1`). Where the
//! Ctrl+Y [shortcut menu] is a working surface (fuzzy filter, record, delete,
//! restore), this one is a *reference*: it takes the same context rows and
//! sorts them into sections — "General" first for everything that is a key of
//! its own, then one section per configured [`WhichKeyGroup`], so the chords
//! the bars fold away (`o b`, `o o`, …) are all listed under the name their
//! group carries.
//!
//! It scrolls rather than filters: `j`/`k` (and the arrows) move a line,
//! `ctrl+d`/`ctrl+u` and the page keys move a screen, `g`/`G` jump to the
//! ends, any other key closes it. The body never grows wider than
//! [`ShortcutOverviewConfig::max_width`] — long names are truncated instead of
//! stretching the popup across the terminal.
//!
//! [shortcut menu]: crate::components::shortcut_menu::ShortcutMenu
//! [`GlobalAction::ShortcutOverview`]: crate::config::keybindings::GlobalAction::ShortcutOverview
//! [`WhichKeyGroup`]: crate::config::tui_config::WhichKeyGroup
//! [`ShortcutOverviewConfig::max_width`]: crate::config::tui_config::ShortcutOverviewConfig::max_width

use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::config::tui_config::WhichKeyGroup;
use crate::key_groups;
use crate::keymap::ShortcutRow;
use crate::ui::panel_chrome::PanelChrome;
use crate::ui::theme::Theme;

/// The section every shortcut lands in that no which-key group claims.
const GENERAL: &str = "General";

/// Smallest gap between a shortcut's name and its keys, in cells.
const GAP: usize = 2;

/// One rendered line of the overview. Building these once on open keeps
/// scrolling a plain slice of a `Vec` instead of a walk over nested sections.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    /// A section heading: the group's title (or its prefix), plus the prefix
    /// itself when the title does not already say it.
    Heading { title: String, prefix: String },
    /// One shortcut: name on the left, keys on the right.
    Entry { name: String, keys: String },
    /// The blank line between two sections.
    Gap,
}

/// The read-only, grouped shortcut list.
pub struct ShortcutOverview {
    theme: Arc<Theme>,
    open: bool,
    /// Widest the body may get, from `shortcut_overview.max_width`.
    max_width: u16,
    /// Every line, sections already flattened.
    rows: Vec<Row>,
    /// First row shown — the scroll offset.
    offset: usize,
    /// How many rows the last render fit, for page scrolling and clamping.
    viewport: usize,
}

impl ShortcutOverview {
    pub fn new(theme: Arc<Theme>, max_width: u16) -> Self {
        Self {
            theme,
            open: false,
            max_width,
            rows: Vec::new(),
            offset: 0,
            viewport: 1,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open the overview for `rows`, grouped by `groups`. Rows without a key
    /// (actions waiting for a binding — the menu's "unbound" view) are left
    /// out: there is nothing to look up about them here.
    pub fn open(&mut self, rows: Vec<ShortcutRow>, groups: &[WhichKeyGroup]) {
        self.rows = build_rows(rows, groups);
        self.offset = 0;
        self.open = true;
    }

    /// Scroll or close. Every key is consumed while the popup is open —
    /// anything that is not a scroll key closes it, the same "one look, then
    /// on with it" feel the which-key popup has.
    pub fn handle_key(&mut self, key: &str) {
        let page = self.viewport.saturating_sub(1).max(1);
        match key {
            "j" | "down" | "ctrl+j" => self.scroll(1),
            "k" | "up" | "ctrl+k" => self.scroll(-1),
            "ctrl+d" | "pagedown" | " " => self.scroll(page as isize),
            "ctrl+u" | "pageup" => self.scroll(-(page as isize)),
            "g" | "home" => self.offset = 0,
            "G" | "end" => self.offset = self.max_offset(),
            _ => self.open = false,
        }
    }

    /// The last offset that still fills the viewport, so the list never
    /// scrolls past its final line.
    fn max_offset(&self) -> usize {
        self.rows.len().saturating_sub(self.viewport)
    }

    fn scroll(&mut self, delta: isize) {
        let next = self.offset as isize + delta;
        self.offset = next.clamp(0, self.max_offset() as isize) as usize;
    }

    /// Draw the panel and the visible slice of the list.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if !self.open || self.rows.is_empty() {
            return;
        }
        let theme = Arc::clone(&self.theme);
        let width = self.content_width();
        let hints = vec![("j/k", "scroll"), ("g/G", "top/bottom"), ("esc", "close")];
        let body = PanelChrome::new(heading(&theme))
            .hints(hints)
            .body(width, self.rows.len() as u16)
            .render(frame, area, &theme);
        let Some(body) = body else { return };

        // The viewport is only known once the chrome has laid the panel out,
        // so the offset is clamped here rather than in `handle_key` — a
        // resize that shrinks the popup must not leave it scrolled past its
        // end.
        self.viewport = body.height as usize;
        self.offset = self.offset.min(self.max_offset());

        for (i, row) in self
            .rows
            .iter()
            .skip(self.offset)
            .take(body.height as usize)
            .enumerate()
        {
            let line = render_row(row, body.width, &theme);
            frame.render_widget(
                Paragraph::new(line),
                Rect::new(body.x, body.y + i as u16, body.width, 1),
            );
        }
    }

    /// The width the body asks for: the widest line, capped at `max_width`.
    fn content_width(&self) -> usize {
        let widest = self
            .rows
            .iter()
            .map(|row| match row {
                Row::Heading { title, prefix } => heading_text(title, prefix).chars().count(),
                Row::Entry { name, keys } => {
                    INDENT.len() + name.chars().count() + GAP + keys.chars().count()
                }
                Row::Gap => 0,
            })
            .max()
            .unwrap_or(0);
        widest.min(self.max_width as usize)
    }
}

/// Entries sit under their heading, so the sections read as blocks.
const INDENT: &str = "  ";

/// The panel heading.
fn heading(theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        "\u{2726} Keyboard shortcuts",
        Style::default()
            .fg(theme.form_accent())
            .add_modifier(Modifier::BOLD),
    ))
}

/// A section heading reads as its title; a titleless group falls back to its
/// prefix, and a titled one gets the prefix appended so the chord to press is
/// never guesswork.
fn heading_text(title: &str, prefix: &str) -> String {
    if title == prefix || prefix.is_empty() {
        title.to_string()
    } else {
        format!("{title}  ({prefix})")
    }
}

/// Sort `rows` into sections and flatten them into rendered lines. "General"
/// comes first and holds everything no group claims; the groups follow in
/// config order, each with the entries bound under its prefix.
fn build_rows(rows: Vec<ShortcutRow>, groups: &[WhichKeyGroup]) -> Vec<Row> {
    // (title, prefix, entries) — General first, then one per group, so the
    // config order survives even for groups that end up empty (dropped below).
    let mut sections: Vec<(String, String, Vec<Row>)> =
        vec![(GENERAL.to_string(), String::new(), Vec::new())];
    for group in groups.iter().filter(|g| !g.prefix.trim().is_empty()) {
        let title = group.title.clone().unwrap_or_else(|| group.prefix.clone());
        sections.push((title, group.prefix.clone(), Vec::new()));
    }

    let mut seen: Vec<(String, String)> = Vec::new();
    for row in rows {
        if row.keys.trim().is_empty() {
            continue;
        }
        // The same shortcut can be projected from several claims (a global
        // key listed per leaf); list it once.
        let ident = (row.name.clone(), row.keys.clone());
        if seen.contains(&ident) {
            continue;
        }
        seen.push(ident);

        let idx = match key_groups::group_of(groups, &row.keys) {
            Some(group) => sections
                .iter()
                .position(|(_, prefix, _)| prefix == &group.prefix)
                .unwrap_or(0),
            None => 0,
        };
        sections[idx].2.push(Row::Entry {
            name: row.name,
            keys: row.keys,
        });
    }

    let mut out: Vec<Row> = Vec::new();
    for (title, prefix, entries) in sections {
        if entries.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(Row::Gap);
        }
        out.push(Row::Heading { title, prefix });
        out.extend(entries);
    }
    out
}

/// One rendered line, truncated to `width`.
fn render_row<'a>(row: &Row, width: u16, theme: &Theme) -> Line<'a> {
    let width = width as usize;
    match row {
        Row::Gap => Line::from(""),
        Row::Heading { title, prefix } => Line::from(Span::styled(
            truncate(&heading_text(title, prefix), width),
            Style::default()
                .fg(theme.form_accent())
                .add_modifier(Modifier::BOLD),
        )),
        Row::Entry { name, keys } => {
            let keys_w = keys.chars().count();
            // Keys are the part worth reading; the name yields when the two
            // cannot both fit.
            let room = width
                .saturating_sub(INDENT.len())
                .saturating_sub(keys_w + GAP);
            let name = truncate(name, room);
            let fill = room.saturating_sub(name.chars().count()) + GAP;
            Line::from(vec![
                Span::styled(INDENT, Style::default()),
                Span::styled(name, Style::default().fg(theme.form_text())),
                Span::styled(
                    format!(" {} ", ".".repeat(fill.saturating_sub(2))),
                    Style::default().fg(theme.form_hint()),
                ),
                Span::styled(
                    keys.clone(),
                    Style::default()
                        .fg(theme.form_accent())
                        .add_modifier(Modifier::BOLD),
                ),
            ])
        }
    }
}

/// Cut `s` to `width` cells, marking the cut with an ellipsis.
fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    if width <= 1 {
        return "\u{2026}".chars().take(width).collect();
    }
    let mut out: String = s.chars().take(width - 1).collect();
    out.push('\u{2026}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThemeConfig;

    fn group(prefix: &str, title: Option<&str>) -> WhichKeyGroup {
        WhichKeyGroup {
            prefix: prefix.to_string(),
            title: title.map(str::to_string),
            collapse_in_bars: false,
        }
    }

    fn row(name: &str, keys: &str) -> ShortcutRow {
        ShortcutRow {
            name: name.to_string(),
            keys: keys.to_string(),
            scope: "Tasks".to_string(),
            source: None,
            key_scope: None,
        }
    }

    fn overview(rows: Vec<ShortcutRow>, groups: &[WhichKeyGroup]) -> ShortcutOverview {
        let mut o = ShortcutOverview::new(Arc::new(Theme::new(ThemeConfig::default())), 50);
        o.open(rows, groups);
        o
    }

    #[test]
    fn general_comes_first_then_the_groups_in_config_order() {
        let groups = vec![group("o", Some("Open ...")), group("q", None)];
        let o = overview(
            vec![
                row("queries", "q q"),
                row("open in browser", "o b"),
                row("edit", "e"),
            ],
            &groups,
        );
        assert_eq!(
            o.rows,
            vec![
                Row::Heading {
                    title: GENERAL.to_string(),
                    prefix: String::new()
                },
                Row::Entry {
                    name: "edit".to_string(),
                    keys: "e".to_string()
                },
                Row::Gap,
                Row::Heading {
                    title: "Open ...".to_string(),
                    prefix: "o".to_string()
                },
                Row::Entry {
                    name: "open in browser".to_string(),
                    keys: "o b".to_string()
                },
                Row::Gap,
                Row::Heading {
                    title: "q".to_string(),
                    prefix: "q".to_string()
                },
                Row::Entry {
                    name: "queries".to_string(),
                    keys: "q q".to_string()
                },
            ]
        );
    }

    /// The bare prefix key is a binding of its own (see [`key_groups`]), so it
    /// belongs in General — the group section lists what the *menu* opens.
    #[test]
    fn the_bare_prefix_key_stays_general_and_keyless_rows_are_dropped() {
        let groups = vec![group("o", Some("Open ..."))];
        let o = overview(vec![row("notes", "o"), row("unbound action", "")], &groups);
        assert_eq!(
            o.rows,
            vec![
                Row::Heading {
                    title: GENERAL.to_string(),
                    prefix: String::new()
                },
                Row::Entry {
                    name: "notes".to_string(),
                    keys: "o".to_string()
                },
            ]
        );
    }

    #[test]
    fn an_empty_group_gets_no_heading() {
        let groups = vec![group("o", Some("Open ...")), group("z", Some("Fold ..."))];
        let o = overview(vec![row("open in browser", "o b")], &groups);
        assert!(!o.rows.iter().any(|r| matches!(
            r,
            Row::Heading { title, .. } if title == "Fold ..."
        )));
    }

    #[test]
    fn the_same_shortcut_is_listed_once() {
        let o = overview(vec![row("quit", "ctrl+c"), row("quit", "ctrl+c")], &[]);
        assert_eq!(
            o.rows
                .iter()
                .filter(|r| matches!(r, Row::Entry { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn scrolling_stops_at_both_ends() {
        let rows: Vec<ShortcutRow> = (0..20).map(|i| row(&format!("action {i}"), "a")).collect();
        let mut o = overview(rows, &[]);
        o.viewport = 5;
        o.handle_key("k");
        assert_eq!(o.offset, 0, "no scrolling above the first line");
        o.handle_key("G");
        assert_eq!(o.offset, o.rows.len() - 5);
        o.handle_key("j");
        assert_eq!(
            o.offset,
            o.rows.len() - 5,
            "no scrolling past the last line"
        );
        o.handle_key("g");
        assert_eq!(o.offset, 0);
        o.handle_key("ctrl+d");
        assert_eq!(
            o.offset, 4,
            "a page is the viewport less one line of overlap"
        );
        assert!(o.is_open(), "scrolling keeps the popup open");
    }

    #[test]
    fn any_other_key_closes_it() {
        let mut o = overview(vec![row("edit", "e")], &[]);
        o.handle_key("esc");
        assert!(!o.is_open());
    }

    #[test]
    fn the_body_never_grows_past_the_configured_width() {
        let mut o = overview(vec![row(&"n".repeat(200), "ctrl+alt+shift+f12")], &[]);
        assert_eq!(o.content_width(), 50);
        o.max_width = 20;
        assert_eq!(o.content_width(), 20);
    }

    #[test]
    fn a_row_renders_name_dots_keys_within_the_width() {
        let theme = Theme::new(ThemeConfig::default());
        let line = render_row(
            &Row::Entry {
                name: "open in browser".to_string(),
                keys: "o b".to_string(),
            },
            30,
            &theme,
        );
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text.chars().count(), 30);
        assert!(text.starts_with("  open in browser "));
        assert!(text.ends_with(" o b"));
    }

    #[test]
    fn a_name_too_long_for_the_width_is_cut_not_the_keys() {
        let theme = Theme::new(ThemeConfig::default());
        let line = render_row(
            &Row::Entry {
                name: "an extremely long shortcut name".to_string(),
                keys: "ctrl+x".to_string(),
            },
            20,
            &theme,
        );
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text.chars().count(), 20);
        assert!(text.contains('\u{2026}'), "the name is elided: {text}");
        assert!(text.ends_with("ctrl+x"));
    }
}
